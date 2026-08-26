//! Errors produced when loading, saving or validating a project.

use std::fmt;
use std::path::PathBuf;

use quickvib_core::ScpiError;

/// Everything that can go wrong with a project file.
///
/// Each variant maps to exactly one SCPI-99 code via [`ProjectError::scpi_error`], so the SCPI
/// layer never has to guess.
#[derive(Debug)]
#[non_exhaustive]
pub enum ProjectError {
    /// The file does not exist. Maps to `-256`.
    NotFound {
        /// The path that was looked for.
        path: PathBuf,
    },
    /// The file exists but could not be read or written. Maps to `-257`.
    Io {
        /// The path being read or written.
        path: PathBuf,
        /// The underlying operating-system error.
        source: std::io::Error,
    },
    /// The bytes are not valid JSON, or do not match the schema. Maps to `-224`.
    Parse {
        /// The path being parsed, if it came from a file.
        path: Option<PathBuf>,
        /// The serde message, including line and column.
        message: String,
    },
    /// A field is present and well-typed but not a legal value. Maps to `-224`.
    Invalid {
        /// Dotted schema path of the offending field, e.g. `device.sampleRateHz`.
        field: &'static str,
        /// Why it was rejected.
        message: String,
    },
    /// A field is outside the permitted range, or the capture-size guard tripped. Maps to
    /// `-222`.
    OutOfRange {
        /// Dotted schema path of the offending field.
        field: &'static str,
        /// Why it was rejected.
        message: String,
    },
}

impl ProjectError {
    /// The SCPI-99 error the UTS should see for this failure.
    #[must_use]
    pub const fn scpi_error(&self) -> ScpiError {
        match self {
            Self::NotFound { .. } => ScpiError::FileNameNotFound,
            Self::Io { .. } => ScpiError::FileNameError,
            Self::Parse { .. } | Self::Invalid { .. } => ScpiError::IllegalParameterValue,
            Self::OutOfRange { .. } => ScpiError::DataOutOfRange,
        }
    }

    /// Convenience constructor for a validation failure.
    pub(crate) fn invalid(field: &'static str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field,
            message: message.into(),
        }
    }

    /// Convenience constructor for a range failure.
    pub(crate) fn out_of_range(field: &'static str, message: impl Into<String>) -> Self {
        Self::OutOfRange {
            field,
            message: message.into(),
        }
    }
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { path } => write!(f, "project file not found: {}", path.display()),
            Self::Io { path, source } => {
                write!(f, "i/o error on {}: {source}", path.display())
            }
            Self::Parse {
                path: Some(path),
                message,
            } => {
                write!(f, "invalid project {}: {message}", path.display())
            }
            Self::Parse {
                path: None,
                message,
            } => write!(f, "invalid project: {message}"),
            Self::Invalid { field, message } => write!(f, "invalid {field}: {message}"),
            Self::OutOfRange { field, message } => write!(f, "{field} out of range: {message}"),
        }
    }
}

impl std::error::Error for ProjectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn error_codes_match_the_catalogue() {
        assert_eq!(
            ProjectError::NotFound { path: "x".into() }.scpi_error(),
            ScpiError::FileNameNotFound
        );
        assert_eq!(
            ProjectError::invalid("device.unit", "bad").scpi_error(),
            ScpiError::IllegalParameterValue
        );
        assert_eq!(
            ProjectError::out_of_range("recording.durationSeconds", "bad").scpi_error(),
            ScpiError::DataOutOfRange
        );
    }
}
