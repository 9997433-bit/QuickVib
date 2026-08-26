//! The QuickVib project file: schema, load/save, validation and the last-project record.
//!
//! A project is a small JSON document (`docs/PLAN.md` 12) describing the device, the recording
//! duration, the measurement options, the export defaults and the `*IDN?` identity. Unknown
//! properties are ignored so a newer file still loads on an older build; a *major*
//! `schemaVersion` mismatch is rejected.
//!
//! ```
//! use quickvib_project::Project;
//!
//! let json = r#"{
//!   "schemaVersion": 1,
//!   "name": "Test",
//!   "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
//!   "recording": { "durationSeconds": 0.5 }
//! }"#;
//! let project = Project::from_json_str(json).unwrap();
//! assert_eq!(project.name, "Test");
//! assert_eq!(project.expected_samples(0.5).unwrap(), 500);
//! ```

#![forbid(unsafe_code)]

pub mod bind_host;
pub mod error;
pub mod last_project;
pub mod schema;
pub mod store;
pub mod validate;

mod serde_enums;

pub use bind_host::{join_host_port, parse_bind_host, BindHostError, DEFAULT_BIND_HOST};
pub use error::ProjectError;
pub use last_project::{state_dir, LastProjectStore, STATE_DIR_ENV};
pub use schema::{
    Device, Export, Identity, Measurement, Mock, MockSignal, Project, Recording, Server,
    SignalComponent, SCHEMA_VERSION,
};
pub use store::ProjectStore;
