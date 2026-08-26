//! The command tree: header paths to [`Command`] values (`docs/PLAN.md` 8, D6).
//!
//! Matching follows SCPI-99: ASCII case-insensitive, and each mnemonic is accepted in either
//! its short form or its full long form (`MEAS` == `MEASure`), never a partial completion.

use std::str::FromStr;

use quickvib_core::ExportFormat;

use crate::command::{Command, ParseError};
use crate::lexer::{Argument, LexedMessage};

/// A mnemonic: its short form and its long form. They are equal when the mnemonic has only
/// one spelling.
type Mnemonic = (&'static str, &'static str);

/// What forms of a header the instrument accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Only `HEADER`.
    CommandOnly,
    /// Only `HEADER?`.
    QueryOnly,
    /// Both, mapping to two different commands.
    Both,
}

/// Turns the arguments of a matched header into a [`Command`].
type Builder = fn(&[Argument]) -> Result<Command, ParseError>;

/// One entry in the command tree.
struct Entry {
    path: &'static [Mnemonic],
    form: Form,
    /// Builder for the command form, given the arguments.
    command: Option<Builder>,
    /// Builder for the query form.
    query: Option<Builder>,
}

const fn m(short: &'static str, long: &'static str) -> Mnemonic {
    (short, long)
}

/// IEEE 488.2 common commands, keyed by the mnemonic after the `*`.
const COMMON: &[(&str, Form, Option<Command>, Option<Command>)] = &[
    ("IDN", Form::QueryOnly, None, Some(Command::Idn)),
    ("RST", Form::CommandOnly, Some(Command::Rst), None),
    ("CLS", Form::CommandOnly, Some(Command::Cls), None),
    (
        "OPC",
        Form::Both,
        Some(Command::Opc),
        Some(Command::OpcQuery),
    ),
];

/// Build a nullary command: it resolves only when no arguments were supplied.
macro_rules! plain {
    ($variant:expr) => {{
        fn build(arguments: &[Argument]) -> Result<Command, ParseError> {
            if arguments.is_empty() {
                Ok($variant)
            } else {
                Err(ParseError::command("command takes no arguments"))
            }
        }
        Some(build as Builder)
    }};
}

fn single_path(arguments: &[Argument]) -> Result<std::path::PathBuf, ParseError> {
    match arguments {
        [argument] => {
            let text = argument.text().trim();
            if text.is_empty() {
                Err(ParseError::illegal_parameter("path must not be empty"))
            } else {
                Ok(std::path::PathBuf::from(text))
            }
        }
        [] => Err(ParseError::command("expected a quoted path argument")),
        _ => Err(ParseError::command("expected exactly one path argument")),
    }
}

fn load_state(arguments: &[Argument]) -> Result<Command, ParseError> {
    single_path(arguments).map(Command::MemoryLoadState)
}

fn store_state(arguments: &[Argument]) -> Result<Command, ParseError> {
    single_path(arguments).map(Command::MemoryStoreState)
}

fn store_trace(arguments: &[Argument]) -> Result<Command, ParseError> {
    single_path(arguments).map(Command::MemoryStoreTrace)
}

fn duration(arguments: &[Argument]) -> Result<Command, ParseError> {
    match arguments {
        [argument] => {
            let text = argument.text().trim();
            let value: f64 = text
                .parse()
                .map_err(|_| ParseError::illegal_parameter(format!("'{text}' is not a number")))?;
            if !value.is_finite() {
                return Err(ParseError::illegal_parameter(format!(
                    "'{text}' is not a finite number"
                )));
            }
            Ok(Command::ConfigureDuration(value))
        }
        [] => Err(ParseError::command("expected a duration argument")),
        _ => Err(ParseError::command(
            "expected exactly one duration argument",
        )),
    }
}

fn format_argument(arguments: &[Argument]) -> Result<Command, ParseError> {
    match arguments {
        [argument] => {
            let text = argument.text().trim();
            ExportFormat::from_str(text)
                .map(Command::Format)
                .map_err(|_| ParseError::illegal_parameter(format!("unknown format '{text}'")))
        }
        [] => Err(ParseError::command("expected a format argument")),
        _ => Err(ParseError::command("expected exactly one format argument")),
    }
}

/// The instrument-specific command tree.
static ENTRIES: &[Entry] = &[
    // ---- SYSTem ----
    Entry {
        path: &[m("SYST", "SYSTEM"), m("ERR", "ERROR")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::SystemErrorQuery),
    },
    Entry {
        path: &[m("SYST", "SYSTEM"), m("ERR", "ERROR"), m("NEXT", "NEXT")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::SystemErrorQuery),
    },
    Entry {
        path: &[m("SYST", "SYSTEM"), m("VERS", "VERSION")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::SystemVersionQuery),
    },
    Entry {
        path: &[
            m("SYST", "SYSTEM"),
            m("DEV", "DEVICE"),
            m("CONN", "CONNECTED"),
        ],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::SystemDeviceConnectedQuery),
    },
    // ---- MMEMory ----
    Entry {
        path: &[m("MMEM", "MMEMORY"), m("LOAD", "LOAD"), m("STAT", "STATE")],
        form: Form::CommandOnly,
        command: Some(load_state),
        query: None,
    },
    Entry {
        path: &[m("MMEM", "MMEMORY"), m("STOR", "STORE"), m("STAT", "STATE")],
        form: Form::CommandOnly,
        command: Some(store_state),
        query: None,
    },
    Entry {
        path: &[m("MMEM", "MMEMORY"), m("LOAD", "LOAD"), m("AUTO", "AUTO")],
        form: Form::Both,
        command: plain!(Command::MemoryLoadAuto),
        query: plain!(Command::MemoryLoadAutoQuery),
    },
    Entry {
        path: &[m("MMEM", "MMEMORY"), m("STOR", "STORE"), m("TRAC", "TRACE")],
        form: Form::CommandOnly,
        command: Some(store_trace),
        query: None,
    },
    // ---- CONFigure / FORMat ----
    Entry {
        path: &[
            m("CONF", "CONFIGURE"),
            m("REC", "RECORD"),
            m("DUR", "DURATION"),
        ],
        form: Form::Both,
        command: Some(duration),
        query: plain!(Command::ConfigureDurationQuery),
    },
    Entry {
        path: &[m("FORM", "FORMAT")],
        form: Form::Both,
        command: Some(format_argument),
        query: plain!(Command::FormatQuery),
    },
    Entry {
        path: &[m("FORM", "FORMAT"), m("DATA", "DATA")],
        form: Form::Both,
        command: Some(format_argument),
        query: plain!(Command::FormatQuery),
    },
    // ---- Recording control ----
    Entry {
        path: &[m("INIT", "INITIATE")],
        form: Form::CommandOnly,
        command: plain!(Command::Initiate),
        query: None,
    },
    Entry {
        path: &[m("INIT", "INITIATE"), m("IMM", "IMMEDIATE")],
        form: Form::CommandOnly,
        command: plain!(Command::Initiate),
        query: None,
    },
    Entry {
        path: &[m("REC", "RECORD"), m("STAR", "START")],
        form: Form::CommandOnly,
        command: plain!(Command::Initiate),
        query: None,
    },
    Entry {
        path: &[m("ABOR", "ABORT")],
        form: Form::CommandOnly,
        command: plain!(Command::Abort),
        query: None,
    },
    Entry {
        path: &[m("REC", "RECORD"), m("STAT", "STATUS")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::RecordStateQuery),
    },
    Entry {
        path: &[m("REC", "RECORD"), m("WAIT", "WAIT")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::RecordWaitQuery),
    },
    // ---- Data retrieval ----
    Entry {
        path: &[m("FETC", "FETCH")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::Fetch),
    },
    Entry {
        path: &[m("TRAC", "TRACE"), m("DATA", "DATA")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::Fetch),
    },
    Entry {
        path: &[m("TRAC", "TRACE"), m("POIN", "POINTS")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::TracePointsQuery),
    },
    // ---- Calculated measurements ----
    Entry {
        path: &[
            m("CALC", "CALCULATE"),
            m("MEAS", "MEASURE"),
            m("PEAK", "PEAK"),
        ],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::CalculatePeak),
    },
    Entry {
        path: &[
            m("CALC", "CALCULATE"),
            m("MEAS", "MEASURE"),
            m("RMS", "RMS"),
        ],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::CalculateRms),
    },
    Entry {
        path: &[m("CALC", "CALCULATE"), m("MEAS", "MEASURE"), m("PP", "PP")],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::CalculatePeakToPeak),
    },
    Entry {
        path: &[
            m("CALC", "CALCULATE"),
            m("MEAS", "MEASURE"),
            m("ALL", "ALL"),
        ],
        form: Form::QueryOnly,
        command: None,
        query: plain!(Command::CalculateAll),
    },
];

/// Does `node` name `mnemonic`, in either its short or its long form?
fn matches(node: &str, mnemonic: Mnemonic) -> bool {
    node.eq_ignore_ascii_case(mnemonic.0) || node.eq_ignore_ascii_case(mnemonic.1)
}

/// Resolve a lexed message to a [`Command`].
///
/// # Errors
/// [`ParseError::UndefinedHeader`] when nothing in the tree matches, and whatever the
/// argument parser for the matched command reports.
pub fn resolve(message: &LexedMessage) -> Result<Command, ParseError> {
    if message.is_common {
        return resolve_common(message);
    }

    for entry in ENTRIES {
        if entry.path.len() != message.nodes.len() {
            continue;
        }
        if !entry
            .path
            .iter()
            .zip(&message.nodes)
            .all(|(mnemonic, node)| matches(node, *mnemonic))
        {
            continue;
        }

        let builder = match (message.query, entry.form) {
            (false, Form::QueryOnly) | (true, Form::CommandOnly) => continue,
            (true, _) => entry.query,
            (false, _) => entry.command,
        };
        if let Some(builder) = builder {
            return builder(&message.arguments);
        }
    }

    Err(ParseError::undefined_header(header_text(message)))
}

fn resolve_common(message: &LexedMessage) -> Result<Command, ParseError> {
    let Some(node) = message.nodes.first() else {
        return Err(ParseError::command("empty common command"));
    };
    for (mnemonic, form, command, query) in COMMON {
        if !node.eq_ignore_ascii_case(mnemonic) {
            continue;
        }
        let selected = match (message.query, form) {
            (false, Form::QueryOnly) | (true, Form::CommandOnly) => continue,
            (true, _) => query.clone(),
            (false, _) => command.clone(),
        };
        if let Some(selected) = selected {
            if !message.arguments.is_empty() {
                return Err(ParseError::command("common commands take no arguments"));
            }
            return Ok(selected);
        }
    }
    Err(ParseError::undefined_header(header_text(message)))
}

/// Reconstruct the header for the error message.
fn header_text(message: &LexedMessage) -> String {
    let mut text = String::new();
    if message.is_common {
        text.push('*');
    } else if message.leading_colon {
        text.push(':');
    }
    text.push_str(&message.nodes.join(":"));
    if message.query {
        text.push('?');
    }
    text
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::lexer::lex_message;
    use quickvib_core::ScpiError;

    fn parse(message: &str) -> Result<Command, ParseError> {
        resolve(&lex_message(message)?)
    }

    #[test]
    fn every_command_in_the_reference_resolves() {
        let cases: &[(&str, Command)] = &[
            ("*IDN?", Command::Idn),
            ("*RST", Command::Rst),
            ("*CLS", Command::Cls),
            ("*OPC", Command::Opc),
            ("*OPC?", Command::OpcQuery),
            ("SYST:ERR?", Command::SystemErrorQuery),
            ("SYST:ERR:NEXT?", Command::SystemErrorQuery),
            ("SYST:VERS?", Command::SystemVersionQuery),
            ("SYST:DEV:CONN?", Command::SystemDeviceConnectedQuery),
            (
                r#"MMEM:LOAD:STAT "Test.proj""#,
                Command::MemoryLoadState("Test.proj".into()),
            ),
            (
                r#"MMEM:STOR:STAT "Test.proj""#,
                Command::MemoryStoreState("Test.proj".into()),
            ),
            ("MMEM:LOAD:AUTO", Command::MemoryLoadAuto),
            ("MMEM:LOAD:AUTO?", Command::MemoryLoadAutoQuery),
            (
                r#"MMEM:STOR:TRAC "run001.csv""#,
                Command::MemoryStoreTrace("run001.csv".into()),
            ),
            ("CONF:REC:DUR 5.0", Command::ConfigureDuration(5.0)),
            ("CONF:REC:DUR?", Command::ConfigureDurationQuery),
            ("FORM CSV", Command::Format(ExportFormat::Csv)),
            ("FORM TXT", Command::Format(ExportFormat::Txt)),
            ("FORM?", Command::FormatQuery),
            ("INIT", Command::Initiate),
            ("REC:STAR", Command::Initiate),
            ("ABOR", Command::Abort),
            ("REC:STAT?", Command::RecordStateQuery),
            ("REC:WAIT?", Command::RecordWaitQuery),
            ("FETC?", Command::Fetch),
            ("TRAC:DATA?", Command::Fetch),
            ("TRAC:POIN?", Command::TracePointsQuery),
            ("CALC:MEAS:PEAK?", Command::CalculatePeak),
            ("CALC:MEAS:RMS?", Command::CalculateRms),
            ("CALC:MEAS:PP?", Command::CalculatePeakToPeak),
            ("CALC:MEAS:ALL?", Command::CalculateAll),
        ];
        for (text, expected) in cases {
            assert_eq!(&parse(text).unwrap(), expected, "for {text}");
        }
    }

    #[test]
    fn long_forms_are_equivalent_to_short_forms() {
        let pairs = [
            ("CONF:REC:DUR 5", "CONFigure:RECord:DURation 5"),
            ("REC:STAT?", "RECord:STATus?"),
            ("REC:STAR", "RECord:STARt"),
            ("CALC:MEAS:ALL?", "CALCulate:MEASure:ALL?"),
            ("SYST:DEV:CONN?", "SYSTem:DEVice:CONNected?"),
            ("MMEM:STOR:TRAC \"a\"", "MMEMory:STORe:TRACe \"a\""),
            ("FETC?", "FETCh?"),
            ("INIT", "INITiate"),
            ("ABOR", "ABORt"),
        ];
        for (short, long) in pairs {
            assert_eq!(
                parse(short).unwrap(),
                parse(long).unwrap(),
                "{short} vs {long}"
            );
        }
    }

    #[test]
    fn matching_is_ascii_case_insensitive() {
        assert_eq!(parse("calc:meas:peak?").unwrap(), Command::CalculatePeak);
        assert_eq!(parse("CaLc:MeAs:PeAk?").unwrap(), Command::CalculatePeak);
        assert_eq!(parse("*idn?").unwrap(), Command::Idn);
    }

    #[test]
    fn partial_completions_are_not_accepted() {
        // `MEASU` is neither the short form nor the full long form.
        assert_eq!(
            parse("CALC:MEASU:PEAK?").unwrap_err().scpi_error(),
            ScpiError::UndefinedHeader
        );
    }

    #[test]
    fn unknown_headers_report_113() {
        for text in ["NOPE", "NOPE?", "CONF:REC:NOPE 1", "*ZZZ?", "CALC:MEAS?"] {
            assert_eq!(
                parse(text).unwrap_err().scpi_error(),
                ScpiError::UndefinedHeader,
                "for {text}"
            );
        }
    }

    #[test]
    fn wrong_form_is_an_undefined_header() {
        // `INIT?` and `FETC` do not exist.
        assert_eq!(
            parse("INIT?").unwrap_err().scpi_error(),
            ScpiError::UndefinedHeader
        );
        assert_eq!(
            parse("FETC").unwrap_err().scpi_error(),
            ScpiError::UndefinedHeader
        );
    }

    #[test]
    fn bad_format_value_is_224() {
        assert_eq!(
            parse("FORM XML").unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }

    #[test]
    fn unparseable_duration_is_224() {
        for text in ["CONF:REC:DUR abc", "CONF:REC:DUR nan", "CONF:REC:DUR inf"] {
            assert_eq!(
                parse(text).unwrap_err().scpi_error(),
                ScpiError::IllegalParameterValue,
                "for {text}"
            );
        }
    }

    #[test]
    fn missing_or_extra_arguments_are_100() {
        for text in [
            "CONF:REC:DUR",
            "CONF:REC:DUR 1,2",
            "FORM",
            "MMEM:LOAD:STAT",
            "INIT 5",
            "*CLS 1",
        ] {
            assert_eq!(
                parse(text).unwrap_err().scpi_error(),
                ScpiError::CommandError,
                "for {text}"
            );
        }
    }

    #[test]
    fn scientific_and_signed_durations_parse() {
        assert_eq!(
            parse("CONF:REC:DUR 1e-3").unwrap(),
            Command::ConfigureDuration(0.001)
        );
        assert_eq!(
            parse("CONF:REC:DUR +2.5").unwrap(),
            Command::ConfigureDuration(2.5)
        );
        assert_eq!(
            parse("CONF:REC:DUR -1").unwrap(),
            Command::ConfigureDuration(-1.0)
        );
    }

    #[test]
    fn leading_colon_does_not_change_resolution() {
        assert_eq!(parse(":CALC:MEAS:RMS?").unwrap(), Command::CalculateRms);
    }
}
