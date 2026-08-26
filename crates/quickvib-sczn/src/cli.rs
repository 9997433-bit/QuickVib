//! Command line for `m300-device-sim`.
//!
//! Same shape as `m300-sim`'s parser — `--flag value` and `--flag=value` both work, unknown
//! flags are an error rather than a warning — so someone who has used one is not surprised by
//! the other. The two programs are not interchangeable, though, and [`usage`] says so.

use std::ffi::{OsStr, OsString};
use std::fmt;

use crate::crc::CrcMode;
use crate::params::DataType;
use crate::upload::Endianness;

/// Default host to dial: the SDK's listener on the same machine.
pub const DEFAULT_HOST: &str = "127.0.0.1";

/// Default port, matching the vendor's own default and QuickVib's `--device-port`.
pub const DEFAULT_PORT: u16 = 9123;

/// Default sample rate, matching `samples/Test.proj`.
pub const DEFAULT_RATE_HZ: f64 = 100_000.0;

/// Default peak amplitude, matching the first mock component in `samples/Test.proj`.
pub const DEFAULT_AMPLITUDE: f64 = 250.0;

/// Default tone, matching the first mock component in `samples/Test.proj`.
pub const DEFAULT_FREQUENCY_HZ: f64 = 120.0;

/// Default samples per `0x04` block.
pub const DEFAULT_BLOCK_SAMPLES: usize = 4096;

/// Default serial number reported in the hardware-information blob.
pub const DEFAULT_SERIAL: &str = "SIM-000001";

/// Default seconds between dial attempts.
pub const DEFAULT_RETRY_SECONDS: f64 = 1.0;

/// Largest block the simulator will send, so `--block` cannot be used to build a frame the
/// codec would then refuse to decode.
pub const MAX_BLOCK_SAMPLES: usize = 262_144;

/// Fully resolved simulator options.
#[derive(Debug, Clone, PartialEq)]
pub struct SimOptions {
    /// Host the SCZN listener is on.
    pub host: String,
    /// Port it is listening on.
    pub port: u16,
    /// Samples per second. Rounded to the nearest rung of the device's rate ladder for the
    /// `sampleRate` parameter, but used verbatim for pacing.
    pub rate_hz: f64,
    /// Peak amplitude of the generated tone.
    pub amplitude: f64,
    /// Frequency of the generated tone.
    pub frequency_hz: f64,
    /// Which quantity the device claims to be uploading.
    pub data_type: DataType,
    /// Serial number in the hardware-information blob.
    pub serial: String,
    /// Samples per `0x04` block.
    pub block_samples: usize,
    /// Which checksum to write into the trailer.
    pub crc: CrcMode,
    /// Byte order for the upload payload's prefix words.
    pub prefix_endian: Endianness,
    /// Stop the whole run after this long. `None` runs until cancelled.
    pub duration_seconds: Option<f64>,
    /// Seconds between dial attempts.
    pub retry_seconds: f64,
    /// Exit when the link drops instead of redialling.
    pub once: bool,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_owned(),
            port: DEFAULT_PORT,
            rate_hz: DEFAULT_RATE_HZ,
            amplitude: DEFAULT_AMPLITUDE,
            frequency_hz: DEFAULT_FREQUENCY_HZ,
            data_type: DataType::Velocity,
            serial: DEFAULT_SERIAL.to_owned(),
            block_samples: DEFAULT_BLOCK_SAMPLES,
            crc: CrcMode::default(),
            prefix_endian: Endianness::default(),
            duration_seconds: None,
            retry_seconds: DEFAULT_RETRY_SECONDS,
            once: false,
        }
    }
}

/// What the caller should do after parsing.
#[derive(Debug, Clone, PartialEq)]
pub enum SimOutcome {
    /// Connect and serve with these options.
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
        "m300-device-sim {version} - an M300 that speaks the SCZN protocol

Dials an SCZN listener, answers its commands, and uploads 0x04 sample blocks while it
has been told to acquire. Pair it with `quickvib --backend m300` on Windows, where the
vendor SDK owns the listening socket.

This is NOT a replacement for `m300-sim`: that one pushes bare little-endian f32 samples
for `--backend tcp`, with no framing at all. Different wire, different backend.

USAGE:
    m300-device-sim [OPTIONS]

OPTIONS:
    --host <host>          Where the listener is                [default: {DEFAULT_HOST}]
    --port <n>             Port it is listening on              [default: {DEFAULT_PORT}]
    --rate <hz>            Samples per second                   [default: {DEFAULT_RATE_HZ}]
    --amplitude <a>        Peak amplitude of the tone           [default: {DEFAULT_AMPLITUDE}]
    --frequency <hz>       Tone frequency; 0 sends a flat line  [default: {DEFAULT_FREQUENCY_HZ}]
    --data-type <kind>     velocity | displacement | acceleration | iq
                                                                [default: velocity]
    --sn <serial>          Serial number, up to 10 characters   [default: {DEFAULT_SERIAL}]
    --block <n>            Samples per 0x04 upload              [default: {DEFAULT_BLOCK_SAMPLES}]
    --crc <mode>           standard | castagnoli | zero         [default: standard]
    --prefix-endian <e>    Byte order of the upload prefix: big | little
                                                                [default: big]
    --retry <s>            Seconds between dial attempts        [default: {DEFAULT_RETRY_SECONDS}]
    --once                 Exit when the link drops, instead of redialling
    --duration <s>         Stop the whole run after this long   [default: until interrupted]
    --version              Print the version and exit
    --help                 Print this help and exit

EXIT CODES:
    0  the run ended after at least one session
    2  bad arguments
    3  the run ended without ever connecting
",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// The `--version` text.
#[must_use]
pub fn version() -> String {
    format!("m300-device-sim {}", env!("CARGO_PKG_VERSION"))
}

/// Parse the arguments *after* `argv[0]`.
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
            "--host" => options.host = take_value("--host", inline, arguments, &mut index)?,
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
            "--data-type" => {
                let value = take_value("--data-type", inline, arguments, &mut index)?;
                options.data_type =
                    DataType::from_str_opt(&value).ok_or_else(|| SimError::BadValue {
                        flag: "--data-type",
                        value: value.clone(),
                        reason: "expected velocity, displacement, acceleration or iq".to_owned(),
                    })?;
            }
            "--sn" => {
                let value = take_value("--sn", inline, arguments, &mut index)?;
                if value.len() > 10 || !value.is_ascii() {
                    return Err(SimError::BadValue {
                        flag: "--sn",
                        value,
                        reason: "expected at most 10 ASCII characters".to_owned(),
                    });
                }
                options.serial = value;
            }
            "--block" => {
                let value = take_value("--block", inline, arguments, &mut index)?;
                options.block_samples = parse_block(&value)?;
            }
            "--crc" => {
                let value = take_value("--crc", inline, arguments, &mut index)?;
                options.crc = CrcMode::from_str_opt(&value).ok_or_else(|| SimError::BadValue {
                    flag: "--crc",
                    value: value.clone(),
                    reason: "expected standard, castagnoli or zero".to_owned(),
                })?;
            }
            "--prefix-endian" => {
                let value = take_value("--prefix-endian", inline, arguments, &mut index)?;
                options.prefix_endian =
                    Endianness::from_str_opt(&value).ok_or_else(|| SimError::BadValue {
                        flag: "--prefix-endian",
                        value: value.clone(),
                        reason: "expected big or little".to_owned(),
                    })?;
            }
            "--retry" => {
                let value = take_value("--retry", inline, arguments, &mut index)?;
                options.retry_seconds = parse_positive("--retry", &value)?;
            }
            "--once" => options.once = true,
            "--duration" => {
                let value = take_value("--duration", inline, arguments, &mut index)?;
                options.duration_seconds = Some(parse_positive("--duration", &value)?);
            }
            other => return Err(SimError::UnknownFlag(other.to_owned())),
        }
    }

    Ok(SimOutcome::Run(Box::new(options)))
}

/// Split `--flag=value` into its two halves.
fn split_inline(text: &str) -> (String, Option<String>) {
    match text.find('=') {
        Some(position) => (
            text[..position].to_owned(),
            Some(text[position + 1..].to_owned()),
        ),
        None => (text.to_owned(), None),
    }
}

/// The value for `flag`, whether it was written inline or as the next argument.
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

/// Lossy, because a port or a rate written in invalid UTF-8 is going to fail the next check
/// anyway and the error is more useful with the bytes in it.
fn os_str_to_string(value: &OsStr) -> String {
    value.to_string_lossy().into_owned()
}

/// A TCP port in `1..=65535`.
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

/// A block size in `1..=MAX_BLOCK_SAMPLES`.
fn parse_block(value: &str) -> Result<usize, SimError> {
    let samples: u64 = value.parse().map_err(|_| SimError::BadValue {
        flag: "--block",
        value: value.to_owned(),
        reason: "expected an integer".to_owned(),
    })?;
    if samples == 0 || samples > MAX_BLOCK_SAMPLES as u64 {
        return Err(SimError::BadValue {
            flag: "--block",
            value: value.to_owned(),
            reason: format!("expected 1..={MAX_BLOCK_SAMPLES} samples"),
        });
    }
    Ok(samples as usize)
}

/// A number that is neither NaN nor infinite.
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

/// A finite number greater than zero.
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

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
    fn defaults_pair_with_the_sample_project_and_the_vendors_own_port() {
        let o = options(&[]);
        assert_eq!(o.host, "127.0.0.1");
        assert_eq!(o.port, 9123);
        assert_eq!(o.rate_hz, 100_000.0);
        assert_eq!(o.amplitude, 250.0);
        assert_eq!(o.frequency_hz, 120.0);
        assert_eq!(o.data_type, DataType::Velocity);
        assert_eq!(o.serial, "SIM-000001");
        assert_eq!(o.block_samples, 4096);
        assert_eq!(o.crc, CrcMode::Standard);
        assert_eq!(o.prefix_endian, Endianness::Big);
        assert_eq!(o.duration_seconds, None);
        assert!(!o.once);
    }

    #[test]
    fn the_documented_invocation_parses() {
        let o = options(&[
            "--host",
            "127.0.0.1",
            "--port",
            "9123",
            "--rate",
            "100000",
            "--amplitude",
            "250",
            "--frequency",
            "120",
            "--data-type",
            "velocity",
            "--sn",
            "SN-0001",
        ]);
        assert_eq!(o.port, 9123);
        assert_eq!(o.rate_hz, 100_000.0);
        assert_eq!(o.serial, "SN-0001");
        assert_eq!(o.data_type, DataType::Velocity);
    }

    #[test]
    fn inline_and_separated_values_are_equivalent() {
        assert_eq!(options(&["--port=6000"]), options(&["--port", "6000"]));
        assert_eq!(
            options(&["--data-type=acceleration"]),
            options(&["--data-type", "acceleration"])
        );
    }

    #[test]
    fn every_data_type_and_checksum_mode_is_selectable() {
        for kind in DataType::ALL {
            assert_eq!(options(&["--data-type", kind.as_str()]).data_type, kind);
        }
        for mode in [CrcMode::Standard, CrcMode::Castagnoli, CrcMode::Zero] {
            assert_eq!(options(&["--crc", mode.as_str()]).crc, mode);
        }
        for order in [Endianness::Big, Endianness::Little] {
            assert_eq!(
                options(&["--prefix-endian", order.as_str()]).prefix_endian,
                order
            );
        }
    }

    #[test]
    fn help_and_version_short_circuit() {
        for flag in ["--help", "-h"] {
            assert!(matches!(parse(&args(&[flag])), Ok(SimOutcome::Print(_))));
        }
        match parse(&args(&["--version"])).unwrap() {
            SimOutcome::Print(text) => assert!(text.starts_with("m300-device-sim ")),
            SimOutcome::Run(_) => panic!("expected the version text"),
        }
    }

    #[test]
    fn usage_mentions_every_flag_and_says_what_this_is_not() {
        let text = usage();
        for flag in [
            "--host",
            "--port",
            "--rate",
            "--amplitude",
            "--frequency",
            "--data-type",
            "--sn",
            "--block",
            "--crc",
            "--prefix-endian",
            "--retry",
            "--once",
            "--duration",
            "--version",
            "--help",
        ] {
            assert!(text.contains(flag), "usage is missing {flag}");
        }
        assert!(
            text.contains("NOT a replacement for `m300-sim`"),
            "the two simulators must not be confused for each other"
        );
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
            vec!["--retry", "0"],
            vec!["--amplitude", "nan"],
            vec!["--frequency", "-1"],
            vec!["--data-type", "pressure"],
            vec!["--crc", "md5"],
            vec!["--prefix-endian", "middle"],
            vec!["--block", "0"],
            vec!["--block", "999999999"],
            vec!["--block", "lots"],
            vec!["--sn", "TOO-LONG-A-SERIAL"],
        ] {
            assert!(
                matches!(parse(&args(&arguments)), Err(SimError::BadValue { .. })),
                "for {arguments:?}"
            );
        }
    }

    #[test]
    fn a_serial_of_exactly_ten_characters_is_accepted() {
        assert_eq!(options(&["--sn", "M300123456"]).serial, "M300123456");
    }

    #[test]
    fn missing_values_and_unknown_flags_are_rejected() {
        for flag in [
            "--host",
            "--port",
            "--rate",
            "--amplitude",
            "--data-type",
            "--sn",
            "--block",
            "--crc",
            "--duration",
        ] {
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
    fn an_empty_inline_value_is_a_missing_value_not_an_empty_host() {
        assert_eq!(
            parse(&args(&["--host="])),
            Err(SimError::MissingValue("--host"))
        );
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
    fn errors_say_which_flag_and_why() {
        let error = parse(&args(&["--port", "0"])).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("--port"), "{text}");
        assert!(text.contains("1..=65535"), "{text}");
        assert!(SimError::MissingValue("--host")
            .to_string()
            .contains("requires a value"));
        assert!(SimError::UnexpectedPositional("x".into())
            .to_string()
            .contains("unexpected argument"));
    }
}
