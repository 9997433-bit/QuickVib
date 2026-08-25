//! Shared domain vocabulary for QuickVib.
//!
//! This crate is the only leaf every other QuickVib crate depends on. It holds the handful of
//! types that more than one crate needs and nothing else — no I/O, no networking, no
//! serialization. Keeping it small is deliberate (see `docs/PLAN.md` 6.2): it should never
//! become a dumping ground.
//!
//! ```
//! use quickvib_core::{SampleUnit, ScpiError};
//!
//! assert_eq!(SampleUnit::VelocityUmPerSec.label(), "um/s");
//! assert_eq!(ScpiError::DataOutOfRange.code(), -222);
//! ```

#![forbid(unsafe_code)]

pub mod cancel;
pub mod clock;
pub mod error;
pub mod format;
pub mod log;
pub mod unit;

pub use cancel::CancelToken;
pub use clock::{Clock, SystemClock, TestClock};
pub use error::{ScpiError, ScpiErrorEntry};
pub use format::{BackendKind, ExportFormat};
pub use log::{Level, Logger, NullLogger};
pub use unit::{ParseUnitError, SampleUnit};
