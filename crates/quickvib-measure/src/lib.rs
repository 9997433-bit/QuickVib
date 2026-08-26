//! Statistics and export over a QuickVib capture buffer.
//!
//! Both halves of this crate are pure functions of the same `&[f32]` (`docs/PLAN.md` 6.2):
//! [`stats::compute`] reduces the buffer to peak / RMS / peak-to-peak, and [`export`] writes it
//! out as CSV or TXT. Nothing here touches the network or the instrument state.
//!
//! ```
//! use quickvib_measure::{compute, MeasureOptions, NonEmptyCapture};
//!
//! // A pure sine of amplitude 2.0 has peak 2.0, RMS 2/sqrt(2) and peak-to-peak 4.0.
//! let samples: Vec<f32> = (0..1000)
//!     .map(|i| (2.0 * (std::f64::consts::TAU * i as f64 / 1000.0).sin()) as f32)
//!     .collect();
//! let capture = NonEmptyCapture::new(&samples).unwrap();
//! let m = compute(capture, MeasureOptions::default());
//! assert!((m.peak - 2.0).abs() < 1e-3);
//! assert!((m.rms - 2.0 / 2.0_f64.sqrt()).abs() < 1e-3);
//! assert!((m.peak_to_peak - 4.0).abs() < 1e-3);
//! ```

#![forbid(unsafe_code)]

pub mod export;
pub mod stats;

pub use export::{write_capture, CaptureMetadata, ExportError};
pub use stats::{compute, format_fixed, MeasureOptions, MeasurementSet, NonEmptyCapture};
