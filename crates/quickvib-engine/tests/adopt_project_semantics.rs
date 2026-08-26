//! Regression coverage for capture invalidation at project and abort boundaries.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use quickvib_core::{Clock, NullLogger, SampleUnit, ScpiError, TestClock};
use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend, MockFault};
use quickvib_engine::{dispatch, Engine, EngineConfig, State};
use quickvib_project::Project;
use quickvib_scpi::{parse_line, Response};

const PROJECT: &str = r#"{
    "schemaVersion": 1,
    "name": "AdoptSemantics",
    "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
    "recording": { "durationSeconds": 0.02 },
    "mock": { "signal": { "components": [
        { "frequencyHz": 50.0, "amplitude": 2.0, "phaseDeg": 0.0 }
    ], "seed": 17 } }
}"#;

fn engine_with(fault: MockFault) -> Arc<Engine> {
    let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
    let mut backend = MockBackend::new(Arc::clone(&clock)).with_fault(fault);
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
        version: "1.0.0-test".to_owned(),
    });
    engine
        .adopt_project(Project::from_json_str(PROJECT).unwrap(), None)
        .unwrap();
    engine
}

fn complete_capture(engine: &Arc<Engine>) {
    engine.start_recording().unwrap();
    assert!(engine.wait_for_run());
    assert_eq!(engine.state(), State::Complete);
    assert!(engine.capture().is_ok());
    assert!(engine.measurements().is_ok());
}

fn send(engine: &Arc<Engine>, line: &str) -> String {
    let commands = parse_line(line.as_bytes()).unwrap();
    let responses: Vec<Response> = commands
        .iter()
        .map(|command| dispatch(engine, command))
        .collect();
    let mut output = Vec::new();
    quickvib_scpi::write_response_line(&mut output, &responses).unwrap();
    String::from_utf8(output).unwrap().trim_end().to_owned()
}

fn assert_scpi_data_is_stale(engine: &Arc<Engine>, query: &str) {
    assert_eq!(send(engine, query), "", "{query} returned stale data");
    assert_eq!(
        send(engine, "SYST:ERR?"),
        "-230,\"Data corrupt or stale\"",
        "{query} did not queue -230"
    );
}

#[test]
fn adopting_project_after_completed_capture_must_clear_complete_status() {
    let engine = engine_with(MockFault::None);
    complete_capture(&engine);

    let mut replacement = Project::from_json_str(PROJECT).unwrap();
    replacement.name = "Replacement".to_owned();
    engine.adopt_project(replacement, None).unwrap();

    assert_ne!(
        engine.state(),
        State::Complete,
        "adopt_project discarded the capture but left REC:STAT? at COMPLETE"
    );
}

#[test]
fn adopting_project_after_completed_capture_makes_fetch_and_measurements_stale() {
    let engine = engine_with(MockFault::None);
    complete_capture(&engine);

    let mut replacement = Project::from_json_str(PROJECT).unwrap();
    replacement.name = "Replacement".to_owned();
    engine.adopt_project(replacement, None).unwrap();

    assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
    assert_eq!(
        engine.measurements().unwrap_err(),
        ScpiError::DataCorruptOrStale
    );
    assert_scpi_data_is_stale(&engine, "FETC?");
    assert_scpi_data_is_stale(&engine, "CALC:MEAS:ALL?");
}

#[test]
fn abort_discards_fetch_and_measurement_data() {
    let engine = engine_with(MockFault::Stall);

    assert_eq!(send(&engine, "INIT"), "");
    assert_eq!(send(&engine, "ABOR"), "");
    assert_eq!(send(&engine, "REC:WAIT?"), "0");
    assert_eq!(send(&engine, "REC:STAT?"), "ABORTED");

    assert_eq!(engine.capture().unwrap_err(), ScpiError::DataCorruptOrStale);
    assert_eq!(
        engine.measurements().unwrap_err(),
        ScpiError::DataCorruptOrStale
    );
    assert_scpi_data_is_stale(&engine, "FETC?");
    assert_scpi_data_is_stale(&engine, "CALC:MEAS:ALL?");
}
