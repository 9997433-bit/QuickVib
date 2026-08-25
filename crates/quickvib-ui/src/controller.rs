//! The bridge between the form and the running instrument.
//!
//! Nothing in here draws anything, and nothing in here needs a display: the window calls
//! these methods and renders whatever comes back, which is what makes the interesting
//! behaviour — what *Apply* does to a live engine, what a rejected field looks like, which
//! edits need a restart — testable in the ordinary `cargo test` run.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use quickvib_core::ScpiError;
use quickvib_device::ConnectionState;
use quickvib_engine::Engine;
use quickvib_project::Project;

use crate::form::{starter_project, FieldError, ProjectForm};
use crate::status::StatusSnapshot;

/// What [`UiController::apply`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    /// Settings that were written into the project but that the already-open transports and
    /// the already-opened backend cannot pick up until QuickVib is restarted.
    pub restart_required: Vec<&'static str>,
}

impl ApplyOutcome {
    /// A sentence for the activity line, or `None` when everything took effect immediately.
    #[must_use]
    pub fn restart_note(&self) -> Option<String> {
        if self.restart_required.is_empty() {
            return None;
        }
        Some(format!(
            "restart QuickVib for: {}",
            self.restart_required.join(", ")
        ))
    }
}

/// The window's model: the form, where it came from, and the engine it feeds.
pub struct UiController {
    engine: Arc<Engine>,
    link: Option<Arc<ConnectionState>>,
    /// The project as the engine currently holds it. Supplies every field the form does not
    /// show, and is the baseline the restart-required comparison is made against.
    base: Project,
    form: ProjectForm,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for UiController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiController")
            .field("project", &self.base.name)
            .field("path", &self.path)
            .field("dirty", &self.is_dirty())
            .finish_non_exhaustive()
    }
}

impl UiController {
    /// Open the window's model over `engine`, seeded from whatever project the engine already
    /// holds — the one `--project` or auto-load-last brought up.
    #[must_use]
    pub fn new(engine: Arc<Engine>) -> Self {
        let path = engine.project_path();
        let base = engine.project().unwrap_or_else(starter_project);
        Self {
            engine,
            link: None,
            form: ProjectForm::from_project(&base),
            base,
            path,
        }
    }

    /// Also watch the inbound device link, so the window can name the peer that dialed in.
    #[must_use]
    pub fn with_link(mut self, link: Arc<ConnectionState>) -> Self {
        self.link = Some(link);
        self
    }

    /// The engine every SCPI session shares.
    #[must_use]
    pub fn engine(&self) -> &Arc<Engine> {
        &self.engine
    }

    /// The form the window edits.
    #[must_use]
    pub fn form(&self) -> &ProjectForm {
        &self.form
    }

    /// The form the window edits, mutably.
    pub fn form_mut(&mut self) -> &mut ProjectForm {
        &mut self.form
    }

    /// Where the project came from, and where *Save* will write it.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether the form has edits the engine has not been told about.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.form != ProjectForm::from_project(&self.base)
    }

    /// Push the form into the live engine.
    ///
    /// The engine adopts the edited project exactly as `MMEM:LOAD:STAT` would: the record
    /// duration and export format go back to the project's values, any stale capture is
    /// dropped, and every subsequent SCPI query — `*IDN?`, `CONF:REC:DUR?`, `TRAC:POIN?` —
    /// answers from the new settings. Fields the open sockets and the opened backend cannot
    /// re-read are reported in [`ApplyOutcome::restart_required`] rather than silently
    /// ignored.
    ///
    /// # Errors
    /// The list of rejected fields, or a single `project` entry when a run is in flight —
    /// the same `-221` rule the SCPI layer applies.
    pub fn apply(&mut self) -> Result<ApplyOutcome, Vec<FieldError>> {
        if self.engine.state().is_running() {
            return Err(vec![FieldError::new(
                "project",
                "a recording is in flight; stop it before applying changes",
            )]);
        }

        let edited = self.form.to_project(&self.base)?;
        let outcome = ApplyOutcome {
            restart_required: restart_required(&self.base, &edited),
        };

        self.engine.adopt_project(edited.clone(), self.path.clone());
        self.base = edited;
        self.form = ProjectForm::from_project(&self.base);
        Ok(outcome)
    }

    /// Throw the edits away and re-read the engine's project.
    pub fn revert(&mut self) {
        self.base = self.engine.project().unwrap_or_else(starter_project);
        self.path = self.engine.project_path();
        self.form = ProjectForm::from_project(&self.base);
    }

    /// Load `path` into the engine and into the form.
    ///
    /// # Errors
    /// A human-readable message: the file is missing, malformed, invalid, or a run is in
    /// flight.
    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();
        self.engine
            .load_project(path)
            .map_err(|error| describe(error, &format!("could not load {}", path.display())))?;
        self.path = Some(path.to_path_buf());
        self.revert();
        Ok(())
    }

    /// Apply the form, then write the project back to the path it came from.
    ///
    /// # Errors
    /// The rejected fields, rendered as one message, or the reason the write failed. Saving
    /// a project that has never been on disk is an error naming *Save as*.
    pub fn save(&mut self) -> Result<PathBuf, String> {
        let Some(path) = self.path.clone() else {
            return Err("this project has no file yet — use Save as".to_owned());
        };
        self.save_as(path)
    }

    /// Apply the form, then write the project to `path` and remember it as the current file.
    ///
    /// # Errors
    /// The rejected fields, rendered as one message, or the reason the write failed.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<PathBuf, String> {
        let path = path.as_ref().to_path_buf();
        self.apply().map_err(render_errors)?;
        self.path = Some(path.clone());
        // Going through the engine keeps the runtime overrides, the auto-load-last record and
        // the log line identical to `MMEM:STOR:STAT`.
        self.engine
            .store_project(&path)
            .map_err(|error| describe(error, &format!("could not save {}", path.display())))?;
        Ok(path)
    }

    /// Start a capture, as `INIT` does.
    ///
    /// # Errors
    /// A human-readable rendering of the SCPI error: no project, already recording, no
    /// device, or a capture that would exceed the buffer cap.
    pub fn start(&self) -> Result<(), String> {
        self.engine
            .start_recording()
            .map_err(|error| describe(error, "could not start the recording"))
    }

    /// Abort the active capture, as `ABOR` does. Never an error.
    pub fn stop(&self) {
        self.engine.abort();
    }

    /// Export the last completed capture, resolving `name` against the project's export
    /// directory exactly as `MMEM:STOR:TRAC` does.
    ///
    /// # Errors
    /// A human-readable message when there is no completed capture or the path is not
    /// writable.
    pub fn export(&self, name: &str) -> Result<PathBuf, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("enter a file name to export to".to_owned());
        }
        self.engine
            .export_capture(Path::new(name))
            .map_err(|error| describe(error, "could not export the capture"))
    }

    /// Read the instrument in one pass.
    #[must_use]
    pub fn snapshot(&self) -> StatusSnapshot {
        let measurements = self.engine.measurements().ok();
        StatusSnapshot {
            state: self.engine.state(),
            connected: self.engine.device_connected(),
            device_peer: self.link.as_ref().and_then(|link| link.peer()),
            identity: self.engine.identity(),
            project_name: self
                .engine
                .project()
                .map_or_else(String::new, |project| project.name),
            duration_seconds: self.engine.duration_seconds(),
            format: self.engine.format(),
            measurements,
            sample_count: self.engine.capture().map_or(0, |samples| samples.len()),
            pending_errors: self.engine.pending_errors(),
            sessions: self.engine.session_count(),
        }
    }
}

/// Which of the edited settings the already-running process cannot pick up.
///
/// The two listeners are bound at startup and the backend is opened with the sample rate and
/// unit that were in force then, so changing any of those is honoured in the saved project but
/// not in this process.
fn restart_required(before: &Project, after: &Project) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if before.device.backend != after.device.backend {
        fields.push("backend");
    }
    if before.device.port != after.device.port {
        fields.push("device port");
    }
    if before.server.scpi_port != after.server.scpi_port {
        fields.push("SCPI port");
    }
    if before.device.sample_rate_hz != after.device.sample_rate_hz {
        fields.push("sample rate");
    }
    if before.device.unit != after.device.unit {
        fields.push("data type");
    }
    fields
}

fn render_errors(errors: Vec<FieldError>) -> String {
    errors
        .iter()
        .map(FieldError::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

fn describe(error: ScpiError, context: &str) -> String {
    format!("{context}: {} ({})", error.message(), error.code())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    use quickvib_core::{Clock, ExportFormat, NullLogger, SampleUnit, TestClock};
    use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend};
    use quickvib_engine::{EngineConfig, State};
    use quickvib_project::ProjectStore;

    const PROJECT: &str = r#"{
        "schemaVersion": 1,
        "name": "UiTest",
        "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
        "recording": { "durationSeconds": 0.05 }
    }"#;

    fn engine() -> Arc<Engine> {
        let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
        let mut backend = MockBackend::new(Arc::clone(&clock));
        backend
            .open(&DeviceOpenOptions::new(
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

    fn loaded() -> UiController {
        let engine = engine();
        engine.adopt_project(Project::from_json_str(PROJECT).unwrap(), None);
        UiController::new(engine)
    }

    #[test]
    fn a_controller_without_a_project_opens_on_the_starter() {
        let controller = UiController::new(engine());
        assert_eq!(controller.form().name, "Untitled");
        assert!(!controller.is_dirty());
        assert!(controller.path().is_none());
    }

    #[test]
    fn apply_pushes_the_edited_project_into_the_engine() {
        let mut controller = loaded();
        controller.form_mut().name = "Renamed".to_owned();
        controller.form_mut().duration_seconds = "0.02".to_owned();
        controller.form_mut().export_format = ExportFormat::Txt;
        assert!(controller.is_dirty());

        let outcome = controller.apply().unwrap();
        assert!(outcome.restart_required.is_empty());
        assert!(!controller.is_dirty());

        let engine = controller.engine();
        assert_eq!(engine.project().unwrap().name, "Renamed");
        assert_eq!(engine.duration_seconds(), 0.02);
        assert_eq!(engine.format(), ExportFormat::Txt);
        // A duration the engine has adopted is a duration `INIT` will use.
        assert_eq!(
            engine.project().unwrap().expected_samples(0.02).unwrap(),
            20
        );
    }

    #[test]
    fn apply_names_the_settings_that_need_a_restart() {
        let mut controller = loaded();
        controller.form_mut().set_sample_rate_text("2000");
        controller.form_mut().device_port = "9200".to_owned();
        controller.form_mut().scpi_port = "5125".to_owned();
        controller.form_mut().unit = SampleUnit::DisplacementUm;
        controller.form_mut().backend = quickvib_core::BackendKind::Tcp;

        let outcome = controller.apply().unwrap();
        assert_eq!(
            outcome.restart_required,
            [
                "backend",
                "device port",
                "SCPI port",
                "sample rate",
                "data type"
            ]
        );
        assert!(outcome.restart_note().unwrap().contains("SCPI port"));
    }

    #[test]
    fn apply_rejects_a_bad_field_and_leaves_the_engine_alone() {
        let mut controller = loaded();
        controller.form_mut().set_sample_rate_text("-1");
        let errors = controller.apply().unwrap_err();
        assert_eq!(errors[0].field, "device.sampleRateHz");
        assert_eq!(
            controller.engine().project().unwrap().device.sample_rate_hz,
            1000.0
        );
    }

    #[test]
    fn apply_is_refused_while_a_run_is_in_flight() {
        let controller = loaded();
        controller.start().unwrap();
        let mut controller = controller;
        controller.form_mut().name = "Nope".to_owned();
        let errors = controller.apply().unwrap_err();
        assert!(errors[0].message.contains("in flight"), "{errors:?}");
        controller.stop();
        assert!(!controller.engine().wait_for_run());
    }

    #[test]
    fn revert_restores_the_engines_project() {
        let mut controller = loaded();
        controller.form_mut().name = "Scratch".to_owned();
        controller.revert();
        assert_eq!(controller.form().name, "UiTest");
        assert!(!controller.is_dirty());
    }

    #[test]
    fn save_as_writes_a_file_the_loader_accepts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Saved.proj");

        let mut controller = loaded();
        controller.form_mut().name = "Saved".to_owned();
        controller.form_mut().high_pass_hz = "5".to_owned();
        controller.form_mut().set_lpf_locked(true);
        controller.form_mut().set_lpf_hz_text("400");
        controller.save_as(&path).unwrap();

        let reloaded = ProjectStore::load(&path).unwrap();
        assert_eq!(reloaded.name, "Saved");
        assert_eq!(reloaded.device.high_pass_hz, 5.0);
        assert_eq!(reloaded.device.lpf_hz, Some(400.0));
        assert_eq!(controller.path(), Some(path.as_path()));
    }

    #[test]
    fn save_without_a_path_asks_for_save_as() {
        let mut controller = loaded();
        assert!(controller.save().unwrap_err().contains("Save as"));
    }

    #[test]
    fn save_then_load_round_trips_through_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Round.proj");

        let mut controller = loaded();
        controller.form_mut().duration_seconds = "0.03".to_owned();
        controller.save_as(&path).unwrap();

        let mut other = UiController::new(engine());
        other.load(&path).unwrap();
        assert_eq!(other.form().duration_seconds, "0.03");
        assert_eq!(other.engine().duration_seconds(), 0.03);
    }

    #[test]
    fn loading_a_missing_file_is_a_message_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let mut controller = loaded();
        let error = controller.load(dir.path().join("nope.proj")).unwrap_err();
        assert!(error.contains("could not load"), "{error}");
    }

    #[test]
    fn start_records_and_the_snapshot_reports_the_measurements() {
        let controller = loaded();
        assert_eq!(controller.snapshot().state, State::Idle);

        controller.start().unwrap();
        assert!(controller.engine().wait_for_run());

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.state, State::Complete);
        assert!(snapshot.connected);
        assert_eq!(snapshot.sample_count, 50);
        assert!(snapshot.measurements.unwrap().rms > 0.0);
        assert_eq!(snapshot.project_name, "UiTest");
        assert!(snapshot.identity.starts_with("QuickVib,"));
        assert_eq!(snapshot.headline(), "COMPLETE | device connected");
    }

    #[test]
    fn start_without_a_project_reports_the_scpi_error() {
        let controller = UiController::new(engine());
        let error = controller.start().unwrap_err();
        assert!(error.contains("-221"), "{error}");
    }

    #[test]
    fn export_writes_the_capture_under_the_projects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine();
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.export.directory = dir.path().to_path_buf();
        engine.adopt_project(project, None);

        let controller = UiController::new(engine);
        assert!(controller.export("run.csv").unwrap_err().contains("-230"));

        controller.start().unwrap();
        assert!(controller.engine().wait_for_run());
        let written = controller.export("run.csv").unwrap();
        assert_eq!(written, dir.path().join("run.csv"));
        assert!(written.is_file());
        assert!(controller.export("  ").unwrap_err().contains("file name"));
    }
}
