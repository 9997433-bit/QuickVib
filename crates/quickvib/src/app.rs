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
use quickvib_project::{
    join_host_port, LastProjectStore, Project, ProjectStore, DEFAULT_BIND_HOST,
};

use crate::backend_factory::{self, BackendError};
use crate::cli::{Options, DEFAULT_DEVICE_PORT, DEFAULT_SCPI_PORT};
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
    bind_host: Option<String>,
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
            bind_host: None,
            clock: None,
            logger: None,
            backend: None,
            last_project: None,
            max_sessions: None,
            fault_injection: false,
            pace_mock: true,
        }
    }

    /// Bind the listeners on this host, ahead of both `--bind` and the project.
    ///
    /// The integration suite is the caller that needs it: every test binds the loopback
    /// interface whatever the project under test says.
    #[must_use]
    pub fn bind_host(mut self, host: impl Into<String>) -> Self {
        self.bind_host = Some(host.into());
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
        // Resolved before the backend rather than after it: with `--backend m300` the listen
        // address is an argument to `m300_server_create_ex`, so the backend cannot be opened
        // until it is known (`docs/M300-NATIVE.md` §6).
        let bind_host = match self.bind_host {
            Some(host) => host,
            None => resolve_bind_host(&self.options, project.as_ref()),
        };

        let kind = backend_factory::select_kind(self.options.backend, project.as_ref());
        let (backend, sink, sdk_owns_device_port) = match self.backend {
            // An injected backend is already open and has no transport of its own, so the
            // device port stays ours whatever `--backend` said.
            Some(backend) => (backend, None, false),
            None => {
                let opened = backend_factory::create(
                    kind,
                    project.as_ref(),
                    Arc::clone(&clock),
                    Arc::clone(&logger),
                    &bind_host,
                    device_port,
                    self.pace_mock,
                )
                .map_err(|e| match e {
                    BackendError::Unavailable { .. } => StartupError::BadArguments(e),
                    BackendError::Open { .. } => StartupError::Backend(e),
                })?;
                (opened.backend, opened.sink, opened.owns_device_listener)
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
            // Nothing has been started yet, so the in-flight refusal cannot fire here.
            let _ = engine.adopt_project(project, path);
        }

        let max_sessions = self
            .max_sessions
            .or(project.as_ref().map(|p| p.server.max_sessions))
            .unwrap_or(crate::scpi_server::DEFAULT_MAX_SESSIONS);

        let scpi_port = resolve_scpi_port(&self.options, project.as_ref());
        let scpi_addr = join_host_port(&bind_host, scpi_port);
        let scpi = ScpiServer::bind(&scpi_addr, Arc::clone(&engine), Arc::clone(&logger))
            .map_err(|source| StartupError::Bind {
                what: "SCPI",
                addr: scpi_addr,
                source,
            })?
            .with_max_sessions(max_sessions)
            .with_fault_injection(self.fault_injection);

        // The SDK binds the device port itself, and binding it here first would make
        // `m300_server_create_ex` fail with `-7 ERR_NETWORK` — QuickVib would have taken the
        // port away from the backend it just opened (`docs/M300-NATIVE.md` §6). The SCPI
        // server is unaffected: the UTS link is ours on every backend.
        let device = if sdk_owns_device_port {
            log(
                &logger,
                Level::Info,
                "device",
                format!(
                    "listener not started: the M300 SDK owns {}",
                    join_host_port(&bind_host, device_port)
                ),
            );
            None
        } else {
            let device_addr = join_host_port(&bind_host, device_port);
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
            // A socket-fed backend has no transport of its own: the listener bound just
            // above is its transport, so the framed samples have to be handed across.
            if let Some(sink) = sink {
                device = device.with_sink(sink);
            }
            Some(device)
        };

        // With no listener of ours there is nothing to count and no peer to name; the
        // instrument's own answer to `SYST:DEV:CONN?` comes from the backend either way.
        let link_stats = device
            .as_ref()
            .map_or_else(|| Arc::new(LinkStats::default()), DeviceServer::stats);
        let link_state = device
            .as_ref()
            .map_or_else(|| Arc::new(ConnectionState::new()), DeviceServer::state);

        Ok(App {
            engine,
            scpi,
            device,
            device_port,
            link_stats,
            link_state,
            logger,
            cancel: CancelToken::new(),
            backend_kind: kind,
            headless: self.options.headless,
        })
    }
}

/// Resolve the SCPI port: `--scpi-port` wins, then the project, then the default.
#[must_use]
pub fn resolve_scpi_port(options: &Options, project: Option<&Project>) -> u16 {
    options
        .scpi_port
        .or_else(|| project.map(|p| p.server.scpi_port))
        .unwrap_or(DEFAULT_SCPI_PORT)
}

/// Resolve the listen host: `--bind` wins, then the project, then every interface.
#[must_use]
pub fn resolve_bind_host(options: &Options, project: Option<&Project>) -> String {
    options
        .bind_host
        .clone()
        .or_else(|| project.map(|p| p.server.bind_host.clone()))
        .unwrap_or_else(|| DEFAULT_BIND_HOST.to_owned())
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

/// A fully wired instrument: engine, SCPI server and — on every backend but `m300` — the
/// device server, both ports already bound.
pub struct App {
    engine: Arc<Engine>,
    scpi: ScpiServer,
    /// `None` with `--backend m300`, where the SDK is the listener on the device port.
    device: Option<DeviceServer>,
    /// The resolved device port, which is the number the SDK was told to bind when there is
    /// no [`DeviceServer`] to read it back from.
    device_port: u16,
    link_stats: Arc<LinkStats>,
    link_state: Arc<ConnectionState>,
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
            .field("device_port", &self.device_port)
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

    /// The bound device address, or `None` when the backend owns the device port.
    ///
    /// `Ok(None)` is the `--backend m300` case: the SDK bound the port from inside
    /// `m300_server_create_ex`, so there is no listener of ours to name
    /// (`docs/M300-NATIVE.md` §6). Use [`App::device_port`] for the number it was given.
    ///
    /// # Errors
    /// Any failure reading the socket name.
    pub fn device_addr(&self) -> std::io::Result<Option<std::net::SocketAddr>> {
        self.device
            .as_ref()
            .map(DeviceServer::local_addr)
            .transpose()
    }

    /// The device port this process resolved, whoever ended up binding it.
    ///
    /// This is the configured number, so it is `0` — meaning "let the OS choose" — when the
    /// caller asked for an ephemeral port and QuickVib is the listener. [`App::device_addr`]
    /// is the one that reports what the OS actually assigned.
    #[must_use]
    pub const fn device_port(&self) -> u16 {
        self.device_port
    }

    /// Whether QuickVib, rather than the backend, is listening on the device port.
    #[must_use]
    pub const fn owns_device_listener(&self) -> bool {
        self.device.is_some()
    }

    /// The token that stops both accept loops.
    #[must_use]
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel.clone()
    }

    /// Counters for the device link: connections, refusals and framed samples.
    ///
    /// All zero with `--backend m300`, where the traffic never passes through a listener of
    /// ours to be counted.
    #[must_use]
    pub fn link_stats(&self) -> Arc<LinkStats> {
        Arc::clone(&self.link_stats)
    }

    /// The observable state of the device link.
    #[must_use]
    pub fn link_state(&self) -> Arc<ConnectionState> {
        Arc::clone(&self.link_state)
    }

    /// The one-line startup banner, printed on the console unless `--headless`.
    ///
    /// # Errors
    /// Any failure reading either socket name.
    pub fn banner(&self) -> std::io::Result<String> {
        Ok(banner_line(
            self.scpi_addr()?.port(),
            self.device_addr()?.map(|addr| addr.port()),
            self.device_port,
            self.backend_kind,
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
        let device = self.device.as_ref();
        let scpi = &self.scpi;

        std::thread::scope(|scope| {
            // Nothing to accept when the SDK owns the device port; the SCPI loop is the whole
            // job then, and it still runs on this thread so `main` blocks here either way.
            if let Some(device) = device {
                scope.spawn(|| device.run(&cancel));
            }
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
        let device_addr = self.device_addr().ok().flatten();
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

/// The startup banner, as a pure function of the four numbers it reports.
///
/// `bound_device_port` is `None` when the backend owns the device port, in which case the
/// banner falls back to the configured `device_port`: it is the number the SDK was handed,
/// and there is no listener of ours to ask what the OS assigned.
fn banner_line(
    scpi_port: u16,
    bound_device_port: Option<u16>,
    device_port: u16,
    backend: BackendKind,
) -> String {
    let device = match bound_device_port {
        Some(port) => format!("device on port {port}"),
        None => format!("device on port {device_port} (bound by the SDK)"),
    };
    format!(
        "QuickVib {} ready: SCPI on port {scpi_port}, {device}, backend {backend}",
        env!("CARGO_PKG_VERSION")
    )
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
            scpi_port: Some(0),
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
        assert!(app.device_addr().unwrap().unwrap().port() > 0);
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
    fn every_backend_that_opens_here_leaves_the_device_port_to_quickvib() {
        // The inverse of the `m300` rule, stated where it can actually be run: `mock` and
        // `tcp` both get a `DeviceServer`, so nothing about the socket-fed path changed when
        // the listener became optional.
        for kind in [BackendKind::Mock, BackendKind::Tcp] {
            let app = builder(Options {
                backend: Some(kind),
                ..options()
            })
            .build()
            .unwrap();
            assert!(app.owns_device_listener(), "for {kind}");
            assert!(app.device_addr().unwrap().is_some(), "for {kind}");
            assert!(
                !backend_factory::owns_device_listener(kind),
                "the factory and the app must agree for {kind}"
            );
        }
    }

    #[test]
    fn an_injected_backend_keeps_the_device_listener_whatever_backend_was_named() {
        // The integration suite injects an already-open backend and still dials the device
        // port; `--backend m300` must not take that listener away, because no SDK was opened
        // to replace it.
        let mut backend = quickvib_device::MockBackend::new(Arc::new(TestClock::at_epoch()));
        backend
            .open(&quickvib_device::DeviceOpenOptions::new(
                1000.0,
                quickvib_core::SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();

        let app = builder(Options {
            backend: Some(BackendKind::M300),
            ..options()
        })
        .backend(Box::new(backend))
        .build()
        .unwrap();

        assert!(app.owns_device_listener());
        assert!(app.device_addr().unwrap().unwrap().port() > 0);
    }

    #[test]
    fn the_banner_names_the_sdk_when_it_owns_the_device_port() {
        // The `m300` half of the banner, which cannot be produced on a host without the SDK.
        let sdk = banner_line(5025, None, 9123, BackendKind::M300);
        assert!(sdk.contains("SCPI on port 5025"), "{sdk}");
        assert!(
            sdk.contains("device on port 9123 (bound by the SDK)"),
            "{sdk}"
        );
        assert!(sdk.ends_with("backend m300"), "{sdk}");

        // The socket-fed half reports what the OS assigned, not what was configured, so a
        // `--device-port 0` run still names a usable number.
        let ours = banner_line(5025, Some(41_234), 0, BackendKind::Tcp);
        assert!(ours.contains("device on port 41234"), "{ours}");
        assert!(!ours.contains("SDK"), "{ours}");
    }

    #[test]
    fn a_port_already_in_use_is_fatal_with_exit_3() {
        let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = squatter.local_addr().unwrap().port();
        let error = builder(Options {
            scpi_port: Some(port),
            ..options()
        })
        .build()
        .unwrap_err();
        assert_eq!(error.exit_code(), EXIT_BIND_FAILURE);
    }

    #[test]
    fn the_scpi_port_falls_back_from_the_cli_to_the_project_to_the_default() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.server.scpi_port = 5125;

        let silent = Options::default();
        assert_eq!(resolve_scpi_port(&silent, Some(&project)), 5125);
        assert_eq!(resolve_scpi_port(&silent, None), DEFAULT_SCPI_PORT);

        let overridden = Options {
            scpi_port: Some(6000),
            ..Options::default()
        };
        assert_eq!(resolve_scpi_port(&overridden, Some(&project)), 6000);
    }

    #[test]
    fn a_project_scpi_port_is_what_the_listener_binds() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Ports.proj");
        let mut project = Project::from_json_str(PROJECT).unwrap();
        // Port 0 asks the OS for a free one, which is the only port a test may bind.
        project.server.scpi_port = 1;
        std::fs::write(&path, project.to_json_string().unwrap()).unwrap();

        let app = builder(Options {
            project: Some(path),
            scpi_port: None,
            device_port: Some(0),
            no_auto_load: true,
            ..Options::default()
        })
        .build();
        // Binding port 1 needs privileges this test does not have; the point is that the
        // project's port — not the 5025 default — is the one that was attempted.
        match app {
            Ok(app) => assert_eq!(app.scpi_addr().unwrap().port(), 1),
            Err(error) => assert_eq!(error.exit_code(), EXIT_BIND_FAILURE),
        }
    }

    #[test]
    fn the_bind_host_falls_back_from_the_cli_to_the_project_to_every_interface() {
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.server.bind_host = "127.0.0.1".to_owned();

        let silent = Options::default();
        assert_eq!(resolve_bind_host(&silent, Some(&project)), "127.0.0.1");
        assert_eq!(resolve_bind_host(&silent, None), DEFAULT_BIND_HOST);

        let overridden = Options {
            bind_host: Some("::1".to_owned()),
            ..Options::default()
        };
        assert_eq!(resolve_bind_host(&overridden, Some(&project)), "::1");
    }

    #[test]
    fn an_ipv6_bind_host_reaches_both_listeners() {
        let built = AppBuilder::new(options())
            .bind_host("::1")
            .clock(Arc::new(TestClock::at_epoch()))
            .logger(Arc::new(NullLogger))
            .last_project(None)
            .build();

        // A host with IPv6 switched off cannot bind the loopback address at all, so the
        // assertion that holds either way is that the literal reached the OS bracketed —
        // `[::1]:0` — rather than as the nonsense `::1:0`.
        match built {
            Ok(app) => {
                assert!(app.scpi_addr().unwrap().is_ipv6());
                assert!(app.device_addr().unwrap().unwrap().is_ipv6());
            }
            Err(StartupError::Bind { addr, .. }) => {
                assert!(addr.starts_with("[::1]:"), "{addr}");
            }
            Err(other) => panic!("unexpected startup failure: {other}"),
        }
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
