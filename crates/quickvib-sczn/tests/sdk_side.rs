//! The pair, from the other end: a minimal SCZN **host** driving `m300-device-sim`.
//!
//! On a bench that host is the vendor's `m300_sdk.dll`, which owns the listening socket
//! (`docs/M300-NATIVE.md` §6) and is a Windows binary we neither ship nor stub (D21). So this
//! file is the smallest thing that behaves like it on the wire: bind a port, accept the device
//! that dials in, send `0x00`, read the `0x04` blocks that follow, send `0x02`.
//!
//! That is enough to prove the protocol end to end without the DLL — what it deliberately does
//! not prove is that the DLL agrees with our reading of the specification. The two places it
//! might not are called out in `docs/SCZN-PROTOCOL.md` §7 and §4, and only a bench run settles
//! them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Child, Command as OsCommand, Stdio};
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, SystemClock};
use quickvib_sczn::cli::SimOptions;
use quickvib_sczn::crc;
use quickvib_sczn::device::{
    id_list_body, parse_device_status_reply, parse_param_list_reply, parse_read_params_reply,
    write_params_body,
};
use quickvib_sczn::packet::{Command, Decoder, Packet, RESULT_OK};
use quickvib_sczn::params::{HardwareInfo, ParamId, StatusId};
use quickvib_sczn::upload::{DataUpload, Endianness, Waveform};
use quickvib_sczn::{CrcMode, DataType, RunSummary, StopReason};

/// How long a test will wait for something the device has to do.
const PATIENCE: Duration = Duration::from_secs(15);

/// A test rate slow enough to be gentle and fast enough that blocks arrive in milliseconds:
/// 200 samples at 20 kS/s is a block every 10 ms. It is also on the device's rate ladder, so
/// the parameter read-back is exact.
const RATE_HZ: f64 = 20_000.0;

/// Samples per block for the tests.
const BLOCK: usize = 200;

/// A stand-in for the SDK's listener.
struct Host {
    /// The bound socket the device dials.
    listener: TcpListener,
}

impl Host {
    /// Bind an ephemeral loopback port.
    fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        Self { listener }
    }

    /// The port to point a simulator at.
    fn port(&self) -> u16 {
        self.listener.local_addr().unwrap().port()
    }

    /// Options that dial this host, with the test's rate and block size.
    fn options(&self) -> SimOptions {
        SimOptions {
            host: "127.0.0.1".to_owned(),
            port: self.port(),
            rate_hz: RATE_HZ,
            amplitude: 2.0,
            frequency_hz: 50.0,
            block_samples: BLOCK,
            retry_seconds: 0.02,
            once: true,
            ..SimOptions::default()
        }
    }

    /// Accept the device's connection.
    fn accept(&self) -> Link {
        self.listener
            .set_nonblocking(false)
            .expect("a blocking accept");
        let (stream, _) = self.listener.accept().expect("the device must dial in");
        Link::new(stream)
    }
}

/// One accepted device link, with a framing reader over it.
struct Link {
    /// The accepted socket.
    stream: TcpStream,
    /// Frame reassembly.
    decoder: Decoder,
    /// Raw bytes as they arrive, kept so a test can assert on the trailer.
    raw: Vec<u8>,
}

impl Link {
    /// Wrap an accepted socket.
    fn new(stream: TcpStream) -> Self {
        stream.set_nodelay(true).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        Self {
            stream,
            decoder: Decoder::new(),
            raw: Vec::new(),
        }
    }

    /// Send one command, checksummed the way the specification's C table says.
    fn send(&mut self, command: Command, command_id: u16, payload: Vec<u8>) {
        let packet = Packet::new(command, command_id, payload);
        self.stream
            .write_all(&packet.encode(CrcMode::Standard))
            .unwrap();
        self.stream.flush().unwrap();
    }

    /// The next frame of any kind, or a panic once [`PATIENCE`] runs out.
    fn recv(&mut self) -> Packet {
        let clock = SystemClock::new();
        let deadline = clock.monotonic() + PATIENCE;
        let mut buffer = [0u8; 8192];
        loop {
            if let Some(packet) = self.decoder.next_packet().expect("a well-formed frame") {
                return packet;
            }
            assert!(
                clock.monotonic() < deadline,
                "timed out waiting for a frame"
            );
            match self.stream.read(&mut buffer) {
                Ok(0) => panic!("the device hung up"),
                Ok(read) => {
                    self.raw.extend_from_slice(&buffer[..read]);
                    self.decoder.push(&buffer[..read]);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(error) => panic!("read failed: {error}"),
            }
        }
    }

    /// The next frame carrying `command`, skipping sample blocks and anything else in flight.
    fn recv_command(&mut self, command: Command) -> Packet {
        let clock = SystemClock::new();
        let deadline = clock.monotonic() + PATIENCE;
        loop {
            let packet = self.recv();
            if packet.command == command {
                return packet;
            }
            assert!(
                clock.monotonic() < deadline,
                "timed out waiting for {command}"
            );
        }
    }

    /// The samples from the next `0x04` block.
    fn recv_block(&mut self) -> DataUpload {
        let packet = self.recv_command(Command::DATA_UPLOAD);
        DataUpload::decode(&packet.payload, Endianness::Big).expect("a decodable upload")
    }

    /// Whether anything at all arrives in `window`. Used to prove a stop really stopped.
    fn quiet_for(&mut self, window: Duration) -> bool {
        let clock = SystemClock::new();
        let deadline = clock.monotonic() + window;
        let mut buffer = [0u8; 8192];
        while clock.monotonic() < deadline {
            match self.stream.read(&mut buffer) {
                Ok(0) => return true,
                Ok(read) => {
                    self.decoder.push(&buffer[..read]);
                    if self
                        .decoder
                        .next_packet()
                        .is_ok_and(|found| found.is_some())
                    {
                        return false;
                    }
                }
                Err(_) => {}
            }
        }
        true
    }
}

/// Run the simulator on a thread and hand back a handle that stops it on drop.
struct Simulator {
    /// Fires when the handle is dropped.
    cancel: CancelToken,
    /// The runner thread.
    thread: Option<std::thread::JoinHandle<RunSummary>>,
}

impl Simulator {
    /// Start a simulator with these options.
    fn start(options: SimOptions) -> Self {
        let cancel = CancelToken::new();
        let token = cancel.clone();
        let thread = std::thread::spawn(move || quickvib_sczn::run(&options, &token));
        Self {
            cancel,
            thread: Some(thread),
        }
    }

    /// Stop it and collect what it did.
    fn finish(mut self) -> RunSummary {
        self.cancel.cancel();
        self.thread.take().expect("one join").join().unwrap()
    }
}

impl Drop for Simulator {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[test]
fn the_device_dials_in_starts_on_command_and_streams_the_tone() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    link.send(Command::START_ACQUISITION, 0x1002, Vec::new());
    let reply = link.recv_command(Command::START_ACQUISITION_REPLY);
    assert_eq!(reply.command_id, 0x1002, "the reply echoes the request id");
    assert_eq!(reply.payload, RESULT_OK.to_be_bytes());

    // The blocks that follow are the tone, continuous across the block boundary.
    let waveform = Waveform::new(RATE_HZ, 2.0, 50.0);
    let mut received = Vec::new();
    for _ in 0..3 {
        let block = link.recv_block();
        assert_eq!(block.data_type, u32::from(DataType::Velocity.code()));
        assert_eq!(block.samples.len(), BLOCK);
        received.extend(block.samples);
    }
    assert_eq!(received, waveform.samples(0, 3 * BLOCK));

    link.send(Command::STOP_ACQUISITION, 0x1004, Vec::new());
    let reply = link.recv_command(Command::STOP_ACQUISITION_REPLY);
    assert_eq!(reply.payload, RESULT_OK.to_be_bytes());
    assert!(
        link.quiet_for(Duration::from_millis(200)),
        "nothing may follow a stop"
    );

    let summary = simulator.finish();
    assert_eq!(summary.sessions, 1);
    assert!(summary.samples_sent >= 3 * BLOCK as u64);
    assert_eq!(summary.requests_handled, 2);
}

#[test]
fn a_device_that_was_never_started_sends_nothing() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    assert!(
        link.quiet_for(Duration::from_millis(300)),
        "samples must wait for 0x00"
    );

    let summary = simulator.finish();
    assert_eq!(summary.blocks_sent, 0);
    assert_eq!(summary.samples_sent, 0);
}

#[test]
fn the_host_configures_the_device_and_reads_the_configuration_back() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    // 10 kS/s displacement, the way the SDK's setters would leave it.
    link.send(
        Command::WRITE_PARAMS,
        0x0001,
        write_params_body(&[
            (ParamId::SAMPLE_RATE, vec![0x02]),
            (ParamId::DATA_TYPE, vec![DataType::Displacement.code()]),
            (ParamId::LOW_PASS, vec![0x05]),
        ]),
    );
    let written = link.recv_command(Command::WRITE_PARAMS_REPLY);
    assert_eq!(written.payload, RESULT_OK.to_be_bytes());

    link.send(
        Command::READ_PARAMS,
        0x0002,
        id_list_body(&[ParamId::SAMPLE_RATE.0, ParamId::DATA_TYPE.0]),
    );
    let read = link.recv_command(Command::READ_PARAMS_REPLY);
    let (result, entries) = parse_read_params_reply(&read.payload).unwrap();
    assert_eq!(result, RESULT_OK);
    assert_eq!(entries[0].value, vec![0x02]);
    assert_eq!(entries[1].value, vec![DataType::Displacement.code()]);

    // And the blocks that follow carry the type that was just written.
    link.send(Command::START_ACQUISITION, 0x0003, Vec::new());
    link.recv_command(Command::START_ACQUISITION_REPLY);
    let block = link.recv_block();
    assert_eq!(block.data_type, u32::from(DataType::Displacement.code()));

    drop(simulator);
}

#[test]
fn the_host_can_enumerate_parameters_and_read_the_hardware_blob() {
    let host = Host::bind();
    let options = SimOptions {
        serial: "BENCH-0042".to_owned(),
        ..host.options()
    };
    let simulator = Simulator::start(options);
    let mut link = host.accept();

    link.send(Command::PARAM_LIST, 0x0001, Vec::new());
    let listed = link.recv_command(Command::PARAM_LIST_REPLY);
    let (result, ids) = parse_param_list_reply(&listed.payload).unwrap();
    assert_eq!(result, RESULT_OK);
    assert!(ids.contains(&ParamId::HARDWARE_INFO));

    link.send(
        Command::READ_PARAMS,
        0x0002,
        id_list_body(&[ParamId::HARDWARE_INFO.0]),
    );
    let read = link.recv_command(Command::READ_PARAMS_REPLY);
    let (_, entries) = parse_read_params_reply(&read.payload).unwrap();
    let info = HardwareInfo::decode(&entries[0].value).unwrap();
    assert_eq!(info.device_type, HardwareInfo::DEVICE_TYPE_M300);
    assert_eq!(info.serial_text(), "BENCH-0042");
    assert_eq!(info.server_port, host.port());

    drop(simulator);
}

#[test]
fn device_status_tracks_the_acquisition_the_host_asked_for() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();
    let query = id_list_body(&[StatusId::RUNNING_STATE.0]);

    link.send(Command::DEVICE_STATUS, 0x0001, query.clone());
    let idle = link.recv_command(Command::DEVICE_STATUS_REPLY);
    let (_, entries) = parse_device_status_reply(&idle.payload).unwrap();
    assert_eq!(entries[0].1, vec![0x00], "idle");

    link.send(Command::START_ACQUISITION, 0x0002, Vec::new());
    link.recv_command(Command::START_ACQUISITION_REPLY);
    link.send(Command::DEVICE_STATUS, 0x0003, query);
    let busy = link.recv_command(Command::DEVICE_STATUS_REPLY);
    let (_, entries) = parse_device_status_reply(&busy.payload).unwrap();
    assert_eq!(entries[0].1, vec![0x01], "acquiring");

    drop(simulator);
}

#[test]
fn dc_removal_is_answered_and_a_firmware_upgrade_is_refused() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    link.send(Command::REMOVE_DC, 0x0001, Vec::new());
    let removed = link.recv_command(Command::REMOVE_DC_REPLY);
    assert_eq!(removed.payload, RESULT_OK.to_be_bytes());

    link.send(Command::UPGRADE_START, 0x0002, Vec::new());
    let refused = link.recv_command(Command::UPGRADE_START_REPLY);
    assert_ne!(
        refused.payload,
        RESULT_OK.to_be_bytes(),
        "there is no firmware here to replace"
    );

    // The link is still usable after the refusal — that is the point of refusing in the
    // documented shape rather than dropping the connection.
    link.send(Command::START_ACQUISITION, 0x0003, Vec::new());
    link.recv_command(Command::START_ACQUISITION_REPLY);
    assert_eq!(link.recv_block().samples.len(), BLOCK);

    drop(simulator);
}

#[test]
fn an_unknown_command_is_ignored_without_dropping_the_link() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    link.send(Command(0x55), 0x0001, vec![1, 2, 3]);
    link.send(Command::PARAM_LIST, 0x0002, Vec::new());
    let listed = link.recv_command(Command::PARAM_LIST_REPLY);
    assert_eq!(
        listed.command_id, 0x0002,
        "the reply belongs to the command that was understood"
    );

    drop(simulator);
}

#[test]
fn frames_the_device_sends_carry_a_standard_crc32_by_default() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    link.send(Command::START_ACQUISITION, 0x0001, Vec::new());
    let reply = link.recv_command(Command::START_ACQUISITION_REPLY);

    // Re-encode and compare: the decoder accepts three checksum variants on purpose, so only
    // rebuilding the frame proves which one actually went out.
    let expected = reply.encode(CrcMode::Standard);
    assert!(
        link.raw.windows(expected.len()).any(|w| w == expected),
        "the reply on the wire is not the standard-CRC encoding of the reply we decoded"
    );
    let body = &expected[..expected.len() - 4];
    assert_eq!(
        u32::from_be_bytes([
            expected[expected.len() - 4],
            expected[expected.len() - 3],
            expected[expected.len() - 2],
            expected[expected.len() - 1],
        ]),
        crc::crc32(body)
    );

    drop(simulator);
}

#[test]
fn the_device_accepts_a_command_whose_checksum_the_specification_lets_a_host_skip() {
    let host = Host::bind();
    let simulator = Simulator::start(host.options());
    let mut link = host.accept();

    // A host short of compute may send zero instead of a checksum; the device must not stall.
    let packet = Packet::new(Command::START_ACQUISITION, 0x0001, Vec::new());
    link.stream
        .write_all(&packet.encode(CrcMode::Zero))
        .unwrap();
    link.stream.flush().unwrap();
    let reply = link.recv_command(Command::START_ACQUISITION_REPLY);
    assert_eq!(reply.payload, RESULT_OK.to_be_bytes());

    drop(simulator);
}

#[test]
fn the_device_redials_after_the_link_drops() {
    let host = Host::bind();
    let simulator = Simulator::start(SimOptions {
        once: false,
        ..host.options()
    });

    // First session: accept it, then hang up as a restarting host would.
    let first = host.accept();
    first.stream.shutdown(Shutdown::Both).unwrap();
    drop(first);

    // Second session: the device came back on its own.
    let mut second = host.accept();
    second.send(Command::START_ACQUISITION, 0x0001, Vec::new());
    let reply = second.recv_command(Command::START_ACQUISITION_REPLY);
    assert_eq!(reply.payload, RESULT_OK.to_be_bytes());

    let summary = simulator.finish();
    assert!(summary.sessions >= 2, "sessions: {}", summary.sessions);
    assert_eq!(summary.stop, Some(StopReason::Cancelled));
}

#[test]
fn an_acquisition_does_not_survive_the_link_that_started_it() {
    let host = Host::bind();
    let simulator = Simulator::start(SimOptions {
        once: false,
        ..host.options()
    });

    let mut first = host.accept();
    first.send(Command::START_ACQUISITION, 0x0001, Vec::new());
    first.recv_command(Command::START_ACQUISITION_REPLY);
    first.recv_block();
    first.stream.shutdown(Shutdown::Both).unwrap();
    drop(first);

    // A device that remembered "acquiring" across a reconnect would start pushing samples at a
    // host that never asked for them.
    let mut second = host.accept();
    assert!(
        second.quiet_for(Duration::from_millis(300)),
        "the new link must start idle"
    );

    drop(simulator);
}

/// The simulator built alongside this test.
fn device_sim() -> OsCommand {
    OsCommand::new(env!("CARGO_BIN_EXE_m300-device-sim"))
}

/// Kills the child when the test ends, however it ends.
struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_binary_prints_help_and_version_and_exits_zero() {
    for flag in ["--help", "--version"] {
        let output = device_sim().arg(flag).output().unwrap();
        assert!(output.status.success(), "{flag} exited {:?}", output.status);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.starts_with("m300-device-sim "),
            "{flag} printed: {text}"
        );
    }
}

#[test]
fn a_bad_argument_exits_two_with_usage_on_stderr() {
    let output = device_sim().arg("--frobnicate").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let text = String::from_utf8(output.stderr).unwrap();
    assert!(text.contains("unknown option"), "{text}");
    assert!(text.contains("USAGE"), "{text}");
}

#[test]
fn a_port_nobody_is_listening_on_exits_three() {
    let squatter = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = squatter.local_addr().unwrap().port();
    drop(squatter);

    let output = device_sim()
        .args(["--host", "127.0.0.1"])
        .args(["--port", &port.to_string()])
        .arg("--once")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8(output.stderr)
        .unwrap()
        .contains("could not connect"));
}

#[test]
fn the_documented_pair_runs_as_two_processes() {
    let host = Host::bind();
    let child = Running(
        device_sim()
            .args(["--host", "127.0.0.1"])
            .args(["--port", &host.port().to_string()])
            .args(["--rate", &RATE_HZ.to_string()])
            .args(["--amplitude", "2"])
            .args(["--frequency", "50"])
            .args(["--data-type", "acceleration"])
            .args(["--sn", "PAIR-01"])
            .args(["--block", &BLOCK.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let mut link = host.accept();
    link.send(
        Command::READ_PARAMS,
        0x0001,
        id_list_body(&[ParamId::HARDWARE_INFO.0]),
    );
    let read = link.recv_command(Command::READ_PARAMS_REPLY);
    let (_, entries) = parse_read_params_reply(&read.payload).unwrap();
    assert_eq!(
        HardwareInfo::decode(&entries[0].value)
            .unwrap()
            .serial_text(),
        "PAIR-01"
    );

    link.send(Command::START_ACQUISITION, 0x0002, Vec::new());
    link.recv_command(Command::START_ACQUISITION_REPLY);

    let waveform = Waveform::new(RATE_HZ, 2.0, 50.0);
    let block = link.recv_block();
    assert_eq!(block.data_type, u32::from(DataType::Acceleration.code()));
    assert_eq!(block.samples, waveform.samples(0, BLOCK));

    link.send(Command::STOP_ACQUISITION, 0x0003, Vec::new());
    link.recv_command(Command::STOP_ACQUISITION_REPLY);
    drop(child);
}
