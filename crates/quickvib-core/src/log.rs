//! The logging facade (`docs/PLAN.md` 16).
//!
//! The output format is a contract with the UTS's log scraper, so there is no `log`/`tracing`
//! facade and no filter DSL — just a trait the binary implements and tests capture.

use std::fmt;

/// Severity of a log record. Ordered: `Trace < Debug < Info < Warn < Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    /// Very fine-grained tracing, off by default.
    Trace,
    /// Diagnostic detail useful when something is wrong.
    Debug,
    /// Normal operational milestones. The default threshold.
    #[default]
    Info,
    /// Something unexpected that did not stop the operation.
    Warn,
    /// An operation failed.
    Error,
}

impl Level {
    /// The fixed-width uppercase name used in log lines.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }

    /// Whether records at this level go to stderr rather than stdout.
    #[must_use]
    pub const fn is_diagnostic(self) -> bool {
        matches!(self, Self::Warn | Self::Error)
    }

    /// Parse a `--log-level` value.
    ///
    /// # Errors
    /// Returns `Err(())` if the string names no level.
    pub fn parse(s: &str) -> Result<Self, ()> {
        for level in [Self::Trace, Self::Debug, Self::Info, Self::Warn, Self::Error] {
            if s.eq_ignore_ascii_case(level.as_str()) {
                return Ok(level);
            }
        }
        Err(())
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Sink for structured log lines.
///
/// Implementations must write a whole line atomically with respect to other threads.
pub trait Logger: Send + Sync {
    /// Emit one record. `component` is a short subsystem tag (`scpi`, `device`, `rec`, …) and
    /// `message` already contains any `key=value` fields.
    fn log(&self, level: Level, component: &str, message: &str);

    /// Whether a record at `level` would be emitted. Lets callers skip formatting work.
    fn enabled(&self, level: Level) -> bool {
        let _ = level;
        true
    }
}

/// A [`Logger`] that discards everything. The default for library tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullLogger;

impl Logger for NullLogger {
    fn log(&self, _level: Level, _component: &str, _message: &str) {}

    fn enabled(&self, _level: Level) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn levels_are_ordered() {
        assert!(Level::Trace < Level::Info);
        assert!(Level::Info < Level::Error);
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(Level::parse("WARN").unwrap(), Level::Warn);
        assert_eq!(Level::parse("trace").unwrap(), Level::Trace);
        assert!(Level::parse("verbose").is_err());
    }

    #[test]
    fn warn_and_error_are_diagnostic() {
        assert!(Level::Warn.is_diagnostic());
        assert!(Level::Error.is_diagnostic());
        assert!(!Level::Info.is_diagnostic());
    }
}
