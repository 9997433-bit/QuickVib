//! Validation rules applied after deserialization (`docs/PLAN.md` 12).

use crate::error::ProjectError;
use crate::schema::{Project, SCHEMA_VERSION};

/// Longest capture QuickVib will accept, in seconds.
pub const MAX_DURATION_SECONDS: f64 = 3600.0;

/// Check every rule the schema table declares.
///
/// # Errors
/// [`ProjectError::Invalid`] for a bad value, [`ProjectError::OutOfRange`] for a value outside
/// its permitted range or one that trips the capture-size guard.
pub fn validate(project: &Project) -> Result<(), ProjectError> {
    if project.schema_version != SCHEMA_VERSION {
        return Err(ProjectError::invalid(
            "schemaVersion",
            format!(
                "expected {SCHEMA_VERSION}, found {}",
                project.schema_version
            ),
        ));
    }

    if project.name.trim().is_empty() {
        return Err(ProjectError::invalid("name", "must not be empty"));
    }

    let rate = project.device.sample_rate_hz;
    if !rate.is_finite() || rate <= 0.0 {
        return Err(ProjectError::invalid(
            "device.sampleRateHz",
            format!("must be a finite positive number, found {rate}"),
        ));
    }

    if project.device.port == 0 {
        return Err(ProjectError::invalid("device.port", "must be in 1..=65535"));
    }

    let connect = project.device.connect_timeout_seconds;
    if !connect.is_finite() || connect < 0.0 {
        return Err(ProjectError::invalid(
            "device.connectTimeoutSeconds",
            format!("must be a finite non-negative number, found {connect}"),
        ));
    }

    if let Some(lpf) = project.device.lpf_hz {
        if !lpf.is_finite() || lpf <= 0.0 {
            return Err(ProjectError::invalid(
                "device.lpfHz",
                format!("must be a finite positive number, found {lpf}"),
            ));
        }
    }

    let high_pass = project.device.high_pass_hz;
    if !high_pass.is_finite() || high_pass < 0.0 {
        return Err(ProjectError::invalid(
            "device.highPassHz",
            format!("must be a finite non-negative number, found {high_pass}"),
        ));
    }
    let lpf = project.device.effective_lpf_hz();
    if high_pass > 0.0 && high_pass >= lpf {
        return Err(ProjectError::invalid(
            "device.highPassHz",
            format!("must be below the low-pass cutoff ({lpf} Hz), found {high_pass}"),
        ));
    }

    for (field, range) in [
        ("device.velocityRange", project.device.velocity_range),
        (
            "device.displacementRange",
            project.device.displacement_range,
        ),
        (
            "device.accelerationRange",
            project.device.acceleration_range,
        ),
    ] {
        if !range.is_finite() || range <= 0.0 {
            return Err(ProjectError::invalid(
                field,
                format!("must be a finite positive number, found {range}"),
            ));
        }
    }

    let duration = project.recording.duration_seconds;
    if !duration.is_finite() || duration <= 0.0 || duration > MAX_DURATION_SECONDS {
        return Err(ProjectError::out_of_range(
            "recording.durationSeconds",
            format!("must be in (0, {MAX_DURATION_SECONDS}], found {duration}"),
        ));
    }

    let multiplier = project.recording.timeout_multiplier;
    if !multiplier.is_finite() || multiplier < 1.0 {
        return Err(ProjectError::invalid(
            "recording.timeoutMultiplier",
            format!("must be >= 1.0, found {multiplier}"),
        ));
    }

    if project.recording.max_capture_bytes == 0 {
        return Err(ProjectError::invalid(
            "recording.maxCaptureBytes",
            "must be > 0",
        ));
    }

    if project.measurement.response_decimals > 9 {
        return Err(ProjectError::invalid(
            "measurement.responseDecimals",
            format!(
                "must be in 0..=9, found {}",
                project.measurement.response_decimals
            ),
        ));
    }

    if project.server.max_sessions == 0 {
        return Err(ProjectError::invalid("server.maxSessions", "must be >= 1"));
    }

    if project.server.scpi_port == 0 {
        return Err(ProjectError::invalid(
            "server.scpiPort",
            "must be in 1..=65535",
        ));
    }

    // Both listeners are bound at startup, so a project that names one port twice cannot be
    // started at all. Rejecting it here means the operator finds out when the file is loaded
    // rather than from a bind failure on the next launch.
    if project.server.scpi_port == project.device.port {
        return Err(ProjectError::invalid(
            "server.scpiPort",
            format!(
                "must differ from device.port, both are {}",
                project.server.scpi_port
            ),
        ));
    }

    for (i, component) in project.mock.signal.components.iter().enumerate() {
        if !component.frequency_hz.is_finite()
            || !component.amplitude.is_finite()
            || !component.phase_deg.is_finite()
        {
            return Err(ProjectError::invalid(
                "mock.signal.components",
                format!("component {i} has a non-finite field"),
            ));
        }
    }

    let noise = project.mock.signal.noise_std_dev;
    if !noise.is_finite() || noise < 0.0 {
        return Err(ProjectError::invalid(
            "mock.signal.noiseStdDev",
            format!("must be a finite non-negative number, found {noise}"),
        ));
    }

    // The capture-size guard, computed with checked arithmetic (7.9).
    project
        .expected_samples(duration)
        .map_err(|_| {
            ProjectError::out_of_range(
                "recording.durationSeconds",
                format!(
                    "ceil({duration} * {rate}) * 4 bytes exceeds maxCaptureBytes ({})",
                    project.recording.max_capture_bytes
                ),
            )
        })
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::ScpiError;

    fn base() -> Project {
        Project::from_json_str(
            r#"{
                "schemaVersion": 1,
                "name": "Base",
                "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
                "recording": { "durationSeconds": 1.0 }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn base_project_is_valid() {
        validate(&base()).unwrap();
    }

    #[test]
    fn schema_version_mismatch_is_rejected() {
        let mut p = base();
        p.schema_version = 2;
        let err = validate(&p).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::IllegalParameterValue);
    }

    #[test]
    fn sample_rate_must_be_positive_and_finite() {
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut p = base();
            p.device.sample_rate_hz = rate;
            assert_eq!(
                validate(&p).unwrap_err().scpi_error(),
                ScpiError::IllegalParameterValue,
                "rate {rate}"
            );
        }
    }

    #[test]
    fn duration_range_is_enforced() {
        for duration in [0.0, -1.0, 3600.1, f64::NAN] {
            let mut p = base();
            p.recording.duration_seconds = duration;
            assert_eq!(
                validate(&p).unwrap_err().scpi_error(),
                ScpiError::DataOutOfRange,
                "duration {duration}"
            );
        }
        let mut p = base();
        p.recording.duration_seconds = 3600.0;
        p.recording.max_capture_bytes = u64::MAX;
        validate(&p).unwrap();
    }

    #[test]
    fn timeout_multiplier_floor_is_one() {
        let mut p = base();
        p.recording.timeout_multiplier = 0.5;
        assert_eq!(
            validate(&p).unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }

    #[test]
    fn response_decimals_range_is_enforced() {
        let mut p = base();
        p.measurement.response_decimals = 10;
        assert_eq!(
            validate(&p).unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
        p.measurement.response_decimals = 9;
        validate(&p).unwrap();
    }

    #[test]
    fn capture_cap_violation_is_out_of_range() {
        let mut p = base();
        p.device.sample_rate_hz = 100_000.0;
        p.recording.duration_seconds = 3600.0;
        let err = validate(&p).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::DataOutOfRange);
        assert!(err.to_string().contains("maxCaptureBytes"));
    }

    #[test]
    fn empty_name_is_rejected() {
        let mut p = base();
        p.name = "   ".to_owned();
        assert_eq!(
            validate(&p).unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }

    #[test]
    fn max_sessions_floor_is_one() {
        let mut p = base();
        p.server.max_sessions = 0;
        assert_eq!(
            validate(&p).unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }

    #[test]
    fn scpi_port_zero_is_rejected() {
        let mut p = base();
        p.server.scpi_port = 0;
        assert_eq!(
            validate(&p).unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }

    #[test]
    fn the_scpi_port_and_the_device_port_must_differ() {
        let mut p = base();
        p.server.scpi_port = 5025;
        p.device.port = 5025;
        let err = validate(&p).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::IllegalParameterValue);
        assert!(err.to_string().contains("device.port"), "{err}");

        p.device.port = 9123;
        validate(&p).unwrap();
    }

    #[test]
    fn lpf_must_be_positive_and_finite_when_set() {
        for lpf in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let mut p = base();
            p.device.lpf_hz = Some(lpf);
            assert_eq!(
                validate(&p).unwrap_err().scpi_error(),
                ScpiError::IllegalParameterValue,
                "lpf {lpf}"
            );
        }
        let mut p = base();
        p.device.lpf_hz = Some(400.0);
        validate(&p).unwrap();
    }

    #[test]
    fn high_pass_must_be_non_negative_and_below_the_low_pass() {
        for hp in [-1.0, f64::NAN, f64::INFINITY] {
            let mut p = base();
            p.device.high_pass_hz = hp;
            assert_eq!(
                validate(&p).unwrap_err().scpi_error(),
                ScpiError::IllegalParameterValue,
                "highPassHz {hp}"
            );
        }

        // base() runs at 1 kHz, so the tracking cutoff is 500 Hz.
        let mut p = base();
        p.device.high_pass_hz = 500.0;
        let err = validate(&p).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::IllegalParameterValue);
        assert!(err.to_string().contains("low-pass"), "{err}");

        p.device.lpf_hz = Some(800.0);
        validate(&p).unwrap();
    }

    #[test]
    fn ranges_must_be_positive_and_finite() {
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            for pick in 0..3 {
                let mut p = base();
                match pick {
                    0 => p.device.velocity_range = bad,
                    1 => p.device.displacement_range = bad,
                    _ => p.device.acceleration_range = bad,
                }
                assert_eq!(
                    validate(&p).unwrap_err().scpi_error(),
                    ScpiError::IllegalParameterValue,
                    "range {bad} on field {pick}"
                );
            }
        }
    }

    #[test]
    fn negative_noise_is_rejected() {
        let mut p = base();
        p.mock.signal.noise_std_dev = -1.0;
        assert_eq!(
            validate(&p).unwrap_err().scpi_error(),
            ScpiError::IllegalParameterValue
        );
    }
}
