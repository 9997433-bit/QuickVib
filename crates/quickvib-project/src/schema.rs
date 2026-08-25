//! The version-1 project schema (`docs/PLAN.md` 12).
//!
//! Property names are camelCase. Unknown properties are ignored on read for forward
//! compatibility; every optional field has an explicit default so a minimal file still loads.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use quickvib_core::{BackendKind, ExportFormat, SampleUnit, ScpiError};

use crate::error::ProjectError;
use crate::serde_enums::{de_backend, de_format, de_unit, ser_backend, ser_format, ser_unit};

/// The schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Default inbound port the M300 dials.
pub const DEFAULT_DEVICE_PORT: u16 = 9123;

/// Default guard on the capture buffer, in bytes (512 MiB).
pub const DEFAULT_MAX_CAPTURE_BYTES: u64 = 536_870_912;

/// A loaded, validated QuickVib project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Schema version. Must have the same major version as [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Human-readable project name.
    pub name: String,
    /// Free text.
    #[serde(default)]
    pub description: String,
    /// Device and transport settings.
    pub device: Device,
    /// Recording settings.
    pub recording: Recording,
    /// Measurement options.
    #[serde(default)]
    pub measurement: Measurement,
    /// Export defaults.
    #[serde(default)]
    pub export: Export,
    /// `*IDN?` fields.
    #[serde(default)]
    pub identity: Identity,
    /// SCPI server settings.
    #[serde(default)]
    pub server: Server,
    /// Mock backend signal definition.
    #[serde(default)]
    pub mock: Mock,
}

/// Device and transport settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    /// Which backend to drive. Overridden by `--backend`.
    #[serde(
        default,
        deserialize_with = "de_backend",
        serialize_with = "ser_backend"
    )]
    pub backend: BackendKind,
    /// Inbound port the M300 dials. Overridden by `--device-port`.
    #[serde(default = "default_device_port")]
    pub port: u16,
    /// Nominal sample rate in hertz. Required.
    pub sample_rate_hz: f64,
    /// Unit of the sample stream. Required.
    #[serde(deserialize_with = "de_unit", serialize_with = "ser_unit")]
    pub unit: SampleUnit,
    /// Inbound peers permitted on the device link. Empty accepts any peer.
    #[serde(default)]
    pub allowed_peers: Vec<String>,
    /// How long to wait for the device to dial in, in seconds.
    #[serde(default = "default_connect_timeout")]
    pub connect_timeout_seconds: f64,
    /// Directory to probe for the native SDK (M300 backend only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdk_path: Option<PathBuf>,
}

/// Recording settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    /// Capture duration in seconds. Required; must be in `(0, 3600]`.
    pub duration_seconds: f64,
    /// Watchdog multiplier: the run is aborted after `duration * multiplier + 1 s`.
    #[serde(default = "default_timeout_multiplier")]
    pub timeout_multiplier: f64,
    /// Upper bound on capture-buffer size in bytes.
    #[serde(default = "default_max_capture_bytes")]
    pub max_capture_bytes: u64,
}

/// Measurement options.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Measurement {
    /// Subtract the mean before computing peak and RMS. Peak-to-peak is unaffected.
    #[serde(default)]
    pub remove_dc: bool,
    /// Decimal places used when formatting `CALC:*` responses. `0..=9`.
    #[serde(default = "default_response_decimals")]
    pub response_decimals: u8,
}

/// Export defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Export {
    /// Active export format.
    #[serde(default, deserialize_with = "de_format", serialize_with = "ser_format")]
    pub format: ExportFormat,
    /// Base directory for relative export paths.
    #[serde(default = "default_export_directory")]
    pub directory: PathBuf,
    /// Whether the CSV metadata preamble and header row are written.
    #[serde(default = "default_true")]
    pub include_header: bool,
}

/// The four `*IDN?` fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    /// `*IDN?` field 1.
    #[serde(default = "default_manufacturer")]
    pub manufacturer: String,
    /// `*IDN?` field 2.
    #[serde(default = "default_model")]
    pub model: String,
    /// `*IDN?` field 3. Defaults to the device serial when connected, else `"0"`.
    #[serde(default = "default_serial")]
    pub serial_number: String,
    /// `*IDN?` field 4.
    #[serde(default = "default_firmware")]
    pub firmware_version: String,
}

/// SCPI server settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Server {
    /// Concurrent SCPI session cap.
    #[serde(default = "default_max_sessions")]
    pub max_sessions: usize,
}

/// Mock backend configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mock {
    /// The synthesized signal.
    #[serde(default)]
    pub signal: MockSignal,
}

/// The deterministic signal the mock backend generates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MockSignal {
    /// Superposed sine components.
    #[serde(default = "default_components")]
    pub components: Vec<SignalComponent>,
    /// Standard deviation of the additive Gaussian noise. `0` disables noise.
    #[serde(default)]
    pub noise_std_dev: f64,
    /// PRNG seed, so the stream is bit-reproducible across platforms.
    #[serde(default = "default_seed")]
    pub seed: u64,
}

/// One sine component of the mock signal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignalComponent {
    /// Frequency in hertz.
    pub frequency_hz: f64,
    /// Peak amplitude, in the project's sample unit.
    pub amplitude: f64,
    /// Phase offset in degrees.
    #[serde(default)]
    pub phase_deg: f64,
}

fn default_device_port() -> u16 {
    DEFAULT_DEVICE_PORT
}
fn default_connect_timeout() -> f64 {
    30.0
}
fn default_timeout_multiplier() -> f64 {
    2.0
}
fn default_max_capture_bytes() -> u64 {
    DEFAULT_MAX_CAPTURE_BYTES
}
fn default_response_decimals() -> u8 {
    4
}
fn default_export_directory() -> PathBuf {
    PathBuf::from(".")
}
fn default_true() -> bool {
    true
}
fn default_manufacturer() -> String {
    "QuickVib".to_owned()
}
fn default_model() -> String {
    "M300-SCPI".to_owned()
}
fn default_serial() -> String {
    "0".to_owned()
}
fn default_firmware() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
fn default_max_sessions() -> usize {
    8
}
fn default_seed() -> u64 {
    12345
}
fn default_components() -> Vec<SignalComponent> {
    vec![SignalComponent {
        frequency_hz: 100.0,
        amplitude: 1.0,
        phase_deg: 0.0,
    }]
}

impl Default for Measurement {
    fn default() -> Self {
        Self {
            remove_dc: false,
            response_decimals: default_response_decimals(),
        }
    }
}

impl Default for Export {
    fn default() -> Self {
        Self {
            format: ExportFormat::default(),
            directory: default_export_directory(),
            include_header: true,
        }
    }
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            manufacturer: default_manufacturer(),
            model: default_model(),
            serial_number: default_serial(),
            firmware_version: default_firmware(),
        }
    }
}

impl Default for Server {
    fn default() -> Self {
        Self {
            max_sessions: default_max_sessions(),
        }
    }
}

impl Default for MockSignal {
    fn default() -> Self {
        Self {
            components: default_components(),
            noise_std_dev: 0.0,
            seed: default_seed(),
        }
    }
}

impl Project {
    /// Parse and validate a project from a JSON string.
    ///
    /// # Errors
    /// [`ProjectError::Parse`] if the document is not valid JSON or does not match the schema,
    /// or any validation error from [`crate::validate::validate`].
    pub fn from_json_str(json: &str) -> Result<Self, ProjectError> {
        let project: Self = serde_json::from_str(json).map_err(|e| ProjectError::Parse {
            path: None,
            message: e.to_string(),
        })?;
        crate::validate::validate(&project)?;
        Ok(project)
    }

    /// Serialize to pretty JSON, exactly as `MMEM:STOR:STAT` writes it.
    ///
    /// # Errors
    /// [`ProjectError::Parse`] if serialization fails, which in practice cannot happen for this
    /// schema (there are no maps with non-string keys and no non-finite floats after
    /// validation).
    pub fn to_json_string(&self) -> Result<String, ProjectError> {
        serde_json::to_string_pretty(self).map_err(|e| ProjectError::Parse {
            path: None,
            message: e.to_string(),
        })
    }

    /// Number of samples a capture of `duration_s` at this project's sample rate must collect.
    ///
    /// Computed with checked arithmetic so an absurd rate times an absurd duration produces
    /// [`ScpiError::DataOutOfRange`] rather than a debug-mode panic or a release-mode wrap
    /// (`docs/PLAN.md` 7.9).
    ///
    /// # Errors
    /// [`ScpiError::DataOutOfRange`] if the sample count is not finite, is zero, overflows, or
    /// would need more than `recording.maxCaptureBytes` of buffer.
    pub fn expected_samples(&self, duration_s: f64) -> Result<usize, ScpiError> {
        let raw = duration_s * self.device.sample_rate_hz;
        if !raw.is_finite() || raw <= 0.0 {
            return Err(ScpiError::DataOutOfRange);
        }
        let ceil = raw.ceil();
        if ceil > u64::MAX as f64 {
            return Err(ScpiError::DataOutOfRange);
        }
        let count = ceil as u64;
        let bytes = count
            .checked_mul(std::mem::size_of::<f32>() as u64)
            .ok_or(ScpiError::DataOutOfRange)?;
        if bytes > self.recording.max_capture_bytes {
            return Err(ScpiError::DataOutOfRange);
        }
        usize::try_from(count).map_err(|_| ScpiError::DataOutOfRange)
    }

    /// The watchdog deadline for a capture of `duration_s`.
    #[must_use]
    pub fn watchdog_timeout(&self, duration_s: f64) -> std::time::Duration {
        let secs = duration_s * self.recording.timeout_multiplier + 1.0;
        std::time::Duration::from_secs_f64(secs.clamp(0.001, 86_400.0))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const MINIMAL: &str = r#"{
        "schemaVersion": 1,
        "name": "Minimal",
        "device": { "sampleRateHz": 1000.0, "unit": "displacement_um" },
        "recording": { "durationSeconds": 1.0 }
    }"#;

    #[test]
    fn minimal_document_gets_every_default() {
        let p = Project::from_json_str(MINIMAL).unwrap();
        assert_eq!(p.description, "");
        assert_eq!(p.device.backend, BackendKind::Mock);
        assert_eq!(p.device.port, 9123);
        assert!(p.device.allowed_peers.is_empty());
        assert_eq!(p.device.connect_timeout_seconds, 30.0);
        assert_eq!(p.recording.timeout_multiplier, 2.0);
        assert_eq!(p.recording.max_capture_bytes, DEFAULT_MAX_CAPTURE_BYTES);
        assert!(!p.measurement.remove_dc);
        assert_eq!(p.measurement.response_decimals, 4);
        assert_eq!(p.export.format, ExportFormat::Csv);
        assert_eq!(p.export.directory, PathBuf::from("."));
        assert!(p.export.include_header);
        assert_eq!(p.identity.manufacturer, "QuickVib");
        assert_eq!(p.identity.model, "M300-SCPI");
        assert_eq!(p.identity.serial_number, "0");
        assert_eq!(p.server.max_sessions, 8);
        assert_eq!(p.mock.signal.components.len(), 1);
        assert_eq!(p.mock.signal.seed, 12345);
    }

    #[test]
    fn unknown_properties_are_ignored() {
        let json = r#"{
            "schemaVersion": 1,
            "name": "Fwd",
            "somethingFromTheFuture": { "nested": [1, 2, 3] },
            "device": { "sampleRateHz": 1.0, "unit": "velocity_um_s", "futureField": 7 },
            "recording": { "durationSeconds": 1.0 }
        }"#;
        assert!(Project::from_json_str(json).is_ok());
    }

    #[test]
    fn missing_required_field_is_a_parse_error() {
        for json in [
            r#"{ "name": "x", "device": { "sampleRateHz": 1.0, "unit": "velocity_um_s" }, "recording": { "durationSeconds": 1.0 } }"#,
            r#"{ "schemaVersion": 1, "device": { "sampleRateHz": 1.0, "unit": "velocity_um_s" }, "recording": { "durationSeconds": 1.0 } }"#,
            r#"{ "schemaVersion": 1, "name": "x", "recording": { "durationSeconds": 1.0 } }"#,
            r#"{ "schemaVersion": 1, "name": "x", "device": { "unit": "velocity_um_s" }, "recording": { "durationSeconds": 1.0 } }"#,
            r#"{ "schemaVersion": 1, "name": "x", "device": { "sampleRateHz": 1.0 }, "recording": { "durationSeconds": 1.0 } }"#,
            r#"{ "schemaVersion": 1, "name": "x", "device": { "sampleRateHz": 1.0, "unit": "velocity_um_s" } }"#,
        ] {
            let err = Project::from_json_str(json).unwrap_err();
            assert_eq!(
                err.scpi_error(),
                ScpiError::IllegalParameterValue,
                "for {json}"
            );
        }
    }

    #[test]
    fn bad_enum_value_is_rejected_by_the_parser() {
        let json = r#"{
            "schemaVersion": 1, "name": "x",
            "device": { "sampleRateHz": 1.0, "unit": "furlongs" },
            "recording": { "durationSeconds": 1.0 }
        }"#;
        let err = Project::from_json_str(json).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::IllegalParameterValue);
        assert!(err.to_string().contains("velocity_um_s"), "{err}");
    }

    #[test]
    fn round_trip_through_json_is_lossless() {
        let p = Project::from_json_str(MINIMAL).unwrap();
        let text = p.to_json_string().unwrap();
        let q = Project::from_json_str(&text).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn expected_samples_rounds_up() {
        let mut p = Project::from_json_str(MINIMAL).unwrap();
        p.device.sample_rate_hz = 1000.0;
        assert_eq!(p.expected_samples(0.5).unwrap(), 500);
        assert_eq!(p.expected_samples(0.0005).unwrap(), 1);
        assert_eq!(p.expected_samples(0.00051).unwrap(), 1);
        assert_eq!(p.expected_samples(0.0011).unwrap(), 2);
    }

    #[test]
    fn expected_samples_guards_the_buffer_cap() {
        let mut p = Project::from_json_str(MINIMAL).unwrap();
        p.device.sample_rate_hz = 100_000.0;
        p.recording.max_capture_bytes = 4_000;
        assert_eq!(p.expected_samples(1.0), Err(ScpiError::DataOutOfRange));
        assert!(p.expected_samples(0.01).is_ok());
    }

    #[test]
    fn expected_samples_does_not_overflow() {
        let mut p = Project::from_json_str(MINIMAL).unwrap();
        p.device.sample_rate_hz = f64::MAX;
        p.recording.max_capture_bytes = u64::MAX;
        assert_eq!(p.expected_samples(3600.0), Err(ScpiError::DataOutOfRange));
    }

    #[test]
    fn watchdog_matches_the_formula() {
        let p = Project::from_json_str(MINIMAL).unwrap();
        assert_eq!(
            p.watchdog_timeout(2.0),
            std::time::Duration::from_secs_f64(5.0)
        );
    }
}
