//! The SCPI-99 error catalogue (`docs/PLAN.md` 9).
//!
//! Standard codes are reused rather than invented (D7), so the UTS's existing error handling
//! works unchanged. The enum owns both the numeric code and the canonical message text so the
//! documentation table and the code cannot drift.

use std::fmt;

/// A SCPI-99 error, as pushed onto the instrument error queue and reported by `SYST:ERR?`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScpiError {
    /// `0` — the queue is empty.
    NoError,
    /// `-100` — malformed message, over-long line, non-UTF-8 input, contained panic.
    CommandError,
    /// `-113` — unknown command header.
    UndefinedHeader,
    /// `-221` — no project loaded, already recording, or a config change during a run.
    SettingsConflict,
    /// `-222` — duration outside `(0, 3600]`, over the capture-size cap, or an overflow on the
    /// size guard.
    DataOutOfRange,
    /// `-224` — bad `FORM` value, invalid project schema, deserialization failure.
    IllegalParameterValue,
    /// `-230` — a data query with no completed capture.
    DataCorruptOrStale,
    /// `-240` — device link dropped during `Armed`/`Recording`; non-zero SDK return code.
    HardwareError,
    /// `-241` — no device connected at `INIT`; SDK library or symbol missing.
    HardwareMissing,
    /// `-256` — project file missing, or no auto-load record.
    FileNameNotFound,
    /// `-257` — path invalid or not writable.
    FileNameError,
    /// `-350` — more than [`ERROR_QUEUE_DEPTH`] unread errors.
    QueueOverflow,
    /// `-365` — the run watchdog fired during a capture.
    TimeoutError,
}

/// Depth of the instrument error queue before [`ScpiError::QueueOverflow`] is reported.
pub const ERROR_QUEUE_DEPTH: usize = 32;

impl ScpiError {
    /// The SCPI-99 numeric code.
    #[must_use]
    pub const fn code(self) -> i16 {
        match self {
            Self::NoError => 0,
            Self::CommandError => -100,
            Self::UndefinedHeader => -113,
            Self::SettingsConflict => -221,
            Self::DataOutOfRange => -222,
            Self::IllegalParameterValue => -224,
            Self::DataCorruptOrStale => -230,
            Self::HardwareError => -240,
            Self::HardwareMissing => -241,
            Self::FileNameNotFound => -256,
            Self::FileNameError => -257,
            Self::QueueOverflow => -350,
            Self::TimeoutError => -365,
        }
    }

    /// The canonical SCPI-99 message text, without surrounding quotes.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::NoError => "No error",
            Self::CommandError => "Command error",
            Self::UndefinedHeader => "Undefined header",
            Self::SettingsConflict => "Settings conflict",
            Self::DataOutOfRange => "Data out of range",
            Self::IllegalParameterValue => "Illegal parameter value",
            Self::DataCorruptOrStale => "Data corrupt or stale",
            Self::HardwareError => "Hardware error",
            Self::HardwareMissing => "Hardware missing",
            Self::FileNameNotFound => "File name not found",
            Self::FileNameError => "File name error",
            Self::QueueOverflow => "Queue overflow",
            Self::TimeoutError => "Time out error",
        }
    }

    /// Attach a human-readable detail string, producing a queue entry.
    ///
    /// The detail never changes the wire response (which is always `<code>,"<message>"`); it is
    /// carried so the log line can say *why* without the UTS seeing a non-standard message.
    #[must_use]
    pub fn detail(self, detail: impl Into<String>) -> ScpiErrorEntry {
        ScpiErrorEntry { error: self, detail: Some(detail.into()) }
    }

    /// Every catalogued error, in code order. Used by the documentation-drift test.
    #[must_use]
    pub const fn all() -> &'static [ScpiError] {
        &[
            Self::NoError,
            Self::CommandError,
            Self::UndefinedHeader,
            Self::SettingsConflict,
            Self::DataOutOfRange,
            Self::IllegalParameterValue,
            Self::DataCorruptOrStale,
            Self::HardwareError,
            Self::HardwareMissing,
            Self::FileNameNotFound,
            Self::FileNameError,
            Self::QueueOverflow,
            Self::TimeoutError,
        ]
    }
}

impl fmt::Display for ScpiError {
    /// Renders the exact `SYST:ERR?` wire form: `<code>,"<message>"`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},\"{}\"", self.code(), self.message())
    }
}

impl std::error::Error for ScpiError {}

/// An error queue entry: the standard error plus an optional local diagnostic detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScpiErrorEntry {
    /// The standard SCPI-99 error.
    pub error: ScpiError,
    /// Optional human-readable detail, for logs only — never sent to the UTS.
    pub detail: Option<String>,
}

impl ScpiErrorEntry {
    /// A queue entry with no extra detail.
    #[must_use]
    pub const fn new(error: ScpiError) -> Self {
        Self { error, detail: None }
    }

    /// The `SYST:ERR?` wire form of this entry.
    #[must_use]
    pub fn wire_form(&self) -> String {
        self.error.to_string()
    }
}

impl From<ScpiError> for ScpiErrorEntry {
    fn from(error: ScpiError) -> Self {
        Self::new(error)
    }
}

impl fmt::Display for ScpiErrorEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.detail {
            Some(detail) => write!(f, "{} ({detail})", self.error),
            None => write!(f, "{}", self.error),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn wire_format_matches_scpi() {
        assert_eq!(ScpiError::NoError.to_string(), "0,\"No error\"");
        assert_eq!(ScpiError::UndefinedHeader.to_string(), "-113,\"Undefined header\"");
        assert_eq!(ScpiError::TimeoutError.to_string(), "-365,\"Time out error\"");
    }

    #[test]
    fn codes_are_unique() {
        let mut codes: Vec<i16> = ScpiError::all().iter().map(|e| e.code()).collect();
        let len = codes.len();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), len);
    }

    #[test]
    fn detail_never_leaks_into_the_wire_form() {
        let entry = ScpiError::IllegalParameterValue.detail("device.sampleRateHz must be > 0");
        assert_eq!(entry.wire_form(), "-224,\"Illegal parameter value\"");
        assert!(entry.to_string().contains("sampleRateHz"));
    }

    #[test]
    fn messages_are_ascii_and_quote_free() {
        for error in ScpiError::all() {
            assert!(error.message().is_ascii());
            assert!(!error.message().contains('"'));
        }
    }
}
