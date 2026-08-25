//! Regression coverage for project replacement during an active run (Round 1 B5).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;

use quickvib_core::{Clock, ExportFormat, NullLogger, SampleUnit, ScpiError, TestClock};
use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend, MockFault};
use quickvib_engine::{Engine, EngineConfig};
use quickvib_project::Project;

const PROJECT: &str = r#"{
    "schemaVersion": 1,
    "name": "ActiveProject",
    "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
    "recording": { "durationSeconds": 0.05 },
    "export": { "format": "csv" }
}"#;

trait IntoAdoptResult {
    fn into_adopt_result(self) -> Result<(), ScpiError>;
}

// This compatibility implementation lets the regression compile against the pre-fix API,
// where `adopt_project` returned `()`, and fail as an assertion rather than a type error.
impl IntoAdoptResult for () {
    fn into_adopt_result(self) -> Result<(), ScpiError> {
        Ok(())
    }
}

impl IntoAdoptResult for Result<(), ScpiError> {
    fn into_adopt_result(self) -> Result<(), ScpiError> {
        self
    }
}

fn stalled_engine() -> Arc<Engine> {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let mut backend = MockBackend::new(Arc::clone(&clock)).with_fault(MockFault::Stall);
    backend
        .open(&DeviceOpenOptions::new(
            1000.0,
            SampleUnit::VelocityUmPerSec,
        ))
        .unwrap();
    let engine = Engine::new(EngineConfig {
        clock,
        logger: Arc::new(NullLogger),
        backend: Box::new(backend),
        last_project: None,
        version: "round2-test".to_owned(),
    });
    engine
        .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
        .into_adopt_result()
        .unwrap();
    engine
}

fn replacement_project() -> Project {
    let mut replacement = Project::from_json_str(PROJECT).unwrap();
    replacement.name = "ForbiddenReplacement".to_owned();
    replacement.recording.duration_seconds = 2.5;
    replacement.export.format = ExportFormat::Txt;
    replacement
}

fn finish_stalled_run(engine: &Arc<Engine>) {
    engine.abort();
    assert!(!engine.wait_for_run());
}

#[test]
fn adopt_project_rejects_active_run_without_clobbering_plan() {
    let engine = stalled_engine();
    engine.start_recording().unwrap();

    let result = engine
        .adopt_project(
            replacement_project(),
            Some(PathBuf::from("replacement.proj")),
        )
        .into_adopt_result();
    let observed = (
        engine.duration_seconds(),
        engine.format(),
        engine.project().unwrap().name,
        engine.project_path(),
    );
    finish_stalled_run(&engine);

    assert_eq!(result, Err(ScpiError::SettingsConflict));
    assert_eq!(observed.0, 0.05);
    assert_eq!(observed.1, ExportFormat::Csv);
    assert_eq!(observed.2, "ActiveProject");
    assert_eq!(observed.3, None);
}

#[test]
fn load_project_rejects_active_run_without_clobbering_plan() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("replacement.proj");
    let replacement = replacement_project();
    quickvib_project::ProjectStore::save(&replacement, &path).unwrap();

    let engine = stalled_engine();
    engine.start_recording().unwrap();

    let result = engine.load_project(&path);
    let observed = (
        engine.duration_seconds(),
        engine.format(),
        engine.project().unwrap().name,
        engine.project_path(),
    );
    finish_stalled_run(&engine);

    assert_eq!(result, Err(ScpiError::SettingsConflict));
    assert_eq!(observed.0, 0.05);
    assert_eq!(observed.1, ExportFormat::Csv);
    assert_eq!(observed.2, "ActiveProject");
    assert_eq!(observed.3, None);
}
