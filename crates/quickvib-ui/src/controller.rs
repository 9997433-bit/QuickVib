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

use crate::form::{starter_project, FieldError, Issue, ProjectForm};
use crate::i18n::{Label, Lang};
use crate::status::StatusSnapshot;

/// What the operator asked the instrument to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Open a project file.
    Load(PathBuf),
    /// Write the project to a file.
    Save(PathBuf),
    /// Start a capture.
    Start,
    /// Export the last capture.
    Export,
}

impl Action {
    /// "Could not …", in `lang`.
    #[must_use]
    pub fn failed_text(&self, lang: Lang) -> String {
        match (lang, self) {
            (Lang::Zh, Self::Load(path)) => format!("无法加载 {}", path.display()),
            (Lang::Zh, Self::Save(path)) => format!("无法保存 {}", path.display()),
            (Lang::Zh, Self::Start) => "无法开始录制".to_owned(),
            (Lang::Zh, Self::Export) => "无法导出数据".to_owned(),
            (Lang::En, Self::Load(path)) => format!("could not load {}", path.display()),
            (Lang::En, Self::Save(path)) => format!("could not save {}", path.display()),
            (Lang::En, Self::Start) => "could not start the recording".to_owned(),
            (Lang::En, Self::Export) => "could not export the capture".to_owned(),
        }
    }
}

/// Why an action the operator asked for did not happen.
///
/// Structured rather than pre-rendered because the window is drawn in Chinese: the reason has
/// to survive the language switch, and the SCPI code inside it has to survive translation
/// untouched so it still matches the manual and the UTS log.
#[derive(Debug, Clone, PartialEq)]
pub enum ActionError {
    /// The instrument refused, with a SCPI-99 code.
    Refused {
        /// What was attempted.
        action: Action,
        /// Why the instrument said no.
        error: ScpiError,
    },
    /// *Save* on a project that has never been written to disk.
    NoProjectFile,
    /// A path or file name field was left blank.
    BlankName,
    /// The form could not be folded into a project.
    Fields(Vec<FieldError>),
}

impl ActionError {
    /// The message the status bar shows, in `lang`.
    #[must_use]
    pub fn localized(&self, lang: Lang) -> String {
        match self {
            Self::Refused { action, error } => {
                crate::i18n::refusal(lang, &action.failed_text(lang), *error)
            }
            Self::NoProjectFile => match lang {
                Lang::Zh => "该项目尚未保存过，请使用「另存为」".to_owned(),
                Lang::En => "this project has no file yet — use Save as".to_owned(),
            },
            Self::BlankName => lang.t(Label::NoticeEmptyFileName).to_owned(),
            Self::Fields(errors) => errors
                .iter()
                .map(|error| error.localized(lang))
                .collect::<Vec<_>>()
                .join("; "),
        }
    }
}

impl std::fmt::Display for ActionError {
    /// The English rendering, which is what log lines and `Debug` output carry.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.localized(Lang::En))
    }
}

/// A setting the running process cannot pick up, because it was consumed once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartField {
    /// Which backend the engine drives.
    Backend,
    /// The port the device dials in on.
    DevicePort,
    /// The port the UTS connects in on.
    ScpiPort,
    /// The sample rate the backend was opened with.
    SampleRate,
    /// The unit the backend was opened with.
    DataType,
}

impl RestartField {
    /// The name of the setting, in `lang`.
    #[must_use]
    pub const fn label(self, lang: Lang) -> &'static str {
        lang.t(match self {
            Self::Backend => Label::FieldBackend,
            Self::DevicePort => Label::FieldDevicePort,
            Self::ScpiPort => Label::FieldScpiPort,
            Self::SampleRate => Label::FieldSampleRate,
            Self::DataType => Label::FieldDataType,
        })
    }
}

/// What [`UiController::apply`] did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ApplyOutcome {
    /// Settings that were written into the project but that the already-open transports and
    /// the already-opened backend cannot pick up until QuickVib is restarted.
    pub restart_required: Vec<RestartField>,
}

impl ApplyOutcome {
    /// A sentence for the activity line, or `None` when everything took effect immediately.
    #[must_use]
    pub fn restart_note(&self, lang: Lang) -> Option<String> {
        if self.restart_required.is_empty() {
            return None;
        }
        let names: Vec<&str> = self
            .restart_required
            .iter()
            .map(|field| field.label(lang))
            .collect();
        let separator = match lang {
            Lang::Zh => "、",
            Lang::En => ", ",
        };
        Some(format!(
            "{}: {}",
            lang.t(Label::RestartHeading),
            names.join(separator)
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
            return Err(vec![FieldError::of(
                "project",
                "a recording is in flight; stop it before applying changes",
                Issue::RunInFlight,
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
    /// [`ActionError::Refused`]: the file is missing, malformed, invalid, or a run is in
    /// flight.
    pub fn load(&mut self, path: impl AsRef<Path>) -> Result<(), ActionError> {
        let path = path.as_ref();
        self.engine
            .load_project(path)
            .map_err(|error| ActionError::Refused {
                action: Action::Load(path.to_path_buf()),
                error,
            })?;
        self.path = Some(path.to_path_buf());
        self.revert();
        Ok(())
    }

    /// Apply the form, then write the project back to the path it came from.
    ///
    /// # Errors
    /// The rejected fields, or the reason the write failed. Saving a project that has never
    /// been on disk is [`ActionError::NoProjectFile`], which names *Save as*.
    pub fn save(&mut self) -> Result<PathBuf, ActionError> {
        let Some(path) = self.path.clone() else {
            return Err(ActionError::NoProjectFile);
        };
        self.save_as(path)
    }

    /// Apply the form, then write the project to `path` and remember it as the current file.
    ///
    /// # Errors
    /// The rejected fields, or the reason the write failed.
    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<PathBuf, ActionError> {
        let path = path.as_ref().to_path_buf();
        self.apply().map_err(ActionError::Fields)?;
        self.path = Some(path.clone());
        // Going through the engine keeps the runtime overrides, the auto-load-last record and
        // the log line identical to `MMEM:STOR:STAT`.
        self.engine
            .store_project(&path)
            .map_err(|error| ActionError::Refused {
                action: Action::Save(path.clone()),
                error,
            })?;
        Ok(path)
    }

    /// Start a capture, as `INIT` does.
    ///
    /// # Errors
    /// [`ActionError::Refused`]: no project, already recording, no device, or a capture that
    /// would exceed the buffer cap.
    pub fn start(&self) -> Result<(), ActionError> {
        self.engine
            .start_recording()
            .map_err(|error| ActionError::Refused {
                action: Action::Start,
                error,
            })
    }

    /// Abort the active capture, as `ABOR` does. Never an error.
    pub fn stop(&self) {
        self.engine.abort();
    }

    /// Export the last completed capture, resolving `name` against the project's export
    /// directory exactly as `MMEM:STOR:TRAC` does.
    ///
    /// # Errors
    /// [`ActionError::BlankName`] for an empty file name, or [`ActionError::Refused`] when
    /// there is no completed capture or the path is not writable.
    pub fn export(&self, name: &str) -> Result<PathBuf, ActionError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ActionError::BlankName);
        }
        self.engine
            .export_capture(Path::new(name))
            .map_err(|error| ActionError::Refused {
                action: Action::Export,
                error,
            })
    }

    /// Read the instrument in one pass.
    #[must_use]
    pub fn snapshot(&self) -> StatusSnapshot {
        let measurements = self.engine.measurements().ok();
        let project = self.engine.project();
        StatusSnapshot {
            state: self.engine.state(),
            connected: self.engine.device_connected(),
            device_peer: self.link.as_ref().and_then(|link| link.peer()),
            identity: self.engine.identity(),
            project_name: project
                .as_ref()
                .map_or_else(String::new, |project| project.name.clone()),
            unit: project.as_ref().map(|project| project.device.unit),
            sample_rate_hz: project
                .as_ref()
                .map_or(0.0, |project| project.device.sample_rate_hz),
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
fn restart_required(before: &Project, after: &Project) -> Vec<RestartField> {
    let mut fields = Vec::new();
    if before.device.backend != after.device.backend {
        fields.push(RestartField::Backend);
    }
    if before.device.port != after.device.port {
        fields.push(RestartField::DevicePort);
    }
    if before.server.scpi_port != after.server.scpi_port {
        fields.push(RestartField::ScpiPort);
    }
    if before.device.sample_rate_hz != after.device.sample_rate_hz {
        fields.push(RestartField::SampleRate);
    }
    if before.device.unit != after.device.unit {
        fields.push(RestartField::DataType);
    }
    fields
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    use quickvib_core::{Clock, ExportFormat, NullLogger, SampleUnit, ScpiError, TestClock};
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
                RestartField::Backend,
                RestartField::DevicePort,
                RestartField::ScpiPort,
                RestartField::SampleRate,
                RestartField::DataType
            ]
        );

        let note = outcome.restart_note(Lang::Zh).unwrap();
        assert_eq!(
            note,
            "需重启后生效: 数据来源、设备端口、SCPI 端口、采样率、数据类型"
        );
        assert!(outcome
            .restart_note(Lang::En)
            .unwrap()
            .contains("SCPI port"));
        assert!(ApplyOutcome::default().restart_note(Lang::Zh).is_none());
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
        assert_eq!(errors[0].issue, Issue::RunInFlight);
        assert_eq!(
            errors[0].localized(Lang::Zh),
            "项目信息: 录制进行中：请先停止再修改配置"
        );
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
        let error = controller.save().unwrap_err();
        assert_eq!(error, ActionError::NoProjectFile);
        assert_eq!(
            error.localized(Lang::Zh),
            "该项目尚未保存过，请使用「另存为」"
        );
        assert!(error.localized(Lang::En).contains("Save as"));
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
        let missing = dir.path().join("nope.proj");
        let error = controller.load(&missing).unwrap_err();
        assert_eq!(
            error,
            ActionError::Refused {
                action: Action::Load(missing.clone()),
                error: ScpiError::FileNameNotFound,
            }
        );

        // The operator reads Chinese; the SCPI code inside the sentence is untranslated on
        // purpose, because that is what the manual and the UTS log show.
        let zh = error.localized(Lang::Zh);
        assert!(zh.starts_with("无法加载 "), "{zh}");
        assert!(zh.contains("文件不存在（-256）"), "{zh}");
        assert!(error.localized(Lang::En).contains("could not load"));
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
        assert_eq!(snapshot.unit, Some(SampleUnit::VelocityUmPerSec));
        assert_eq!(snapshot.sample_rate_hz, 1000.0);
        assert!(snapshot.identity.starts_with("QuickVib,"));
        assert_eq!(snapshot.headline(Lang::Zh), "完成 · 已连接");
    }

    #[test]
    fn start_without_a_project_reports_the_scpi_error() {
        let controller = UiController::new(engine());
        let error = controller.start().unwrap_err();
        assert!(error.localized(Lang::Zh).contains("-221"), "{error}");
        assert_eq!(error.localized(Lang::Zh), "无法开始录制：设置冲突（-221）");
    }

    #[test]
    fn export_writes_the_capture_under_the_projects_directory() {
        let dir = tempfile::tempdir().unwrap();
        let engine = engine();
        let mut project = Project::from_json_str(PROJECT).unwrap();
        project.export.directory = dir.path().to_path_buf();
        engine.adopt_project(project, None);

        let controller = UiController::new(engine);
        assert!(controller
            .export("run.csv")
            .unwrap_err()
            .localized(Lang::Zh)
            .contains("-230"));

        controller.start().unwrap();
        assert!(controller.engine().wait_for_run());
        let written = controller.export("run.csv").unwrap();
        assert_eq!(written, dir.path().join("run.csv"));
        assert!(written.is_file());

        let blank = controller.export("  ").unwrap_err();
        assert_eq!(blank, ActionError::BlankName);
        assert_eq!(blank.localized(Lang::Zh), "请先输入导出文件名");
    }
}
