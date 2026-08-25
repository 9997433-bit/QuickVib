//! The composition root (`docs/PLAN.md` 7.1).
//!
//! Everything the process needs is wired here and nowhere else: resolve the project, choose
//! and open the backend, build the engine, bind both listeners, then run until cancelled.
//! The binary's `main` is a thin shell over this, and the integration suite drives the very
//! same type in-process on ephemeral ports, so the code under test is the code that ships.

use std::path::PathBuf;
use std::sync::Arc;

use quickvib_core::{BackendKind, CancelToken, Clock, Level, Logger, SystemClock};
use quickvib_device::{ConnectionState, DeviceBackend};
use quickvib_engine::{Engine, EngineConfig};
use quickvib_project::{LastProjectStore, Project, ProjectStore};

use crate::backend_factory::{self, BackendError};
use crate::cli::{Options, DEFAULT_DEVICE_PORT};
use crate::device_server::{DeviceServer, LinkStats};
use crate::log::LineLogger;
use crate::scpi_server::ScpiServer;

/// Exit code for a clean shutdown.
pub const EXIT_OK: i32 = 0;
/// Exit code for a bad command line.
pub const EXIT_BAD_ARGUMENTS: i32 = 2;
/// Exit code for a port that could not be bound.
pub const EXIT_BIND_FAILURE: i32 = 3;
/// Exit code for an explicitly requested project that would not load.
pub const EXIT_PROJECT_FAILURE: i32 = 4;
/// Exit code for a backend that would not open.
pub const EXIT_BACKEND_FAILURE: i32 = 5;

/// Why startup stopped, and with which exit code (`docs/PLAN.md` 7.1).
#[derive(Debug)]
#[non_exhaustive]
pub enum StartupError {
    /// The requested backend is unavailable on this host or in this build. Exits `2`.
    BadArguments(BackendError),
    /// A listener could not be bound. Exits `3`.
    Bind {
        /// Which listener.
        what: &'static str,
        /// The address that was attempted.
        addr: String,
        /// What the operating system said.
        source: std::io::Error,
    },
    /// `--project` was given and the file would not load. Exits `4`.
    Project {
        /// The path that was attempted.
        path: PathBuf,
        /// What the project store said.
        source: quickvib_project::ProjectError,
    },
    /// The backend refused to open. Exits `5`.
    Backend(BackendError),
}

impl StartupError {
    /// The process exit code for this failure.
    #[must_use]
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::BadArguments(_) => EXIT_BAD_ARGUMENTS,
            Self::Bind { .. } => EXIT_BIND_FAILURE,
            Self::Project { .. } => EXIT_PROJECT_FAILURE,
            Self::Backend(_) => EXIT_BACKEND_FAILURE,
        }
    }
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadArguments(source) | Self::Backend(source) => write!(f, "{source}"),
            Self::Bind { what, addr, source } => {
                write!(f, "could not bind the {what} listener on {addr}: {source}")
            }
            Self::Project { path, source } => {
                write!(f, "could not load {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for StartupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::BadArguments(source) | Self::Backend(source) => Some(source),
            Self::Bind { source, .. } => Some(source),
            Self::Project { source, .. } => Some(source),
        }
    }
}

/// Assembles an [`App`].
///
/// Defaults match the shipped binary; the setters exist for the integration suite, which
/// needs a virtual clock, a capturing logger, an injected faulty backend and a smaller
/// session cap.
pub struct AppBuilder {
    options: Options,
    bind_host: String,
    clock: Option<Arc<dyn Clock>>,
    logger: Option<Arc<dyn Logger>>,
    backend: Option<Box<dyn DeviceBackend + Send>>,
    last_project: Option<Option<LastProjectStore>>,
    max_sessions: Option<usize>,
    fault_injection: bool,
    pace_mock: bool,
}

impl std::fmt::Debug for AppBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppBuilder")
            .field("options", &self.options)
            .field("bind_host", &self.bind_host)
            .finish_non_exhaustive()
    }
}

impl AppBuilder {
    /// A builder for these options, with the production clock and a stdio logger.
    #[must_use]
    pub fn new(options: Options) -> Self {
        Self {
            options,
            bind_host: "0.0.0.0".to_owned(),
            clock: None,
            logger: None,
            backend: None,
            last_project: None,
            max_sessions: None,
            fault_injection: false,
            pace_mock: true,
        }
    }

    /// Bind the listeners on this host instead of every interface.
    #[must_use]
    pub fn bind_host(mut self, host: impl Into<String>) -> Self {
        self.bind_host = host.into();
        self
    }

    /// Use an injected clock (D20).
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = Some(clock);
        self
    }

    /// Send log lines somewhere other than stdio.
    #[must_use]
    pub fn logger(mut self, logger: Arc<dyn Logger>) -> Self {
        self.logger = Some(logger);
        self
    }

    /// Use an already-opened backend instead of asking the factory for one.
    #[must_use]
    pub fn backend(mut self, backend: Box<dyn DeviceBackend + Send>) -> Self {
        self.backend = Some(backend);
        self
    }

    /// Override where the auto-load-last record lives. `None` disables the feature.
    #[must_use]
    pub fn last_project(mut self, store: Option<LastProjectStore>) -> Self {
        self.last_project = Some(store);
        self
    }

    /// Override the concurrent session cap from the project's `server.maxSessions`.
    #[must_use]
    pub fn max_sessions(mut self, max: usize) -> Self {
        self.max_sessions = Some(max);
        self
    }

    /// Enable the test-only fault-injection command.
    #[must_use]
    pub fn fault_injection(mut self, enabled: bool) -> Self {
        self.fault_injection = enabled;
        self
    }

    /// Whether the mock backend delivers samples in real time. On with the system clock so a
    /// "5 second" capture takes five seconds; harmless with a test clock, which satisfies a
    /// sleep by advancing virtual time.
    #[must_use]
    pub fn pace_mock(mut self, pace: bool) -> Self {
        self.pace_mock = pace;
        self
    }

    /// Resolve everything and bind both listeners.
    ///
    /// # Errors
    /// [`StartupError`], whose [`StartupError::exit_code`] is the process exit code.
    pub fn build(self) -> Result<App, StartupError> {
        let clock: Arc<dyn Clock> = self.clock.unwrap_or_else(|| Arc::new(SystemClock::new()));
        let logger: Arc<dyn Logger> = self
            .logger
            .unwrap_or_else(|| Arc::new(LineLogger::stdio(self.options.log_level, clock.clone())));
        let last_project = self.last_project.unwrap_or_else(LastProjectStore::discover);

        let project = resolve_project(&self.options, &last_project, &logger)?;
        let device_port = resolve_device_port(&self.options, project.as_ref());

        let kind = backend_factory::select_kind(self.options.backend, project.as_ref());
        let (backend, sink) = match self.backend {
            Some(backend) => (backend, None),
            None => {
                let opened = backend_factory::create(
                    kind,
                    project.as_ref(),
                    Arc::clone(&clock),
                    device_port,
                    self.pace_mock,
                )
                .map_err(|e| match e {
                    BackendError::Unavailable { .. } => StartupError::BadArguments(e),
                    BackendError::Open { .. } => StartupError::Backend(e),
                })?;
                (opened.backend, opened.sink)
            }
        };

        let engine = Engine::new(EngineConfig {
            clock,
            logger: Arc::clone(&logger),
            backend,
            last_project,
            version: env!("CARGO_PKG_VERSION").to_owned(),
        });
        if let Some(project) = project.clone() {
            let path = self.options.project.clone().or_else(|| {
                // An auto-loaded project still records where it came from.
                engine.auto_load_path()
            });
            engine.adopt_project(project, path);
        }

        let max_sessions = self
            .max_sessions
            .or(project.as_ref().map(|p| p.server.max_sessions))
            .unwrap_or(crate::scpi_server::DEFAULT_MAX_SESSIONS);

        let scpi_addr = format!("{}:{}", self.bind_host, self.options.scpi_port);
        let scpi = ScpiServer::bind(&scpi_addr, Arc::clone(&engine), Arc::clone(&logger))
            .map_err(|source| StartupError::Bind {
                what: "SCPI",
                addr: scpi_addr,
                source,
            })?
            .with_max_sessions(max_sessions)
            .with_fault_injection(self.fault_injection);

        let device_addr = format!("{}:{device_port}", self.bind_host);
        let allowed_peers = project
            .as_ref()
            .map(|p| p.device.allowed_peers.clone())
            .unwrap_or_default();
        let mut device = DeviceServer::bind(&device_addr, allowed_peers, Arc::clone(&logger))
            .map_err(|source| StartupError::Bind {
                what: "device",
                addr: device_addr,
                source,
            })?;
        // A socket-fed backend has no transport of its own: the listener bound just above is
        // its transport, so the framed samples have to be handed across.
        if let Some(sink) = sink {
            device = device.with_sink(sink);
        }

        Ok(App {
            engine,
            scpi,
            device,
            logger,
            cancel: CancelToken::new(),
            backend_kind: kind,
            headless: self.options.headless,
        })
    }
}

/// Resolve the inbound device port: `--device-port` wins, then the project, then the default.
#[must_use]
pub fn resolve_device_port(options: &Options, project: Option<&Project>) -> u16 {
    options
        .device_port
        .or_else(|| project.map(|p| p.device.port))
        .unwrap_or(DEFAULT_DEVICE_PORT)
}

/// Resolve the project to open: `--project`, then auto-load-last, then nothing.
fn resolve_project(
    options: &Options,
    last_project: &Option<LastProjectStore>,
    logger: &Arc<dyn Logger>,
) -> Result<Option<Project>, StartupError> {
    if let Some(path) = &options.project {
        let project = ProjectStore::load(path).map_err(|source| StartupError::Project {
            path: path.clone(),
            source,
        })?;
        if let Some(store) = last_project {
            if let Err(e) = store.record(path) {
                log(
                    logger,
                    Level::Warn,
                    "proj",
                    format!("could not record: {e}"),
                );
            }
        }
        log(
            logger,
            Level::Info,
            "proj",
            format!("loaded path={} name={}", path.display(), project.name),
        );
        return Ok(Some(project));
    }

    if options.no_auto_load {
        return Ok(None);
    }

    let Some(path) = last_project
        .as_ref()
        .and_then(LastProjectStore::read_existing)
    else {
        log(logger, Level::Info, "proj", "no project loaded at startup");
        return Ok(None);
    };

    // A stale auto-load record is a log line, never a fatal startup error.
    match ProjectStore::load(&path) {
        Ok(project) => {
            log(
                logger,
                Level::Info,
                "proj",
                format!("auto-loaded path={} name={}", path.display(), project.name),
            );
            Ok(Some(project))
        }
        Err(e) => {
            log(
                logger,
                Level::Warn,
                "proj",
                format!("auto-load skipped path={}: {e}", path.display()),
            );
            Ok(None)
        }
    }
}

fn log(logger: &Arc<dyn Logger>, level: Level, component: &str, message: impl AsRef<str>) {
    if logger.enabled(level) {
        logger.log(level, component, message.as_ref());
    }
}

/// A fully wired instrument: engine, SCPI server and device server, both ports already bound.
pub struct App {
    engine: Arc<Engine>,
    scpi: ScpiServer,
    device: DeviceServer,
    logger: Arc<dyn Logger>,
    cancel: CancelToken,
    backend_kind: BackendKind,
    headless: bool,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("scpi", &self.scpi)
            .field("device", &self.device)
            .field("backend", &self.backend_kind)
            .field("headless", &self.headless)
            .finish()
    }
}

impl App {
    /// The shared instrument model.
    #[must_use]
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// The bound SCPI address, including the OS-assigned port when `0` was requested.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn scpi_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.scpi.local_addr()
    }

    /// The bound device address.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn device_addr(&self) -> std::io::Result<std::net::SocketAddr> {
        self.device.local_addr()
    }

    /// The token that stops both accept loops.
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Counters for the device link: connections, refusals and framed samples.
    #[must_use]
    pub fn link_stats(&self) -> Arc<LinkStats> {
        self.device.stats()
    }

    /// The observable state of the device link.
    #[must_use]
    pub fn link_state(&self) -> Arc<ConnectionState> {
        self.device.state()
    }

    /// The one-line startup banner, printed on the console unless `--headless`.
    ///
    /// # Errors
    /// Any failure reading either socket name.
    pub fn banner(&self) -> std::io::Result<String> {
        Ok(format!(
            "QuickVib {} ready: SCPI on port {}, device on port {}, backend {}",
            env!("CARGO_PKG_VERSION"),
            self.scpi_addr()?.port(),
            self.device_addr()?.port(),
            self.backend_kind
        ))
    }

    /// Whether the banner should be suppressed.
    #[must_use]
    pub const fn headless(&self) -> bool {
        self.headless
    }

    /// Which backend this process opened.
    #[must_use]
    pub const fn backend(&self) -> BackendKind {
        self.backend_kind
    }

    /// Run both accept loops until the cancellation token fires.
    ///
    /// The device accept loop gets its own thread; this one hosts the SCPI accept loop, so
    /// `main` blocks here for the life of the process.
    pub fn run(&self) {
        let cancel = self.cancel.clone();
        let device = &self.device;
        let scpi = &self.scpi;

        std::thread::scope(|scope| {
            scope.spawn(|| device.run(&cancel));
            scpi.run(&cancel);
        });

        // A run that ends with a capture still in flight would otherwise leave the reader
        // thread streaming into a buffer nobody will ever read.
        self.engine.abort();
        if self.logger.enabled(Level::Info) {
            self.logger.log(Level::Info, "app", "shutdown complete");
        }
    }

    /// Run on a background thread, returning a handle the caller can shut down.
    ///
    /// This is what the integration suite uses: it keeps the addresses and the engine to
    /// hand while the server runs.
    #[must_use]
    pub fn spawn(self) -> AppHandle {
        let cancel = self.cancel.clone();
        let engine = Arc::clone(&self.engine);
        let link_stats = self.link_stats();
        let link_state = self.link_state();
        let scpi_addr = self.scpi_addr().ok();
        let device_addr = self.device_addr().ok();
        let join = std::thread::Builder::new()
            .name("quickvib-app".to_owned())
            .spawn(move || self.run())
            .ok();

        AppHandle {
            cancel,
            engine,
            link_stats,
            link_state,
            scpi_addr,
            device_addr,
            join,
        }
    }
}

/// A running [`App`] on its own thread.
#[derive(Debug)]
pub struct AppHandle {
    cancel: CancelToken,
    engine: Arc<Engine>,
    link_stats: Arc<LinkStats>,
    link_state: Arc<ConnectionState>,
    scpi_addr: Option<std::net::SocketAddr>,
    device_addr: Option<std::net::SocketAddr>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl AppHandle {
    /// The shared instrument model.
    #[must_use]
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// Counters for the device link.
    #[must_use]
    pub fn link_stats(&self) -> &Arc<LinkStats> {
        &self.link_stats
    }

    /// The observable state of the device link.
    #[must_use]
    pub fn link_state(&self) -> &Arc<ConnectionState> {
        &self.link_state
    }

    /// The address the SCPI server is listening on.
    #[must_use]
    pub fn scpi_addr(&self) -> std::net::SocketAddr {
        self.scpi_addr.unwrap_or(unspecified())
    }

    /// The address the device server is listening on.
    #[must_use]
    pub fn device_addr(&self) -> std::net::SocketAddr {
        self.device_addr.unwrap_or(unspecified())
    }

    /// Block until the accept loops stop on their own.
    ///
    /// The GUI path uses this when the window could not be opened: the servers are already
    /// running behind it, and the process should go on serving the UTS from the console
    /// rather than exit because there was no display.
    pub fn wait(mut self) {
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }

    /// Stop both accept loops and wait for the thread to finish.
    pub fn shutdown(mut self) {
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for AppHandle {
    fn drop(&mut self) {
        // A test that forgets to shut down must not leave an accept loop running for the
        // rest of the process.
        self.cancel.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn unspecified() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([0, 0, 0, 0], 0))
}

/// Convenience for `main`: build the app for `options` with production defaults.
///
/// # Errors
/// [`StartupError`], whose [`StartupError::exit_code`] is the process exit code.
pub fn build(options: Options) -> Result<App, StartupError> {
    AppBuilder::new(options).build()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::{NullLogger, TestClock};

    const PROJECT: &str = r#"{
        "schemaVersion": 1,
        "name": "AppTest",
        "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
        "recording": { "durationSeconds": 0.02 }
    }"#;

    fn options() -> Options {
        Options {
            scpi_port: 0,
            device_port: Some(0),
            no_auto_load: true,
            ..Options::default()
        }
    }

    fn builder(options: Options) -> AppBuilder {
        AppBuilder::new(options)
            .bind_host("127.0.0.1")
            .clock(Arc::new(TestClock::at_epoch()))
            .logger(Arc::new(NullLogger))
            .last_project(None)
    }

    #[test]
    fn an_app_starts_without_a_project() {
        let app = builder(options()).build().unwrap();
        assert!(app.engine().project().is_none());
        assert!(app.scpi_addr().unwrap().port() > 0);
        assert!(app.device_addr().unwrap().port() > 0);
        assert!(app.banner().unwrap().contains("QuickVib"));
    }

    #[test]
    fn an_explicit_project_is_loaded_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Startup.proj");
        std::fs::write(&path, PROJECT).unwrap();

        let app = builder(Options {
            project: Some(path.clone()),
            ..options()
        })
        .build()
        .unwrap();

        assert_eq!(app.engine().project().unwrap().name, "AppTest");
        assert_eq!(app.engine().project_path(), Some(path));
    }

    #[test]
    fn a_missing_explicit_project_is_fatal_with_exit_4() {
        let dir = tempfile::tempdir().unwrap();
        let error = builder(Options {
            project: Some(dir.path().join("nope.proj")),
            ..options()
        })
        .build()
        .unwrap_err();
        assert_eq!(error.exit_code(), EXIT_PROJECT_FAILURE);
    }

    #[test]
    fn selecting_m300_on_this_host_is_fatal_with_exit_2() {
        let error = builder(Options {
            backend: Some(BackendKind::M300),
            ..options()
        })
        .build()
        .unwrap_err();
        assert_eq!(error.exit_code(), EXIT_BAD_ARGUMENTS);
    }

    #[test]
    fn a_port_already_in_use_is_fatal_with_exit_3() {
        let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = squatter.local_addr().unwrap().port();
        let error = builder(Options {
            scpi_port: port,
            ..options()
        })
        .build()
        .unwrap_err();
        assert_eq!(error.exit_code(), EXIT_BIND_FAILURE);
    }

    #[test]
    fn the_device_port_falls_back_from_the_cli_to_the_project_to_the_default() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.device.port = 4321;

        let silent = Options {
            device_port: None,
            ..Options::default()
        };
        assert_eq!(resolve_device_port(&silent, Some(&project)), 4321);
        assert_eq!(resolve_device_port(&silent, None), DEFAULT_DEVICE_PORT);

        let overridden = Options {
            device_port: Some(6000),
            ..Options::default()
        };
        assert_eq!(resolve_device_port(&overridden, Some(&project)), 6000);
    }

    #[test]
    fn auto_load_reopens_the_recorded_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Auto.proj");
        std::fs::write(&path, PROJECT).unwrap();
        let store = LastProjectStore::new(dir.path().join("state"));
        store.record(&path).unwrap();

        let app = AppBuilder::new(Options {
            no_auto_load: false,
            ..options()
        })
        .bind_host("127.0.0.1")
        .clock(Arc::new(TestClock::at_epoch()))
        .logger(Arc::new(NullLogger))
        .last_project(Some(store))
        .build()
        .unwrap();

        assert_eq!(app.engine().project().unwrap().name, "AppTest");
    }

    #[test]
    fn a_stale_auto_load_record_is_skipped_rather_than_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Gone.proj");
        std::fs::write(&path, PROJECT).unwrap();
        let store = LastProjectStore::new(dir.path().join("state"));
        store.record(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        let app = AppBuilder::new(Options {
            no_auto_load: false,
            ..options()
        })
        .bind_host("127.0.0.1")
        .clock(Arc::new(TestClock::at_epoch()))
        .logger(Arc::new(NullLogger))
        .last_project(Some(store))
        .build()
        .unwrap();

        assert!(app.engine().project().is_none());
    }

    #[test]
    fn no_auto_load_suppresses_the_recorded_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Auto.proj");
        std::fs::write(&path, PROJECT).unwrap();
        let store = LastProjectStore::new(dir.path().join("state"));
        store.record(&path).unwrap();

        let app = AppBuilder::new(options())
            .bind_host("127.0.0.1")
            .clock(Arc::new(TestClock::at_epoch()))
            .logger(Arc::new(NullLogger))
            .last_project(Some(store))
            .build()
            .unwrap();

        assert!(app.engine().project().is_none());
    }
}
