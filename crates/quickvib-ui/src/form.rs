//! The project form: what the window shows, as plain data.
//!
//! Every editable control in the window is a field of [`ProjectForm`], and numeric fields are
//! held as the operator's own text rather than as parsed numbers. That is the whole reason
//! this type exists: a half-typed `"1e"` has to survive a repaint, and a rejected value has to
//! stay on screen next to the message explaining why. Parsing happens in one place —
//! [`ProjectForm::to_project`] — which either yields a [`Project`] that
//! [`quickvib_project::validate`] has already accepted, or the list of things to fix.

use std::path::PathBuf;

use quickvib_core::{BackendKind, ExportFormat, SampleUnit};
use quickvib_project::schema::{
    DEFAULT_ACCELERATION_RANGE, DEFAULT_DEVICE_PORT, DEFAULT_DISPLACEMENT_RANGE,
    DEFAULT_MAX_CAPTURE_BYTES, DEFAULT_SCPI_PORT, DEFAULT_VELOCITY_RANGE,
};
use quickvib_project::validate::MAX_DURATION_SECONDS;
use quickvib_project::{
    Device, Export, Identity, Measurement, Mock, MockSignal, Project, ProjectError, Recording,
    Server, SignalComponent, SCHEMA_VERSION,
};

use crate::i18n::{self, Lang};

/// Why a field was rejected, as data rather than as a sentence.
///
/// The window is drawn in Simplified Chinese by default, so a rejection cannot be a baked
/// English string: it has to survive translation. Every rule the form applies is one of these
/// variants, and [`FieldError::localized`] turns it into the sentence the operator reads.
#[derive(Debug, Clone, PartialEq)]
pub enum Issue {
    /// The field was left blank.
    Empty,
    /// The text is not a finite number.
    NotANumber {
        /// What was typed.
        input: String,
    },
    /// The value parsed but is zero or negative where only a positive value is meaningful.
    NotPositive {
        /// The offending value.
        value: f64,
    },
    /// The value parsed but is negative where zero is the floor.
    Negative {
        /// The offending value.
        value: f64,
    },
    /// The text is not a TCP port in `1..=65535`.
    BadPort {
        /// What was typed.
        input: String,
    },
    /// The SCPI port and the device port are the same number.
    DuplicatePort,
    /// The high-pass cutoff is at or above the low-pass cutoff.
    AboveLowPass {
        /// The low-pass cutoff in force, in hertz.
        lpf_hz: f64,
    },
    /// The requested capture is longer than QuickVib will record.
    DurationTooLong {
        /// The longest capture the instrument accepts, in seconds.
        max_seconds: f64,
    },
    /// Rate times duration would need more memory than the project's capture cap allows.
    CaptureTooLarge {
        /// The cap from `recording.maxCaptureBytes`.
        max_bytes: u64,
    },
    /// A run is in flight, so nothing may be applied.
    RunInFlight,
    /// A rule the project validator applied that the form does not model itself. The English
    /// sentence in [`FieldError::message`] is the whole story.
    Schema,
}

/// One rejected field, ready to be shown next to the control that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldError {
    /// The schema path of the offending field, e.g. `device.sampleRateHz`.
    pub field: String,
    /// What is wrong with it, in a sentence an operator can act on. Always English: this is
    /// the text that reaches log lines and `save()`'s error string. The window shows
    /// [`FieldError::localized`] instead.
    pub message: String,
    /// The same rejection as data, for translation.
    pub issue: Issue,
}

impl FieldError {
    /// A rejection of `field` because of `message`, with no structured issue behind it.
    #[must_use]
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            issue: Issue::Schema,
        }
    }

    /// A rejection of `field` that carries `issue`, so the window can translate it.
    #[must_use]
    pub fn of(field: impl Into<String>, message: impl Into<String>, issue: Issue) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
            issue,
        }
    }

    /// The rejection as the operator reads it: the field's name in `lang`, then the reason.
    #[must_use]
    pub fn localized(&self, lang: Lang) -> String {
        format!(
            "{}: {}",
            i18n::field_label(lang, &self.field),
            self.reason(lang)
        )
    }

    /// The reason on its own, without the field name.
    #[must_use]
    pub fn reason(&self, lang: Lang) -> String {
        i18n::issue_text(lang, &self.issue).unwrap_or_else(|| self.message.clone())
    }
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// The editable state of the window.
///
/// Construct one with [`ProjectForm::from_project`], let the window mutate the public fields,
/// then fold it back into a project with [`ProjectForm::to_project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectForm {
    /// Project name. Must not be blank.
    pub name: String,
    /// Free text shown under the name.
    pub description: String,
    /// Which backend to drive.
    pub backend: BackendKind,
    /// Sample rate in hertz. Edit through [`ProjectForm::set_sample_rate_text`] so the
    /// low-pass cutoff keeps tracking Nyquist.
    sample_rate_hz: String,
    /// Low-pass cutoff in hertz. Only meaningful while [`ProjectForm::lpf_locked`] is set;
    /// otherwise it mirrors Nyquist and is written back to the project as "unset".
    lpf_hz: String,
    /// Whether the operator pinned the low-pass cutoff instead of letting it track Nyquist.
    lpf_locked: bool,
    /// High-pass cutoff in hertz. `0` disables the filter.
    pub high_pass_hz: String,
    /// Unit of the sample stream.
    pub unit: SampleUnit,
    /// Velocity measuring range, in micrometres per second.
    pub velocity_range: String,
    /// Displacement measuring range, in micrometres.
    pub displacement_range: String,
    /// Acceleration measuring range, in metres per second squared.
    pub acceleration_range: String,
    /// Capture duration in seconds.
    pub duration_seconds: String,
    /// Port the SCPI server listens on for the UTS.
    pub scpi_port: String,
    /// Port the device dials in on.
    pub device_port: String,
    /// Export format.
    pub export_format: ExportFormat,
    /// Base directory for relative export paths.
    pub export_directory: String,
    /// Whether the CSV preamble and header row are written.
    pub include_header: bool,
    /// Subtract the mean before peak and RMS.
    pub remove_dc: bool,
}

impl Default for ProjectForm {
    fn default() -> Self {
        Self::from_project(&starter_project())
    }
}

impl ProjectForm {
    /// Fill the form from `project`.
    #[must_use]
    pub fn from_project(project: &Project) -> Self {
        let device = &project.device;
        Self {
            name: project.name.clone(),
            description: project.description.clone(),
            backend: device.backend,
            sample_rate_hz: number(device.sample_rate_hz),
            lpf_hz: number(device.effective_lpf_hz()),
            lpf_locked: device.lpf_hz.is_some(),
            high_pass_hz: number(device.high_pass_hz),
            unit: device.unit,
            velocity_range: number(device.velocity_range),
            displacement_range: number(device.displacement_range),
            acceleration_range: number(device.acceleration_range),
            duration_seconds: number(project.recording.duration_seconds),
            scpi_port: project.server.scpi_port.to_string(),
            device_port: device.port.to_string(),
            export_format: project.export.format,
            export_directory: project.export.directory.display().to_string(),
            include_header: project.export.include_header,
            remove_dc: project.measurement.remove_dc,
        }
    }

    /// The sample-rate text, exactly as typed.
    #[must_use]
    pub fn sample_rate_text(&self) -> &str {
        &self.sample_rate_hz
    }

    /// Replace the sample rate, moving the low-pass cutoff to the new Nyquist frequency
    /// unless the operator pinned it with [`ProjectForm::set_lpf_locked`].
    pub fn set_sample_rate_text(&mut self, text: impl Into<String>) {
        self.sample_rate_hz = text.into();
        if !self.lpf_locked {
            self.lpf_hz = self.nyquist_text();
        }
    }

    /// The low-pass cutoff text. While the cutoff is unpinned this is Nyquist, recomputed
    /// from the sample rate, and the window shows it read-only.
    #[must_use]
    pub fn lpf_hz_text(&self) -> &str {
        &self.lpf_hz
    }

    /// Replace the low-pass cutoff. Ignored while the cutoff is tracking Nyquist, so a
    /// stray keystroke cannot silently pin it.
    pub fn set_lpf_hz_text(&mut self, text: impl Into<String>) {
        if self.lpf_locked {
            self.lpf_hz = text.into();
        }
    }

    /// Whether the low-pass cutoff is pinned rather than tracking Nyquist.
    #[must_use]
    pub const fn lpf_locked(&self) -> bool {
        self.lpf_locked
    }

    /// Pin or unpin the low-pass cutoff. Unpinning immediately snaps it back to Nyquist.
    pub fn set_lpf_locked(&mut self, locked: bool) {
        self.lpf_locked = locked;
        if !locked {
            self.lpf_hz = self.nyquist_text();
        }
    }

    /// Nyquist for the sample rate currently in the form, as text. Empty when the rate does
    /// not parse, which the sample-rate field reports on its own.
    fn nyquist_text(&self) -> String {
        match self.sample_rate_hz.trim().parse::<f64>() {
            Ok(rate) if rate.is_finite() => number(rate / 2.0),
            _ => String::new(),
        }
    }

    /// Fold the form back into a project, using `base` for everything the window does not
    /// show — the identity block, the mock signal, the watchdog multiplier and so on.
    ///
    /// # Errors
    /// Every field that will not parse, plus every rule
    /// [`quickvib_project::validate`] rejects, in one list. The list is never empty on the
    /// error path.
    pub fn to_project(&self, base: &Project) -> Result<Project, Vec<FieldError>> {
        let mut errors = Vec::new();
        let mut project = base.clone();

        project.schema_version = SCHEMA_VERSION;
        project.name = self.name.trim().to_owned();
        project.description = self.description.clone();

        project.device.backend = self.backend;
        project.device.unit = self.unit;
        project.device.sample_rate_hz =
            positive(&mut errors, "device.sampleRateHz", &self.sample_rate_hz);
        project.device.lpf_hz = if self.lpf_locked {
            Some(positive(&mut errors, "device.lpfHz", &self.lpf_hz))
        } else {
            None
        };
        project.device.high_pass_hz =
            non_negative(&mut errors, "device.highPassHz", &self.high_pass_hz);
        project.device.velocity_range =
            positive(&mut errors, "device.velocityRange", &self.velocity_range);
        project.device.displacement_range = positive(
            &mut errors,
            "device.displacementRange",
            &self.displacement_range,
        );
        project.device.acceleration_range = positive(
            &mut errors,
            "device.accelerationRange",
            &self.acceleration_range,
        );
        let device_port = port(&mut errors, "device.port", &self.device_port);
        project.device.port = device_port.unwrap_or(1);

        project.recording.duration_seconds = positive(
            &mut errors,
            "recording.durationSeconds",
            &self.duration_seconds,
        );

        let scpi_port = port(&mut errors, "server.scpiPort", &self.scpi_port);
        project.server.scpi_port = scpi_port.unwrap_or(1);

        project.export.format = self.export_format;
        project.export.directory = PathBuf::from(self.export_directory.trim());
        project.export.include_header = self.include_header;
        project.measurement.remove_dc = self.remove_dc;

        if project.export.directory.as_os_str().is_empty() {
            project.export.directory = PathBuf::from(".");
        }
        // Compared as numbers, not as text: `5025` and `05025` are the same port, and two
        // fields that failed to parse are already reporting that on their own.
        if matches!((scpi_port, device_port), (Some(scpi), Some(device)) if scpi == device) {
            errors.push(FieldError::of(
                "server.scpiPort",
                "must differ from the device port",
                Issue::DuplicatePort,
            ));
        }

        if errors.is_empty() {
            cross_field_rules(&project, &mut errors);
        }
        if !errors.is_empty() {
            return Err(errors);
        }

        // The same rules the SCPI `MMEM:LOAD:STAT` path enforces, so a project saved from the
        // window is a project the instrument would have accepted from disk.
        quickvib_project::validate::validate(&project)
            .map_err(|error| vec![schema_error(&error)])?;
        Ok(project)
    }
}

/// The project a window opens with when nothing has been loaded yet.
///
/// Spelled out field by field rather than parsed from a JSON template so that a new schema
/// field is a compile error here — a form that silently stops round-tripping a field is the
/// one bug this crate must not have.
#[must_use]
pub fn starter_project() -> Project {
    Project {
        schema_version: SCHEMA_VERSION,
        name: "Untitled".to_owned(),
        description: String::new(),
        device: Device {
            backend: BackendKind::Mock,
            port: DEFAULT_DEVICE_PORT,
            sample_rate_hz: 100_000.0,
            unit: SampleUnit::VelocityUmPerSec,
            allowed_peers: Vec::new(),
            connect_timeout_seconds: 30.0,
            sdk_path: None,
            lpf_hz: None,
            high_pass_hz: 0.0,
            velocity_range: DEFAULT_VELOCITY_RANGE,
            displacement_range: DEFAULT_DISPLACEMENT_RANGE,
            acceleration_range: DEFAULT_ACCELERATION_RANGE,
        },
        recording: Recording {
            duration_seconds: 1.0,
            timeout_multiplier: 2.0,
            max_capture_bytes: DEFAULT_MAX_CAPTURE_BYTES,
        },
        measurement: Measurement::default(),
        export: Export::default(),
        identity: Identity::default(),
        server: Server {
            max_sessions: 8,
            scpi_port: DEFAULT_SCPI_PORT,
        },
        mock: Mock {
            signal: MockSignal {
                components: vec![SignalComponent {
                    frequency_hz: 120.0,
                    amplitude: 250.0,
                    phase_deg: 0.0,
                }],
                noise_std_dev: 0.0,
                seed: 12_345,
            },
        },
    }
}

/// Re-label a schema rejection as a form field error, keeping the dotted path the schema
/// itself uses so the message points at the control that produced it.
fn schema_error(error: &ProjectError) -> FieldError {
    match error {
        ProjectError::Invalid { field, message } | ProjectError::OutOfRange { field, message } => {
            FieldError::new(*field, message.clone())
        }
        other => FieldError::new("project", other.to_string()),
    }
}

/// Format a number the way the form shows it: shortest round-tripping form, no exponent for
/// the magnitudes an operator actually types.
fn number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn parse(errors: &mut Vec<FieldError>, field: &str, text: &str) -> Option<f64> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        errors.push(FieldError::of(field, "must not be empty", Issue::Empty));
        return None;
    }
    match trimmed.parse::<f64>() {
        Ok(value) if value.is_finite() => Some(value),
        _ => {
            errors.push(FieldError::of(
                field,
                format!("'{trimmed}' is not a finite number"),
                Issue::NotANumber {
                    input: trimmed.to_owned(),
                },
            ));
            None
        }
    }
}

fn positive(errors: &mut Vec<FieldError>, field: &str, text: &str) -> f64 {
    match parse(errors, field, text) {
        Some(value) if value > 0.0 => value,
        Some(value) => {
            errors.push(FieldError::of(
                field,
                format!("must be greater than zero, found {value}"),
                Issue::NotPositive { value },
            ));
            0.0
        }
        None => 0.0,
    }
}

fn non_negative(errors: &mut Vec<FieldError>, field: &str, text: &str) -> f64 {
    match parse(errors, field, text) {
        Some(value) if value >= 0.0 => value,
        Some(value) => {
            errors.push(FieldError::of(
                field,
                format!("must not be negative, found {value}"),
                Issue::Negative { value },
            ));
            0.0
        }
        None => 0.0,
    }
}

/// Parse a port field, recording a rejection and returning `None` when it will not parse.
fn port(errors: &mut Vec<FieldError>, field: &str, text: &str) -> Option<u16> {
    match text.trim().parse::<u32>() {
        Ok(value) if (1..=65_535).contains(&value) => Some(value as u16),
        _ => {
            errors.push(FieldError::of(
                field,
                format!("'{}' is not a port in 1..=65535", text.trim()),
                Issue::BadPort {
                    input: text.trim().to_owned(),
                },
            ));
            None
        }
    }
}

/// The rules that need the whole assembled project rather than one field's text.
///
/// [`quickvib_project::validate`] enforces all of them too and remains the last gate before a
/// project is adopted; they are repeated here so the operator gets a translated sentence
/// pointing at a control instead of the validator's English one-liner.
fn cross_field_rules(project: &Project, errors: &mut Vec<FieldError>) {
    if project.name.trim().is_empty() {
        errors.push(FieldError::of("name", "must not be empty", Issue::Empty));
    }

    let lpf = project.device.effective_lpf_hz();
    let high_pass = project.device.high_pass_hz;
    if high_pass > 0.0 && high_pass >= lpf {
        errors.push(FieldError::of(
            "device.highPassHz",
            format!("must be below the low-pass cutoff ({lpf} Hz), found {high_pass}"),
            Issue::AboveLowPass { lpf_hz: lpf },
        ));
    }

    let duration = project.recording.duration_seconds;
    if duration > MAX_DURATION_SECONDS {
        errors.push(FieldError::of(
            "recording.durationSeconds",
            format!("must be in (0, {MAX_DURATION_SECONDS}], found {duration}"),
            Issue::DurationTooLong {
                max_seconds: MAX_DURATION_SECONDS,
            },
        ));
    } else if project.expected_samples(duration).is_err() {
        let max_bytes = project.recording.max_capture_bytes;
        errors.push(FieldError::of(
            "recording.durationSeconds",
            format!(
                "ceil({duration} * {}) * 4 bytes exceeds maxCaptureBytes ({max_bytes})",
                project.device.sample_rate_hz
            ),
            Issue::CaptureTooLarge { max_bytes },
        ));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    use crate::i18n::Lang;

    fn project() -> Project {
        Project::from_json_str(
            r#"{
                "schemaVersion": 1,
                "name": "FormTest",
                "description": "a sample",
                "device": {
                    "backend": "mock",
                    "port": 9123,
                    "sampleRateHz": 100000.0,
                    "unit": "velocity_um_s",
                    "highPassHz": 2.0,
                    "velocityRange": 2500.0
                },
                "recording": { "durationSeconds": 5.0 },
                "export": { "format": "TXT", "directory": "./out" },
                "server": { "scpiPort": 5025 }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn a_form_round_trips_a_project_unchanged() {
        let original = project();
        let form = ProjectForm::from_project(&original);
        assert_eq!(form.to_project(&original).unwrap(), original);
    }

    #[test]
    fn every_shown_field_reaches_the_project() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.name = "Edited".to_owned();
        form.description = "new text".to_owned();
        form.backend = BackendKind::Tcp;
        form.unit = SampleUnit::AccelerationMPerSec2;
        form.set_sample_rate_text("48000");
        form.high_pass_hz = "10".to_owned();
        form.velocity_range = "500".to_owned();
        form.displacement_range = "250".to_owned();
        form.acceleration_range = "50".to_owned();
        form.duration_seconds = "2.5".to_owned();
        form.scpi_port = "5125".to_owned();
        form.device_port = "9223".to_owned();
        form.export_format = ExportFormat::Csv;
        form.export_directory = "/tmp/captures".to_owned();
        form.include_header = false;
        form.remove_dc = true;

        let edited = form.to_project(&base).unwrap();
        assert_eq!(edited.name, "Edited");
        assert_eq!(edited.description, "new text");
        assert_eq!(edited.device.backend, BackendKind::Tcp);
        assert_eq!(edited.device.unit, SampleUnit::AccelerationMPerSec2);
        assert_eq!(edited.device.sample_rate_hz, 48_000.0);
        assert_eq!(edited.device.high_pass_hz, 10.0);
        assert_eq!(edited.device.velocity_range, 500.0);
        assert_eq!(edited.device.displacement_range, 250.0);
        assert_eq!(edited.device.acceleration_range, 50.0);
        assert_eq!(edited.recording.duration_seconds, 2.5);
        assert_eq!(edited.server.scpi_port, 5125);
        assert_eq!(edited.device.port, 9223);
        assert_eq!(edited.export.format, ExportFormat::Csv);
        assert_eq!(edited.export.directory, PathBuf::from("/tmp/captures"));
        assert!(!edited.export.include_header);
        assert!(edited.measurement.remove_dc);
    }

    #[test]
    fn fields_the_window_does_not_show_survive_a_round_trip() {
        let mut base = project();
        base.identity.serial_number = "SN-42".to_owned();
        base.recording.timeout_multiplier = 3.0;
        base.mock.signal.noise_std_dev = 1.5;
        base.server.max_sessions = 3;
        base.device.allowed_peers = vec!["10.0.0.1".to_owned()];

        let form = ProjectForm::from_project(&base);
        let edited = form.to_project(&base).unwrap();
        assert_eq!(edited.identity.serial_number, "SN-42");
        assert_eq!(edited.recording.timeout_multiplier, 3.0);
        assert_eq!(edited.mock.signal.noise_std_dev, 1.5);
        assert_eq!(edited.server.max_sessions, 3);
        assert_eq!(edited.device.allowed_peers, ["10.0.0.1"]);
    }

    #[test]
    fn the_low_pass_tracks_nyquist_until_it_is_pinned() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        assert!(!form.lpf_locked());
        assert_eq!(form.lpf_hz_text(), "50000");

        form.set_sample_rate_text("48000");
        assert_eq!(form.lpf_hz_text(), "24000");
        assert_eq!(form.to_project(&base).unwrap().device.lpf_hz, None);

        form.set_lpf_locked(true);
        form.set_lpf_hz_text("20000");
        form.set_sample_rate_text("96000");
        assert_eq!(form.lpf_hz_text(), "20000", "a pinned cutoff must not move");
        assert_eq!(
            form.to_project(&base).unwrap().device.lpf_hz,
            Some(20_000.0)
        );

        form.set_lpf_locked(false);
        assert_eq!(form.lpf_hz_text(), "48000");
    }

    #[test]
    fn an_unpinned_cutoff_ignores_direct_edits() {
        let mut form = ProjectForm::from_project(&project());
        form.set_lpf_hz_text("7");
        assert_eq!(form.lpf_hz_text(), "50000");
    }

    #[test]
    fn unparsable_numbers_are_reported_per_field() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.set_sample_rate_text("fast");
        form.duration_seconds = String::new();
        form.high_pass_hz = "-3".to_owned();

        let errors = form.to_project(&base).unwrap_err();
        let fields: Vec<&str> = errors.iter().map(|e| e.field.as_str()).collect();
        assert!(fields.contains(&"device.sampleRateHz"), "{fields:?}");
        assert!(fields.contains(&"recording.durationSeconds"), "{fields:?}");
        assert!(fields.contains(&"device.highPassHz"), "{fields:?}");
    }

    #[test]
    fn ports_must_be_in_range_and_must_differ() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.scpi_port = "70000".to_owned();
        assert!(form
            .to_project(&base)
            .unwrap_err()
            .iter()
            .any(|e| e.field == "server.scpiPort"));

        let mut form = ProjectForm::from_project(&base);
        form.scpi_port = form.device_port.clone();
        let errors = form.to_project(&base).unwrap_err();
        assert!(errors.iter().any(|e| e.message.contains("device port")));
    }

    #[test]
    fn a_duplicate_port_is_caught_however_it_is_spelled() {
        let base = project();
        // Each spelling parses to 9123, the port the device is already using, so the texts
        // differ while the ports collide.
        for spelling in ["09123", "+9123", " 9123"] {
            let mut form = ProjectForm::from_project(&base);
            form.scpi_port = spelling.to_owned();
            let errors = form.to_project(&base).unwrap_err();
            assert!(
                errors.iter().any(|e| e.issue == Issue::DuplicatePort),
                "{spelling:?} must collide with the device port: {errors:?}"
            );
        }
    }

    #[test]
    fn two_unparsable_ports_do_not_look_like_a_duplicate() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.scpi_port = String::new();
        form.device_port = String::new();
        let errors = form.to_project(&base).unwrap_err();
        assert!(
            !errors.iter().any(|e| e.issue == Issue::DuplicatePort),
            "a blank field reports itself, not a port collision: {errors:?}"
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
    }

    #[test]
    fn cross_field_rules_point_at_a_control_in_the_operators_language() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        // Below Nyquist is fine; above the low-pass cutoff is not.
        form.high_pass_hz = "60000".to_owned();
        let errors = form.to_project(&base).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "device.highPassHz");
        assert!(errors[0].message.contains("low-pass"), "{:?}", errors[0]);
        assert_eq!(
            errors[0].issue,
            Issue::AboveLowPass { lpf_hz: 50_000.0 },
            "the rejection has to survive translation"
        );
        assert_eq!(
            errors[0].localized(Lang::Zh),
            "高通滤波: 必须低于低通截止频率（50 kHz）"
        );
    }

    #[test]
    fn an_over_long_capture_is_rejected_by_the_capture_cap() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.duration_seconds = "3600".to_owned();
        let errors = form.to_project(&base).unwrap_err();
        assert_eq!(errors[0].field, "recording.durationSeconds");
        assert!(
            matches!(errors[0].issue, Issue::CaptureTooLarge { .. }),
            "{:?}",
            errors[0]
        );
        assert!(errors[0].localized(Lang::Zh).starts_with("录制时长: "));

        form.duration_seconds = "7200".to_owned();
        let errors = form.to_project(&base).unwrap_err();
        assert_eq!(
            errors[0].issue,
            Issue::DurationTooLong {
                max_seconds: 3600.0
            }
        );
    }

    #[test]
    fn a_blank_name_is_rejected() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.name = "   ".to_owned();
        let errors = form.to_project(&base).unwrap_err();
        assert_eq!(errors[0].field, "name");
        assert_eq!(errors[0].issue, Issue::Empty);
        assert_eq!(errors[0].localized(Lang::Zh), "项目名称: 不能为空");
    }

    #[test]
    fn every_rejection_the_form_makes_is_translatable() {
        // A rejection that falls through to `Issue::Schema` shows the validator's English
        // sentence, which is exactly what this test exists to keep out of the window.
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.set_sample_rate_text("fast");
        form.duration_seconds = String::new();
        form.high_pass_hz = "-3".to_owned();
        form.scpi_port = "70000".to_owned();
        form.velocity_range = "0".to_owned();

        let errors = form.to_project(&base).unwrap_err();
        assert!(errors.len() >= 5);
        for error in &errors {
            assert_ne!(error.issue, Issue::Schema, "{error:?} cannot be translated");
            let shown = error.localized(Lang::Zh);
            assert!(
                shown
                    .chars()
                    .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)),
                "{shown} is not Chinese"
            );
        }
    }

    #[test]
    fn an_empty_export_directory_falls_back_to_the_working_directory() {
        let base = project();
        let mut form = ProjectForm::from_project(&base);
        form.export_directory = String::new();
        assert_eq!(
            form.to_project(&base).unwrap().export.directory,
            PathBuf::from(".")
        );
    }

    #[test]
    fn the_starter_project_is_valid_and_round_trips() {
        let starter = starter_project();
        quickvib_project::validate::validate(&starter).unwrap();
        let form = ProjectForm::default();
        assert_eq!(form.to_project(&starter).unwrap(), starter);
    }

    #[test]
    fn numbers_are_shown_without_a_trailing_zero() {
        assert_eq!(number(100_000.0), "100000");
        assert_eq!(number(2.5), "2.5");
        assert_eq!(number(0.0005), "0.0005");
    }
}
