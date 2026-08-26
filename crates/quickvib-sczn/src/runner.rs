//! The transport: dial the listener, serve SCZN, upload while acquiring, redial when dropped.
//!
//! The M300 is the client on this link — the specification is explicit that "the host is the
//! server, the device is the client, and the device connects on a timer until it succeeds"
//! — so this is a TCP client with a reconnect loop, not a listener. That matches
//! `docs/M300-NATIVE.md` §6: with `--backend m300` the vendor SDK owns the listening socket
//! and QuickVib never binds the device port at all.
//!
//! One thread, no async, no channels: the socket read timeout is set to however long is left
//! before the next block is due, so the loop blocks on the command stream and wakes up exactly
//! when it has samples to send.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, SystemClock};

use crate::cli::SimOptions;
use crate::device::Device;
use crate::packet::{encode_parts_into, Command, Decoder, VERSION};
use crate::params::{
    low_pass_code_for, nearest_sample_rate_code, HardwareInfo, ParamId, ParamStore,
};
use crate::upload::Waveform;

/// Longest the loop will sit in a single `read`. Keeps cancellation responsive when the device
/// is idle and nothing is arriving.
const MAX_POLL: Duration = Duration::from_millis(50);

/// Shortest read timeout. Zero means "block forever" to the sockets API, which is the one thing
/// this loop must never do.
const MIN_POLL: Duration = Duration::from_millis(1);

/// Read buffer size. Commands are tens of bytes; this is one page and never the bottleneck.
const READ_BUFFER: usize = 4096;

/// Why the run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `--duration` elapsed.
    DurationElapsed,
    /// The cancellation token fired, e.g. the process was asked to shut down.
    Cancelled,
    /// The link dropped and `--once` said not to redial.
    LinkClosed,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DurationElapsed => "durationElapsed",
            Self::Cancelled => "cancelled",
            Self::LinkClosed => "linkClosed",
        })
    }
}

/// What one run of the simulator did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSummary {
    /// Links established.
    pub sessions: u64,
    /// Dial attempts that failed.
    pub connect_failures: u64,
    /// Requests answered across every session.
    pub requests_handled: u64,
    /// `0x04` blocks sent.
    pub blocks_sent: u64,
    /// Samples across those blocks.
    pub samples_sent: u64,
    /// Why the run ended. `None` only before it has.
    pub stop: Option<StopReason>,
}

impl RunSummary {
    /// Whether the simulator ever got a link. The binary's exit code turns on this.
    #[must_use]
    pub const fn connected(&self) -> bool {
        self.sessions > 0
    }
}

/// The parameter set a device started with these options reports.
///
/// The sample rate and low-pass band are codes, not hertz: `--rate 100000` becomes rung `0x05`
/// and the matching 100 kHz filter band, because that pairing is the one rule the vendor
/// repeats in every document. A rate that is not on the ladder takes the nearest rung, and the
/// simulator still *paces* at the rate that was asked for — so a deliberate mismatch between
/// what the device says it is doing and what it is doing is reproducible on demand.
#[must_use]
pub fn params_for(options: &SimOptions) -> ParamStore {
    let mut params = ParamStore::default();
    let mut info = HardwareInfo::default();
    info.set_serial(&options.serial);
    info.server_port = options.port;

    let mut set = |id: ParamId, value: Vec<u8>| {
        let _ = params.set(id, &value);
    };
    set(
        ParamId::SAMPLE_RATE,
        vec![nearest_sample_rate_code(options.rate_hz)],
    );
    set(ParamId::LOW_PASS, vec![low_pass_code_for(options.rate_hz)]);
    set(ParamId::DATA_TYPE, vec![options.data_type.code()]);
    set(ParamId::HARDWARE_INFO, info.encode());
    params
}

/// Run until `cancel` fires or `--duration` elapses, redialling as a real device would.
///
/// Never returns an error: a listener that is not up yet is the normal state of affairs at boot,
/// and a device that gave up on it would be a worse stand-in than one that keeps trying. Dial
/// failures are counted in [`RunSummary::connect_failures`] and the caller decides what to make
/// of a run that never connected.
pub fn run(options: &SimOptions, cancel: &CancelToken) -> RunSummary {
    let clock = SystemClock::new();
    let started_at = clock.monotonic();
    let deadline = options
        .duration_seconds
        .map(|seconds| started_at + Duration::from_secs_f64(seconds));

    let mut summary = RunSummary::default();
    let retry = Duration::from_secs_f64(options.retry_seconds);

    loop {
        if cancel.is_cancelled() {
            summary.stop = Some(StopReason::Cancelled);
            break;
        }
        if deadline.is_some_and(|at| clock.monotonic() >= at) {
            summary.stop = Some(StopReason::DurationElapsed);
            break;
        }

        match TcpStream::connect((options.host.as_str(), options.port)) {
            Ok(stream) => {
                summary.sessions += 1;
                serve(stream, options, cancel, &clock, deadline, &mut summary);
                if options.once {
                    summary.stop.get_or_insert(StopReason::LinkClosed);
                }
            }
            Err(_) => {
                summary.connect_failures += 1;
                if options.once {
                    summary.stop = Some(StopReason::LinkClosed);
                }
            }
        }

        if summary.stop.is_some() {
            break;
        }
        // Wait out the retry interval, but wake immediately on cancellation rather than
        // leaving a `Ctrl-C` sitting in a sleep.
        if cancel.wait_timeout(retry) {
            summary.stop = Some(StopReason::Cancelled);
            break;
        }
    }

    summary
}

/// Serve one connection until it drops, the deadline passes, or cancellation fires.
fn serve(
    mut stream: TcpStream,
    options: &SimOptions,
    cancel: &CancelToken,
    clock: &SystemClock,
    deadline: Option<Duration>,
    summary: &mut RunSummary,
) {
    let _ = stream.set_nodelay(true);

    // Unblock a read or write parked in the kernel when the process is asked to stop.
    if let Ok(handle) = stream.try_clone() {
        cancel.on_cancel(move || {
            let _ = handle.shutdown(Shutdown::Both);
        });
    }

    let mut device =
        Device::with_params(params_for(options)).with_prefix_endianness(options.prefix_endian);
    let waveform = Waveform::new(options.rate_hz, options.amplitude, options.frequency_hz);
    let mut decoder = Decoder::new();

    let mut read_buffer = [0u8; READ_BUFFER];
    let mut samples = Vec::with_capacity(options.block_samples);
    let mut payload = Vec::with_capacity(options.block_samples * 4 + 8);
    let mut frame = Vec::with_capacity(payload.capacity() + 16);

    // Absolute sample index, reset at each start command so the tone always begins at a zero
    // crossing and two runs of the same script capture the same waveform.
    let mut emitted: u64 = 0;
    let mut acquisition_started_at = clock.monotonic();

    loop {
        if cancel.is_cancelled() || deadline.is_some_and(|at| clock.monotonic() >= at) {
            break;
        }

        let due = if device.is_acquiring() {
            acquisition_started_at + Duration::from_secs_f64(emitted as f64 / options.rate_hz)
        } else {
            clock.monotonic() + MAX_POLL
        };
        let wait = due
            .saturating_sub(clock.monotonic())
            .clamp(MIN_POLL, MAX_POLL);

        let _ = stream.set_read_timeout(Some(wait));
        match stream.read(&mut read_buffer) {
            Ok(0) => break,
            Ok(read) => decoder.push(&read_buffer[..read]),
            Err(error) if is_timeout(&error) => {}
            Err(_) => break,
        }

        // Drain everything that arrived before deciding what to send, so a stop command that
        // shared a read with something else takes effect before the next block goes out.
        let mut hung_up = false;
        loop {
            let request = match decoder.next_packet() {
                Ok(Some(request)) => request,
                Ok(None) => break,
                // A frame that fails its checksum is dropped. The protocol gives a device no
                // way to ask for a resend, so dropping it and carrying on is the only honest
                // behaviour; the decoder has already skipped past it.
                Err(_) => continue,
            };

            let before = device.is_acquiring();
            let Some(reply) = device.handle(&request) else {
                continue;
            };
            summary.requests_handled += 1;
            if !before && device.is_acquiring() {
                emitted = 0;
                acquisition_started_at = clock.monotonic();
            }

            frame.clear();
            reply.encode_into(options.crc, &mut frame);
            if write_frame(&mut stream, &frame).is_err() {
                hung_up = true;
                break;
            }
        }
        if hung_up {
            break;
        }

        if device.is_acquiring() && clock.monotonic() >= due {
            waveform.samples_into(emitted, options.block_samples, &mut samples);
            if device.upload_into(&samples, &mut payload).is_some() {
                frame.clear();
                encode_parts_into(
                    VERSION,
                    Command::DATA_UPLOAD,
                    device.take_upload_id(),
                    &payload,
                    options.crc,
                    &mut frame,
                );
                if write_frame(&mut stream, &frame).is_err() {
                    break;
                }
                emitted += samples.len() as u64;
                summary.blocks_sent += 1;
                summary.samples_sent += samples.len() as u64;
            }
        }
    }

    device.reset_link();
    let _ = stream.shutdown(Shutdown::Both);
}

/// Push one encoded frame out and flush it. A failure here is how a peer disconnect is
/// observed, not an error to report.
fn write_frame(stream: &mut TcpStream, frame: &[u8]) -> std::io::Result<()> {
    stream.write_all(frame)?;
    stream.flush()
}

/// Whether a read error is just the timeout expiring. Which errno that is depends on the
/// platform: Unix reports `WouldBlock`, Windows reports `TimedOut`.
fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::params::{DataType, SAMPLE_RATES_HZ};

    #[test]
    fn the_reported_rate_is_the_nearest_rung_with_its_matching_filter_band() {
        let options = SimOptions {
            rate_hz: 100_000.0,
            ..SimOptions::default()
        };
        let params = params_for(&options);
        assert_eq!(params.get_byte(ParamId::SAMPLE_RATE), Some(0x05));
        assert_eq!(params.get_byte(ParamId::LOW_PASS), Some(0x09));
        assert_eq!(params.sample_rate_hz(), Some(100_000.0));
    }

    #[test]
    fn a_rate_off_the_ladder_is_reported_as_the_nearest_one() {
        let options = SimOptions {
            rate_hz: 96_000.0,
            ..SimOptions::default()
        };
        let params = params_for(&options);
        assert_eq!(params.sample_rate_hz(), Some(SAMPLE_RATES_HZ[0x05]));
    }

    #[test]
    fn the_serial_and_data_type_reach_the_parameters_the_host_will_read() {
        let options = SimOptions {
            serial: "BENCH-0007".to_owned(),
            data_type: DataType::Acceleration,
            port: 9999,
            ..SimOptions::default()
        };
        let params = params_for(&options);
        assert_eq!(params.data_type(), DataType::Acceleration);
        let info = params.hardware_info().unwrap();
        assert_eq!(info.serial_text(), "BENCH-0007");
        assert_eq!(info.server_port, 9999);
        assert_eq!(info.device_type, HardwareInfo::DEVICE_TYPE_M300);
    }

    #[test]
    fn a_listener_that_never_comes_up_ends_the_run_without_a_session() {
        let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = squatter.local_addr().unwrap().port();
        drop(squatter);

        let options = SimOptions {
            host: "127.0.0.1".to_owned(),
            port,
            once: true,
            ..SimOptions::default()
        };
        let summary = run(&options, &CancelToken::new());
        assert!(!summary.connected());
        assert_eq!(summary.connect_failures, 1);
        assert_eq!(summary.stop, Some(StopReason::LinkClosed));
    }

    #[test]
    fn a_run_cancelled_before_it_starts_does_nothing() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let summary = run(&SimOptions::default(), &cancel);
        assert_eq!(summary.stop, Some(StopReason::Cancelled));
        assert_eq!(summary.sessions, 0);
        assert_eq!(summary.connect_failures, 0);
    }

    #[test]
    fn stop_reasons_have_stable_names_for_the_summary_line() {
        assert_eq!(StopReason::DurationElapsed.to_string(), "durationElapsed");
        assert_eq!(StopReason::Cancelled.to_string(), "cancelled");
        assert_eq!(StopReason::LinkClosed.to_string(), "linkClosed");
    }

    #[test]
    fn a_read_timeout_is_recognised_on_both_platforms_spellings() {
        assert!(is_timeout(&std::io::Error::from(ErrorKind::WouldBlock)));
        assert!(is_timeout(&std::io::Error::from(ErrorKind::TimedOut)));
        assert!(!is_timeout(&std::io::Error::from(ErrorKind::BrokenPipe)));
    }
}
