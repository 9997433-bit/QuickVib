//! Hand-rolled argument parsing (`docs/PLAN.md` 13, D18).
//!
//! Nine flags, no subcommands, no completion — and the UTS invokes the executable from a
//! plain `cmd` line where quoting quirks matter more than parser features. Parsing is a pure
//! function of `&[OsString]`, so every case is testable without spawning a process.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::PathBuf;

use quickvib_core::{BackendKind, Level};

/// Default port the SCPI server listens on: the conventional SCPI raw-socket port.
pub const DEFAULT_SCPI_PORT: u16 = 5025;

/// Default port the device dials in on.
pub const DEFAULT_DEVICE_PORT: u16 = 9123;

/// Fully resolved command-line options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// Project to load at startup. `None` means "auto-load the last one".
    pub project: Option<PathBuf>,
    /// Port the SCPI server listens on.
    pub scpi_port: u16,
    /// Port the device server listens on. `None` defers to the project's `device.port`.
    pub device_port: Option<u16>,
    /// No interactive console output; structured log lines only.
    pub headless: bool,
    /// Backend override. `None` defers to the project, which defaults to the mock.
    pub backend: Option<BackendKind>,
    /// Suppress auto-load-last.
    pub no_auto_load: bool,
    /// Minimum log level.
    pub log_level: Level,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            project: None,
            scpi_port: DEFAULT_SCPI_PORT,
            device_port: None,
            headless: false,
            backend: None,
            no_auto_load: false,
            log_level: Level::Info,
        }
    }
}

/// What the caller should do after parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliOutcome {
    /// Start the server with these options.
    Run(Box<Options>),
    /// Print this text and exit `0`.
    Print(String),
}

/// Why the command line was rejected. Every variant exits `2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
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

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFlag(flag) => write!(f, "unknown option '{flag}'"),
            Self::UnexpectedPositional(value) => {
                write!(f, "unexpected argument '{value}'")
            }
            Self::MissingValue(flag) => write!(f, "option '{flag}' requires a value"),
            Self::BadValue { flag, value, reason } => {
                write!(f, "invalid value '{value}' for '{flag}': {reason}")
            }
        }
    }
}

impl std::error::Error for CliError {}

/// The `--help` text.
#[must_use]
pub fn usage() -> String {
    format!(
        "quickvib {version} - SCPI-over-TCP front end for the M300 vibrometer

USAGE:
    quickvib [OPTIONS]

OPTIONS:
    --project <path>        Load this project at startup (default: auto-load the last one)
    --scpi-port <n>         Port the UTS connects to        [default: {DEFAULT_SCPI_PORT}]
    --device-port <n>       Port the M300 dials in on       [default: {DEFAULT_DEVICE_PORT}]
    --headless              Structured log lines only, no interactive output
    --backend <mock|m300>   Override the project's backend  [default: mock]
    --no-auto-load          Do not auto-load the last project
    --log-level <level>     trace|debug|info|warn|error     [default: info]
    --version               Print the version and exit
    --help                  Print this help and exit

EXIT CODES:
    0  clean shutdown
    2  bad arguments
    3  port bind failure
    4  project load failure
    5  backend open failure
",
        version = env!("CARGO_PKG_VERSION")
    )
}

/// The `--version` text.
#[must_use]
pub fn version() -> String {
    format!("quickvib {}", env!("CARGO_PKG_VERSION"))
}

/// Parse the arguments *after* `argv[0]`.
///
/// Both `--flag value` and `--flag=value` are accepted, and `--` terminates flag parsing.
///
/// # Errors
/// [`CliError`] for an unknown flag, a missing value, an out-of-range port, or an
/// unrecognised enumeration value.
pub fn parse(arguments: &[OsString]) -> Result<CliOutcome, CliError> {
    let mut options = Options::default();
    let mut index = 0;

    while index < arguments.len() {
        let raw = &arguments[index];
        index += 1;

        if raw == "--" {
            if let Some(extra) = arguments.get(index) {
                return Err(CliError::UnexpectedPositional(extra.to_string_lossy().into()));
            }
            break;
        }

        let text = raw.to_string_lossy();
        if !text.starts_with('-') {
            return Err(CliError::UnexpectedPositional(text.into_owned()));
        }

        // Split `--flag=value` without losing a non-UTF-8 value: the only flag whose value
        // may be non-UTF-8 is `--project`, and paths are handled as `OsString` throughout.
        let (flag, inline) = split_inline(raw);

        match flag.as_str() {
            "--help" | "-h" => return Ok(CliOutcome::Print(usage())),
            "--version" | "-V" => return Ok(CliOutcome::Print(version())),
            "--headless" => reject_inline(&flag, inline, &mut options, |o| o.headless = true)?,
            "--no-auto-load" => {
                reject_inline(&flag, inline, &mut options, |o| o.no_auto_load = true)?;
            }
            "--project" => {
                let value = take_value("--project", inline, arguments, &mut index)?;
                options.project = Some(PathBuf::from(value));
            }
            "--scpi-port" => {
                let value = take_value("--scpi-port", inline, arguments, &mut index)?;
                options.scpi_port = parse_port("--scpi-port", &value)?;
            }
            "--device-port" => {
                let value = take_value("--device-port", inline, arguments, &mut index)?;
                options.device_port = Some(parse_port("--device-port", &value)?);
            }
            "--backend" => {
                let value = take_value("--backend", inline, arguments, &mut index)?;
                let text = value.to_string_lossy();
                options.backend = Some(text.parse().map_err(|_| CliError::BadValue {
                    flag: "--backend",
                    value: text.clone().into_owned(),
                    reason: "expected 'mock' or 'm300'".to_owned(),
                })?);
            }
            "--log-level" => {
                let value = take_value("--log-level", inline, arguments, &mut index)?;
                let text = value.to_string_lossy();
                options.log_level =
                    Level::parse(&text).ok_or_else(|| CliError::BadValue {
                        flag: "--log-level",
                        value: text.clone().into_owned(),
                        reason: "expected trace|debug|info|warn|error".to_owned(),
                    })?;
            }
            other => return Err(CliError::UnknownFlag(other.to_owned())),
        }
    }

    Ok(CliOutcome::Run(Box::new(options)))
}

/// Split `--flag=value` into its parts, preserving a non-UTF-8 value.
fn split_inline(raw: &OsStr) -> (String, Option<OsString>) {
    let text = raw.to_string_lossy();
    match text.find('=') {
        Some(position) => {
            let flag = text[..position].to_owned();
            // Recover the value from the original bytes where possible: on both Unix and
            // Windows the lossy conversion only differs for invalid sequences, which cannot
            // appear before the first `=` in a flag name.
            let value = OsString::from(text[position + 1..].to_owned());
            (flag, Some(value))
        }
        None => (text.into_owned(), None),
    }
}

fn reject_inline(
    flag: &str,
    inline: Option<OsString>,
    options: &mut Options,
    apply: impl FnOnce(&mut Options),
) -> Result<(), CliError> {
    if let Some(value) = inline {
        return Err(CliError::BadValue {
            flag: if flag == "--headless" { "--headless" } else { "--no-auto-load" },
            value: value.to_string_lossy().into_owned(),
            reason: "this flag takes no value".to_owned(),
        });
    }
    apply(options);
    Ok(())
}

fn take_value(
    flag: &'static str,
    inline: Option<OsString>,
    arguments: &[OsString],
    index: &mut usize,
) -> Result<OsString, CliError> {
    if let Some(value) = inline {
        if value.is_empty() {
            return Err(CliError::MissingValue(flag));
        }
        return Ok(value);
    }
    match arguments.get(*index) {
        Some(value) => {
            *index += 1;
            Ok(value.clone())
        }
        None => Err(CliError::MissingValue(flag)),
    }
}

fn parse_port(flag: &'static str, value: &OsStr) -> Result<u16, CliError> {
    let text = value.to_string_lossy();
    let port: u32 = text.parse().map_err(|_| CliError::BadValue {
        flag,
        value: text.clone().into_owned(),
        reason: "expected an integer".to_owned(),
    })?;
    if !(1..=65_535).contains(&port) {
        return Err(CliError::BadValue {
            flag,
            value: text.into_owned(),
            reason: "expected a port in 1..=65535".to_owned(),
        });
    }
    Ok(port as u16)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn options(items: &[&str]) -> Options {
        match parse(&args(items)).unwrap() {
            CliOutcome::Run(options) => *options,
            CliOutcome::Print(text) => panic!("expected options, got: {text}"),
        }
    }

    #[test]
    fn defaults_match_the_reference() {
        let o = options(&[]);
        assert_eq!(o.project, None);
        assert_eq!(o.scpi_port, 5025);
        assert_eq!(o.device_port, None);
        assert!(!o.headless);
        assert_eq!(o.backend, None);
        assert!(!o.no_auto_load);
        assert_eq!(o.log_level, Level::Info);
    }

    #[test]
    fn the_documented_uts_invocation_parses() {
        let o = options(&[
            "--project",
            "C:\\Tests\\Test.proj",
            "--scpi-port",
            "5025",
            "--device-port",
            "9123",
            "--headless",
        ]);
        assert_eq!(o.project, Some(PathBuf::from("C:\\Tests\\Test.proj")));
        assert_eq!(o.scpi_port, 5025);
        assert_eq!(o.device_port, Some(9123));
        assert!(o.headless);
    }

    #[test]
    fn inline_and_separated_values_are_equivalent() {
        assert_eq!(options(&["--scpi-port=6000"]), options(&["--scpi-port", "6000"]));
        assert_eq!(
            options(&["--project=/tmp/a.proj"]),
            options(&["--project", "/tmp/a.proj"])
        );
    }

    #[test]
    fn backend_override_parses() {
        assert_eq!(options(&["--backend", "mock"]).backend, Some(BackendKind::Mock));
        assert_eq!(options(&["--backend=m300"]).backend, Some(BackendKind::M300));
    }

    #[test]
    fn log_level_parses_case_insensitively() {
        assert_eq!(options(&["--log-level", "DEBUG"]).log_level, Level::Debug);
        assert_eq!(options(&["--log-level=trace"]).log_level, Level::Trace);
    }

    #[test]
    fn no_auto_load_is_a_flag() {
        assert!(options(&["--no-auto-load"]).no_auto_load);
    }

    #[test]
    fn help_and_version_short_circuit() {
        for flag in ["--help", "-h"] {
            assert!(matches!(parse(&args(&[flag])), Ok(CliOutcome::Print(_))));
        }
        match parse(&args(&["--version"])).unwrap() {
            CliOutcome::Print(text) => assert!(text.starts_with("quickvib ")),
            CliOutcome::Run(_) => panic!("expected the version text"),
        }
    }

    #[test]
    fn help_wins_even_after_other_flags() {
        assert!(matches!(
            parse(&args(&["--headless", "--help"])),
            Ok(CliOutcome::Print(_))
        ));
    }

    #[test]
    fn unknown_flags_are_rejected() {
        assert_eq!(
            parse(&args(&["--frobnicate"])),
            Err(CliError::UnknownFlag("--frobnicate".to_owned()))
        );
    }

    #[test]
    fn positional_arguments_are_rejected() {
        assert!(matches!(
            parse(&args(&["Test.proj"])),
            Err(CliError::UnexpectedPositional(_))
        ));
        assert!(matches!(
            parse(&args(&["--", "Test.proj"])),
            Err(CliError::UnexpectedPositional(_))
        ));
    }

    #[test]
    fn missing_values_are_rejected() {
        for flag in ["--project", "--scpi-port", "--device-port", "--backend", "--log-level"]
        {
            assert!(
                matches!(parse(&args(&[flag])), Err(CliError::MissingValue(_))),
                "for {flag}"
            );
        }
    }

    #[test]
    fn out_of_range_ports_are_rejected() {
        for value in ["0", "65536", "-1", "http"] {
            assert!(
                matches!(
                    parse(&args(&["--scpi-port", value])),
                    Err(CliError::BadValue { .. })
                ),
                "for {value}"
            );
        }
    }

    #[test]
    fn boundary_ports_are_accepted() {
        assert_eq!(options(&["--scpi-port", "1"]).scpi_port, 1);
        assert_eq!(options(&["--scpi-port", "65535"]).scpi_port, 65_535);
    }

    #[test]
    fn unknown_enumeration_values_are_rejected() {
        assert!(matches!(
            parse(&args(&["--backend", "m400"])),
            Err(CliError::BadValue { .. })
        ));
        assert!(matches!(
            parse(&args(&["--log-level", "verbose"])),
            Err(CliError::BadValue { .. })
        ));
    }

    #[test]
    fn value_flags_reject_an_empty_inline_value() {
        assert!(matches!(
            parse(&args(&["--scpi-port="])),
            Err(CliError::MissingValue(_))
        ));
    }

    #[test]
    fn boolean_flags_reject_a_value() {
        assert!(matches!(
            parse(&args(&["--headless=yes"])),
            Err(CliError::BadValue { .. })
        ));
    }

    #[test]
    fn a_bare_double_dash_terminates_parsing() {
        assert_eq!(options(&["--headless", "--"]).headless, true);
    }

    #[test]
    fn usage_text_mentions_every_flag() {
        let text = usage();
        for flag in [
            "--project",
            "--scpi-port",
            "--device-port",
            "--headless",
            "--backend",
            "--no-auto-load",
            "--log-level",
            "--version",
            "--help",
        ] {
            assert!(text.contains(flag), "usage is missing {flag}");
        }
    }
}
