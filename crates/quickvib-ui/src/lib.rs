//! The QuickVib desktop front end.
//!
//! The crate is deliberately split in two. [`form`] and [`controller`] are the view *model*:
//! plain data, plain functions, no windowing dependency, and therefore unit-testable on a CI
//! box with no display. [`window`] is the egui/eframe view, compiled only with the `gui`
//! feature, and it contains no rules of its own — every edit it makes goes through
//! [`UiController`].
//!
//! The GUI never owns the instrument. It holds an `Arc<Engine>` — the same engine the SCPI
//! sessions dispatch onto — so a field changed in the window and a `CONF:REC:DUR` sent over
//! port 5025 land in exactly one place.
//!
//! ```
//! use quickvib_project::Project;
//! use quickvib_ui::ProjectForm;
//!
//! let project = Project::from_json_str(
//!     r#"{
//!         "schemaVersion": 1,
//!         "name": "Demo",
//!         "device": { "sampleRateHz": 100000.0, "unit": "velocity_um_s" },
//!         "recording": { "durationSeconds": 5.0 }
//!     }"#,
//! )
//! .unwrap();
//!
//! let mut form = ProjectForm::from_project(&project);
//! form.set_sample_rate_text("48000");
//! // The low-pass filter tracks Nyquist until the operator pins it.
//! assert_eq!(form.lpf_hz_text(), "24000");
//!
//! let edited = form.to_project(&project).unwrap();
//! assert_eq!(edited.device.sample_rate_hz, 48_000.0);
//! assert_eq!(edited.device.effective_lpf_hz(), 24_000.0);
//! ```

#![forbid(unsafe_code)]

pub mod controller;
pub mod form;
pub mod status;

#[cfg(feature = "gui")]
pub mod window;

pub use controller::{ApplyOutcome, UiController};
pub use form::{starter_project, FieldError, ProjectForm};
pub use status::StatusSnapshot;

#[cfg(feature = "gui")]
pub use window::{run, GuiOptions};
