//! What the window reads back from the running instrument, in one poll.
//!
//! The window repaints several times a second and must never block on the engine lock any
//! longer than it has to, so every field it displays is collected in a single pass into this
//! plain, `Copy`-cheap snapshot rather than queried control by control during layout.

use std::net::SocketAddr;

use quickvib_core::{ExportFormat, SampleUnit, ScpiErrorEntry};
use quickvib_engine::State;
use quickvib_measure::MeasurementSet;

use crate::i18n::{self, Label, Lang};

/// One consistent reading of the instrument, taken between repaints.
#[derive(Debug, Clone, PartialEq)]
pub struct StatusSnapshot {
    /// The instrument state, exactly as `REC:STAT?` reports it.
    pub state: State,
    /// Whether the device backend reports a live transport.
    pub connected: bool,
    /// The peer on the inbound device link, when one has dialed in.
    pub device_peer: Option<SocketAddr>,
    /// `*IDN?`, so the window shows the identity the UTS will see.
    pub identity: String,
    /// Name of the loaded project, empty when none is loaded.
    pub project_name: String,
    /// The unit the loaded project configures the device for, which is the unit every
    /// measurement on screen carries. `None` when no project is loaded.
    pub unit: Option<SampleUnit>,
    /// The sample rate of the loaded project, in hertz. `0.0` when no project is loaded.
    pub sample_rate_hz: f64,
    /// The record duration in force, including any `CONF:REC:DUR` override.
    pub duration_seconds: f64,
    /// The export format in force, including any `FORM` override.
    pub format: ExportFormat,
    /// Peak, RMS and peak-to-peak of the last completed capture.
    pub measurements: Option<MeasurementSet>,
    /// Sample count of the last completed capture.
    pub sample_count: usize,
    /// How many SCPI errors are queued and unread.
    pub pending_errors: usize,
    /// How many SCPI sessions are connected.
    pub sessions: usize,
}

impl StatusSnapshot {
    /// Whether a run is in flight, which is what greys out the form.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.state.is_running()
    }

    /// A one-line summary for the status bar, in `lang`: 录制中 · 已连接.
    #[must_use]
    pub fn headline(&self, lang: Lang) -> String {
        format!(
            "{} · {}",
            i18n::state_label(lang, self.state),
            lang.t(self.link_label())
        )
    }

    /// Which of 已连接 / 未连接 describes the device link.
    #[must_use]
    pub const fn link_label(&self) -> Label {
        if self.connected {
            Label::Connected
        } else {
            Label::Disconnected
        }
    }
}

/// A message shown in the window's activity line: the outcome of the last thing the operator
/// asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Whether the action succeeded.
    pub ok: bool,
    /// What happened.
    pub text: String,
}

impl Notice {
    /// A success message.
    #[must_use]
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            ok: true,
            text: text.into(),
        }
    }

    /// A failure message.
    #[must_use]
    pub fn failed(text: impl Into<String>) -> Self {
        Self {
            ok: false,
            text: text.into(),
        }
    }

    /// A failure message built from a popped SCPI error.
    #[must_use]
    pub fn from_scpi(action: &str, entry: &ScpiErrorEntry) -> Self {
        Self::failed(format!("{action} failed: {entry}"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn snapshot(state: State, connected: bool) -> StatusSnapshot {
        StatusSnapshot {
            state,
            connected,
            device_peer: None,
            identity: String::new(),
            project_name: String::new(),
            unit: None,
            sample_rate_hz: 0.0,
            duration_seconds: 1.0,
            format: ExportFormat::Csv,
            measurements: None,
            sample_count: 0,
            pending_errors: 0,
            sessions: 0,
        }
    }

    #[test]
    fn the_headline_names_the_state_and_the_link_in_chinese() {
        let s = snapshot(State::Recording, true);
        assert_eq!(s.headline(Lang::default()), "录制中 · 已连接");
        assert!(s.is_running());

        let s = snapshot(State::Idle, false);
        assert_eq!(s.headline(Lang::Zh), "空闲 · 未连接");
        assert!(!s.is_running());
    }

    #[test]
    fn the_headline_follows_the_language_switch() {
        let s = snapshot(State::Recording, true);
        assert_eq!(s.headline(Lang::En), "RECORDING · Connected");
    }

    #[test]
    fn notices_carry_their_outcome() {
        assert!(Notice::ok("saved").ok);
        assert!(!Notice::failed("nope").ok);
    }
}
