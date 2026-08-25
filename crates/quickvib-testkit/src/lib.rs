//! Test-only helpers for driving a running QuickVib over real loopback sockets.
//!
//! The integration suite in `crates/quickvib/tests/` talks to the instrument exactly as a UTS
//! would — a TCP socket, ASCII lines, `\n` terminators — so the helpers here are deliberately
//! thin: a line-oriented SCPI client that knows how to tell an out-of-band `#`-prefixed
//! notification from a response, and a stand-in for the M300 that dials into the device port
//! and pushes little-endian `f32` bytes.
//!
//! Nothing in this crate is used by the shipped binary; it exists so the same helpers are
//! shared by every test file rather than copy-pasted (`docs/PLAN.md` 17.2).

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Default socket timeout. Long enough that a loaded CI runner never trips it, short enough
/// that a genuinely hung server fails the test rather than the job.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(20);

/// The `#` prefix that marks a line as an out-of-band notification (D17).
pub const NOTIFICATION_PREFIX: char = '#';

/// A line-oriented SCPI client.
#[derive(Debug)]
pub struct ScpiClient {
    reader: BufReader<TcpStream>,
    writer: TcpStream,
    notifications: VecDeque<String>,
}

impl ScpiClient {
    /// Connect to a SCPI server with [`DEFAULT_TIMEOUT`].
    ///
    /// # Errors
    /// Any connect or socket-option failure.
    pub fn connect(addr: impl ToSocketAddrs) -> std::io::Result<Self> {
        Self::connect_with_timeout(addr, DEFAULT_TIMEOUT)
    }

    /// Connect with an explicit read/write timeout.
    ///
    /// # Errors
    /// Any connect or socket-option failure.
    pub fn connect_with_timeout(
        addr: impl ToSocketAddrs,
        timeout: Duration,
    ) -> std::io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        stream.set_nodelay(true)?;
        Ok(Self {
            reader: BufReader::new(stream.try_clone()?),
            writer: stream,
            notifications: VecDeque::new(),
        })
    }

    /// Change the read timeout, for tests that deliberately expect silence.
    ///
    /// # Errors
    /// Any socket-option failure.
    pub fn set_read_timeout(&mut self, timeout: Duration) -> std::io::Result<()> {
        self.writer.set_read_timeout(Some(timeout))
    }

    /// The local address of this session, which is what the server logs as the peer.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.writer.local_addr()
    }

    /// Send one message, appending the terminator when the caller did not.
    ///
    /// # Errors
    /// Any write failure.
    pub fn send(&mut self, message: &str) -> std::io::Result<()> {
        let mut bytes = message.as_bytes().to_vec();
        if !message.ends_with('\n') {
            bytes.push(b'\n');
        }
        self.writer.write_all(&bytes)?;
        self.writer.flush()
    }

    /// Send bytes verbatim, for tests that exercise `\r\n`, over-long lines or invalid UTF-8.
    ///
    /// # Errors
    /// Any write failure.
    pub fn send_raw(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    /// Read the next line of any kind, notifications included, without its terminator.
    ///
    /// # Errors
    /// Any read failure, and [`std::io::ErrorKind::UnexpectedEof`] when the peer closed.
    pub fn read_any_line(&mut self) -> std::io::Result<String> {
        let mut line = String::new();
        let read = self.reader.read_line(&mut line)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "the server closed the session",
            ));
        }
        Ok(line.trim_end_matches(['\r', '\n']).to_owned())
    }

    /// Read the next *response* line, setting aside any notifications that arrive first.
    ///
    /// # Errors
    /// Any read failure.
    pub fn read_response(&mut self) -> std::io::Result<String> {
        loop {
            let line = self.read_any_line()?;
            if line.starts_with(NOTIFICATION_PREFIX) {
                self.notifications.push_back(line);
                continue;
            }
            return Ok(line);
        }
    }

    /// Send a query and read its response.
    ///
    /// # Errors
    /// Any read or write failure.
    pub fn query(&mut self, message: &str) -> std::io::Result<String> {
        self.send(message)?;
        self.read_response()
    }

    /// Send a command that produces no response.
    ///
    /// # Errors
    /// Any write failure.
    pub fn command(&mut self, message: &str) -> std::io::Result<()> {
        self.send(message)
    }

    /// Ask for the oldest queued error, as `SYST:ERR?` reports it.
    ///
    /// # Errors
    /// Any read or write failure.
    pub fn error(&mut self) -> std::io::Result<String> {
        self.query("SYST:ERR?")
    }

    /// The numeric part of the oldest queued error.
    ///
    /// # Errors
    /// Any read or write failure. A response that does not start with an integer reports
    /// [`std::io::ErrorKind::InvalidData`].
    pub fn error_code(&mut self) -> std::io::Result<i32> {
        let response = self.error()?;
        response
            .split(',')
            .next()
            .and_then(|code| code.trim().parse().ok())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("not an error response: {response}"),
                )
            })
    }

    /// A notification already set aside by [`ScpiClient::read_response`], if any.
    pub fn take_notification(&mut self) -> Option<String> {
        self.notifications.pop_front()
    }

    /// Wait up to `timeout` for a notification, including ones already set aside.
    ///
    /// # Errors
    /// Any read failure other than a timeout.
    pub fn wait_notification(&mut self, timeout: Duration) -> std::io::Result<Option<String>> {
        if let Some(line) = self.notifications.pop_front() {
            return Ok(Some(line));
        }
        let previous = self.writer.read_timeout()?;
        self.writer.set_read_timeout(Some(timeout))?;
        let result = self.read_any_line();
        self.writer.set_read_timeout(previous)?;
        match result {
            Ok(line) if line.starts_with(NOTIFICATION_PREFIX) => Ok(Some(line)),
            Ok(line) => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("expected a notification, got: {line}"),
            )),
            Err(e) if is_timeout(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Assert that nothing arrives within `window`. Used to prove a notification was *not*
    /// sent.
    ///
    /// # Errors
    /// Any read failure other than a timeout.
    pub fn expect_silence(&mut self, window: Duration) -> std::io::Result<bool> {
        let previous = self.writer.read_timeout()?;
        self.writer.set_read_timeout(Some(window))?;
        let result = self.read_any_line();
        self.writer.set_read_timeout(previous)?;
        match result {
            Ok(_) => Ok(false),
            Err(e) if is_timeout(&e) => Ok(true),
            Err(e) => Err(e),
        }
    }

    /// Close the session the way a UTS does when it is finished.
    pub fn close(self) {
        let _ = self.writer.shutdown(Shutdown::Both);
    }
}

/// Whether an error is a socket read timeout, which differs by platform.
#[must_use]
pub fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

/// Parse a comma-separated `FETC?` response back into samples.
///
/// # Errors
/// [`std::io::ErrorKind::InvalidData`] when a field is not a float.
pub fn parse_samples(response: &str) -> std::io::Result<Vec<f32>> {
    if response.is_empty() {
        return Ok(Vec::new());
    }
    response
        .split(',')
        .map(|field| {
            field.trim().parse::<f32>().map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("not a sample: {field}"),
                )
            })
        })
        .collect()
}

/// A stand-in for the M300: dials into the device port and pushes little-endian `f32` bytes.
#[derive(Debug)]
pub struct FakeDevice {
    stream: TcpStream,
}

impl FakeDevice {
    /// Dial into the device listener.
    ///
    /// # Errors
    /// Any connect or socket-option failure.
    pub fn connect(addr: impl ToSocketAddrs) -> std::io::Result<Self> {
        let stream = TcpStream::connect(addr)?;
        stream.set_read_timeout(Some(DEFAULT_TIMEOUT))?;
        stream.set_write_timeout(Some(DEFAULT_TIMEOUT))?;
        Ok(Self { stream })
    }

    /// Push samples as little-endian `f32`.
    ///
    /// # Errors
    /// Any write failure.
    pub fn push(&mut self, samples: &[f32]) -> std::io::Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 4);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.stream.write_all(&bytes)?;
        self.stream.flush()
    }

    /// Push raw bytes, for tests that split a sample across two writes.
    ///
    /// # Errors
    /// Any write failure.
    pub fn push_bytes(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }

    /// Whether the server closed this connection, which is how a refusal is observed.
    ///
    /// # Errors
    /// Any read failure other than a timeout.
    pub fn was_refused(&mut self, within: Duration) -> std::io::Result<bool> {
        self.stream.set_read_timeout(Some(within))?;
        let mut buf = [0u8; 1];
        match self.stream.read(&mut buf) {
            Ok(0) => Ok(true),
            Ok(_) => Ok(false),
            Err(e) if is_timeout(&e) => Ok(false),
            // A reset is the other way a refused connection shows up.
            Err(_) => Ok(true),
        }
    }

    /// Disconnect, as a device does when it is unplugged.
    pub fn disconnect(self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Poll `condition` until it holds or `timeout` elapses. Returns whether it held.
///
/// Real elapsed time is the right tool here: these helpers bound a *test's* patience with a
/// concurrent server, which is the one thing the injected clock cannot model.
#[must_use]
#[allow(clippy::disallowed_methods)]
pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn samples_round_trip_through_the_wire_form() {
        assert_eq!(parse_samples("").unwrap(), Vec::<f32>::new());
        assert_eq!(parse_samples("1,-2.5,0.125").unwrap(), [1.0, -2.5, 0.125]);
        assert!(parse_samples("1,oops").is_err());
    }

    #[test]
    fn wait_until_reports_a_condition_that_never_holds() {
        assert!(!wait_until(Duration::from_millis(10), || false));
        assert!(wait_until(Duration::from_millis(10), || true));
    }

    #[test]
    fn timeouts_are_recognised_on_every_platform() {
        for kind in [std::io::ErrorKind::WouldBlock, std::io::ErrorKind::TimedOut] {
            assert!(is_timeout(&std::io::Error::new(kind, "x")));
        }
        assert!(!is_timeout(&std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "x"
        )));
    }
}
