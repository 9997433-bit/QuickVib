//! The inbound device listener (`docs/PLAN.md` 7.3).
//!
//! QuickVib is the server on both links; the M300 dials in. Only one device connection is
//! honoured at a time — a second inbound connection while one is live is reported and closed
//! immediately rather than queued.

use std::io::ErrorKind;
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use quickvib_core::CancelToken;

use crate::backend::ConnectionObserver;

/// How often the accept loop wakes to check for cancellation.
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Why an inbound connection was turned away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The peer is not in `device.allowedPeers`.
    PeerNotAllowed,
    /// A device connection is already live.
    AlreadyConnected,
}

/// Shared, observable state of the device link.
#[derive(Debug, Default)]
pub struct ConnectionState {
    peer: Mutex<Option<SocketAddr>>,
    changed: Condvar,
}

impl ConnectionState {
    /// A state that starts disconnected.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a device is currently connected. Backs `SYST:DEV:CONN?` for socket-fed
    /// backends.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.peer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// The connected peer, if any.
    #[must_use]
    pub fn peer(&self) -> Option<SocketAddr> {
        *self.peer.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Block until a device is connected or `timeout` elapses. Returns the connected state.
    #[must_use]
    pub fn wait_connected(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, true)
    }

    /// Block until no device is connected or `timeout` elapses. Returns the connected state.
    #[must_use]
    pub fn wait_disconnected(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, false)
    }

    fn wait_for(&self, timeout: Duration, connected: bool) -> bool {
        let guard = self.peer.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |p| p.is_some() != connected)
            .unwrap_or_else(PoisonError::into_inner);
        guard.is_some()
    }

    fn set(&self, peer: Option<SocketAddr>) {
        let mut guard = self.peer.lock().unwrap_or_else(PoisonError::into_inner);
        *guard = peer;
        drop(guard);
        self.changed.notify_all();
    }
}

/// Accepts the device's inbound connection and hands the stream to a handler.
pub struct InboundDeviceServer {
    listener: TcpListener,
    allowed_peers: Vec<String>,
    state: Arc<ConnectionState>,
    observer: Option<Arc<dyn ConnectionObserver>>,
}

impl std::fmt::Debug for InboundDeviceServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InboundDeviceServer")
            .field("local_addr", &self.listener.local_addr().ok())
            .field("allowed_peers", &self.allowed_peers)
            .field("connected", &self.state.is_connected())
            .finish()
    }
}

impl InboundDeviceServer {
    /// Bind the listener. Pass port `0` to let the OS choose, then read it back with
    /// [`InboundDeviceServer::local_addr`].
    ///
    /// # Errors
    /// Any bind failure from the operating system, typically "address already in use".
    pub fn bind(addr: impl ToSocketAddrs) -> std::io::Result<Self> {
        let listener = TcpListener::bind(addr)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            allowed_peers: Vec::new(),
            state: Arc::new(ConnectionState::new()),
            observer: None,
        })
    }

    /// Restrict inbound connections to these peers. An entry matches either the peer's IP
    /// address or its full `address:port`. An empty list accepts any peer.
    #[must_use]
    pub fn with_allowed_peers(mut self, peers: Vec<String>) -> Self {
        self.allowed_peers = peers;
        self
    }

    /// Publish connection transitions to `observer`.
    #[must_use]
    pub fn with_observer(mut self, observer: Arc<dyn ConnectionObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// The bound address, including the OS-assigned port when `0` was requested.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// A handle to the observable link state.
    #[must_use]
    pub fn state(&self) -> Arc<ConnectionState> {
        Arc::clone(&self.state)
    }

    /// Whether `peer` passes the allow-list.
    #[must_use]
    pub fn is_peer_allowed(&self, peer: SocketAddr) -> bool {
        if self.allowed_peers.is_empty() {
            return true;
        }
        let ip = peer.ip().to_string();
        let full = peer.to_string();
        self.allowed_peers.iter().any(|p| p == &ip || p == &full)
    }

    /// Run the accept loop until `cancel` fires.
    ///
    /// `handler` runs on its own thread, so the loop keeps accepting and can refuse a second
    /// inbound connection while one is live. `on_refused` is called for every turned-away
    /// connection, which is where the caller logs it.
    pub fn run(
        &self,
        cancel: &CancelToken,
        handler: Arc<dyn Fn(TcpStream, SocketAddr) + Send + Sync>,
        on_refused: &dyn Fn(SocketAddr, Refusal),
    ) {
        while !cancel.is_cancelled() {
            match self.listener.accept() {
                Ok((stream, peer)) => self.dispatch(stream, peer, &handler, on_refused),
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    // Nothing pending. Park briefly so cancellation is observed promptly
                    // without spinning a core.
                    let _ = cancel.wait_timeout(ACCEPT_POLL_INTERVAL);
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    let _ = cancel.wait_timeout(ACCEPT_POLL_INTERVAL);
                }
            }
        }
    }

    fn dispatch(
        &self,
        stream: TcpStream,
        peer: SocketAddr,
        handler: &Arc<dyn Fn(TcpStream, SocketAddr) + Send + Sync>,
        on_refused: &dyn Fn(SocketAddr, Refusal),
    ) {
        if !self.is_peer_allowed(peer) {
            let _ = stream.shutdown(Shutdown::Both);
            on_refused(peer, Refusal::PeerNotAllowed);
            return;
        }
        if self.state.is_connected() {
            let _ = stream.shutdown(Shutdown::Both);
            on_refused(peer, Refusal::AlreadyConnected);
            return;
        }
        if stream.set_nonblocking(false).is_err() {
            let _ = stream.shutdown(Shutdown::Both);
            on_refused(peer, Refusal::PeerNotAllowed);
            return;
        }

        self.state.set(Some(peer));
        if let Some(observer) = &self.observer {
            observer.on_connection_changed(true, Some(peer));
        }

        let handler = Arc::clone(handler);
        let state = Arc::clone(&self.state);
        let observer = self.observer.clone();
        let spawned = std::thread::Builder::new()
            .name("quickvib-device-link".to_owned())
            .spawn(move || {
                handler(stream, peer);
                state.set(None);
                if let Some(observer) = observer {
                    observer.on_connection_changed(false, Some(peer));
                }
            });

        if spawned.is_err() {
            self.state.set(None);
            if let Some(observer) = &self.observer {
                observer.on_connection_changed(false, Some(peer));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // These tests bound their own polling loops, which is the one legitimate use of real
    // time in the suite; the product code all goes through the injected `Clock`.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)]

    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct RecordingObserver {
        transitions: Mutex<Vec<bool>>,
    }

    impl ConnectionObserver for RecordingObserver {
        fn on_connection_changed(&self, connected: bool, _peer: Option<SocketAddr>) {
            self.transitions
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(connected);
        }
    }

    /// Drain the stream to EOF, counting bytes.
    fn draining_handler(
        counter: Arc<AtomicUsize>,
    ) -> Arc<dyn Fn(TcpStream, SocketAddr) + Send + Sync> {
        Arc::new(move |mut stream: TcpStream, _peer| {
            let mut buf = [0u8; 1024];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        counter.fetch_add(n, Ordering::SeqCst);
                    }
                }
            }
        })
    }

    fn spawn_server(
        server: InboundDeviceServer,
        cancel: CancelToken,
        handler: Arc<dyn Fn(TcpStream, SocketAddr) + Send + Sync>,
        refusals: Arc<AtomicUsize>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            server.run(&cancel, handler, &|_peer, _why| {
                refusals.fetch_add(1, Ordering::SeqCst);
            });
        })
    }

    #[test]
    fn accepts_a_connection_and_reports_state() {
        let observer = Arc::new(RecordingObserver {
            transitions: Mutex::new(Vec::new()),
        });
        let server = InboundDeviceServer::bind("127.0.0.1:0")
            .unwrap()
            .with_observer(Arc::clone(&observer) as Arc<dyn ConnectionObserver>);
        let addr = server.local_addr().unwrap();
        let state = server.state();
        assert!(!state.is_connected());

        let cancel = CancelToken::new();
        let bytes = Arc::new(AtomicUsize::new(0));
        let refusals = Arc::new(AtomicUsize::new(0));
        let join = spawn_server(
            server,
            cancel.clone(),
            draining_handler(Arc::clone(&bytes)),
            Arc::clone(&refusals),
        );

        let mut client = TcpStream::connect(addr).unwrap();
        assert!(state.wait_connected(Duration::from_secs(5)));
        assert_eq!(state.peer().map(|p| p.ip()), Some(addr.ip()));

        client.write_all(&[1, 2, 3, 4]).unwrap();
        client.shutdown(Shutdown::Both).unwrap();
        drop(client);

        assert!(!state.wait_disconnected(Duration::from_secs(5)));
        assert_eq!(bytes.load(Ordering::SeqCst), 4);
        assert_eq!(refusals.load(Ordering::SeqCst), 0);

        cancel.cancel();
        join.join().unwrap();

        // The handler thread publishes the disconnect after clearing the state, so give it a
        // bounded moment to be seen rather than racing it.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let transitions = observer.transitions.lock().unwrap().clone();
            if transitions.len() == 2 || std::time::Instant::now() >= deadline {
                assert_eq!(transitions, vec![true, false]);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn second_connection_is_refused_while_one_is_live() {
        let server = InboundDeviceServer::bind("127.0.0.1:0").unwrap();
        let addr = server.local_addr().unwrap();
        let state = server.state();

        let cancel = CancelToken::new();
        let bytes = Arc::new(AtomicUsize::new(0));
        let refusals = Arc::new(AtomicUsize::new(0));
        let join = spawn_server(
            server,
            cancel.clone(),
            draining_handler(Arc::clone(&bytes)),
            Arc::clone(&refusals),
        );

        let first = TcpStream::connect(addr).unwrap();
        assert!(state.wait_connected(Duration::from_secs(5)));

        let mut second = TcpStream::connect(addr).unwrap();
        let mut buf = [0u8; 4];
        // The refused peer sees an immediate clean close rather than a hung socket.
        let read = second.read(&mut buf);
        assert!(matches!(read, Ok(0) | Err(_)), "{read:?}");

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while refusals.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(refusals.load(Ordering::SeqCst), 1);
        assert!(state.is_connected(), "the first link must be undisturbed");

        drop(first);
        cancel.cancel();
        join.join().unwrap();
    }

    #[test]
    fn peer_allow_list_filters_connections() {
        let server = InboundDeviceServer::bind("127.0.0.1:0")
            .unwrap()
            .with_allowed_peers(vec!["10.0.0.1".to_owned()]);
        let addr = server.local_addr().unwrap();
        let state = server.state();

        let cancel = CancelToken::new();
        let refusals = Arc::new(AtomicUsize::new(0));
        let join = spawn_server(
            server,
            cancel.clone(),
            draining_handler(Arc::new(AtomicUsize::new(0))),
            Arc::clone(&refusals),
        );

        let _client = TcpStream::connect(addr).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while refusals.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(refusals.load(Ordering::SeqCst), 1);
        assert!(!state.is_connected());

        cancel.cancel();
        join.join().unwrap();
    }

    #[test]
    fn empty_allow_list_accepts_any_peer() {
        let server = InboundDeviceServer::bind("127.0.0.1:0").unwrap();
        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        assert!(server.is_peer_allowed(peer));
    }

    #[test]
    fn allow_list_matches_ip_or_full_address() {
        let peer: SocketAddr = "127.0.0.1:41234".parse().unwrap();
        let by_ip = InboundDeviceServer::bind("127.0.0.1:0")
            .unwrap()
            .with_allowed_peers(vec!["127.0.0.1".to_owned()]);
        assert!(by_ip.is_peer_allowed(peer));

        let by_full = InboundDeviceServer::bind("127.0.0.1:0")
            .unwrap()
            .with_allowed_peers(vec!["127.0.0.1:41234".to_owned()]);
        assert!(by_full.is_peer_allowed(peer));
        let other_port: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        assert!(!by_full.is_peer_allowed(other_port));
    }

    #[test]
    fn run_returns_promptly_when_cancelled() {
        let server = InboundDeviceServer::bind("127.0.0.1:0").unwrap();
        let cancel = CancelToken::new();
        let join = spawn_server(
            server,
            cancel.clone(),
            draining_handler(Arc::new(AtomicUsize::new(0))),
            Arc::new(AtomicUsize::new(0)),
        );
        cancel.cancel();
        join.join().unwrap();
    }

    #[test]
    fn wait_connected_times_out_when_nobody_dials_in() {
        let state = ConnectionState::new();
        assert!(!state.wait_connected(Duration::from_millis(20)));
    }
}
