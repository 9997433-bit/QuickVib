//! Failures reported by a [`crate::DeviceBackend`].

use std::fmt;

use quickvib_core::ScpiError;

/// Everything a device backend can fail with.
#[derive(Debug)]
#[non_exhaustive]
pub enum DeviceError {
    /// The transport is not up. Maps to `-241`.
    NotConnected,
    /// The link dropped, or the device stopped sending before the run finished. Maps to
    /// `-240`.
    LinkLost {
        /// What the transport reported.
        detail: String,
    },
    /// The device did not dial in, or stalled long enough to trip the watchdog. Maps to
    /// `-365`.
    Timeout {
        /// What was being waited for.
        detail: String,
    },
    /// A socket or file error. Maps to `-240`.
    Io {
        /// The underlying operating-system error.
        source: std::io::Error,
    },
    /// The native SDK returned a non-zero status. Maps to `-240`.
    Native {
        /// The vendor's numeric status.
        code: i32,
        /// A human-readable description, when the SDK provides one.
        message: String,
    },
    /// The requested configuration cannot be satisfied by this backend. Maps to `-221`.
    Unsupported {
        /// What was asked for.
        detail: String,
    },
    /// A value handed to the transport was rejected as illegal — a null pointer or an
    /// out-of-range enum index in a native call. Maps to `-224`.
    ///
    /// Distinct from [`DeviceError::Unsupported`], which is a configuration this backend cannot
    /// satisfy: this one is a value the transport itself calls invalid, which is usually a
    /// QuickVib bug and is logged as such. The M300 SDK's `M300_ERR_INVALID_ARG` is the only
    /// producer today (`docs/M300-NATIVE.md` §5).
    IllegalParameter {
        /// What was rejected.
        detail: String,
    },
}

impl DeviceError {
    /// The SCPI-99 error the UTS should see for this failure.
    #[must_use]
    pub const fn scpi_error(&self) -> ScpiError {
        match self {
            Self::NotConnected => ScpiError::HardwareMissing,
            Self::LinkLost { .. } | Self::Io { .. } | Self::Native { .. } => {
                ScpiError::HardwareError
            }
            Self::Timeout { .. } => ScpiError::TimeoutError,
            Self::Unsupported { .. } => ScpiError::SettingsConflict,
            Self::IllegalParameter { .. } => ScpiError::IllegalParameterValue,
        }
    }

    /// Convenience constructor for a lost link.
    #[must_use]
    pub fn link_lost(detail: impl Into<String>) -> Self {
        Self::LinkLost {
            detail: detail.into(),
        }
    }

    /// Convenience constructor for a timeout.
    #[must_use]
    pub fn timeout(detail: impl Into<String>) -> Self {
        Self::Timeout {
            detail: detail.into(),
        }
    }

    /// Convenience constructor for an unsupported request.
    #[must_use]
    pub fn unsupported(detail: impl Into<String>) -> Self {
        Self::Unsupported {
            detail: detail.into(),
        }
    }

    /// Convenience constructor for a value the transport rejected as illegal.
    #[must_use]
    pub fn illegal_parameter(detail: impl Into<String>) -> Self {
        Self::IllegalParameter {
            detail: detail.into(),
        }
    }
}

impl fmt::Display for DeviceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConnected => f.write_str("device is not connected"),
            Self::LinkLost { detail } => write!(f, "device link lost: {detail}"),
            Self::Timeout { detail } => write!(f, "device timed out: {detail}"),
            Self::Io { source } => write!(f, "device i/o error: {source}"),
            Self::Native { code, message } => {
                write!(f, "device sdk error {code}: {message}")
            }
            Self::Unsupported { detail } => write!(f, "unsupported device request: {detail}"),
            Self::IllegalParameter { detail } => {
                write!(f, "illegal device parameter: {detail}")
            }
        }
    }
}

impl std::error::Error for DeviceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source } => Some(source),
            _ => None,
        }
    }
}

impl From<std::io::Error> for DeviceError {
    fn from(source: std::io::Error) -> Self {
        Self::Io { source }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn error_mapping_matches_the_catalogue() {
        assert_eq!(
            DeviceError::NotConnected.scpi_error(),
            ScpiError::HardwareMissing
        );
        assert_eq!(
            DeviceError::link_lost("reset").scpi_error(),
            ScpiError::HardwareError
        );
        assert_eq!(
            DeviceError::timeout("dial-in").scpi_error(),
            ScpiError::TimeoutError
        );
        assert_eq!(
            DeviceError::unsupported("rate").scpi_error(),
            ScpiError::SettingsConflict
        );
        assert_eq!(
            DeviceError::illegal_parameter("rate index 255").scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }

    #[test]
    fn io_errors_convert_and_report_hardware_error() {
        let err: DeviceError =
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset").into();
        assert_eq!(err.scpi_error(), ScpiError::HardwareError);
        assert!(err.to_string().contains("reset"));
    }
}
