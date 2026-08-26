//! Unit-level regression coverage for the per-session socket write deadline.

#![allow(clippy::disallowed_methods, clippy::expect_used, clippy::unwrap_used)]

use super::*;
use std::net::IpAddr;
use std::sync::Arc;

use quickvib_core::{Clock, NullLogger, SampleUnit, TestClock};
use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend};
use quickvib_engine::EngineConfig;

fn engine() -> Arc<Engine> {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let mut backend = MockBackend::new(Arc::clone(&clock));
    backend
        .open(&DeviceOpenOptions::new(
            1000.0,
            SampleUnit::VelocityUmPerSec,
        ))
        .unwrap();

    Engine::new(EngineConfig {
        clock,
        logger: Arc::new(NullLogger),
        backend: Box::new(backend),
        last_project: None,
        version: "write-timeout-test".to_owned(),
    })
}

#[test]
fn every_accepted_session_socket_has_a_write_timeout() {
    let logger: Arc<dyn Logger> = Arc::new(NullLogger);
    let server = ScpiServer::bind("127.0.0.1:0", engine(), logger).unwrap();
    let address = server.local_addr().unwrap();
    assert_eq!(address.ip(), IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));

    let client = TcpStream::connect(address).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let (stream, peer) = loop {
        match server.listener.accept() {
            Ok(accepted) => break accepted,
            Err(error)
                if error.kind() == ErrorKind::WouldBlock
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(error) => panic!("session was not accepted: {error}"),
        }
    };

    // Socket options apply to the socket itself, so a clone lets the test observe what the
    // session setup does after ownership of the original stream moves into the worker.
    let probe = stream.try_clone().unwrap();
    server.accept(stream, peer);

    let timeout = loop {
        if let Some(timeout) = probe.write_timeout().unwrap() {
            break timeout;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "accepted session retained an unbounded write timeout"
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(!timeout.is_zero(), "the write timeout must be positive");

    drop(client);
}
