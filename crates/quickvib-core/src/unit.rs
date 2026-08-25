//! The unit of a sample stream, as configured on the device.
//!
//! QuickVib performs **no unit conversion** (`docs/PLAN.md` 7.7). It reports samples in whatever
//! unit the device is configured for and labels them accordingly.

use std::fmt;
use std::str::FromStr;

/// Unit of the sample stream, as configured on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SampleUnit {
    /// Velocity, micrometres per second.
    VelocityUmPerSec,
    /// Displacement, micrometres.
    DisplacementUm,
    /// Acceleration, metres per second squared.
    AccelerationMPerSec2,
}

impl SampleUnit {
    /// The wire/schema spelling used in project files (`"velocity_um_s"`, …).
    #[must_use]
    pub const fn as_schema_str(self) -> &'static str {
        match self {
            Self::VelocityUmPerSec => "velocity_um_s",
            Self::DisplacementUm => "displacement_um",
            Self::AccelerationMPerSec2 => "acceleration_m_s2",
        }
    }

    /// An ASCII-safe engineering label, suitable for a CSV preamble or a log line.
    ///
    /// Deliberately ASCII: the CSV files are read by UTS tooling of unknown encoding
    /// tolerance, so `um/s` is used rather than `μm/s`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::VelocityUmPerSec => "um/s",
            Self::DisplacementUm => "um",
            Self::AccelerationMPerSec2 => "m/s^2",
        }
    }

    /// Every unit, in declaration order. Useful for error messages and exhaustive tests.
    #[must_use]
    pub const fn all() -> &'static [SampleUnit] {
        &[
            Self::VelocityUmPerSec,
            Self::DisplacementUm,
            Self::AccelerationMPerSec2,
        ]
    }
}

impl fmt::Display for SampleUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_schema_str())
    }
}

/// Returned when a string does not name a known [`SampleUnit`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseUnitError {
    /// The rejected input.
    pub input: String,
}

impl fmt::Display for ParseUnitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown sample unit '{}'", self.input)
    }
}

impl std::error::Error for ParseUnitError {}

impl FromStr for SampleUnit {
    type Err = ParseUnitError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        for unit in Self::all() {
            if s.eq_ignore_ascii_case(unit.as_schema_str()) {
                return Ok(*unit);
            }
        }
        Err(ParseUnitError {
            input: s.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn schema_strings_round_trip() {
        for unit in SampleUnit::all() {
            assert_eq!(SampleUnit::from_str(unit.as_schema_str()).unwrap(), *unit);
        }
    }

    #[test]
    fn parsing_is_ascii_case_insensitive() {
        assert_eq!(
            SampleUnit::from_str("VELOCITY_UM_S").unwrap(),
            SampleUnit::VelocityUmPerSec
        );
    }

    #[test]
    fn unknown_unit_is_rejected() {
        let err = SampleUnit::from_str("furlongs_per_fortnight").unwrap_err();
        assert!(err.to_string().contains("furlongs_per_fortnight"));
    }

    #[test]
    fn labels_are_ascii() {
        for unit in SampleUnit::all() {
            assert!(unit.label().is_ascii());
        }
    }
}
