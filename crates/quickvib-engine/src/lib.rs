//! The QuickVib instrument engine.
//!
//! This is where the state machine, the error queue, the OPC register and the recording
//! pipeline live. It depends on `quickvib-scpi` for the parsed [`quickvib_scpi::Command`] type
//! but never the other way round, which is what keeps the parser free of instrument state
//! (`docs/PLAN.md` 6.2).
//!
//! ```
//! use std::sync::Arc;
//! use quickvib_core::{Clock, NullLogger, SampleUnit, TestClock};
//! use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend};
//! use quickvib_engine::{dispatch, Engine, EngineConfig};
//! use quickvib_project::Project;
//! use quickvib_scpi::{parse_line, Response};
//!
//! let clock: Arc<dyn Clock> = Arc::new(TestClock::at_epoch());
//! let mut backend = MockBackend::new(Arc::clone(&clock));
//! backend.open(&DeviceOpenOptions::new(1000.0, SampleUnit::VelocityUmPerSec)).unwrap();
//!
//! let engine = Engine::new(EngineConfig {
//!     clock,
//!     logger: Arc::new(NullLogger),
//!     backend: Box::new(backend),
//!     last_project: None,
//!     version: "1.0.0".to_owned(),
//! });
//! engine.adopt_project(
//!     Project::from_json_str(
//!         r#"{ "schemaVersion": 1, "name": "Doc",
//!              "device": { "sampleRateHz": 1000.0, "unit": "velocity_um_s" },
//!              "recording": { "durationSeconds": 0.01 } }"#,
//!     )
//!     .unwrap(),
//!     None,
//! );
//!
//! for command in parse_line(b"INIT").unwrap() {
//!     dispatch(&engine, &command);
//! }
//! assert!(engine.wait_for_run());
//! assert_eq!(engine.capture().unwrap().len(), 10);
//! ```

#![forbid(unsafe_code)]

pub mod dispatch;
pub mod engine;
pub mod errors;
pub mod opc;
pub mod recording;
pub mod state;

pub use dispatch::dispatch;
pub use engine::{Engine, EngineConfig, NotificationSink};
pub use errors::ErrorQueue;
pub use opc::Opc;
pub use recording::{RunOutcome, RunPlan, RunResult};
pub use state::{transition, Event, IllegalTransition, State};
