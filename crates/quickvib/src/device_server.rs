//! The device link server (`docs/PLAN.md` 7.3).
//!
//! QuickVib is the server on both links: the UTS dials `--scpi-port` and the M300 dials
//! `--device-port`. This module owns the second of those — it binds the port, applies the
//! project's peer allow-list, frames the inbound little-endian `f32` stream, and publishes
//! what it sees.
//!
//! On the mock path there is no device to dial in, so the listener is idle and the mock
//! backend supplies the samples. The port is still bound and still framed, which is what
//! keeps the accept, refusal and framing paths — the parts that carry the real risk on the
//! M300 path — exercised by Linux CI rather than discovered on the bench.
//!
//! With `--backend tcp` the same framed samples are also handed to a
//! [`quickvib_device::SampleSink`], which is what turns the inbound link from something the
//! server merely counts into the source the engine records from.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use quickvib_core::{CancelToken, Level, Logger};
use quickvib_device::framer::Framed;
use quickvib_device::listener::Refusal;
use quickvib_device::{ConnectionState, Framer, InboundDeviceServer, SampleSink};

/// What the device link has seen since startup. Cheap to read from any thread.
#[derive(Debug, Default)]
pub struct LinkStats {
    connections: AtomicU64,
    refusals: AtomicU64,
    samples: AtomicU64,
}

impl LinkStats {
    /// Connections accepted since startup.
    #[must_use]
    pub fn connections(&self) -> u64 {
        self.connections.load(Ordering::Relaxed)
    }

    /// Connections turned away since startup.
    #[must_use]
    pub fn refusals(&self) -> u64 {
        self.refusals.load(Ordering::Relaxed)
    }

    /// Samples framed off the device link since startup.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }
}

/// Accepts the device's inbound connection and frames its sample stream.
pub struct DeviceServer {
    inner: InboundDeviceServer,
    logger: Arc<dyn Logger>,
    stats: Arc<LinkStats>,
    sink: Option<Arc<dyn SampleSink>>,
}

impl std::fmt::Debug for DeviceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeviceServer")
            .field("inner", &self.inner)
            .field("stats", &self.stats)
            .finish()
    }
}

impl DeviceServer {
    /// Bind the device port. Pass port `0` to let the OS choose.
    ///
    /// # Errors
    /// Any bind failure from the operating system.
    pub fn bind(
        addr: impl ToSocketAddrs,
        allowed_peers: Vec<String>,
        logger: Arc<dyn Logger>,
    ) -> std::io::Result<Self> {
        let inner = InboundDeviceServer::bind(addr)?.with_allowed_peers(allowed_peers);
        Ok(Self {
            inner,
            logger,
            stats: Arc::new(LinkStats::default()),
            sink: None,
        })
    }

    /// Forward every framed batch to `sink`, which is how the `tcp` backend is fed.
    #[must_use]
    pub fn with_sink(mut self, sink: Arc<dyn SampleSink>) -> Self {
        self.sink = Some(sink);
        self
    }

    /// The bound address, including the OS-assigned port when `0` was requested.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.local_addr()
    }

    /// A handle to the observable link state, which is what `SYST:DEV:CONN?` reports for a
    /// socket-fed backend.
    #[must_use]
    pub fn state(&self) -> Arc<ConnectionState> {
        self.inner.state()
    }

    /// Counters for the tests and for the shutdown summary line.
    #[must_use]
    pub fn stats(&self) -> Arc<LinkStats> {
        Arc::clone(&self.stats)
    }

    fn log(&self, level: Level, message: impl AsRef<str>) {
        if self.logger.enabled(level) {
            self.logger.log(level, "device", message.as_ref());
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

        let logger = Arc::clone(&self.logger);
        let stats = Arc::clone(&self.stats);
        let sink = self.sink.clone();
        let handler: Arc<dyn Fn(TcpStream, SocketAddr) + Send + Sync> =
            Arc::new(move |stream, peer| {
                handle_link(&logger, &stats, sink.as_ref(), stream, peer);
            });

        self.inner.run(cancel, handler, &|peer, why| {
            self.stats.refusals.fetch_add(1, Ordering::Relaxed);
            let reason = match why {
                Refusal::PeerNotAllowed => "peerNotAllowed",
                Refusal::AlreadyConnected => "alreadyConnected",
                Refusal::SocketError => "socketError",
            };
            self.log(Level::Warn, format!("refused peer={peer} reason={reason}"));
        });

        self.log(Level::Info, "accept loop stopped");
    }
}

/// Drain one device connection, framing it into samples.
fn handle_link(
    logger: &Arc<dyn Logger>,
    stats: &Arc<LinkStats>,
    sink: Option<&Arc<dyn SampleSink>>,
    stream: TcpStream,
    peer: SocketAddr,
) {
    stats.connections.fetch_add(1, Ordering::Relaxed);
    if logger.enabled(Level::Info) {
        logger.log(Level::Info, "device", &format!("connected peer={peer}"));
    }
    if let Some(sink) = sink {
        sink.on_connected(Some(peer));
    }

    let mut framer = Framer::new(stream);
    let reason = loop {
        match framer.read_batch() {
            Ok(Framed::Batch) => {
                stats
                    .samples
                    .fetch_add(framer.samples().len() as u64, Ordering::Relaxed);
                if let Some(sink) = sink {
                    sink.on_samples(framer.samples());
                }
            }
            Ok(Framed::WouldBlock) => continue,
            Ok(Framed::Eof) => break "closed".to_owned(),
            Err(error) => break format!("failed: {error}"),
        }
    };

    if let Some(sink) = sink {
        sink.on_disconnected();
    }

    if logger.enabled(Level::Info) {
        logger.log(
            Level::Info,
            "device",
            &format!(
                "disconnected peer={peer} samples={} reason={reason}",
                framer.total_samples()
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::NullLogger;
    use quickvib_device::SampleChannel;
    use std::io::Write;
    use std::time::Duration;

    fn server(allowed: Vec<String>) -> DeviceServer {
        DeviceServer::bind("127.0.0.1:0", allowed, Arc::new(NullLogger)).unwrap()
    }

    /// Poll until `condition` holds, bounded so a hang fails the test instead of the job.
    #[allow(clippy::disallowed_methods)]
    fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if condition() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        condition()
    }

    #[test]
    fn a_device_can_dial_in_and_push_samples() {
        let server = server(Vec::new());
        let addr = server.local_addr().unwrap();
        let state = server.state();
        let stats = server.stats();
        let cancel = CancelToken::new();

        let running = cancel.clone();
        let join = std::thread::spawn(move || server.run(&running));

        let mut device = TcpStream::connect(addr).unwrap();
        assert!(state.wait_connected(Duration::from_secs(5)));

        for sample in [1.0_f32, -2.0, 3.5] {
            device.write_all(&sample.to_le_bytes()).unwrap();
        }
        device.flush().unwrap();
        drop(device);

        assert!(wait_until(|| stats.samples() == 3));
        assert_eq!(stats.connections(), 1);
        assert_eq!(stats.refusals(), 0);

        cancel.cancel();
        join.join().unwrap();
    }

    #[test]
    fn samples_split_across_writes_are_reassembled() {
        let server = server(Vec::new());
        let addr = server.local_addr().unwrap();
        let stats = server.stats();
        let cancel = CancelToken::new();

        let running = cancel.clone();
        let join = std::thread::spawn(move || server.run(&running));

        let mut device = TcpStream::connect(addr).unwrap();
        // One byte at a time is the pathological case the framer exists to survive.
        for byte in 42.0_f32.to_le_bytes() {
            device.write_all(&[byte]).unwrap();
            device.flush().unwrap();
        }
        assert!(wait_until(|| stats.samples() == 1));

        drop(device);
        cancel.cancel();
        join.join().unwrap();
    }

    #[test]
    fn framed_samples_reach_the_sink() {
        let channel = Arc::new(SampleChannel::default());
        let server = DeviceServer::bind("127.0.0.1:0", Vec::new(), Arc::new(NullLogger))
            .unwrap()
            .with_sink(Arc::clone(&channel) as Arc<dyn SampleSink>);
        let addr = server.local_addr().unwrap();
        let cancel = CancelToken::new();

        let running = cancel.clone();
        let join = std::thread::spawn(move || server.run(&running));

        let mut device = TcpStream::connect(addr).unwrap();
        assert!(channel.wait_connected(Duration::from_secs(5)));

        for sample in [1.0_f32, -2.0, 3.5] {
            device.write_all(&sample.to_le_bytes()).unwrap();
        }
        device.flush().unwrap();
        assert!(wait_until(|| channel.received() == 3));

        drop(device);
        assert!(wait_until(|| !channel.is_connected()));

        cancel.cancel();
        join.join().unwrap();
    }

    #[test]
    fn a_disallowed_peer_is_turned_away() {
        let server = server(vec!["10.0.0.1".to_owned()]);
        let addr = server.local_addr().unwrap();
        let state = server.state();
        let stats = server.stats();
        let cancel = CancelToken::new();

        let running = cancel.clone();
        let join = std::thread::spawn(move || server.run(&running));

        let device = TcpStream::connect(addr).unwrap();
        assert!(wait_until(|| stats.refusals() == 1));
        assert!(!state.is_connected());
        assert_eq!(stats.connections(), 0);

        drop(device);
        cancel.cancel();
        join.join().unwrap();
    }
}
