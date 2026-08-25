//! The device abstraction: one thin trait, a deterministic mock, the little-endian `f32`
//! framer and the inbound TCP listener.
//!
//! Everything in this crate is pure `std` and runs on any platform. That is deliberate
//! (`docs/PLAN.md` 6.2 and 15): the framing and the accept/disconnect handling carry most of
//! the real risk on the M300 path, so they live here where Linux CI exercises them, leaving
//! `quickvib-m300` holding little more than FFI declarations.
//!
//! ```
//! use std::sync::Arc;
//! use quickvib_core::{CancelToken, SampleUnit, TestClock};
//! use quickvib_device::{DeviceBackend, DeviceOpenOptions, MockBackend, StreamRequest};
//!
//! let mut backend = MockBackend::new(Arc::new(TestClock::at_epoch()));
//! backend.open(&DeviceOpenOptions::new(1000.0, SampleUnit::VelocityUmPerSec)).unwrap();
//! assert!(backend.is_connected());
//!
//! let mut collected = Vec::new();
//! let request = StreamRequest::new(100, SampleUnit::VelocityUmPerSec, 1000.0);
//! backend
//!     .stream(&request, &mut |batch| {
//!         collected.extend_from_slice(batch.samples);
//!         Ok(())
//!     }, &CancelToken::new())
//!     .unwrap();
//! assert_eq!(collected.len(), 100);
//! ```

#![forbid(unsafe_code)]

pub mod framer;
pub mod listener;
pub mod mock;

mod backend;
mod error;

pub use backend::{
    ConnectionObserver, DeviceBackend, DeviceCapabilities, DeviceOpenOptions, SampleBatch,
    StreamOutcome, StreamRequest, BATCH_SAMPLES,
};
pub use error::DeviceError;
pub use framer::{Framer, FramerError, READ_BUFFER_BYTES};
pub use listener::{ConnectionState, InboundDeviceServer};
pub use mock::{MockBackend, MockFault, MockSignalSpec, SignalComponent};
