//! The instrument engine: everything the SCPI layer dispatches onto.
//!
//! One `Mutex<EngineState>` guards the state machine, the loaded project, the capture and the
//! error queue; a paired `Condvar` is what `REC:WAIT?` and `*OPC?` park on, so a blocking
//! query never holds the lock while it waits (`docs/PLAN.md` 5.4).

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use quickvib_core::clock::format_iso8601;
use quickvib_core::{
    CancelToken, Clock, ExportFormat, Level, Logger, SampleUnit, ScpiError, ScpiErrorEntry,
};
use quickvib_device::{DeviceBackend, DeviceCapabilities, StreamOutcome, StreamRequest};
use quickvib_measure::{
    compute, write_capture, CaptureMetadata, MeasureOptions, MeasurementSet, NonEmptyCapture,
};
use quickvib_project::{LastProjectStore, Project, ProjectStore};
use quickvib_scpi::{NOTIFY_RECORD_ABORTED, NOTIFY_RECORD_DONE};

use crate::errors::ErrorQueue;
use crate::opc::Opc;
use crate::recording::{RunOutcome, RunPlan, RunResult};
use crate::state::{transition, Event, State};

/// Somewhere an out-of-band notification line can be delivered. One per SCPI session.
pub trait NotificationSink: Send + Sync {
    /// Deliver `line` (already `#`-prefixed) to this session.
    fn notify(&self, line: &str);
}

/// Everything the engine needs at construction.
pub struct EngineConfig {
    /// Injected time source (D20).
    pub clock: Arc<dyn Clock>,
    /// Where structured log lines go.
    pub logger: Arc<dyn Logger>,
    /// The device backend, already opened by the factory.
    pub backend: Box<dyn DeviceBackend + Send>,
    /// Where the auto-load-last record lives, when one is available.
    pub last_project: Option<LastProjectStore>,
    /// Application version, used as the default `*IDN?` firmware field.
    pub version: String,
}

struct EngineState {
    state: State,
    project: Option<Project>,
    project_path: Option<PathBuf>,
    duration_seconds: f64,
    format: ExportFormat,
    result: Option<RunResult>,
    errors: ErrorQueue,
    opc: Opc,
    sequence: u64,
    outcome: Option<RunOutcome>,
    /// Set by `ABOR` or the watchdog so the reader thread can report *why* it was cancelled.
    abort_reason: Option<RunOutcome>,
    cancel: CancelToken,
    watchdog: Duration,
    any_run_started: bool,
    /// Set while the end-of-run notification is being delivered. Blocking queries treat it
    /// as "still busy" so a client using both `#REC:DONE` and `REC:WAIT?` never sees them
    /// out of order, without the engine lock being held across a socket write.
    notifying: bool,
}

/// The shared instrument model. One per process; every SCPI session talks to the same one
/// (D8).
pub struct Engine {
    state: Mutex<EngineState>,
    changed: Condvar,
    clock: Arc<dyn Clock>,
    logger: Arc<dyn Logger>,
    backend: Mutex<Option<Box<dyn DeviceBackend + Send>>>,
    /// Cached connectivity, for the window in which the reader thread owns the backend.
    backend_connected: AtomicBool,
    capabilities: Option<DeviceCapabilities>,
    sinks: Mutex<Vec<(u64, Arc<dyn NotificationSink>)>>,
    next_session_id: AtomicU64,
    last_project: Option<LastProjectStore>,
    version: String,
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Engine")
            .field("state", &self.state())
            .field("connected", &self.device_connected())
            .finish()
    }
}

impl Engine {
    /// Build an engine around an already-opened backend.
    #[must_use]
    pub fn new(config: EngineConfig) -> Arc<Self> {
        let connected = config.backend.is_connected();
        let capabilities = config.backend.capabilities().ok();
        Arc::new(Self {
            state: Mutex::new(EngineState {
                state: State::Idle,
                project: None,
                project_path: None,
                duration_seconds: 0.0,
                format: ExportFormat::Csv,
                result: None,
                errors: ErrorQueue::new(),
                opc: Opc::new(),
                sequence: 0,
                outcome: None,
                abort_reason: None,
                cancel: CancelToken::new(),
                watchdog: Duration::from_secs(1),
                any_run_started: false,
                notifying: false,
            }),
            changed: Condvar::new(),
            clock: config.clock,
            logger: config.logger,
            backend: Mutex::new(Some(config.backend)),
            backend_connected: AtomicBool::new(connected),
            capabilities,
            sinks: Mutex::new(Vec::new()),
            next_session_id: AtomicU64::new(1),
            last_project: config.last_project,
            version: config.version,
        })
    }

    // ---------------------------------------------------------------- locking helpers

    /// Lock the engine state.
    ///
    /// Poisoning is recovered from deliberately (`docs/PLAN.md` 7.10): the state is plain
    /// data whose invariants the state machine re-validates on every transition, so a panic
    /// mid-mutation cannot leave anything the next command will not correct.
    fn lock(&self) -> MutexGuard<'_, EngineState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn log(&self, level: Level, component: &str, message: impl AsRef<str>) {
        if self.logger.enabled(level) {
            self.logger.log(level, component, message.as_ref());
        }
    }

    // ---------------------------------------------------------------- sessions

    /// Register a session's notification sink. Returns an id for later removal.
    pub fn register_session(&self, sink: Arc<dyn NotificationSink>) -> u64 {
        let id = self.next_session_id.fetch_add(1, Ordering::Relaxed);
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((id, sink));
        id
    }

    /// Remove a session's notification sink.
    pub fn unregister_session(&self, id: u64) {
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(existing, _)| *existing != id);
    }

    /// How many sessions are currently registered.
    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn broadcast(&self, line: &str) {
        let sinks: Vec<Arc<dyn NotificationSink>> = self
            .sinks
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(_, sink)| Arc::clone(sink))
            .collect();
        for sink in sinks {
            sink.notify(line);
        }
    }

    // ---------------------------------------------------------------- error queue

    /// Push an error onto the queue and log it at `warn`.
    pub fn push_error(&self, entry: impl Into<ScpiErrorEntry>) {
        let entry = entry.into();
        self.log(Level::Warn, "scpi", format!("error {entry}"));
        self.lock().errors.push(entry);
    }

    /// Pop the oldest error, or `0,"No error"`.
    pub fn pop_error(&self) -> ScpiErrorEntry {
        self.lock().errors.pop()
    }

    /// Number of pending errors.
    #[must_use]
    pub fn pending_errors(&self) -> usize {
        self.lock().errors.len()
    }

    // ---------------------------------------------------------------- device

    /// Whether the device backend reports a live transport. Backs `SYST:DEV:CONN?`.
    #[must_use]
    pub fn device_connected(&self) -> bool {
        let guard = self.backend.lock().unwrap_or_else(PoisonError::into_inner);
        match guard.as_ref() {
            Some(backend) => backend.is_connected(),
            // The reader thread owns the backend for the duration of a run.
            None => self.backend_connected.load(Ordering::Acquire),
        }
    }

    /// Capabilities read at construction, if the backend was connected then.
    #[must_use]
    pub fn capabilities(&self) -> Option<&DeviceCapabilities> {
        self.capabilities.as_ref()
    }

    fn take_backend(&self) -> Option<Box<dyn DeviceBackend + Send>> {
        let mut guard = self.backend.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(backend) = guard.as_ref() {
            self.backend_connected
                .store(backend.is_connected(), Ordering::Release);
        }
        guard.take()
    }

    fn restore_backend(&self, backend: Box<dyn DeviceBackend + Send>) {
        self.backend_connected
            .store(backend.is_connected(), Ordering::Release);
        *self.backend.lock().unwrap_or_else(PoisonError::into_inner) = Some(backend);
    }

    // ---------------------------------------------------------------- identity

    /// The `*IDN?` response: manufacturer, model, serial, firmware.
    #[must_use]
    pub fn identity(&self) -> String {
        let guard = self.lock();
        let identity = guard.project.as_ref().map(|p| p.identity.clone());
        drop(guard);

        let device_serial = self.capabilities.as_ref().map(|c| c.serial_number.clone());
        match identity {
            Some(identity) => {
                // The schema default is "0"; when the project leaves it at the default and a
                // device is connected, the device's own serial is the better answer.
                let serial = if identity.serial_number == "0" {
                    device_serial.unwrap_or_else(|| "0".to_owned())
                } else {
                    identity.serial_number
                };
                format!(
                    "{},{},{},{}",
                    identity.manufacturer, identity.model, serial, identity.firmware_version
                )
            }
            None => format!(
                "QuickVib,M300-SCPI,{},{}",
                device_serial.unwrap_or_else(|| "0".to_owned()),
                self.version
            ),
        }
    }

    /// The SCPI standard version reported by `SYST:VERS?`.
    #[must_use]
    pub fn scpi_version(&self) -> &'static str {
        "1999.0"
    }

    // ---------------------------------------------------------------- project

    /// The loaded project, if any.
    #[must_use]
    pub fn project(&self) -> Option<Project> {
        self.lock().project.clone()
    }

    /// Path of the loaded project, if it came from disk.
    #[must_use]
    pub fn project_path(&self) -> Option<PathBuf> {
        self.lock().project_path.clone()
    }

    /// Load a project file, replacing any runtime overrides with its values.
    ///
    /// # Errors
    /// `-221` while a run is in flight, or whatever
    /// [`quickvib_project::ProjectError::scpi_error`] reports.
    pub fn load_project(&self, path: &Path) -> Result<(), ScpiError> {
        // A fast failure, not the authoritative one: reading the file releases the lock, so
        // an `INIT` from another session can land in between. `adopt_project` re-checks
        // under the same lock it writes through.
        if self.lock().state.is_running() {
            return Err(ScpiError::SettingsConflict);
        }
        let project = ProjectStore::load(path).map_err(|e| {
            self.log(Level::Warn, "proj", format!("load failed: {e}"));
            e.scpi_error()
        })?;
        self.adopt_project(project, Some(path.to_path_buf()))?;
        self.remember_project(path);
        self.log(
            Level::Info,
            "proj",
            format!(
                "loaded path={} name={}",
                path.display(),
                self.project_name()
            ),
        );
        Ok(())
    }

    /// Install an already-parsed project, as the CLI does at startup and the GUI's *Apply*
    /// does at runtime.
    ///
    /// The previous capture is discarded, and the run status goes with it: `REC:STAT?` must
    /// never answer `COMPLETE` while `FETC?` answers `-230`.
    ///
    /// The in-flight check happens under the same lock hold as the write. A caller that
    /// looked at [`Engine::state`] first and then lost the race to `INIT` — the window
    /// `MMEM:LOAD:STAT` opens while it reads the file off disk — is refused here instead of
    /// replacing the duration, format and measurement settings the run in flight is already
    /// working from.
    ///
    /// # Errors
    /// [`ScpiError::SettingsConflict`] (`-221`) while a run is in flight.
    pub fn adopt_project(&self, project: Project, path: Option<PathBuf>) -> Result<(), ScpiError> {
        let mut guard = self.lock();
        if guard.state.is_running() {
            return Err(ScpiError::SettingsConflict);
        }
        guard.duration_seconds = project.recording.duration_seconds;
        guard.format = project.export.format;
        guard.result = None;
        guard.outcome = None;
        // `Invalidate` has an edge out of every settled state, and the two running states
        // were refused above.
        guard.state = transition(guard.state, Event::Invalidate).unwrap_or(State::Idle);
        guard.project = Some(project);
        guard.project_path = path;
        drop(guard);
        self.changed.notify_all();
        Ok(())
    }

    fn project_name(&self) -> String {
        self.lock()
            .project
            .as_ref()
            .map_or_else(String::new, |p| p.name.clone())
    }

    /// Save the current project — including runtime `CONF:REC:DUR` and `FORM` overrides — to
    /// `path`.
    ///
    /// # Errors
    /// `-221` when no project is loaded, `-257` when the path is not writable.
    pub fn store_project(&self, path: &Path) -> Result<(), ScpiError> {
        let snapshot = {
            let guard = self.lock();
            let mut project = guard.project.clone().ok_or(ScpiError::SettingsConflict)?;
            project.recording.duration_seconds = guard.duration_seconds;
            project.export.format = guard.format;
            project
        };
        ProjectStore::save(&snapshot, path).map_err(|e| {
            self.log(Level::Warn, "proj", format!("save failed: {e}"));
            e.scpi_error()
        })?;
        self.lock().project_path = Some(path.to_path_buf());
        self.remember_project(path);
        self.log(
            Level::Info,
            "proj",
            format!("saved path={}", path.display()),
        );
        Ok(())
    }

    /// The path `MMEM:LOAD:AUTO` would use, if any.
    #[must_use]
    pub fn auto_load_path(&self) -> Option<PathBuf> {
        self.last_project
            .as_ref()
            .and_then(LastProjectStore::read_existing)
    }

    /// Load the most recently used project.
    ///
    /// # Errors
    /// `-256` when no usable record exists, plus the errors from [`Engine::load_project`].
    pub fn load_auto(&self) -> Result<(), ScpiError> {
        let path = self.auto_load_path().ok_or(ScpiError::FileNameNotFound)?;
        self.load_project(&path)
    }

    fn remember_project(&self, path: &Path) {
        if let Some(store) = &self.last_project {
            if let Err(e) = store.record(path) {
                self.log(
                    Level::Warn,
                    "proj",
                    format!("could not record last project: {e}"),
                );
            }
        }
    }

    // ---------------------------------------------------------------- configuration

    /// The active record duration in seconds.
    #[must_use]
    pub fn duration_seconds(&self) -> f64 {
        self.lock().duration_seconds
    }

    /// Override the record duration for subsequent runs.
    ///
    /// # Errors
    /// `-221` while a run is in flight, `-222` outside `(0, 3600]` or over the capture cap.
    pub fn set_duration(&self, seconds: f64) -> Result<(), ScpiError> {
        let mut guard = self.lock();
        if guard.state.is_running() {
            return Err(ScpiError::SettingsConflict);
        }
        if !seconds.is_finite() || seconds <= 0.0 || seconds > 3600.0 {
            return Err(ScpiError::DataOutOfRange);
        }
        if let Some(project) = &guard.project {
            project.expected_samples(seconds)?;
        }
        guard.duration_seconds = seconds;
        Ok(())
    }

    /// The active export format.
    #[must_use]
    pub fn format(&self) -> ExportFormat {
        self.lock().format
    }

    /// Select the export format.
    ///
    /// # Errors
    /// `-221` while a run is in flight.
    pub fn set_format(&self, format: ExportFormat) -> Result<(), ScpiError> {
        let mut guard = self.lock();
        if guard.state.is_running() {
            return Err(ScpiError::SettingsConflict);
        }
        guard.format = format;
        Ok(())
    }

    /// Decimal places used for `CALC:*` responses.
    #[must_use]
    pub fn response_decimals(&self) -> u8 {
        self.lock()
            .project
            .as_ref()
            .map_or(4, |p| p.measurement.response_decimals)
    }

    // ---------------------------------------------------------------- state

    /// The current instrument state.
    #[must_use]
    pub fn state(&self) -> State {
        self.lock().state
    }

    /// Handle `*CLS`.
    pub fn clear_status(&self) {
        let mut guard = self.lock();
        guard.errors.clear();
        guard.opc.clear();
    }

    /// Handle `*RST`: abort any run, discard the capture, restore the project's duration and
    /// format, clear the error queue, and return to `Idle`.
    pub fn reset(&self) {
        self.abort();
        let mut guard = self.lock();
        if let Some((duration, format)) = guard
            .project
            .as_ref()
            .map(|p| (p.recording.duration_seconds, p.export.format))
        {
            guard.duration_seconds = duration;
            guard.format = format;
        }
        guard.result = None;
        guard.outcome = None;
        guard.abort_reason = None;
        guard.any_run_started = false;
        guard.notifying = false;
        guard.errors.clear();
        guard.opc = Opc::new();
        guard.state = transition(guard.state, Event::Reset).unwrap_or(State::Idle);
        drop(guard);
        self.changed.notify_all();
        self.log(Level::Info, "scpi", "reset");
    }

    /// Handle `*OPC`.
    pub fn request_opc(&self) {
        self.lock().opc.request();
    }

    /// Handle `*OPC?`: block until no overlapped operation is pending, then return.
    ///
    /// Bounded by the run watchdog, so it cannot hang past `duration * multiplier + 1 s`.
    pub fn wait_operation_complete(&self) {
        let guard = self.lock();
        let bound = guard.watchdog + Duration::from_secs(1);
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, bound, |s| {
                s.opc.is_operation_in_progress() || s.notifying
            })
            .unwrap_or_else(PoisonError::into_inner);
        drop(guard);
    }

    /// Handle `REC:WAIT?`: block until the active run finishes.
    ///
    /// Returns `true` for a completed run and `false` for an aborted one, a timeout, or a
    /// call made before any run was ever started.
    #[must_use]
    pub fn wait_for_run(&self) -> bool {
        let guard = self.lock();
        if !guard.any_run_started {
            return false;
        }
        let bound = guard.watchdog + Duration::from_secs(1);
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, bound, |s| s.state.is_running() || s.notifying)
            .unwrap_or_else(PoisonError::into_inner);
        guard.state == State::Complete
    }

    // ---------------------------------------------------------------- data

    /// The last completed capture.
    ///
    /// # Errors
    /// `-230` when there is no completed capture — including after an abort (D9).
    pub fn capture(&self) -> Result<Arc<Vec<f32>>, ScpiError> {
        let guard = self.lock();
        match (&guard.result, guard.state) {
            (Some(result), State::Complete) => Ok(Arc::clone(&result.samples)),
            _ => Err(ScpiError::DataCorruptOrStale),
        }
    }

    /// The measurements over the last completed capture.
    ///
    /// # Errors
    /// `-230` when there is no completed capture.
    pub fn measurements(&self) -> Result<MeasurementSet, ScpiError> {
        let guard = self.lock();
        match (&guard.result, guard.state) {
            (Some(result), State::Complete) => Ok(result.measurements),
            _ => Err(ScpiError::DataCorruptOrStale),
        }
    }

    /// Export the last completed capture with the active format.
    ///
    /// Relative paths resolve against the project's `export.directory`.
    ///
    /// # Errors
    /// `-230` when there is no completed capture, `-257` when the path is not writable.
    pub fn export_capture(&self, path: &Path) -> Result<PathBuf, ScpiError> {
        let (samples, metadata, format, directory) = {
            let guard = self.lock();
            let result = match (&guard.result, guard.state) {
                (Some(result), State::Complete) => result,
                _ => return Err(ScpiError::DataCorruptOrStale),
            };
            let project = guard.project.as_ref();
            let metadata = CaptureMetadata {
                project_name: project.map_or_else(String::new, |p| p.name.clone()),
                timestamp: format_iso8601(result.finished_at),
                sample_rate_hz: project.map_or(0.0, |p| p.device.sample_rate_hz),
                unit: project.map_or(SampleUnit::VelocityUmPerSec, |p| p.device.unit),
                duration_seconds: result.duration_seconds,
                include_header: project.is_none_or(|p| p.export.include_header),
            };
            let directory =
                project.map_or_else(|| PathBuf::from("."), |p| p.export.directory.clone());
            (
                Arc::clone(&result.samples),
                metadata,
                guard.format,
                directory,
            )
        };

        let resolved = quickvib_measure::export::resolve_path(path, &directory);
        write_capture(&resolved, format, &metadata, &samples).map_err(|e| {
            self.log(Level::Warn, "export", format!("{e}"));
            e.scpi_error()
        })?;
        self.log(
            Level::Info,
            "export",
            format!(
                "wrote path={} samples={} format={format}",
                resolved.display(),
                samples.len()
            ),
        );
        Ok(resolved)
    }

    // ---------------------------------------------------------------- recording

    /// Handle `INIT` / `REC:STAR`.
    ///
    /// Non-blocking: the reader and watchdog threads are spawned and the call returns.
    ///
    /// # Errors
    /// `-221` when no project is loaded or a run is already in flight, `-241` when the device
    /// is not connected, `-222` when the capture would exceed the buffer cap.
    pub fn start_recording(self: &Arc<Self>) -> Result<(), ScpiError> {
        let connected = self.device_connected();

        let (plan, unit, cancel) = {
            let mut guard = self.lock();
            let project = guard.project.clone().ok_or(ScpiError::SettingsConflict)?;
            if guard.state.is_running() {
                return Err(ScpiError::SettingsConflict);
            }
            if !connected {
                return Err(ScpiError::HardwareMissing);
            }
            let duration = guard.duration_seconds;
            let expected = project.expected_samples(duration)?;
            let next = transition(guard.state, Event::Init).map_err(|e| e.scpi_error())?;

            guard.state = next;
            guard.sequence += 1;
            guard.result = None;
            guard.outcome = None;
            guard.abort_reason = None;
            guard.any_run_started = true;
            guard.notifying = false;
            guard.cancel = CancelToken::new();
            guard.watchdog = project.watchdog_timeout(duration);
            guard.opc.begin_operation();

            let plan = RunPlan {
                sequence: guard.sequence,
                expected_samples: expected,
                duration_seconds: duration,
                sample_rate_hz: project.device.sample_rate_hz,
                watchdog: guard.watchdog,
            };
            (plan, project.device.unit, guard.cancel.clone())
        };
        self.changed.notify_all();

        self.log(
            Level::Info,
            "rec",
            format!(
                "started duration={:.3} expectedSamples={}",
                plan.duration_seconds, plan.expected_samples
            ),
        );

        self.spawn_reader(plan.clone(), unit, cancel.clone());
        self.spawn_watchdog(plan, cancel);
        Ok(())
    }

    fn spawn_reader(self: &Arc<Self>, plan: RunPlan, unit: SampleUnit, cancel: CancelToken) {
        let engine = Arc::clone(self);
        let sequence = plan.sequence;
        let spawned = spawn_detached(READER_THREAD, move || {
            // Panic containment (D27): a panic in the reader aborts the run with -240
            // rather than taking the process down.
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                engine.run_capture(&plan, unit, &cancel);
            }));
            if outcome.is_err() {
                engine.log(Level::Error, "rec", "reader thread panicked");
                engine.finish_run(sequence, RunOutcome::LinkLost, Vec::new(), Duration::ZERO);
            }
        });
        if spawned.is_err() {
            self.log(Level::Error, "rec", "could not spawn the reader thread");
            // Naming the run matters: `INIT` already advanced the sequence, and
            // `finish_run` refuses anything that does not match it. Getting this wrong
            // leaves the instrument pinned in `Armed` until the next `*RST`.
            self.finish_run(sequence, RunOutcome::LinkLost, Vec::new(), Duration::ZERO);
        }
    }

    fn spawn_watchdog(self: &Arc<Self>, plan: RunPlan, cancel: CancelToken) {
        let engine = Arc::clone(self);
        let sequence = plan.sequence;
        let spawned = spawn_detached(WATCHDOG_THREAD, move || {
            // Completion cancels the token, which wakes this thread early instead of
            // leaving it parked for the full watchdog period.
            if !cancel.wait_timeout(plan.watchdog) {
                engine.fire_watchdog(plan.sequence);
            }
        });
        if spawned.is_err() {
            self.log(Level::Error, "rec", "could not spawn the watchdog thread");
            // An unsupervised run has nothing left to end it if the device stalls, so end
            // it now. The reader — which may well have started — sees the cancellation and
            // finishes the run itself; if it never started, this is a no-op because the
            // reader's own failure path already settled the state.
            self.cancel_run(Some(sequence), RunOutcome::LinkLost);
        }
    }

    fn run_capture(&self, plan: &RunPlan, unit: SampleUnit, cancel: &CancelToken) {
        let Some(mut backend) = self.take_backend() else {
            self.finish_run(
                plan.sequence,
                RunOutcome::LinkLost,
                Vec::new(),
                Duration::ZERO,
            );
            return;
        };

        // `stream` owns the backend for the whole run, so `stop_backend` has an empty slot
        // to look at and `DeviceBackend::stop` is unreachable until the run ends. A backend
        // that parks in a blocking read hands out its wake-up in advance instead; hooking it
        // onto the run's token is what makes `ABOR` and the watchdog reach it.
        if let Some(stop) = backend.stop_handle() {
            cancel.on_cancel(move || stop());
        }

        let started_at = self.clock.monotonic();
        let request = StreamRequest::new(plan.expected_samples as u64, unit, plan.sample_rate_hz);
        let mut buffer: Vec<f32> = Vec::with_capacity(plan.expected_samples);
        let mut first_batch = true;

        let stream_result = backend.stream(
            &request,
            &mut |batch| {
                if first_batch {
                    first_batch = false;
                    self.mark_recording(plan.sequence);
                }
                let room = plan.expected_samples.saturating_sub(buffer.len());
                let take = room.min(batch.samples.len());
                buffer.extend_from_slice(&batch.samples[..take]);
                Ok(())
            },
            cancel,
        );

        let elapsed = self.clock.monotonic().saturating_sub(started_at);
        self.restore_backend(backend);

        let outcome = match stream_result {
            Ok(StreamOutcome::Completed) if buffer.len() >= plan.expected_samples => {
                RunOutcome::Completed
            }
            Ok(StreamOutcome::Completed) => RunOutcome::LinkLost,
            Ok(StreamOutcome::Cancelled) => RunOutcome::Aborted,
            Ok(StreamOutcome::LinkLost) => RunOutcome::LinkLost,
            Ok(StreamOutcome::TimedOut) => RunOutcome::TimedOut,
            Err(error) => {
                self.log(Level::Warn, "rec", format!("stream failed: {error}"));
                match error.scpi_error() {
                    ScpiError::TimeoutError => RunOutcome::TimedOut,
                    _ => RunOutcome::LinkLost,
                }
            }
        };

        self.finish_run(plan.sequence, outcome, buffer, elapsed);
    }

    fn mark_recording(&self, sequence: u64) {
        let mut guard = self.lock();
        if guard.sequence != sequence || guard.state != State::Armed {
            return;
        }
        if let Ok(next) = transition(guard.state, Event::FirstSample) {
            guard.state = next;
        }
        drop(guard);
        self.changed.notify_all();
    }

    /// Cancel the run in flight and record how the reader should report it.
    ///
    /// `sequence` names the run to cancel, so a caller left over from a previous one cannot
    /// touch its successor; `None` means "whatever is running now". Returns whether a run
    /// was actually cancelled.
    fn cancel_run(&self, sequence: Option<u64>, reason: RunOutcome) -> bool {
        let cancel = {
            let mut guard = self.lock();
            if !guard.state.is_running() || sequence.is_some_and(|s| s != guard.sequence) {
                return false;
            }
            guard.abort_reason = Some(reason);
            guard.cancel.clone()
        };
        self.stop_backend();
        // Runs every hook registered on the token, which is how a backend parked in a
        // blocking read is woken (see `run_capture`).
        cancel.cancel();
        true
    }

    fn fire_watchdog(&self, sequence: u64) {
        if self.cancel_run(Some(sequence), RunOutcome::TimedOut) {
            self.log(Level::Warn, "rec", "watchdog fired");
        }
    }

    /// Handle `ABOR`. Never an error, even when nothing is running.
    pub fn abort(&self) {
        if self.cancel_run(None, RunOutcome::Aborted) {
            self.log(Level::Info, "rec", "abort requested");
        }
    }

    fn stop_backend(&self) {
        let guard = self.backend.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(backend) = guard.as_ref() {
            let _ = backend.stop();
        }
    }

    fn finish_run(&self, sequence: u64, outcome: RunOutcome, buffer: Vec<f32>, elapsed: Duration) {
        let (notification, summary) = {
            let mut guard = self.lock();
            if guard.sequence != sequence || !guard.state.is_running() {
                return;
            }

            // A cancellation is reported as whatever asked for it, so a watchdog abort is a
            // -365 and an operator abort is not an error at all.
            let outcome = match (outcome, guard.abort_reason.take()) {
                (RunOutcome::Completed, _) => RunOutcome::Completed,
                (_, Some(reason)) => reason,
                (outcome, None) => outcome,
            };

            let remove_dc = guard
                .project
                .as_ref()
                .is_some_and(|p| p.measurement.remove_dc);
            // Still the value the run was armed with: every setter and `adopt_project`
            // answer `-221` until the run settles, and this runs before it does.
            let duration_seconds = guard.duration_seconds;

            let (next_event, summary) = if outcome.is_success() {
                match NonEmptyCapture::new(&buffer) {
                    Some(capture) => {
                        let measurements = compute(capture, MeasureOptions { remove_dc });
                        let summary = format!(
                            "complete samples={} elapsed={:.3} peak={:.4} rms={:.4} pp={:.4}",
                            buffer.len(),
                            elapsed.as_secs_f64(),
                            measurements.peak,
                            measurements.rms,
                            measurements.peak_to_peak
                        );
                        if measurements.has_non_finite {
                            self.log(
                                Level::Warn,
                                "rec",
                                "capture contains NaN or infinite samples",
                            );
                        }
                        guard.result = Some(RunResult {
                            samples: Arc::new(buffer),
                            measurements,
                            elapsed,
                            duration_seconds,
                            finished_at: self.clock.wall_clock(),
                        });
                        (Event::Completed, summary)
                    }
                    // A completed run always has expected_samples > 0, so this is
                    // unreachable in practice; treat it as a lost link rather than
                    // publishing an empty capture.
                    None => (Event::Abort, "aborted samples=0".to_owned()),
                }
            } else {
                (
                    Event::Abort,
                    format!("aborted outcome={outcome:?} samples={}", buffer.len()),
                )
            };

            guard.state = transition(guard.state, next_event).unwrap_or(State::Aborted);
            guard.outcome = Some(outcome);
            guard.opc.end_operation();

            if guard.state != State::Complete {
                guard.result = None;
                if let Some(error) = outcome.scpi_error() {
                    guard.errors.push(error);
                }
            }

            let notification = if guard.state == State::Complete {
                NOTIFY_RECORD_DONE
            } else {
                NOTIFY_RECORD_ABORTED
            };
            guard.notifying = true;
            let cancel = guard.cancel.clone();
            drop(guard);
            // Release the watchdog thread, which is parked on this token.
            cancel.cancel();
            (notification, summary)
        };

        // The notification goes out *before* the blocked `REC:WAIT?` and `*OPC?` queries are
        // released, so a UTS that uses both never sees them out of order. The `notifying`
        // flag — not the engine lock — is what holds those queries back, so a slow
        // notification consumer cannot stall the state machine.
        self.broadcast(notification);
        self.lock().notifying = false;
        self.changed.notify_all();
        self.log(Level::Info, "rec", summary);
    }
}

const READER_THREAD: &str = "quickvib-reader";
const WATCHDOG_THREAD: &str = "quickvib-watchdog";

#[cfg(test)]
thread_local! {
    /// Test seam: the thread whose next spawn must fail. Exhausting the real thread limit
    /// is not something a unit test can do without destabilising the rest of the suite, and
    /// the recovery paths are exactly the ones that leave a run wedged when they are wrong.
    /// Thread-local, so a test setting it cannot disturb one running beside it.
    static FAIL_SPAWN: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Spawn a named, detached engine thread.
fn spawn_detached(name: &'static str, body: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    #[cfg(test)]
    if FAIL_SPAWN.with(std::cell::Cell::get) == Some(name) {
        return Err(std::io::Error::other("injected spawn failure"));
    }
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(body)
        .map(drop)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)]

    use super::*;
    use quickvib_core::{NullLogger, TestClock};
    use quickvib_device::{
        DeviceError, DeviceOpenOptions, MockBackend, MockFault, MockSignalSpec, SampleBatch,
        SignalComponent,
    };

    const PROJECT: &str = r#"{
        "schemaVersion": 1,
        "name": "EngineTest",
        "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
        "recording": { "durationSeconds": 0.05 },
        "mock": { "signal": { "components": [
            { "frequencyHz": 50.0, "amplitude": 2.0, "phaseDeg": 0.0 } ], "seed": 1 } }
    }"#;

    struct Recorder {
        lines: Mutex<Vec<String>>,
    }

    impl NotificationSink for Recorder {
        fn notify(&self, line: &str) {
            self.lines
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(line.to_owned());
        }
    }

    fn engine_with(fault: MockFault) -> Arc<Engine> {
        let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
        let mut backend = MockBackend::new(Arc::clone(&clock))
            .with_signal(MockSignalSpec {
                components: vec![SignalComponent {
                    frequency_hz: 50.0,
                    amplitude: 2.0,
                    phase_deg: 0.0,
                }],
                noise_std_dev: 0.0,
                seed: 1,
            })
            .with_fault(fault);
        backend
            .open(&quickvib_device::DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();

        Engine::new(EngineConfig {
            clock,
            logger: Arc::new(NullLogger),
            backend: Box::new(backend),
            last_project: None,
            version: "1.0.0-test".to_owned(),
        })
    }

    fn loaded_engine() -> Arc<Engine> {
        let engine = engine_with(MockFault::None);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine
    }

    /// A backend that parks in a blocking read and wakes only through the handle it hands
    /// out — the shape a native SDK read has, and the one polling the cancellation token
    /// cannot rescue.
    struct BlockingBackend {
        released: Arc<(Mutex<bool>, Condvar)>,
    }

    impl BlockingBackend {
        fn new() -> Self {
            Self {
                released: Arc::new((Mutex::new(false), Condvar::new())),
            }
        }

        fn release(released: &(Mutex<bool>, Condvar)) {
            let (lock, changed) = released;
            *lock.lock().unwrap_or_else(PoisonError::into_inner) = true;
            changed.notify_all();
        }
    }

    impl DeviceBackend for BlockingBackend {
        fn is_connected(&self) -> bool {
            true
        }

        fn open(&mut self, _options: &DeviceOpenOptions) -> Result<(), DeviceError> {
            Ok(())
        }

        fn capabilities(&self) -> Result<DeviceCapabilities, DeviceError> {
            Ok(DeviceCapabilities {
                model: "Blocking".to_owned(),
                serial_number: "BLOCK-0001".to_owned(),
                firmware_version: "1.0.0-test".to_owned(),
                sample_rate_hz: 1000.0,
                unit: SampleUnit::VelocityUmPerSec,
                max_record_seconds: 3600.0,
            })
        }

        fn stream(
            &mut self,
            _request: &StreamRequest,
            _on_batch: &mut dyn FnMut(SampleBatch<'_>) -> Result<(), DeviceError>,
            _cancel: &CancelToken,
        ) -> Result<StreamOutcome, DeviceError> {
            let (lock, changed) = &*self.released;
            let guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
            // The token is deliberately never polled. The timeout is only a net so a
            // regression fails the test instead of hanging the suite.
            let (guard, expired) = changed
                .wait_timeout_while(guard, Duration::from_secs(5), |released| !*released)
                .unwrap_or_else(PoisonError::into_inner);
            drop(guard);
            if expired.timed_out() {
                Ok(StreamOutcome::TimedOut)
            } else {
                Ok(StreamOutcome::Cancelled)
            }
        }

        fn stop(&self) -> Result<(), DeviceError> {
            Self::release(&self.released);
            Ok(())
        }

        fn stop_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
            let released = Arc::clone(&self.released);
            Some(Arc::new(move || Self::release(&released)))
        }
    }

    fn blocking_engine() -> Arc<Engine> {
        let engine = Engine::new(EngineConfig {
            clock: Arc::new(TestClock::at_epoch()),
            logger: Arc::new(NullLogger),
            backend: Box::new(BlockingBackend::new()),
            last_project: None,
            version: "1.0.0-test".to_owned(),
        });
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine
    }

    /// Run `body` with the next spawn of `thread` forced to fail.
    fn with_failing_spawn<T>(thread: &'static str, body: impl FnOnce() -> T) -> T {
        FAIL_SPAWN.with(|cell| cell.set(Some(thread)));
        let result = body();
        FAIL_SPAWN.with(|cell| cell.set(None));
        result
    }

    fn run_to_completion(engine: &Arc<Engine>) -> bool {
        engine.start_recording().unwrap();
        engine.wait_for_run()
    }

    #[test]
    fn a_fresh_engine_is_idle_and_connected() {
        let engine = engine_with(MockFault::None);
        assert_eq!(engine.state(), State::Idle);
        assert!(engine.device_connected());
        assert_eq!(engine.pop_error().error, ScpiError::NoError);
    }

    #[test]
    fn init_without_a_project_is_a_settings_conflict() {
        let engine = engine_with(MockFault::None);
        assert_eq!(engine.start_recording(), Err(ScpiError::SettingsConflict));
    }

    #[test]
    fn a_full_run_completes_and_publishes_data() {
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));
        assert_eq!(engine.state(), State::Complete);

        let samples = engine.capture().unwrap();
        assert_eq!(samples.len(), 50);

        let m = engine.measurements().unwrap();
        assert!(m.peak > 0.0);
        assert!(m.rms > 0.0);
    }

    #[test]
    fn a_second_init_discards_the_previous_capture() {
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));
        assert!(engine.capture().is_ok());

        engine.set_duration(0.02).unwrap();
        assert!(run_to_completion(&engine));
        assert_eq!(engine.capture().unwrap().len(), 20);
    }

    #[test]
    fn adopting_a_project_clears_the_completed_run_status() {
        let dir = tempfile::tempdir().unwrap();
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));
        assert_eq!(engine.state(), State::Complete);

        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();

        // The whole point: no state that claims a fetchable capture, and no fetchable data.
        assert_eq!(engine.state(), State::Idle);
        assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
        assert_eq!(
            engine.measurements().unwrap_err(),
            ScpiError::DataCorruptOrStale
        );
        assert_eq!(
            engine.export_capture(&dir.path().join("run.csv")),
            Err(ScpiError::DataCorruptOrStale)
        );
        assert!(!engine.wait_for_run());
    }

    #[test]
    fn loading_a_project_from_disk_clears_the_completed_run_status() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Reloaded.proj");
        std::fs::write(&path, PROJECT).unwrap();
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));

        engine.load_project(&path).unwrap();

        assert_eq!(engine.state(), State::Idle);
        assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
    }

    #[test]
    fn adopting_a_project_does_not_resurrect_an_aborted_capture() {
        let engine = engine_with(MockFault::Stall);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine.start_recording().unwrap();
        engine.abort();
        assert!(!engine.wait_for_run());
        assert_eq!(engine.state(), State::Aborted);

        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();

        assert_eq!(engine.state(), State::Idle);
        assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
    }

    #[test]
    fn adopting_a_project_mid_run_leaves_the_run_in_flight() {
        let engine = engine_with(MockFault::Stall);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine.start_recording().unwrap();

        let mut replacement = Project::from_json_str(PROJECT).unwrap();
        replacement.recording.duration_seconds = 2.0;
        replacement.export.format = ExportFormat::Txt;
        assert_eq!(
            engine.adopt_project(replacement, None),
            Err(ScpiError::SettingsConflict)
        );

        assert!(engine.state().is_running());
        // Nothing the run is working from moved underneath it.
        assert_eq!(engine.duration_seconds(), 0.05);
        assert_eq!(engine.format(), ExportFormat::Csv);

        engine.abort();
        assert!(!engine.wait_for_run());
        assert_eq!(engine.state(), State::Aborted);
    }

    #[test]
    fn adopting_a_project_mid_run_is_refused_under_the_lock() {
        let engine = engine_with(MockFault::Stall);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine.set_duration(0.03).unwrap();

        // The window `MMEM:LOAD:STAT` opens: the caller saw a settled instrument, then
        // spent time reading the file off disk while another session armed a run. The
        // refusal has to come from the same lock hold as the write, not from that stale
        // observation.
        assert!(!engine.state().is_running());
        engine.start_recording().unwrap();

        let mut replacement = Project::from_json_str(PROJECT).unwrap();
        replacement.name = "Replacement".to_owned();
        replacement.recording.duration_seconds = 1.5;
        replacement.export.format = ExportFormat::Txt;
        assert_eq!(
            engine.adopt_project(replacement, None),
            Err(ScpiError::SettingsConflict)
        );

        assert!(engine.state().is_running());
        assert_eq!(engine.duration_seconds(), 0.03);
        assert_eq!(engine.format(), ExportFormat::Csv);
        assert_eq!(engine.project().unwrap().name, "EngineTest");

        engine.abort();
        assert!(!engine.wait_for_run());
    }

    #[test]
    fn a_reader_that_cannot_be_spawned_settles_the_run() {
        let engine = loaded_engine();
        with_failing_spawn(READER_THREAD, || engine.start_recording().unwrap());

        // Nothing is left to finish this run, so `INIT` must have finished it itself
        // rather than leaving the instrument pinned in `Armed` until the next `*RST`.
        assert_eq!(engine.state(), State::Aborted);
        assert!(!engine.wait_for_run());
        assert_eq!(engine.pop_error().error, ScpiError::HardwareError);
        // And the instrument still works.
        assert!(run_to_completion(&engine));
        assert_eq!(engine.capture().unwrap().len(), 50);
    }

    #[test]
    fn a_watchdog_that_cannot_be_spawned_ends_the_run_it_cannot_supervise() {
        let engine = engine_with(MockFault::Stall);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();

        with_failing_spawn(WATCHDOG_THREAD, || engine.start_recording().unwrap());

        // A stalled run with no watchdog would never end on its own.
        assert!(!engine.wait_for_run());
        assert_eq!(engine.state(), State::Aborted);
        assert_eq!(engine.pop_error().error, ScpiError::HardwareError);
    }

    #[test]
    fn abort_reaches_a_backend_that_only_wakes_through_its_stop_handle() {
        let engine = blocking_engine();
        engine.start_recording().unwrap();
        // Let the reader thread get into the blocking read.
        std::thread::sleep(Duration::from_millis(20));

        engine.abort();

        assert!(!engine.wait_for_run());
        assert_eq!(engine.state(), State::Aborted);
        // The stop handle is what released the read. Without it the reader would still be
        // parked when the watchdog fires a second later, and this would be a -365.
        assert_eq!(engine.pop_error().error, ScpiError::NoError);
    }

    #[test]
    fn rec_done_is_broadcast_to_registered_sessions() {
        let engine = loaded_engine();
        let recorder = Arc::new(Recorder {
            lines: Mutex::new(Vec::new()),
        });
        engine.register_session(Arc::clone(&recorder) as Arc<dyn NotificationSink>);
        assert!(run_to_completion(&engine));
        assert_eq!(
            recorder.lines.lock().unwrap().as_slice(),
            [NOTIFY_RECORD_DONE]
        );
    }

    #[test]
    fn unregistered_sessions_stop_receiving_notifications() {
        let engine = loaded_engine();
        let recorder = Arc::new(Recorder {
            lines: Mutex::new(Vec::new()),
        });
        let id = engine.register_session(Arc::clone(&recorder) as Arc<dyn NotificationSink>);
        engine.unregister_session(id);
        assert_eq!(engine.session_count(), 0);
        assert!(run_to_completion(&engine));
        assert!(recorder.lines.lock().unwrap().is_empty());
    }

    #[test]
    fn an_aborted_run_publishes_no_data() {
        let engine = engine_with(MockFault::Stall);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        let recorder = Arc::new(Recorder {
            lines: Mutex::new(Vec::new()),
        });
        engine.register_session(Arc::clone(&recorder) as Arc<dyn NotificationSink>);

        engine.start_recording().unwrap();
        engine.abort();
        assert!(!engine.wait_for_run());

        assert_eq!(engine.state(), State::Aborted);
        assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
        assert_eq!(
            engine.measurements().unwrap_err(),
            ScpiError::DataCorruptOrStale
        );
        assert_eq!(
            recorder.lines.lock().unwrap().as_slice(),
            [NOTIFY_RECORD_ABORTED]
        );
        // An operator abort is not an error condition.
        assert_eq!(engine.pop_error().error, ScpiError::NoError);
    }

    #[test]
    fn a_stalled_run_trips_the_watchdog_with_365() {
        let engine = engine_with(MockFault::Stall);
        let mut project = Project::from_json_str(PROJECT).unwrap();
        // Watchdog = duration * multiplier + 1 s, so keep it just over a second.
        project.recording.duration_seconds = 0.01;
        engine.adopt_project(project, None).unwrap();

        engine.start_recording().unwrap();
        assert!(!engine.wait_for_run());
        assert_eq!(engine.state(), State::Aborted);
        assert_eq!(engine.pop_error().error, ScpiError::TimeoutError);
    }

    #[test]
    fn a_short_stream_aborts_with_240() {
        let engine = engine_with(MockFault::ShortStream(10));
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine.start_recording().unwrap();
        assert!(!engine.wait_for_run());
        assert_eq!(engine.state(), State::Aborted);
        assert_eq!(engine.pop_error().error, ScpiError::HardwareError);
    }

    #[test]
    fn a_dropped_link_aborts_with_240() {
        let engine = engine_with(MockFault::LinkLostAfter(10));
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine.start_recording().unwrap();
        assert!(!engine.wait_for_run());
        assert_eq!(engine.pop_error().error, ScpiError::HardwareError);
    }

    #[test]
    fn rec_wait_before_any_run_returns_false_immediately() {
        let engine = loaded_engine();
        assert!(!engine.wait_for_run());
    }

    #[test]
    fn duration_range_is_enforced() {
        let engine = loaded_engine();
        for bad in [0.0, -1.0, 3600.1, f64::NAN] {
            assert_eq!(
                engine.set_duration(bad),
                Err(ScpiError::DataOutOfRange),
                "{bad}"
            );
        }
        engine.set_duration(1.5).unwrap();
        assert_eq!(engine.duration_seconds(), 1.5);
    }

    #[test]
    fn duration_respects_the_capture_cap() {
        let engine = engine_with(MockFault::None);
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.device.sample_rate_hz = 100_000.0;
        project.recording.max_capture_bytes = 4_000;
        engine.adopt_project(project, None).unwrap();
        assert_eq!(engine.set_duration(10.0), Err(ScpiError::DataOutOfRange));
    }

    #[test]
    fn reset_returns_to_idle_and_restores_project_values() {
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));
        engine.set_duration(2.0).unwrap();
        engine.set_format(ExportFormat::Txt).unwrap();
        engine.push_error(ScpiError::CommandError);

        engine.reset();

        assert_eq!(engine.state(), State::Idle);
        assert_eq!(engine.duration_seconds(), 0.05);
        assert_eq!(engine.format(), ExportFormat::Csv);
        assert_eq!(engine.pop_error().error, ScpiError::NoError);
        assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
    }

    #[test]
    fn cls_clears_the_error_queue_but_not_the_data() {
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));
        engine.push_error(ScpiError::CommandError);
        engine.clear_status();
        assert_eq!(engine.pop_error().error, ScpiError::NoError);
        assert!(engine.capture().is_ok());
    }

    #[test]
    fn opc_resolves_after_a_run() {
        let engine = loaded_engine();
        engine.request_opc();
        engine.start_recording().unwrap();
        engine.wait_operation_complete();
        assert!(!engine.state().is_running());
    }

    #[test]
    fn identity_uses_the_project_when_one_is_loaded() {
        let engine = loaded_engine();
        assert_eq!(engine.identity(), "QuickVib,M300-SCPI,MOCK-0001,1.0.0");
    }

    #[test]
    fn identity_falls_back_to_the_device_serial_without_a_project() {
        let engine = engine_with(MockFault::None);
        assert_eq!(engine.identity(), "QuickVib,M300-SCPI,MOCK-0001,1.0.0-test");
    }

    #[test]
    fn a_configured_identity_is_reported_verbatim() {
        let engine = engine_with(MockFault::None);
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.identity.manufacturer = "Acme".to_owned();
        project.identity.model = "VibMaster".to_owned();
        project.identity.serial_number = "SN-42".to_owned();
        project.identity.firmware_version = "9.9".to_owned();
        engine.adopt_project(project, None).unwrap();
        assert_eq!(engine.identity(), "Acme,VibMaster,SN-42,9.9");
    }

    #[test]
    fn loading_a_missing_project_is_256() {
        let engine = engine_with(MockFault::None);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            engine.load_project(&dir.path().join("nope.proj")),
            Err(ScpiError::FileNameNotFound)
        );
    }

    #[test]
    fn project_round_trips_through_disk_with_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("saved.proj");
        let engine = loaded_engine();
        engine.set_duration(0.25).unwrap();
        engine.set_format(ExportFormat::Txt).unwrap();
        engine.store_project(&path).unwrap();

        let other = engine_with(MockFault::None);
        other.load_project(&path).unwrap();
        assert_eq!(other.duration_seconds(), 0.25);
        assert_eq!(other.format(), ExportFormat::Txt);
    }

    #[test]
    fn auto_load_reopens_the_last_project() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Auto.proj");
        std::fs::write(&path, PROJECT).unwrap();

        let store = LastProjectStore::new(dir.path().join("state"));
        let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
        let mut backend = MockBackend::new(Arc::clone(&clock));
        backend
            .open(&quickvib_device::DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        let engine = Engine::new(EngineConfig {
            clock,
            logger: Arc::new(NullLogger),
            backend: Box::new(backend),
            last_project: Some(store),
            version: "1.0.0-test".to_owned(),
        });

        assert_eq!(engine.load_auto(), Err(ScpiError::FileNameNotFound));
        engine.load_project(&path).unwrap();
        assert!(engine.auto_load_path().is_some());
        engine.reset();
        engine.load_auto().unwrap();
        assert_eq!(engine.project().unwrap().name, "EngineTest");
    }

    #[test]
    fn export_requires_a_completed_capture() {
        let dir = tempfile::tempdir().unwrap();
        let engine = loaded_engine();
        assert_eq!(
            engine.export_capture(&dir.path().join("run.csv")),
            Err(ScpiError::DataCorruptOrStale)
        );
        assert!(run_to_completion(&engine));
        let written = engine.export_capture(&dir.path().join("run.csv")).unwrap();
        assert!(written.is_file());
    }

    #[test]
    fn export_metadata_reports_the_runs_duration_not_a_later_override() {
        let dir = tempfile::tempdir().unwrap();
        let engine = loaded_engine();
        assert!(run_to_completion(&engine));

        // Legal from `Complete`, and it must not rewrite the preamble of a capture that is
        // already on the books.
        engine.set_duration(2.0).unwrap();
        let written = engine.export_capture(&dir.path().join("run.csv")).unwrap();

        let csv = std::fs::read_to_string(written).unwrap();
        let preamble: String = csv.lines().take(7).collect::<Vec<_>>().join(" | ");
        assert!(preamble.contains("# durationSeconds=0.05"), "{preamble}");
        assert!(preamble.contains("# samples=50"), "{preamble}");
    }

    #[test]
    fn export_resolves_relative_paths_against_the_project_directory() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine_with(MockFault::None);
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.export.directory = dir.path().join("out");
        engine.adopt_project(project, None).unwrap();
        assert!(run_to_completion(&engine));
        let written = engine.export_capture(Path::new("run.csv")).unwrap();
        assert_eq!(written, dir.path().join("out").join("run.csv"));
        assert!(written.is_file());
    }

    #[test]
    fn configuration_changes_during_a_run_are_rejected() {
        let engine = engine_with(MockFault::Stall);
        engine
            .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
            .unwrap();
        engine.start_recording().unwrap();

        assert_eq!(engine.set_duration(1.0), Err(ScpiError::SettingsConflict));
        assert_eq!(
            engine.set_format(ExportFormat::Txt),
            Err(ScpiError::SettingsConflict)
        );
        assert_eq!(engine.start_recording(), Err(ScpiError::SettingsConflict));
        assert_eq!(
            engine.load_project(Path::new("whatever.proj")),
            Err(ScpiError::SettingsConflict)
        );

        engine.abort();
        assert!(!engine.wait_for_run());
    }

    #[test]
    fn abort_when_idle_is_a_no_op() {
        let engine = loaded_engine();
        engine.abort();
        assert_eq!(engine.state(), State::Idle);
        assert_eq!(engine.pop_error().error, ScpiError::NoError);
    }

    #[test]
    fn measurements_match_the_analytic_oracle_for_a_pure_sine() {
        let engine = loaded_engine();
        engine.set_duration(2.0).unwrap();
        assert!(run_to_completion(&engine));
        let m = engine.measurements().unwrap();
        assert!((m.peak - 2.0).abs() < 0.02, "peak {}", m.peak);
        assert!((m.rms - 2.0 / 2.0_f64.sqrt()).abs() < 0.02, "rms {}", m.rms);
        assert!((m.peak_to_peak - 4.0).abs() < 0.05, "pp {}", m.peak_to_peak);
    }
}
