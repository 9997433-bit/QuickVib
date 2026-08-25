//! The [`DeviceBackend`] trait and its supporting value types (`docs/PLAN.md` 10).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use quickvib_core::{CancelToken, SampleUnit};

use crate::error::DeviceError;

/// Samples per callback. Large enough that per-batch overhead is negligible at 100 kS/s,
/// small enough that cancellation is observed promptly.
pub const BATCH_SAMPLES: usize = 4096;

/// Immutable description of what the connected device can do.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceCapabilities {
    /// Device model string.
    pub model: String,
    /// Device serial number, used as `*IDN?` field 3 when the project does not override it.
    pub serial_number: String,
    /// Device firmware version.
    pub firmware_version: String,
    /// Sample rate the device reports.
    pub sample_rate_hz: f64,
    /// Unit the device is configured for.
    pub unit: SampleUnit,
    /// Longest capture the device will sustain, in seconds.
    pub max_record_seconds: f64,
}

/// One contiguous chunk of samples, already converted from little-endian `f32`.
///
/// Borrowed rather than owned: the reader thread reuses its buffer, so there is no allocation
/// per batch. The consumer copies into the capture buffer inside the callback.
#[derive(Debug, Clone, Copy)]
pub struct SampleBatch<'a> {
    /// The samples in this batch.
    pub samples: &'a [f32],
    /// Index of `samples[0]` within the run.
    pub start_index: u64,
    /// Monotonic time, from the injected clock, at which the batch was produced.
    pub arrived_at: Duration,
}

/// How a stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamOutcome {
    /// The requested sample count was delivered.
    Completed,
    /// The cancellation token fired.
    Cancelled,
    /// The transport dropped before the run finished.
    LinkLost,
    /// No data arrived within the backend's own timeout.
    TimedOut,
}

/// What the engine is asking the backend to deliver.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamRequest {
    /// Number of samples to deliver before reporting [`StreamOutcome::Completed`].
    pub expected_samples: u64,
    /// Nominal duration of the capture, for backends that pace themselves.
    pub duration: Duration,
    /// Unit the samples are expected to be in.
    pub unit: SampleUnit,
    /// Nominal sample rate in hertz.
    pub sample_rate_hz: f64,
}

impl StreamRequest {
    /// A request for `expected_samples` at `sample_rate_hz`, with the duration derived from
    /// the two.
    #[must_use]
    pub fn new(expected_samples: u64, unit: SampleUnit, sample_rate_hz: f64) -> Self {
        let seconds = if sample_rate_hz > 0.0 {
            expected_samples as f64 / sample_rate_hz
        } else {
            0.0
        };
        Self {
            expected_samples,
            duration: Duration::from_secs_f64(seconds.clamp(0.0, 86_400.0)),
            unit,
            sample_rate_hz,
        }
    }
}

/// Everything a backend needs in order to bring its transport up.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceOpenOptions {
    /// Nominal sample rate declared by the project (D13).
    pub sample_rate_hz: f64,
    /// Unit declared by the project.
    pub unit: SampleUnit,
    /// Inbound port the device dials.
    pub device_port: u16,
    /// Peers permitted on the inbound link. Empty accepts any peer.
    pub allowed_peers: Vec<String>,
    /// How long to wait for the device to dial in.
    pub connect_timeout: Duration,
    /// Directory to probe for the native SDK, when the backend needs one.
    pub sdk_path: Option<PathBuf>,
}

impl DeviceOpenOptions {
    /// Options with the defaults from the project schema and no SDK path.
    #[must_use]
    pub fn new(sample_rate_hz: f64, unit: SampleUnit) -> Self {
        Self {
            sample_rate_hz,
            unit,
            device_port: 9123,
            allowed_peers: Vec::new(),
            connect_timeout: Duration::from_secs(30),
            sdk_path: None,
        }
    }
}

/// A source of vibration samples.
///
/// Six methods, deliberately: buffering, duration enforcement, measurement and export all live
/// above this line in platform-neutral crates, so they are written and tested once.
pub trait DeviceBackend: Send {
    /// Whether the transport is live. Backs `SYST:DEV:CONN?`.
    fn is_connected(&self) -> bool;

    /// Bring the transport up. Idempotent.
    ///
    /// # Errors
    /// Any [`DeviceError`] the transport reports while opening.
    fn open(&mut self, options: &DeviceOpenOptions) -> Result<(), DeviceError>;

    /// Describe the currently connected device.
    ///
    /// # Errors
    /// [`DeviceError::NotConnected`] when [`DeviceBackend::is_connected`] is false.
    fn capabilities(&self) -> Result<DeviceCapabilities, DeviceError>;

    /// Deliver batches to `on_batch` on the calling thread until the requested sample count
    /// has been delivered, the token is cancelled, or the link fails.
    ///
    /// # Errors
    /// Any [`DeviceError`] the transport reports, or an error returned by `on_batch`.
    fn stream(
        &mut self,
        request: &StreamRequest,
        on_batch: &mut dyn FnMut(SampleBatch<'_>) -> Result<(), DeviceError>,
        cancel: &CancelToken,
    ) -> Result<StreamOutcome, DeviceError>;

    /// Stop an in-flight stream promptly from another thread. Safe to call when idle.
    ///
    /// Takes `&self` precisely because the `ABOR` handler must be able to call it while
    /// [`DeviceBackend::stream`] holds `&mut self` on the reader thread.
    ///
    /// # Errors
    /// Any [`DeviceError`] the transport reports while stopping.
    fn stop(&self) -> Result<(), DeviceError>;

    /// Tear the transport down, reporting failures that `Drop` cannot.
    ///
    /// # Errors
    /// Any [`DeviceError`] the transport reports while closing.
    fn close(&mut self) -> Result<(), DeviceError> {
        Ok(())
    }
}

/// Connection transitions, published to whoever is interested. Replaces the `event` a
/// garbage-collected language would use.
pub trait ConnectionObserver: Send + Sync {
    /// Called after the transport connects or disconnects.
    fn on_connection_changed(&self, connected: bool, peer: Option<SocketAddr>);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn stream_request_derives_its_duration() {
        let r = StreamRequest::new(500, SampleUnit::VelocityUmPerSec, 1000.0);
        assert_eq!(r.duration, Duration::from_millis(500));
    }

    #[test]
    fn stream_request_tolerates_a_zero_rate() {
        let r = StreamRequest::new(500, SampleUnit::VelocityUmPerSec, 0.0);
        assert_eq!(r.duration, Duration::ZERO);
    }

    #[test]
    fn open_options_carry_the_schema_defaults() {
        let o = DeviceOpenOptions::new(100_000.0, SampleUnit::DisplacementUm);
        assert_eq!(o.device_port, 9123);
        assert_eq!(o.connect_timeout, Duration::from_secs(30));
        assert!(o.allowed_peers.is_empty());
        assert!(o.sdk_path.is_none());
    }
}
