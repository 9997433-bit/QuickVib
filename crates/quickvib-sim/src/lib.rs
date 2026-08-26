//! `m300-sim`: an inbound M300 stand-in (`docs/PLAN.md` 7.3, 15, D21).
//!
//! The M300 is the dialling party on the device link — QuickVib is the server on both sides —
//! so a stand-in for it is a TCP *client* that connects to `--device-port` and pushes raw
//! little-endian `f32` samples until it is told to stop. That is the entire wire protocol; it
//! is the same one [`quickvib_device::framer`] decodes, and there is no second one.
//!
//! This exists so a UTS integrator, or anyone without a Windows bench, can drive the real
//! record → measure → export path end to end. It is emphatically **not** a fake SDK: no DLL is
//! shipped or stubbed (D21), the simulator speaks only the socket, and the code it exercises
//! inside QuickVib is exactly the code a real vibrometer would exercise.
//!
//! ```
//! use quickvib_sim::Waveform;
//!
//! let wave = Waveform::new(1000.0, 250.0, 0.0);
//! // A 0 Hz "sine" is flat, which makes the encoding easy to eyeball.
//! assert_eq!(wave.encode(0, 1), 0.0_f32.to_le_bytes().to_vec());
//! ```

#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, SystemClock};

/// Default host to dial: QuickVib on the same machine.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Default device port, matching QuickVib's `--device-port`.
pub const DEFAULT_PORT: u16 = 9123;

/// Default sample rate, matching `samples/Test.proj`.
pub const DEFAULT_RATE_HZ: f64 = 100_000.0;

/// Default peak amplitude, matching the first mock component in `samples/Test.proj`.
pub const DEFAULT_AMPLITUDE: f64 = 250.0;

/// Default tone, matching the first mock component in `samples/Test.proj`.
pub const DEFAULT_FREQUENCY_HZ: f64 = 120.0;

/// Nominal time covered by one socket write. Small enough that QuickVib sees a steady trickle
/// rather than bursts, large enough that the syscall rate stays trivial even at 100 kS/s.
const CHUNK_SECONDS: f64 = 0.010;

/// Bytes per sample on the wire, as [`quickvib_device::framer`] expects them.
const BYTES_PER_SAMPLE: usize = 4;

/// A pure sine generator, indexed by absolute sample number so phase is continuous across
/// however the stream happens to be chunked.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waveform {
    /// Samples per second.
    pub sample_rate_hz: f64,
    /// Peak amplitude, in whatever unit the project declares. No conversion happens anywhere.
    pub amplitude: f64,
    /// Tone frequency in hertz. Zero produces a flat line.
    pub frequency_hz: f64,
}

impl Waveform {
    /// A waveform with these parameters.
    #[must_use]
    pub const fn new(sample_rate_hz: f64, amplitude: f64, frequency_hz: f64) -> Self {
        Self {
            sample_rate_hz,
            amplitude,
            frequency_hz,
        }
    }

    /// The sample at absolute index `index`.
    #[must_use]
    pub fn value_at(&self, index: u64) -> f32 {
        let rate = if self.sample_rate_hz > 0.0 {
            self.sample_rate_hz
        } else {
            1.0
        };
        let t = index as f64 / rate;
        (self.amplitude * (std::f64::consts::TAU * self.frequency_hz * t).sin()) as f32
    }

    /// `count` samples starting at `start`, already little-endian encoded.
    #[must_use]
    pub fn encode(&self, start: u64, count: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(count * BYTES_PER_SAMPLE);
        self.encode_into(start, count, &mut bytes);
        bytes
    }

    /// As [`Waveform::encode`], but into a buffer the caller reuses across chunks.
    pub fn encode_into(&self, start: u64, count: usize, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(count * BYTES_PER_SAMPLE);
        for i in 0..count as u64 {
            out.extend_from_slice(&self.value_at(start + i).to_le_bytes());
        }
    }
}

/// Fully resolved simulator options.
#[derive(Debug, Clone, PartialEq)]
pub struct SimOptions {
    /// Host QuickVib is listening on.
    pub host: String,
    /// Port QuickVib is listening on, i.e. its `--device-port`.
    pub port: u16,
    /// Samples per second to push.
    pub rate_hz: f64,
    /// Peak amplitude of the generated tone.
    pub amplitude: f64,
    /// Frequency of the generated tone.
    pub frequency_hz: f64,
    /// Stop after this many seconds. `None` streams until the peer hangs up.
    pub duration_seconds: Option<f64>,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
            rate_hz: DEFAULT_RATE_HZ,
            amplitude: DEFAULT_AMPLITUDE,
            frequency_hz: DEFAULT_FREQUENCY_HZ,
            duration_seconds: None,
        }
    }
}

impl SimOptions {
    /// The waveform these options describe.
    #[must_use]
    pub const fn waveform(&self) -> Waveform {
        Waveform::new(self.rate_hz, self.amplitude, self.frequency_hz)
    }

    /// Samples to send before stopping, or `None` for "until the peer hangs up".
    #[must_use]
    pub fn total_samples(&self) -> Option<u64> {
        self.duration_seconds
            .map(|seconds| (seconds * self.rate_hz).round().max(0.0) as u64)
    }

    /// Samples per socket write.
    #[must_use]
    pub fn chunk_samples(&self) -> usize {
        let nominal = (self.rate_hz * CHUNK_SECONDS).round();
        (nominal as usize).clamp(1, 8192)
    }
}

/// What the caller should do after parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum SimOutcome {
    /// Connect and stream with these options.
    Run(Box<SimOptions>),
    /// Print this text and exit `0`.
    Print(String),
}

/// Why the command line was rejected. Every variant exits `2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    /// A flag nobody recognises.
    UnknownFlag(String),
    /// A positional argument, which this program has none of.
    UnexpectedPositional(String),
    /// A flag that needs a value did not get one.
    MissingValue(&'static str),
    /// A flag's value could not be used.
    BadValue {
        /// The flag.
        flag: &'static str,
        /// What was supplied.
        value: String,
        /// Why it was rejected.
        reason: String,
    },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option '{flag}'"),
            Self::UnexpectedPositional(value) => write!(f, "unexpected argument '{value}'"),
            Self::MissingValue(flag) => write!(f, "option '{flag}' requires a value"),
            Self::BadValue {
                flag,
                value,
                reason,
            } => write!(f, "invalid value '{value}' for '{flag}': {reason}"),
        }
    }
}

impl std::error::Error for SimError {}

/// The `--help` text.
#[must_use]
pub fn usage() -> String {
    format!(
        "m300-sim {version} - inbound M300 device simulator for QuickVib

Dials QuickVib's device port and streams little-endian f32 samples, the way an M300
does. Pair it with `quickvib --backend tcp --device-port <n>`.

USAGE:
    m300-sim [OPTIONS]

OPTIONS:
    --host <host>       QuickVib's address              [default: {DEFAULT_HOST}]
    --port <n>          QuickVib's --device-port        [default: {DEFAULT_PORT}]
    --rate <hz>         Samples per second              [default: {DEFAULT_RATE_HZ}]
    --amplitude <a>     Peak amplitude of the tone      [default: {DEFAULT_AMPLITUDE}]
    --frequency <hz>    Tone frequency                  [default: {DEFAULT_FREQUENCY_HZ}]
    --duration <s>      Stop after this long            [default: until disconnected]
    --version           Print the version and exit
    --help              Print this help and exit

EXIT CODES:
    0  the peer closed, or --duration elapsed
    2  bad arguments
    3  could not connect
",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// The `--version` text.
#[must_use]
pub fn version() -> String {
    format!("m300-sim {}", env!("CARGO_PKG_VERSION"))
}

/// Parse the arguments *after* `argv[0]`.
///
/// Both `--flag value` and `--flag=value` are accepted, matching `quickvib`'s own parser.
///
/// # Errors
/// [`SimError`] for an unknown flag, a missing value, or a value out of range.
pub fn parse(arguments: &[OsString]) -> Result<SimOutcome, SimError> {
    let mut options = SimOptions::default();
    let mut index = 0;

    while index < arguments.len() {
        let raw = &arguments[index];
        index += 1;

        if raw == "--" {
            if let Some(extra) = arguments.get(index) {
                return Err(SimError::UnexpectedPositional(
                    extra.to_string_lossy().into_owned(),
                ));
            }
            break;
        }

        let text = raw.to_string_lossy();
        if !text.starts_with('-') {
            return Err(SimError::UnexpectedPositional(text.into_owned()));
        }

        let (flag, inline) = split_inline(&text);
        match flag.as_str() {
            "--help" | "-h" => return Ok(SimOutcome::Print(usage())),
            "--version" | "-V" => return Ok(SimOutcome::Print(version())),
            "--host" => {
                options.host = take_value("--host", inline, arguments, &mut index)?;
            }
            "--port" => {
                let value = take_value("--port", inline, arguments, &mut index)?;
                options.port = parse_port(&value)?;
            }
            "--rate" => {
                let value = take_value("--rate", inline, arguments, &mut index)?;
                options.rate_hz = parse_positive("--rate", &value)?;
            }
            "--amplitude" => {
                let value = take_value("--amplitude", inline, arguments, &mut index)?;
                options.amplitude = parse_finite("--amplitude", &value)?;
            }
            "--frequency" => {
                let value = take_value("--frequency", inline, arguments, &mut index)?;
                let frequency = parse_finite("--frequency", &value)?;
                if frequency < 0.0 {
                    return Err(SimError::BadValue {
                        flag: "--frequency",
                        value,
                        reason: "expected a non-negative number".to_owned(),
                    });
                }
                options.frequency_hz = frequency;
            }
            "--duration" => {
                let value = take_value("--duration", inline, arguments, &mut index)?;
                options.duration_seconds = Some(parse_positive("--duration", &value)?);
            }
            other => return Err(SimError::UnknownFlag(other.to_owned())),
        }
    }

    Ok(SimOutcome::Run(Box::new(options)))
}

fn split_inline(text: &str) -> (String, Option<String>) {
    match text.find('=') {
        Some(position) => (
            text[..position].to_owned(),
            Some(text[position + 1..].to_owned()),
        ),
        None => (text.to_owned(), None),
    }
}

fn take_value(
    flag: &'static str,
    inline: Option<String>,
    arguments: &[OsString],
    index: &mut usize,
) -> Result<String, SimError> {
    if let Some(value) = inline {
        if value.is_empty() {
            return Err(SimError::MissingValue(flag));
        }
        return Ok(value);
    }
    match arguments.get(*index) {
        Some(value) => {
            *index += 1;
            Ok(os_str_to_string(value))
        }
        None => Err(SimError::MissingValue(flag)),
    }
}

fn os_str_to_string(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

fn parse_port(value: &str) -> Result<u16, SimError> {
    let port: u32 = value.parse().map_err(|_| SimError::BadValue {
        flag: "--port",
        value: value.to_owned(),
        reason: "expected an integer".to_owned(),
    })?;
    if !(1..=65_535).contains(&port) {
        return Err(SimError::BadValue {
            flag: "--port",
            value: value.to_owned(),
            reason: "expected a port in 1..=65535".to_owned(),
        });
    }
    Ok(port as u16)
}

fn parse_finite(flag: &'static str, value: &str) -> Result<f64, SimError> {
    let parsed: f64 = value.parse().map_err(|_| SimError::BadValue {
        flag,
        value: value.to_owned(),
        reason: "expected a number".to_owned(),
    })?;
    if !parsed.is_finite() {
        return Err(SimError::BadValue {
            flag,
            value: value.to_owned(),
            reason: "expected a finite number".to_owned(),
        });
    }
    Ok(parsed)
}

fn parse_positive(flag: &'static str, value: &str) -> Result<f64, SimError> {
    let parsed = parse_finite(flag, value)?;
    if parsed <= 0.0 {
        return Err(SimError::BadValue {
            flag,
            value: value.to_owned(),
            reason: "expected a positive number".to_owned(),
        });
    }
    Ok(parsed)
}

/// Why the simulator stopped streaming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// `--duration` elapsed.
    DurationElapsed,
    /// QuickVib closed the link, which is the normal end of an open-ended run.
    PeerClosed,
    /// The cancellation token fired, e.g. the process was asked to shut down.
    Cancelled,
}

impl fmt::Display for StopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DurationElapsed => "durationElapsed",
            Self::PeerClosed => "peerClosed",
            Self::Cancelled => "cancelled",
        })
    }
}

/// What one simulator run did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimSummary {
    /// Samples handed to the socket.
    pub samples_sent: u64,
    /// Why the run ended.
    pub stop: StopReason,
}

/// Connect to QuickVib and stream until `cancel` fires, `--duration` elapses, or the peer
/// hangs up.
///
/// Pacing is against real time deliberately: the simulator stands in for hardware, so it is
/// the one component that must not be able to run faster than the instrument it imitates.
///
/// # Errors
/// Any connect failure. A write failure is not an error — it is how a peer disconnect is
/// observed, and it is reported as [`StopReason::PeerClosed`].
pub fn run(options: &SimOptions, cancel: &CancelToken) -> std::io::Result<SimSummary> {
    let mut stream = TcpStream::connect((options.host.as_str(), options.port))?;
    stream.set_nodelay(true)?;

    // Unblock a write parked in the kernel when the process is asked to stop.
    if let Ok(handle) = stream.try_clone() {
        cancel.on_cancel(move || {
            let _ = handle.shutdown(Shutdown::Both);
        });
    }

    let clock = SystemClock::new();
    let started_at = clock.monotonic();
    let waveform = options.waveform();
    let total = options.total_samples();
    let chunk = options.chunk_samples();

    let mut bytes = Vec::with_capacity(chunk * BYTES_PER_SAMPLE);
    let mut sent: u64 = 0;

    let stop = loop {
        if cancel.is_cancelled() {
            break StopReason::Cancelled;
        }
        let this_chunk = match total {
            Some(total) if sent >= total => break StopReason::DurationElapsed,
            Some(total) => chunk.min((total - sent) as usize),
            None => chunk,
        };

        waveform.encode_into(sent, this_chunk, &mut bytes);
        if stream
            .write_all(&bytes)
            .and_then(|()| stream.flush())
            .is_err()
        {
            break if cancel.is_cancelled() {
                StopReason::Cancelled
            } else {
                StopReason::PeerClosed
            };
        }
        sent += this_chunk as u64;

        // Sleep until the sample after the last one sent is nominally due, so the stream runs
        // at the requested rate however long the encode and the write took.
        let due = Duration::from_secs_f64(sent as f64 / options.rate_hz);
        let elapsed = clock.monotonic().saturating_sub(started_at);
        if let Some(remaining) = due.checked_sub(elapsed) {
            clock.sleep(remaining);
        }
    };

    let _ = stream.shutdown(Shutdown::Both);
    Ok(SimSummary {
        samples_sent: sent,
        stop,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::io::Read;
    use std::net::TcpListener;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn options(items: &[&str]) -> SimOptions {
        match parse(&args(items)).unwrap() {
            SimOutcome::Run(options) => *options,
            SimOutcome::Print(text) => panic!("expected options, got: {text}"),
        }
    }

    #[test]
    fn defaults_pair_with_the_sample_project() {
        let o = options(&[]);
        assert_eq!(o.host, "127.0.0.1");
        assert_eq!(o.port, 9123);
        assert_eq!(o.rate_hz, 100_000.0);
        assert_eq!(o.amplitude, 250.0);
        assert_eq!(o.frequency_hz, 120.0);
        assert_eq!(o.duration_seconds, None);
    }

    #[test]
    fn the_documented_invocation_parses() {
        let o = options(&[
            "--host",
            "127.0.0.1",
            "--port",
            "9123",
            "--rate",
            "1000",
            "--amplitude",
            "2",
            "--duration",
            "0.5",
        ]);
        assert_eq!(o.port, 9123);
        assert_eq!(o.rate_hz, 1000.0);
        assert_eq!(o.amplitude, 2.0);
        assert_eq!(o.duration_seconds, Some(0.5));
    }

    #[test]
    fn inline_and_separated_values_are_equivalent() {
        assert_eq!(options(&["--port=6000"]), options(&["--port", "6000"]));
        assert_eq!(options(&["--rate=2500"]), options(&["--rate", "2500"]));
    }

    #[test]
    fn help_and_version_short_circuit() {
        for flag in ["--help", "-h"] {
            assert!(matches!(parse(&args(&[flag])), Ok(SimOutcome::Print(_))));
        }
        match parse(&args(&["--version"])).unwrap() {
            SimOutcome::Print(text) => assert!(text.starts_with("m300-sim ")),
            SimOutcome::Run(_) => panic!("expected the version text"),
        }
    }

    #[test]
    fn usage_mentions_every_flag() {
        let text = usage();
        for flag in [
            "--host",
            "--port",
            "--rate",
            "--amplitude",
            "--frequency",
            "--duration",
            "--version",
            "--help",
        ] {
            assert!(text.contains(flag), "usage is missing {flag}");
        }
    }

    #[test]
    fn bad_values_are_rejected() {
        for arguments in [
            vec!["--port", "0"],
            vec!["--port", "70000"],
            vec!["--port", "http"],
            vec!["--rate", "0"],
            vec!["--rate", "-1"],
            vec!["--rate", "fast"],
            vec!["--duration", "0"],
            vec!["--amplitude", "nan"],
            vec!["--frequency", "-1"],
        ] {
            assert!(
                matches!(parse(&args(&arguments)), Err(SimError::BadValue { .. })),
                "for {arguments:?}"
            );
        }
    }

    #[test]
    fn missing_values_and_unknown_flags_are_rejected() {
        for flag in ["--host", "--port", "--rate", "--amplitude", "--duration"] {
            assert!(
                matches!(parse(&args(&[flag])), Err(SimError::MissingValue(_))),
                "for {flag}"
            );
        }
        assert_eq!(
            parse(&args(&["--frobnicate"])),
            Err(SimError::UnknownFlag("--frobnicate".to_owned()))
        );
        assert!(matches!(
            parse(&args(&["9123"])),
            Err(SimError::UnexpectedPositional(_))
        ));
    }

    #[test]
    fn a_bare_double_dash_terminates_parsing() {
        assert_eq!(options(&["--port", "7000", "--"]).port, 7000);
        assert!(matches!(
            parse(&args(&["--", "extra"])),
            Err(SimError::UnexpectedPositional(_))
        ));
    }

    #[test]
    fn total_samples_follows_the_duration_and_rate() {
        let mut o = SimOptions {
            rate_hz: 1000.0,
            duration_seconds: Some(0.05),
            ..SimOptions::default()
        };
        assert_eq!(o.total_samples(), Some(50));
        o.duration_seconds = None;
        assert_eq!(o.total_samples(), None);
    }

    #[test]
    fn chunks_stay_within_sane_bounds() {
        for (rate, expected) in [(1.0, 1_usize), (1000.0, 10), (100_000.0, 1000)] {
            let o = SimOptions {
                rate_hz: rate,
                ..SimOptions::default()
            };
            assert_eq!(o.chunk_samples(), expected, "at {rate} Hz");
        }
        let fast = SimOptions {
            rate_hz: 100_000_000.0,
            ..SimOptions::default()
        };
        assert_eq!(fast.chunk_samples(), 8192);
    }

    #[test]
    fn the_waveform_is_a_sine_of_the_requested_amplitude() {
        // A quarter period at 1 Hz / 4 S/s lands exactly on the peak.
        let wave = Waveform::new(4.0, 3.0, 1.0);
        assert!((wave.value_at(1) - 3.0).abs() < 1e-5);
        assert!((wave.value_at(3) + 3.0).abs() < 1e-5);
        assert!(wave.value_at(0).abs() < 1e-5);
    }

    #[test]
    fn the_waveform_encodes_little_endian_f32() {
        let wave = Waveform::new(4.0, 1.0, 1.0);
        let bytes = wave.encode(1, 1);
        assert_eq!(bytes.len(), 4);
        assert!((f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn phase_is_continuous_across_chunk_boundaries() {
        let wave = Waveform::new(1000.0, 1.0, 50.0);
        let whole = wave.encode(0, 40);
        let mut split = wave.encode(0, 17);
        split.extend_from_slice(&wave.encode(17, 23));
        assert_eq!(whole, split);
    }

    #[test]
    fn a_zero_rate_waveform_does_not_divide_by_zero() {
        let wave = Waveform::new(0.0, 1.0, 1.0);
        assert!(wave.value_at(7).is_finite());
    }

    #[test]
    fn a_bounded_run_sends_exactly_the_requested_samples() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let collector = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).unwrap();
            bytes
        });

        let options = SimOptions {
            host: addr.ip().to_string(),
            port: addr.port(),
            rate_hz: 10_000.0,
            amplitude: 2.0,
            frequency_hz: 50.0,
            duration_seconds: Some(0.02),
        };
        let summary = run(&options, &CancelToken::new()).unwrap();
        assert_eq!(summary.samples_sent, 200);
        assert_eq!(summary.stop, StopReason::DurationElapsed);

        let bytes = collector.join().unwrap();
        assert_eq!(bytes.len(), 200 * BYTES_PER_SAMPLE);
        let first = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        assert!(first.abs() < 1e-5, "the tone starts at zero crossing");
    }

    #[test]
    fn a_peer_that_hangs_up_ends_the_run_cleanly() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let closer = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(30));
            let _ = stream.shutdown(Shutdown::Both);
            drop(stream);
        });

        let options = SimOptions {
            host: addr.ip().to_string(),
            port: addr.port(),
            rate_hz: 20_000.0,
            duration_seconds: None,
            ..SimOptions::default()
        };
        let summary = run(&options, &CancelToken::new()).unwrap();
        closer.join().unwrap();
        assert_eq!(summary.stop, StopReason::PeerClosed);
    }

    #[test]
    fn cancellation_stops_an_open_ended_run() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let draining = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut sink = [0u8; 4096];
            while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
        });

        let cancel = CancelToken::new();
        let token = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            token.cancel();
        });

        let options = SimOptions {
            host: addr.ip().to_string(),
            port: addr.port(),
            rate_hz: 20_000.0,
            duration_seconds: None,
            ..SimOptions::default()
        };
        let summary = run(&options, &cancel).unwrap();
        assert_eq!(summary.stop, StopReason::Cancelled);
        assert!(summary.samples_sent > 0);
        draining.join().unwrap();
    }

    #[test]
    fn connecting_to_nothing_is_an_error_not_a_panic() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let options = SimOptions {
            host: addr.ip().to_string(),
            port: addr.port(),
            duration_seconds: Some(0.01),
            ..SimOptions::default()
        };
        assert!(run(&options, &CancelToken::new()).is_err());
    }
}
