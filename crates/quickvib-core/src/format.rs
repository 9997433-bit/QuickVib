//! Small enumerations shared by the project schema, the exporter and the backend factory.

use std::fmt;
use std::str::FromStr;

/// Export file format selected by `FORM CSV|TXT` and by `export.format` in the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ExportFormat {
    /// Comma-separated values with an optional metadata preamble and header row.
    #[default]
    Csv,
    /// One value per line, no header.
    Txt,
}

impl ExportFormat {
    /// The uppercase SCPI spelling, as returned by `FORM?`.
    #[must_use]
    pub const fn as_scpi_str(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Txt => "TXT",
        }
    }

    /// The conventional file extension for this format.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Txt => "txt",
        }
    }
}

impl fmt::Display for ExportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_scpi_str())
    }
}

/// Returned when a string names neither a known [`ExportFormat`] nor a known [`BackendKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseEnumError {
    /// What was being parsed, e.g. `"export format"`.
    pub what: &'static str,
    /// The rejected input.
    pub input: String,
}

impl fmt::Display for ParseEnumError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} '{}'", self.what, self.input)
    }
}

impl std::error::Error for ParseEnumError {}

impl FromStr for ExportFormat {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("csv") {
            Ok(Self::Csv)
        } else if s.eq_ignore_ascii_case("txt") {
            Ok(Self::Txt)
        } else {
            Err(ParseEnumError {
                what: "export format",
                input: s.to_owned(),
            })
        }
    }
}

/// Which device backend the engine should drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BackendKind {
    /// Pure-Rust deterministic signal generator. The default everywhere (D4).
    #[default]
    Mock,
    /// The real M300 vibrometer, via the Windows-only `quickvib-m300` crate.
    M300,
}

impl BackendKind {
    /// The schema/CLI spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::M300 => "m300",
        }
    }
}

impl fmt::Display for BackendKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for BackendKind {
    type Err = ParseEnumError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("mock") {
            Ok(Self::Mock)
        } else if s.eq_ignore_ascii_case("m300") {
            Ok(Self::M300)
        } else {
            Err(ParseEnumError {
                what: "backend",
                input: s.to_owned(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn export_format_parses_case_insensitively() {
        assert_eq!(ExportFormat::from_str("csv").unwrap(), ExportFormat::Csv);
        assert_eq!(ExportFormat::from_str("TXT").unwrap(), ExportFormat::Txt);
        assert!(ExportFormat::from_str("XML").is_err());
    }

    #[test]
    fn backend_kind_parses_case_insensitively() {
        assert_eq!(BackendKind::from_str("MOCK").unwrap(), BackendKind::Mock);
        assert_eq!(BackendKind::from_str("m300").unwrap(), BackendKind::M300);
        assert!(BackendKind::from_str("m400").is_err());
    }

    #[test]
    fn defaults_match_the_plan() {
        assert_eq!(ExportFormat::default(), ExportFormat::Csv);
        assert_eq!(BackendKind::default(), BackendKind::Mock);
    }
}
