//! The parsed command surface (`docs/PLAN.md` 8) and the errors parsing can produce.

use std::fmt;
use std::path::PathBuf;

use quickvib_core::{ExportFormat, ScpiError};

/// One fully parsed SCPI message.
///
/// The engine dispatches on this with an exhaustive `match`, so adding a variant without a
/// handler is a compile error rather than a runtime gap.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    // ---- IEEE 488.2 mandated (8.1) ----
    /// `*IDN?`
    Idn,
    /// `*RST`
    Rst,
    /// `*CLS`
    Cls,
    /// `*OPC`
    Opc,
    /// `*OPC?`
    OpcQuery,
    /// `SYST:ERR?` (and `SYST:ERR:NEXT?`)
    SystemErrorQuery,

    // ---- Project / mass memory (8.2) ----
    /// `MMEM:LOAD:STAT "<path>"`
    MemoryLoadState(PathBuf),
    /// `MMEM:STOR:STAT "<path>"`
    MemoryStoreState(PathBuf),
    /// `MMEM:LOAD:AUTO`
    MemoryLoadAuto,
    /// `MMEM:LOAD:AUTO?`
    MemoryLoadAutoQuery,
    /// `MMEM:STOR:TRAC "<path>"`
    MemoryStoreTrace(PathBuf),

    // ---- Configuration (8.3) ----
    /// `CONF:REC:DUR <seconds>`
    ConfigureDuration(f64),
    /// `CONF:REC:DUR?`
    ConfigureDurationQuery,
    /// `FORM CSV|TXT`
    Format(ExportFormat),
    /// `FORM?`
    FormatQuery,

    // ---- Recording control (8.4) ----
    /// `INIT`, and its alias `REC:STAR`
    Initiate,
    /// `ABOR`
    Abort,
    /// `REC:STAT?`
    RecordStateQuery,
    /// `REC:WAIT?`
    RecordWaitQuery,

    // ---- Data retrieval (8.5) ----
    /// `FETC?`, and its alias `TRAC:DATA?`
    Fetch,
    /// `TRAC:POIN?`
    TracePointsQuery,

    // ---- Calculated measurements (8.6) ----
    /// `CALC:MEAS:PEAK?`
    CalculatePeak,
    /// `CALC:MEAS:RMS?`
    CalculateRms,
    /// `CALC:MEAS:PP?`
    CalculatePeakToPeak,
    /// `CALC:MEAS:ALL?`
    CalculateAll,

    // ---- System / device (8.7) ----
    /// `SYST:DEV:CONN?`
    SystemDeviceConnectedQuery,
    /// `SYST:VERS?`
    SystemVersionQuery,
}

/// Why a line could not be turned into commands.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseError {
    /// Malformed message, over-long line, or non-UTF-8 input. Maps to `-100`.
    Command {
        /// What was wrong.
        detail: String,
    },
    /// The header names no known command. Maps to `-113`.
    UndefinedHeader {
        /// The header as received.
        header: String,
    },
    /// An argument is present and well-formed but not a legal value. Maps to `-224`.
    IllegalParameter {
        /// What was wrong.
        detail: String,
    },
}

impl ParseError {
    /// The SCPI-99 error the UTS should see.
    #[must_use]
    pub const fn scpi_error(&self) -> ScpiError {
        match self {
            Self::Command { .. } => ScpiError::CommandError,
            Self::UndefinedHeader { .. } => ScpiError::UndefinedHeader,
            Self::IllegalParameter { .. } => ScpiError::IllegalParameterValue,
        }
    }

    /// Convenience constructor for `-100`.
    #[must_use]
    pub fn command(detail: impl Into<String>) -> Self {
        Self::Command {
            detail: detail.into(),
        }
    }

    /// Convenience constructor for `-113`.
    #[must_use]
    pub fn undefined_header(header: impl Into<String>) -> Self {
        Self::UndefinedHeader {
            header: header.into(),
        }
    }

    /// Convenience constructor for `-224`.
    #[must_use]
    pub fn illegal_parameter(detail: impl Into<String>) -> Self {
        Self::IllegalParameter {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command { detail } => write!(f, "command error: {detail}"),
            Self::UndefinedHeader { header } => write!(f, "undefined header '{header}'"),
            Self::IllegalParameter { detail } => write!(f, "illegal parameter: {detail}"),
        }
    }
}

impl std::error::Error for ParseError {}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn error_mapping_matches_the_catalogue() {
        assert_eq!(
            ParseError::command("x").scpi_error(),
            ScpiError::CommandError
        );
        assert_eq!(
            ParseError::undefined_header("NOPE").scpi_error(),
            ScpiError::UndefinedHeader
        );
        assert_eq!(
            ParseError::illegal_parameter("XML").scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }
}
