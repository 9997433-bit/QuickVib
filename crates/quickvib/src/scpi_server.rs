//! The SCPI server (`docs/PLAN.md` 7.2).
//!
//! One accept loop, one thread per session, one shared [`Engine`] behind an `Arc` — the
//! instrument model is global, exactly as it is on a real box where two sessions can both
//! talk to one instrument. What is *not* shared is the write half of each socket: every
//! session owns a `Mutex<TcpStream>`, which is what guarantees an out-of-band `#REC:DONE`
//! can never appear in the middle of another session's response line.

use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use quickvib_core::{CancelToken, Level, Logger, ScpiError};
use quickvib_engine::{dispatch, Engine, NotificationSink};
use quickvib_scpi::{parse_line, write_response_line, Response, MAX_LINE_BYTES};

/// Concurrent sessions accepted before further connections are refused.
pub const DEFAULT_MAX_SESSIONS: usize = 8;

/// How often the accept loop wakes to check for cancellation.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// The undocumented line that makes a session thread panic, so the containment rule in D27
/// can be tested end to end. Recognised only when fault injection is switched on, which the
/// shipped binary never does.
pub const FAULT_INJECTION_PANIC: &str = "SYST:TEST:PANIC";

/// Accepts UTS connections and runs a session thread for each.
pub struct ScpiServer {
    listener: TcpListener,
    engine: Arc<Engine>,
    logger: Arc<dyn Logger>,
    max_sessions: usize,
    fault_injection: bool,
    live_sessions: Arc<AtomicUsize>,
}

impl std::fmt::Debug for ScpiServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScpiServer")
            .field("local_addr", &self.listener.local_addr().ok())
            .field("max_sessions", &self.max_sessions)
            .field("live_sessions", &self.live_sessions.load(Ordering::Relaxed))
            .finish()
    }
}

impl ScpiServer {
    /// Bind the listener. Pass port `0` to let the OS choose, then read the assigned port
    /// back with [`ScpiServer::local_addr`].
    ///
    /// # Errors
    /// Any bind failure from the operating system, typically "address already in use".
    pub fn bind(
        addr: impl ToSocketAddrs,
        engine: Arc<Engine>,
        logger: Arc<dyn Logger>,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            engine,
            logger,
            max_sessions: DEFAULT_MAX_SESSIONS,
            fault_injection: false,
            live_sessions: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Cap concurrent sessions at `max`.
    #[must_use]
    pub fn with_max_sessions(mut self, max: usize) -> Self {
        self.max_sessions = max.max(1);
        self
    }

    /// Enable the test-only fault-injection command.
    #[must_use]
    pub fn with_fault_injection(mut self, enabled: bool) -> Self {
        self.fault_injection = enabled;
        self
    }

    /// The bound address, including the OS-assigned port when `0` was requested.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// How many sessions are live right now.
    #[must_use]
    pub fn live_sessions(&self) -> usize {
        self.live_sessions.load(Ordering::Relaxed)
    }

    fn log(&self, level: Level, message: impl AsRef<str>) {
        if self.logger.enabled(level) {
            self.logger.log(level, "scpi", message.as_ref());
        }
    }

    /// Run the accept loop until `cancel` fires.
    pub fn run(&self, cancel: &CancelToken) {
        self.log(
            Level::Info,
            format!(
                "listening port={}",
                self.local_addr().map_or(0, |a| a.port())
            ),
        );

        while !cancel.is_cancelled() {
            match self.listener.accept() {
                Ok((stream, peer)) => self.accept(stream, peer),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    let _ = cancel.wait_timeout(ACCEPT_POLL_INTERVAL);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.log(Level::Warn, format!("accept failed: {e}"));
                    let _ = cancel.wait_timeout(ACCEPT_POLL_INTERVAL);
                }
            }
        }
        self.log(Level::Info, "accept loop stopped");
    }

    fn accept(&self, stream: TcpStream, peer: SocketAddr) {
        if stream.set_nonblocking(false).is_err() {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        let _ = stream.set_nodelay(true);

        // Reserve a slot before spawning so two simultaneous accepts cannot both squeeze past
        // the cap.
        let taken = self
            .live_sessions
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |live| {
                (live < self.max_sessions).then_some(live + 1)
            });
        if taken.is_err() {
            self.log(
                Level::Warn,
                format!(
                    "refused peer={peer} reason=sessionCap max={}",
                    self.max_sessions
                ),
            );
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }

        let engine = Arc::clone(&self.engine);
        let logger = Arc::clone(&self.logger);
        let live = Arc::clone(&self.live_sessions);
        let fault_injection = self.fault_injection;

        let spawned = std::thread::Builder::new()
            .name("quickvib-scpi-session".to_owned())
            .spawn(move || {
                serve_session(&engine, &logger, stream, peer, fault_injection);
                live.fetch_sub(1, Ordering::AcqRel);
            });

        if spawned.is_err() {
            self.live_sessions.fetch_sub(1, Ordering::AcqRel);
            self.log(
                Level::Error,
                format!("could not spawn a session for {peer}"),
            );
        }
    }
}

/// One session's socket, plus the write mutex that keeps lines whole.
struct Session {
    writer: Mutex<TcpStream>,
}

impl Session {
    fn write_responses(&self, responses: &[Response]) -> std::io::Result<()> {
        let guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        // A `FETC?` of a 500 000-sample capture is several megabytes of ASCII; buffering
        // turns that into a handful of syscalls instead of one per sample.
        let mut buffered = BufWriter::with_capacity(64 * 1024, &*guard);
        write_response_line(&mut buffered, responses)?;
        buffered.flush()
    }

    fn write_line(&self, line: &str) -> std::io::Result<()> {
        let mut guard = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        guard.write_all(line.as_bytes())?;
        guard.write_all(b"\n")?;
        guard.flush()
    }
}

impl NotificationSink for Session {
    fn notify(&self, line: &str) {
        // A session whose peer has vanished must not disturb the others.
        let _ = self.write_line(line);
    }
}

/// Serve one session until the peer closes, wrapped so a panic closes only this session
/// (D27).
fn serve_session(
    engine: &Arc<Engine>,
    logger: &Arc<dyn Logger>,
    stream: TcpStream,
    peer: SocketAddr,
    fault_injection: bool,
) {
    let Ok(write_half) = stream.try_clone() else {
        let _ = stream.shutdown(Shutdown::Both);
        return;
    };

    let session = Arc::new(Session {
        writer: Mutex::new(write_half),
    });
    let id = engine.register_session(Arc::clone(&session) as Arc<dyn NotificationSink>);
    if logger.enabled(Level::Info) {
        logger.log(Level::Info, "scpi", &format!("session opened peer={peer}"));
    }

    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        session_loop(engine, &session, stream, fault_injection);
    }));

    engine.unregister_session(id);
    if outcome.is_err() {
        // The queue entry is what the UTS sees; the log line is what an engineer sees.
        engine.push_error(ScpiError::CommandError.detail(format!("session {peer} panicked")));
        if logger.enabled(Level::Error) {
            logger.log(
                Level::Error,
                "scpi",
                &format!("session panicked peer={peer}; the session was closed"),
            );
        }
    } else if logger.enabled(Level::Info) {
        logger.log(Level::Info, "scpi", &format!("session closed peer={peer}"));
    }

    let guard = session
        .writer
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let _ = guard.shutdown(Shutdown::Both);
}

fn session_loop(
    engine: &Arc<Engine>,
    session: &Arc<Session>,
    stream: TcpStream,
    fault_injection: bool,
) {
    let mut reader = BufReader::new(stream);
    let mut line = Vec::with_capacity(256);

    loop {
        line.clear();
        match read_line_capped(&mut reader, &mut line) {
            Ok(ReadOutcome::Eof) => return,
            Ok(ReadOutcome::Line) => {}
            Ok(ReadOutcome::TooLong) => {
                engine.push_error(
                    ScpiError::CommandError
                        .detail(format!("input line exceeded {MAX_LINE_BYTES} bytes")),
                );
                continue;
            }
            Ok(ReadOutcome::Unterminated) => {
                // IEEE 488.2 wants a terminator, so the fragment is not executed; the peer is
                // already gone, so there is nothing left to serve after recording the error.
                engine.push_error(
                    ScpiError::CommandError.detail("line not terminated before EOF"),
                );
                return;
            }
            Err(_) => return,
        }

        if fault_injection && is_panic_probe(&line) {
            panic!("fault injection: {FAULT_INJECTION_PANIC}");
        }

        let responses = match parse_line(&line) {
            Ok(commands) => commands
                .iter()
                .map(|command| dispatch(engine, command))
                .collect::<Vec<Response>>(),
            Err(error) => {
                engine.push_error(error.scpi_error().detail(error.to_string()));
                continue;
            }
        };

        if session.write_responses(&responses).is_err() {
            // The peer went away mid-response. That is its business, not the instrument's.
            return;
        }
    }
}

/// What one read produced.
enum ReadOutcome {
    /// A complete line is in the buffer.
    Line,
    /// The peer closed.
    Eof,
    /// The line ran past [`MAX_LINE_BYTES`] and was discarded up to its terminator.
    TooLong,
    /// The peer closed part-way through a line, so the fragment has no terminator.
    Unterminated,
}

/// Read one `\n`-terminated line, refusing to buffer more than [`MAX_LINE_BYTES`].
///
/// Reading raw bytes rather than `read_line` means invalid UTF-8 from a misbehaving client is
/// a `-100` from the parser instead of an error that kills the session.
fn read_line_capped<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> std::io::Result<ReadOutcome> {
    // One byte of headroom distinguishes "exactly at the limit" from "over it".
    let read = reader
        .by_ref()
        .take(MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', line)?;

    if read == 0 {
        return Ok(ReadOutcome::Eof);
    }
    if !line.ends_with(b"\n") {
        // Two ways to end up here: the line outran the cap, or the peer closed mid-line. Only
        // the first has more of the same line still coming.
        if line.len() > MAX_LINE_BYTES {
            discard_to_terminator(reader)?;
            return Ok(ReadOutcome::TooLong);
        }
        return Ok(ReadOutcome::Unterminated);
    }
    if line.len() > MAX_LINE_BYTES {
        return Ok(ReadOutcome::TooLong);
    }
    Ok(ReadOutcome::Line)
}

fn discard_to_terminator<R: BufRead>(reader: &mut R) -> std::io::Result<()> {
    let mut sink = Vec::with_capacity(4096);
    loop {
        sink.clear();
        let read = reader
            .by_ref()
            .take(MAX_LINE_BYTES as u64)
            .read_until(b'\n', &mut sink)?;
        if read == 0 || sink.ends_with(b"\n") {
            return Ok(());
        }
    }
}

fn is_panic_probe(line: &[u8]) -> bool {
    std::str::from_utf8(line)
        .is_ok_and(|text| text.trim().eq_ignore_ascii_case(FAULT_INJECTION_PANIC))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_terminated_line_is_returned_whole() {
        let mut reader = BufReader::new(Cursor::new(b"*IDN?\n*CLS\n".to_vec()));
        let mut line = Vec::new();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::Line)
        ));
        assert_eq!(line, b"*IDN?\n");
    }

    #[test]
    fn a_closed_peer_reports_eof() {
        let mut reader = BufReader::new(Cursor::new(Vec::new()));
        let mut line = Vec::new();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::Eof)
        ));
    }

    #[test]
    fn an_over_long_line_is_discarded_up_to_the_next_terminator() {
        let mut input = vec![b'A'; MAX_LINE_BYTES + 10];
        input.push(b'\n');
        input.extend_from_slice(b"*IDN?\n");

        let mut reader = BufReader::new(Cursor::new(input));
        let mut line = Vec::new();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::TooLong)
        ));

        line.clear();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::Line)
        ));
        assert_eq!(line, b"*IDN?\n");
    }

    #[test]
    fn a_line_cut_short_by_eof_is_not_reported_as_over_long() {
        let mut reader = BufReader::new(Cursor::new(b"*IDN?".to_vec()));
        let mut line = Vec::new();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::Unterminated)
        ));
        assert_eq!(line, b"*IDN?");
    }

    #[test]
    fn an_over_long_line_cut_short_by_eof_is_still_over_long() {
        let input = vec![b'A'; MAX_LINE_BYTES + 10];
        let mut reader = BufReader::new(Cursor::new(input));
        let mut line = Vec::new();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::TooLong)
        ));
    }

    #[test]
    fn a_line_at_exactly_the_limit_is_accepted() {
        let mut input = vec![b'A'; MAX_LINE_BYTES - 1];
        input.push(b'\n');
        let mut reader = BufReader::new(Cursor::new(input));
        let mut line = Vec::new();
        assert!(matches!(
            read_line_capped(&mut reader, &mut line),
            Ok(ReadOutcome::Line)
        ));
        assert_eq!(line.len(), MAX_LINE_BYTES);
    }

    #[test]
    fn the_panic_probe_is_matched_case_insensitively() {
        assert!(is_panic_probe(b"SYST:TEST:PANIC\n"));
        assert!(is_panic_probe(b"  syst:test:panic \r\n"));
        assert!(!is_panic_probe(b"*IDN?\n"));
    }
}
