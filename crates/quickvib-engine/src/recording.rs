//! Value types describing a run (`docs/PLAN.md` 7.6).

use std::sync::Arc;
use std::time::Duration;

use quickvib_core::ScpiError;
use quickvib_measure::MeasurementSet;

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Every expected sample arrived.
    Completed,
    /// `ABOR` was issued.
    Aborted,
    /// The watchdog fired.
    TimedOut,
    /// The device link failed, or the stream ended short.
    LinkLost,
}

impl RunOutcome {
    /// The SCPI-99 error to push when a run ends this way, if any.
    #[must_use]
    pub const fn scpi_error(self) -> Option<ScpiError> {
        match self {
            Self::Completed | Self::Aborted => None,
            Self::TimedOut => Some(ScpiError::TimeoutError),
            Self::LinkLost => Some(ScpiError::HardwareError),
        }
    }

    /// Whether the run produced fetchable data.
    #[must_use]
    pub const fn is_success(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// The parameters a run was started with.
#[derive(Debug, Clone, PartialEq)]
pub struct RunPlan {
    /// Monotonic sequence number, so a late thread from a previous run cannot corrupt the
    /// current one.
    pub sequence: u64,
    /// Samples the run must collect.
    pub expected_samples: usize,
    /// Requested duration in seconds.
    pub duration_seconds: f64,
    /// Nominal sample rate.
    pub sample_rate_hz: f64,
    /// Watchdog deadline: `duration * timeoutMultiplier + 1 s`.
    pub watchdog: Duration,
}

/// The result of a completed run.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// The capture buffer.
    pub samples: Arc<Vec<f32>>,
    /// Peak, RMS and peak-to-peak over that buffer.
    pub measurements: MeasurementSet,
    /// Monotonic time from the first sample to the last.
    pub elapsed: Duration,
    /// The record duration this run was armed with, kept so export metadata describes the
    /// capture rather than whatever `CONF:REC:DUR` was set to afterwards.
    pub duration_seconds: f64,
    /// Wall-clock time the run finished, for the CSV preamble.
    pub finished_at: std::time::SystemTime,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn outcome_error_mapping_matches_the_catalogue() {
        assert_eq!(RunOutcome::Completed.scpi_error(), None);
        assert_eq!(RunOutcome::Aborted.scpi_error(), None);
        assert_eq!(
            RunOutcome::TimedOut.scpi_error(),
            Some(ScpiError::TimeoutError)
        );
        assert_eq!(
            RunOutcome::LinkLost.scpi_error(),
            Some(ScpiError::HardwareError)
        );
    }

    #[test]
    fn only_completed_runs_are_successful() {
        assert!(RunOutcome::Completed.is_success());
        for outcome in [
            RunOutcome::Aborted,
            RunOutcome::TimedOut,
            RunOutcome::LinkLost,
        ] {
            assert!(!outcome.is_success());
        }
    }
}
