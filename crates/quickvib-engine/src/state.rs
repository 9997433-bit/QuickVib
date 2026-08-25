//! The instrument state machine (`docs/PLAN.md` 5.3).
//!
//! Every transition goes through one [`transition`] function, so illegal edges are a
//! `match` arm rather than scattered `if` checks.

use std::fmt;

use quickvib_core::ScpiError;

/// The instrument state, exactly as reported by `REC:STAT?`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum State {
    /// No run has been started, or the last one was cleared by `*RST`.
    #[default]
    Idle,
    /// A run has been requested; no sample has arrived yet.
    Armed,
    /// Samples are arriving.
    Recording,
    /// The expected sample count was reached; data and measurements are available.
    Complete,
    /// The run was aborted, timed out, or lost the device link.
    Aborted,
}

impl State {
    /// The `REC:STAT?` wire form.
    #[must_use]
    pub const fn as_scpi_str(self) -> &'static str {
        match self {
            Self::Idle => "IDLE",
            Self::Armed => "ARMED",
            Self::Recording => "RECORDING",
            Self::Complete => "COMPLETE",
            Self::Aborted => "ABORTED",
        }
    }

    /// Whether a run is in flight, which is what makes configuration changes a `-221`.
    #[must_use]
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Armed | Self::Recording)
    }

    /// Every state, for exhaustive table tests.
    #[must_use]
    pub const fn all() -> &'static [State] {
        &[
            Self::Idle,
            Self::Armed,
            Self::Recording,
            Self::Complete,
            Self::Aborted,
        ]
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_scpi_str())
    }
}

/// What just happened to the instrument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Event {
    /// `INIT` or `REC:STAR`.
    Init,
    /// The first sample of a run arrived.
    FirstSample,
    /// The expected sample count was reached.
    Completed,
    /// `ABOR`, a watchdog timeout, or a lost link.
    Abort,
    /// `*RST`.
    Reset,
}

impl Event {
    /// Every event, for exhaustive table tests.
    #[must_use]
    pub const fn all() -> &'static [Event] {
        &[
            Self::Init,
            Self::FirstSample,
            Self::Completed,
            Self::Abort,
            Self::Reset,
        ]
    }
}

/// An event that has no edge from the current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IllegalTransition {
    /// The state the instrument was in.
    pub from: State,
    /// The event that could not be applied.
    pub event: Event,
}

impl IllegalTransition {
    /// The SCPI-99 error an illegal transition reports.
    #[must_use]
    pub const fn scpi_error(self) -> ScpiError {
        ScpiError::SettingsConflict
    }
}

impl fmt::Display for IllegalTransition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot apply {:?} while {}", self.event, self.from)
    }
}

impl std::error::Error for IllegalTransition {}

/// Apply `event` to `state`.
///
/// `Reset` always lands in [`State::Idle`], and `Abort` outside a run is the documented
/// no-op that standard SCPI leniency requires.
///
/// # Errors
/// [`IllegalTransition`] when the edge does not exist — most importantly `INIT` while a run
/// is already in flight, which the UTS sees as `-221`.
pub fn transition(state: State, event: Event) -> Result<State, IllegalTransition> {
    use Event::{Abort, Completed, FirstSample, Init, Reset};
    use State::{Aborted, Armed, Complete, Idle, Recording};

    let next = match (state, event) {
        (_, Reset) => Idle,

        (Idle | Complete | Aborted, Init) => Armed,
        (Armed | Recording, Init) => return Err(IllegalTransition { from: state, event }),

        (Armed, FirstSample) => Recording,
        // A batch after the first does not re-transition.
        (Recording, FirstSample) => Recording,
        (Idle | Complete | Aborted, FirstSample) => {
            return Err(IllegalTransition { from: state, event })
        }

        (Recording, Completed) => Complete,
        // A run whose whole capture arrived in one batch never observes `Recording`.
        (Armed, Completed) => Complete,
        (Idle | Complete | Aborted, Completed) => {
            return Err(IllegalTransition { from: state, event })
        }

        (Armed | Recording, Abort) => Aborted,
        // ABOR outside a run is a no-op, not an error.
        (Idle, Abort) => Idle,
        (Complete, Abort) => Complete,
        (Aborted, Abort) => Aborted,
    };
    Ok(next)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn wire_forms_match_the_reference() {
        assert_eq!(State::Idle.as_scpi_str(), "IDLE");
        assert_eq!(State::Armed.as_scpi_str(), "ARMED");
        assert_eq!(State::Recording.as_scpi_str(), "RECORDING");
        assert_eq!(State::Complete.as_scpi_str(), "COMPLETE");
        assert_eq!(State::Aborted.as_scpi_str(), "ABORTED");
    }

    #[test]
    fn the_happy_path_walks_the_diagram() {
        let s = transition(State::Idle, Event::Init).unwrap();
        assert_eq!(s, State::Armed);
        let s = transition(s, Event::FirstSample).unwrap();
        assert_eq!(s, State::Recording);
        let s = transition(s, Event::Completed).unwrap();
        assert_eq!(s, State::Complete);
    }

    #[test]
    fn init_while_running_is_a_settings_conflict() {
        for from in [State::Armed, State::Recording] {
            let err = transition(from, Event::Init).unwrap_err();
            assert_eq!(err.scpi_error(), ScpiError::SettingsConflict);
        }
    }

    #[test]
    fn a_new_run_can_start_from_complete_or_aborted() {
        assert_eq!(
            transition(State::Complete, Event::Init).unwrap(),
            State::Armed
        );
        assert_eq!(
            transition(State::Aborted, Event::Init).unwrap(),
            State::Armed
        );
    }

    #[test]
    fn reset_from_every_state_lands_in_idle() {
        for state in State::all() {
            assert_eq!(transition(*state, Event::Reset).unwrap(), State::Idle);
        }
    }

    #[test]
    fn abort_outside_a_run_is_a_no_op() {
        for state in [State::Idle, State::Complete, State::Aborted] {
            assert_eq!(transition(state, Event::Abort).unwrap(), state);
        }
    }

    #[test]
    fn abort_during_a_run_lands_in_aborted() {
        for state in [State::Armed, State::Recording] {
            assert_eq!(transition(state, Event::Abort).unwrap(), State::Aborted);
        }
    }

    #[test]
    fn the_full_state_event_table_is_covered() {
        // Exhaustive over both enums: every pair either has a documented target or is a
        // documented rejection. The point is that nothing panics and nothing is forgotten.
        let mut legal = 0;
        let mut rejected = 0;
        for state in State::all() {
            for event in Event::all() {
                match transition(*state, *event) {
                    Ok(_) => legal += 1,
                    Err(e) => {
                        assert_eq!(e.from, *state);
                        assert_eq!(e.event, *event);
                        rejected += 1;
                    }
                }
            }
        }
        assert_eq!(legal + rejected, State::all().len() * Event::all().len());
        // Init from Armed/Recording, FirstSample from Idle/Complete/Aborted, and Completed
        // from Idle/Complete/Aborted.
        assert_eq!(rejected, 8);
    }

    #[test]
    fn is_running_only_covers_armed_and_recording() {
        assert!(State::Armed.is_running());
        assert!(State::Recording.is_running());
        assert!(!State::Idle.is_running());
        assert!(!State::Complete.is_running());
        assert!(!State::Aborted.is_running());
    }
}
