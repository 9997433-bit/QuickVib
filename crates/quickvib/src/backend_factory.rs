//! Backend selection (`docs/PLAN.md` 7.4, 15).
//!
//! Mock is the default. The M300 backend is constructed only when it is explicitly selected,
//! the `m300` feature is on, **and** the host is Windows; anything else is a startup error
//! rather than a silent fallback, so a UTS never records mock data believing it came from
//! hardware.

use std::fmt;
use std::sync::Arc;

use quickvib_core::{BackendKind, Clock, SampleUnit};
use quickvib_device::{
    DeviceBackend, DeviceOpenOptions, MockBackend, MockSignalSpec, SignalComponent,
};
use quickvib_project::Project;

/// Why a backend could not be created or opened.
#[derive(Debug)]
#[non_exhaustive]
pub enum BackendError {
    /// The requested backend is not available in this build or on this host.
    Unavailable {
        /// Which backend was asked for.
        kind: BackendKind,
        /// Why it cannot be used.
        reason: String,
    },
    /// The backend exists but refused to open.
    Open {
        /// Which backend was asked for.
        kind: BackendKind,
        /// What the backend reported.
        source: quickvib_device::DeviceError,
    },
}

impl fmt::Display for BackendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { kind, reason } => {
                write!(f, "backend '{kind}' is unavailable: {reason}")
            }
            Self::Open { kind, source } => write!(f, "backend '{kind}' failed to open: {source}"),
        }
    }
}

impl std::error::Error for BackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Open { source, .. } => Some(source),
            Self::Unavailable { .. } => None,
        }
    }
}

/// Decide which backend to use: the `--backend` override wins, then the project, then mock.
#[must_use]
pub fn select_kind(override_kind: Option<BackendKind>, project: Option<&Project>) -> BackendKind {
    override_kind
        .or_else(|| project.map(|p| p.device.backend))
        .unwrap_or_default()
}

/// Create and open the backend.
///
/// # Errors
/// [`BackendError::Unavailable`] when `m300` is selected on a host or build that cannot
/// provide it, and [`BackendError::Open`] when the backend refuses to open.
pub fn create(
    kind: BackendKind,
    project: Option<&Project>,
    clock: Arc<dyn Clock>,
    device_port: u16,
    pace: bool,
) -> Result<Box<dyn DeviceBackend + Send>, BackendError> {
    let sample_rate = project.map_or(1000.0, |p| p.device.sample_rate_hz);
    let unit = project.map_or(SampleUnit::VelocityUmPerSec, |p| p.device.unit);

    let mut options = DeviceOpenOptions::new(sample_rate, unit);
    options.device_port = device_port;
    if let Some(project) = project {
        options.allowed_peers = project.device.allowed_peers.clone();
        options.connect_timeout =
            std::time::Duration::from_secs_f64(project.device.connect_timeout_seconds.max(0.0));
        options.sdk_path = project.device.sdk_path.clone();
    }

    match kind {
        BackendKind::Mock => {
            // Paced against the system clock the mock behaves like an instrument: a
            // five-second capture takes five seconds. Against a test clock a paced stream
            // still finishes instantly, because the clock satisfies a sleep by advancing.
            let mut backend = MockBackend::new(clock)
                .with_signal(mock_signal(project))
                .with_pacing(pace);
            backend
                .open(&options)
                .map_err(|source| BackendError::Open { kind, source })?;
            Ok(Box::new(backend))
        }
        BackendKind::M300 => Err(BackendError::Unavailable {
            kind,
            reason: m300_unavailable_reason(),
        }),
    }
}

/// Why the M300 backend cannot be used in this build.
fn m300_unavailable_reason() -> String {
    if !cfg!(windows) {
        "the M300 backend requires a Windows host".to_owned()
    } else if !cfg!(feature = "m300") {
        "this build was compiled without the 'm300' feature".to_owned()
    } else {
        // The Windows FFI backend arrives in Phase 6, gated on the SDK questions in
        // docs/PLAN.md 21.1.
        "the M300 backend is not implemented in this build".to_owned()
    }
}

/// Translate the project's mock section into the device crate's signal specification.
fn mock_signal(project: Option<&Project>) -> MockSignalSpec {
    let Some(project) = project else {
        return MockSignalSpec::default();
    };
    MockSignalSpec {
        components: project
            .mock
            .signal
            .components
            .iter()
            .map(|c| SignalComponent {
                frequency_hz: c.frequency_hz,
                amplitude: c.amplitude,
                phase_deg: c.phase_deg,
            })
            .collect(),
        noise_std_dev: project.mock.signal.noise_std_dev,
        seed: project.mock.signal.seed,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::TestClock;

    const PROJECT: &str = r#"{
        "schemaVersion": 1,
        "name": "FactoryTest",
        "device": { "backend": "mock", "sampleRateHz": 2000.0, "unit": "displacement_um" },
        "recording": { "durationSeconds": 0.01 },
        "mock": { "signal": {
            "components": [ { "frequencyHz": 7.0, "amplitude": 1.5, "phaseDeg": 45.0 } ],
            "noiseStdDev": 0.25, "seed": 99 } }
    }"#;

    fn clock() -> Arc<dyn Clock> {
        Arc::new(TestClock::at_epoch())
    }

    #[test]
    fn mock_is_the_default_everywhere() {
        assert_eq!(select_kind(None, None), BackendKind::Mock);
    }

    #[test]
    fn the_project_selects_the_backend_when_the_cli_does_not() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.device.backend = BackendKind::M300;
        assert_eq!(select_kind(None, Some(&project)), BackendKind::M300);
    }

    #[test]
    fn the_cli_override_wins_over_the_project() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.device.backend = BackendKind::M300;
        assert_eq!(
            select_kind(Some(BackendKind::Mock), Some(&project)),
            BackendKind::Mock
        );
    }

    #[test]
    fn the_mock_backend_opens_with_the_project_settings() {
        let project = Project::from_json_str(PROJECT).unwrap();
        let backend = create(BackendKind::Mock, Some(&project), clock(), 9123, false).unwrap();
        assert!(backend.is_connected());
        let capabilities = backend.capabilities().unwrap();
        assert_eq!(capabilities.sample_rate_hz, 2000.0);
        assert_eq!(capabilities.unit, SampleUnit::DisplacementUm);
    }

    #[test]
    fn the_mock_backend_opens_without_a_project() {
        let backend = create(BackendKind::Mock, None, clock(), 9123, false).unwrap();
        assert!(backend.is_connected());
    }

    #[test]
    fn selecting_m300_here_is_an_error_not_a_silent_fallback() {
        // `Box<dyn DeviceBackend>` is not `Debug`, so the success arm cannot be unwrapped.
        let Err(error) = create(BackendKind::M300, None, clock(), 9123, false) else {
            panic!("the M300 backend must not be constructible here");
        };
        assert!(matches!(error, BackendError::Unavailable { .. }));
        assert!(error.to_string().contains("m300"));
    }

    #[test]
    fn the_project_signal_reaches_the_mock() {
        let project = Project::from_json_str(PROJECT).unwrap();
        let signal = mock_signal(Some(&project));
        assert_eq!(signal.seed, 99);
        assert_eq!(signal.noise_std_dev, 0.25);
        assert_eq!(signal.components.len(), 1);
        assert_eq!(signal.components[0].frequency_hz, 7.0);
        assert_eq!(signal.components[0].phase_deg, 45.0);
    }
}
