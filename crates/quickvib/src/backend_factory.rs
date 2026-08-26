//! Backend selection (`docs/PLAN.md` 7.4, 15).
//!
//! Mock is the default. `tcp` records from whatever dials into the device port — a real M300
//! or the `m300-sim` stand-in — using nothing but `std` sockets and the shared framer. The
//! M300 backend is constructed only when it is explicitly selected, the `m300` feature is on,
//! **and** the host is Windows; anything else is a startup error rather than a silent
//! fallback, so a UTS never records mock data believing it came from hardware.
//!
//! The three backends differ in who owns the device port, and that difference is what
//! [`OpenedBackend`] carries back to the caller:
//!
//! | Backend | Device port | Sample path |
//! | --- | --- | --- |
//! | `mock` | bound by QuickVib, idle | generated in process |
//! | `tcp` | bound by QuickVib | listener → framer → [`SampleSink`] → backend |
//! | `m300` | bound by the **SDK** | SDK callback → bounded queue → backend |
//!
//! The last row is why `m300` is not simply "`tcp` with a different reader": QuickVib must
//! leave the port alone, because `m300_server_create_ex` binds and listens on it and the two
//! would otherwise fight over it (`docs/M300-NATIVE.md` §6).

use std::fmt;
use std::sync::Arc;

use quickvib_core::{BackendKind, Clock, SampleUnit};
use quickvib_device::{
    DeviceBackend, DeviceOpenOptions, MockBackend, MockSignalSpec, SampleChannel, SampleSink,
    SignalComponent, StreamBackend,
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

/// An opened backend, plus whatever the caller still has to wire up for it.
pub struct OpenedBackend {
    /// The backend itself, already opened.
    pub backend: Box<dyn DeviceBackend + Send>,
    /// Where the device server must deliver framed samples. `None` for in-process backends,
    /// which need no help from the listener.
    pub sink: Option<Arc<dyn SampleSink>>,
    /// Whether this backend has already bound the device port itself, in which case the
    /// caller must **not** start a [`crate::DeviceServer`] on it. See
    /// [`owns_device_listener`].
    pub owns_device_listener: bool,
}

impl fmt::Debug for OpenedBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenedBackend")
            .field("connected", &self.backend.is_connected())
            .field("socket_fed", &self.sink.is_some())
            .field("owns_device_listener", &self.owns_device_listener)
            .finish()
    }
}

/// Whether the backend binds the device port itself, leaving QuickVib nothing to listen on.
///
/// True only for `m300`. The SDK binds and listens from inside `m300_server_create_ex`, so a
/// [`crate::DeviceServer`] on the same address would lose the race — whichever of the two got
/// there second reports "address already in use", which the SDK surfaces as `-7 ERR_NETWORK`
/// at open (`docs/M300-NATIVE.md` §6). `--device-port` and `--bind` therefore stop being
/// arguments to *our* listener and become arguments to the SDK's.
///
/// This is a property of the kind rather than of the opened backend, so it can be asserted on
/// a Linux host that has no SDK to open.
#[must_use]
pub const fn owns_device_listener(kind: BackendKind) -> bool {
    match kind {
        BackendKind::Mock | BackendKind::Tcp => false,
        BackendKind::M300 => true,
    }
}

/// Create and open the backend.
///
/// `bind_host` and `device_port` are the resolved listen address. For `mock` and `tcp` they
/// describe the listener the caller is about to bind and the backend merely records them; for
/// `m300` they are handed straight to `m300_server_create_ex`, because there the SDK is the
/// listener.
///
/// # Errors
/// [`BackendError::Unavailable`] when `m300` is selected on a host or build that cannot
/// provide it, and [`BackendError::Open`] when the backend refuses to open.
pub fn create(
    kind: BackendKind,
    project: Option<&Project>,
    clock: Arc<dyn Clock>,
    bind_host: &str,
    device_port: u16,
    pace: bool,
) -> Result<OpenedBackend, BackendError> {
    let options = open_options(project, bind_host, device_port);

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
            Ok(OpenedBackend {
                backend: Box::new(backend),
                sink: None,
                owns_device_listener: false,
            })
        }
        BackendKind::Tcp => {
            // The device server owns the socket and the framer; this backend only drains the
            // channel between them, so there is nothing platform-specific and nothing unsafe
            // on the whole path.
            // The serial stays "0": nothing on this wire protocol reports one, so `*IDN?`
            // falls back to the project's `identity.serialNumber` rather than inventing one.
            let channel = Arc::new(SampleChannel::default());
            let mut backend =
                StreamBackend::new(Arc::clone(&channel), clock).with_model("QuickVib-Stream");
            backend
                .open(&options)
                .map_err(|source| BackendError::Open { kind, source })?;
            Ok(OpenedBackend {
                backend: Box::new(backend),
                sink: Some(channel as Arc<dyn SampleSink>),
                owns_device_listener: false,
            })
        }
        BackendKind::M300 => open_m300(kind, &options, clock),
    }
}

/// Fold the project and the resolved listen address into the options every backend opens
/// with.
fn open_options(project: Option<&Project>, bind_host: &str, device_port: u16) -> DeviceOpenOptions {
    let sample_rate = project.map_or(1000.0, |p| p.device.sample_rate_hz);
    let unit = project.map_or(SampleUnit::VelocityUmPerSec, |p| p.device.unit);

    let mut options = DeviceOpenOptions::new(sample_rate, unit);
    options.bind_host = bind_host.to_owned();
    options.device_port = device_port;
    if let Some(project) = project {
        options.allowed_peers = project.device.allowed_peers.clone();
        options.connect_timeout =
            std::time::Duration::from_secs_f64(project.device.connect_timeout_seconds.max(0.0));
        options.sdk_path = project.device.sdk_path.clone();
    }
    options
}

/// Open the native SDK backend, or say why this host or build cannot.
///
/// Refusing is deliberately not a fallback to the mock: a UTS that asked for hardware and
/// silently got a signal generator would record plausible numbers that mean nothing
/// (`docs/PLAN.md` 15.3).
fn open_m300(
    kind: BackendKind,
    _options: &DeviceOpenOptions,
    _clock: Arc<dyn Clock>,
) -> Result<OpenedBackend, BackendError> {
    let reason = if !cfg!(windows) {
        "the M300 backend requires a Windows host"
    } else if !cfg!(feature = "m300") {
        "this build was compiled without the 'm300' feature"
    } else {
        // The Windows FFI backend arrives in Phase 6, gated on the SDK questions in
        // docs/PLAN.md 21.1.
        "the M300 backend is not implemented in this build"
    };
    Err(BackendError::Unavailable {
        kind,
        reason: reason.to_owned(),
    })
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
        let opened = create(
            BackendKind::Mock,
            Some(&project),
            clock(),
            "127.0.0.1",
            9123,
            false,
        )
        .unwrap();
        assert!(opened.backend.is_connected());
        assert!(opened.sink.is_none(), "the mock needs no listener");
        let capabilities = opened.backend.capabilities().unwrap();
        assert_eq!(capabilities.sample_rate_hz, 2000.0);
        assert_eq!(capabilities.unit, SampleUnit::DisplacementUm);
    }

    #[test]
    fn the_mock_backend_opens_without_a_project() {
        let opened = create(BackendKind::Mock, None, clock(), "127.0.0.1", 9123, false).unwrap();
        assert!(opened.backend.is_connected());
    }

    #[test]
    fn the_tcp_backend_opens_disconnected_and_asks_for_a_sink() {
        let project = Project::from_json_str(PROJECT).unwrap();
        let opened = create(
            BackendKind::Tcp,
            Some(&project),
            clock(),
            "127.0.0.1",
            9123,
            false,
        )
        .unwrap();
        // Nothing has dialled in yet, so `SYST:DEV:CONN?` is `0` and `INIT` is `-241`.
        assert!(!opened.backend.is_connected());
        let capabilities = opened.backend.capabilities().unwrap();
        assert_eq!(capabilities.sample_rate_hz, 2000.0);
        assert_eq!(capabilities.unit, SampleUnit::DisplacementUm);

        let sink = opened.sink.expect("the tcp backend is fed by the listener");
        sink.on_connected(None);
        assert!(opened.backend.is_connected());
    }

    #[test]
    fn the_tcp_backend_needs_no_project() {
        let opened = create(BackendKind::Tcp, None, clock(), "127.0.0.1", 9123, false).unwrap();
        assert!(opened.sink.is_some());
        assert_eq!(
            opened.backend.capabilities().unwrap().sample_rate_hz,
            1000.0
        );
    }

    #[test]
    fn selecting_m300_here_is_an_error_not_a_silent_fallback() {
        let Err(error) = create(BackendKind::M300, None, clock(), "127.0.0.1", 9123, false) else {
            panic!("the M300 backend must not be constructible here");
        };
        assert!(matches!(error, BackendError::Unavailable { .. }));
        assert!(error.to_string().contains("m300"));
    }

    #[test]
    fn only_the_m300_backend_brings_its_own_listener() {
        assert!(!owns_device_listener(BackendKind::Mock));
        assert!(!owns_device_listener(BackendKind::Tcp));
        assert!(owns_device_listener(BackendKind::M300));
    }

    #[test]
    fn a_backend_that_the_factory_opens_here_always_wants_our_listener() {
        // The counterpart of the assertion above, on the value the app layer actually reads:
        // both backends that can be opened on this host leave the device port to QuickVib.
        for kind in [BackendKind::Mock, BackendKind::Tcp] {
            let opened = create(kind, None, clock(), "127.0.0.1", 9123, false).unwrap();
            assert!(!opened.owns_device_listener, "for {kind}");
            assert_eq!(opened.owns_device_listener, owns_device_listener(kind));
        }
    }

    #[test]
    fn the_project_can_select_the_tcp_backend() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.device.backend = BackendKind::Tcp;
        assert_eq!(select_kind(None, Some(&project)), BackendKind::Tcp);
    }

    #[test]
    fn the_resolved_listen_address_reaches_the_open_options() {
        // For `m300` these two fields are the arguments to `m300_server_create_ex`, so a
        // `--bind` that stopped here would put the SDK on every interface after the operator
        // asked for one (docs/M300-NATIVE.md 6).
        let options = open_options(None, "127.0.0.1", 4321);
        assert_eq!(options.bind_host, "127.0.0.1");
        assert_eq!(options.device_port, 4321);
    }

    #[test]
    fn the_projects_sdk_path_and_peer_list_reach_the_open_options() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.device.sdk_path = Some(std::path::PathBuf::from("C:\\M300\\bin"));
        project.device.allowed_peers = vec!["10.0.0.7".to_owned()];

        let options = open_options(Some(&project), "0.0.0.0", 9123);
        assert_eq!(
            options.sdk_path,
            Some(std::path::PathBuf::from("C:\\M300\\bin"))
        );
        assert_eq!(options.allowed_peers, vec!["10.0.0.7".to_owned()]);
        assert_eq!(options.sample_rate_hz, 2000.0);
        assert_eq!(options.unit, SampleUnit::DisplacementUm);
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
