//! The committed `samples/Test.proj` must always load and validate.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use quickvib_core::{BackendKind, ExportFormat, SampleUnit};
use quickvib_project::ProjectStore;

fn sample_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("samples")
        .join("Test.proj")
}

#[test]
fn sample_project_loads_and_validates() {
    let project = ProjectStore::load(sample_path()).unwrap();

    assert_eq!(project.schema_version, 1);
    assert_eq!(project.name, "Test");
    assert_eq!(project.device.backend, BackendKind::Mock);
    assert_eq!(project.device.port, 9123);
    assert_eq!(project.device.sample_rate_hz, 100_000.0);
    assert_eq!(project.device.unit, SampleUnit::VelocityUmPerSec);
    assert_eq!(project.device.effective_lpf_hz(), 50_000.0);
    assert_eq!(project.device.high_pass_hz, 0.0);
    assert_eq!(project.device.active_range(), 1000.0);
    assert_eq!(project.server.scpi_port, 5025);
    assert_eq!(project.recording.duration_seconds, 5.0);
    assert_eq!(project.measurement.response_decimals, 4);
    assert_eq!(project.export.format, ExportFormat::Csv);
    assert_eq!(project.identity.serial_number, "SN-0001");
    assert_eq!(project.mock.signal.components.len(), 2);
    assert_eq!(project.mock.signal.seed, 12345);
}

#[test]
fn sample_project_capture_fits_the_default_buffer_cap() {
    let project = ProjectStore::load(sample_path()).unwrap();
    assert_eq!(
        project
            .expected_samples(project.recording.duration_seconds)
            .unwrap(),
        500_000
    );
}
