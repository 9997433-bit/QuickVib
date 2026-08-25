//! The socket-fed backend (`docs/PLAN.md` 7.4, 15).
//!
//! Where [`crate::MockBackend`] synthesizes samples in process, this one consumes whatever the
//! inbound device link framed: the caller binds the device port, points the listener at a
//! [`SampleChannel`], and hands the same channel to a [`StreamBackend`]. That is the shape the
//! M300 path has — a real instrument dials in and pushes little-endian `f32` — with no native
//! SDK anywhere in it, so Linux CI exercises the whole route from socket to measurement.
//!
//! Opening does **not** wait for a device. A vibrometer that has not been switched on yet must
//! leave the instrument answering `SYST:DEV:CONN? -> 0` and `INIT -> -241`, not stall startup.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, SampleUnit};

use crate::backend::{
    DeviceBackend, DeviceCapabilities, DeviceOpenOptions, SampleBatch, StreamOutcome,
    StreamRequest, BATCH_SAMPLES,
};
use crate::error::DeviceError;
use crate::sink::{SampleChannel, Taken};

/// How long a `take` parks before the loop re-checks cancellation. Short enough that `ABOR`
/// feels instant, long enough that an idle link costs nothing.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// A backend fed by the inbound device link.
pub struct StreamBackend {
    channel: Arc<SampleChannel>,
    clock: Arc<dyn Clock>,
    open: bool,
    sample_rate_hz: f64,
    unit: SampleUnit,
    model: String,
    serial_number: String,
    stopped: Arc<AtomicBool>,
}

impl std::fmt::Debug for StreamBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamBackend")
            .field("open", &self.open)
            .field("connected", &self.channel.is_connected())
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("unit", &self.unit)
            .finish()
    }
}

impl StreamBackend {
    /// A backend reading from `channel`, closed until [`DeviceBackend::open`] is called.
    #[must_use]
    pub fn new(channel: Arc<SampleChannel>, clock: Arc<dyn Clock>) -> Self {
        Self {
            channel,
            clock,
            open: false,
            sample_rate_hz: 0.0,
            unit: SampleUnit::VelocityUmPerSec,
            model: "QuickVib-Stream".to_owned(),
            serial_number: "0".to_owned(),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Override the model reported through [`DeviceBackend::capabilities`].
    #[must_use]
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Override the serial number reported through [`DeviceBackend::capabilities`].
    #[must_use]
    pub fn with_serial_number(mut self, serial: impl Into<String>) -> Self {
        self.serial_number = serial.into();
        self
    }

    /// The channel this backend drains, for wiring the device listener to it.
    #[must_use]
    pub fn channel(&self) -> Arc<SampleChannel> {
        Arc::clone(&self.channel)
    }
}

impl DeviceBackend for StreamBackend {
    fn is_connected(&self) -> bool {
        self.open && self.channel.is_connected()
    }

    fn open(&mut self, options: &DeviceOpenOptions) -> Result<(), DeviceError> {
        if options.sample_rate_hz <= 0.0 || !options.sample_rate_hz.is_finite() {
            return Err(DeviceError::unsupported(format!(
                "sample rate {} is not a positive finite number",
                options.sample_rate_hz
            )));
        }
        self.sample_rate_hz = options.sample_rate_hz;
        self.unit = options.unit;
        self.open = true;
        self.stopped.store(false, Ordering::Release);
        Ok(())
    }

    fn capabilities(&self) -> Result<DeviceCapabilities, DeviceError> {
        if !self.open {
            return Err(DeviceError::NotConnected);
        }
        Ok(DeviceCapabilities {
            model: self.model.clone(),
            serial_number: self.serial_number.clone(),
            firmware_version: env!("CARGO_PKG_VERSION").to_owned(),
            sample_rate_hz: self.sample_rate_hz,
            unit: self.unit,
            max_record_seconds: 3600.0,
        })
    }

    fn stream(
        &mut self,
        request: &StreamRequest,
        on_batch: &mut dyn FnMut(SampleBatch<'_>) -> Result<(), DeviceError>,
        cancel: &CancelToken,
    ) -> Result<StreamOutcome, DeviceError> {
        if !self.open {
            return Err(DeviceError::NotConnected);
        }
        if !self.channel.is_connected() {
            return Err(DeviceError::NotConnected);
        }
        self.stopped.store(false, Ordering::Release);
        // A run captures from `INIT` onwards; whatever the device pushed while the instrument
        // sat idle is not part of it.
        self.channel.reset();

        let mut buffer: Vec<f32> = Vec::with_capacity(BATCH_SAMPLES);
        let mut delivered: u64 = 0;

        while delivered < request.expected_samples {
            if cancel.is_cancelled() || self.stopped.load(Ordering::Acquire) {
                return Ok(StreamOutcome::Cancelled);
            }

            let remaining = (request.expected_samples - delivered).min(BATCH_SAMPLES as u64);
            buffer.clear();
            match self
                .channel
                .take(&mut buffer, remaining as usize, POLL_INTERVAL)
            {
                Taken::Samples => {}
                Taken::Idle => continue,
                Taken::Disconnected => return Ok(StreamOutcome::LinkLost),
            }

            on_batch(SampleBatch {
                samples: &buffer,
                start_index: delivered,
                arrived_at: self.clock.monotonic(),
            })?;
            delivered += buffer.len() as u64;
        }

        Ok(StreamOutcome::Completed)
    }

    fn stop(&self) -> Result<(), DeviceError> {
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        self.open = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::sink::SampleSink;
    use quickvib_core::TestClock;

    fn clock() -> Arc<dyn Clock> {
        Arc::new(TestClock::at_epoch())
    }

    fn open_backend(channel: &Arc<SampleChannel>) -> StreamBackend {
        let mut backend = StreamBackend::new(Arc::clone(channel), clock());
        backend
            .open(&DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        backend
    }

    fn request(samples: u64) -> StreamRequest {
        StreamRequest::new(samples, SampleUnit::VelocityUmPerSec, 1000.0)
    }

    #[test]
    fn opening_does_not_wait_for_a_device() {
        let channel = Arc::new(SampleChannel::default());
        let backend = open_backend(&channel);
        assert!(!backend.is_connected());
        // Capabilities are answerable before anything dials in, which is what lets `*IDN?`
        // work on a cold instrument.
        assert_eq!(backend.capabilities().unwrap().sample_rate_hz, 1000.0);
    }

    #[test]
    fn opening_with_a_bad_rate_is_rejected() {
        let mut backend = StreamBackend::new(Arc::new(SampleChannel::default()), clock());
        let mut options = DeviceOpenOptions::new(0.0, SampleUnit::VelocityUmPerSec);
        assert!(backend.open(&options).is_err());
        options.sample_rate_hz = f64::INFINITY;
        assert!(backend.open(&options).is_err());
    }

    #[test]
    fn connectivity_follows_the_link() {
        let channel = Arc::new(SampleChannel::default());
        let backend = open_backend(&channel);
        channel.on_connected(None);
        assert!(backend.is_connected());
        channel.on_disconnected();
        assert!(!backend.is_connected());
    }

    #[test]
    fn streaming_without_a_device_is_not_connected() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        let result = backend.stream(&request(10), &mut |_| Ok(()), &CancelToken::new());
        assert!(matches!(result, Err(DeviceError::NotConnected)));
    }

    #[test]
    fn a_full_stream_completes_with_exactly_the_requested_samples() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);

        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            for chunk in 0..10 {
                producer.on_samples(&vec![chunk as f32; 100]);
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let mut collected = Vec::new();
        let outcome = backend
            .stream(
                &request(1000),
                &mut |batch| {
                    collected.extend_from_slice(batch.samples);
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();

        join.join().unwrap();
        assert_eq!(outcome, StreamOutcome::Completed);
        assert_eq!(collected.len(), 1000);
        assert_eq!(collected[0], 0.0);
        assert_eq!(collected[999], 9.0);
    }

    #[test]
    fn extra_samples_beyond_the_request_stay_queued() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);

        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            producer.on_samples(&[1.0; 50]);
        });

        let mut collected = Vec::new();
        backend
            .stream(
                &request(20),
                &mut |batch| {
                    collected.extend_from_slice(batch.samples);
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();
        join.join().unwrap();
        assert_eq!(collected.len(), 20);
        // The device keeps streaming after the run ends; the surplus is simply still there.
        assert_eq!(channel.len(), 30);
    }

    #[test]
    fn samples_queued_before_the_run_are_discarded() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);
        // Pushed while the instrument was idle: not part of the capture.
        channel.on_samples(&[99.0; 500]);

        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            producer.on_samples(&[1.0; 10]);
        });

        let mut collected = Vec::new();
        backend
            .stream(
                &request(10),
                &mut |batch| {
                    collected.extend_from_slice(batch.samples);
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();
        join.join().unwrap();
        assert_eq!(collected, vec![1.0; 10]);
    }

    #[test]
    fn a_dropped_link_mid_run_reports_link_lost() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);

        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            producer.on_samples(&[1.0; 10]);
            std::thread::sleep(Duration::from_millis(20));
            producer.on_disconnected();
        });

        let mut delivered = 0usize;
        let outcome = backend
            .stream(
                &request(1000),
                &mut |batch| {
                    delivered += batch.samples.len();
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();
        join.join().unwrap();
        assert_eq!(outcome, StreamOutcome::LinkLost);
        assert_eq!(delivered, 10);
    }

    #[test]
    fn cancellation_stops_promptly() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);

        let cancel = CancelToken::new();
        let token = cancel.clone();
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            token.cancel();
        });

        let outcome = backend
            .stream(&request(1_000_000), &mut |_| Ok(()), &cancel)
            .unwrap();
        join.join().unwrap();
        assert_eq!(outcome, StreamOutcome::Cancelled);
    }

    #[test]
    fn stop_from_another_thread_ends_the_stream() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);

        let stopped = Arc::clone(&backend.stopped);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            stopped.store(true, Ordering::Release);
        });

        let outcome = backend
            .stream(&request(1_000_000), &mut |_| Ok(()), &CancelToken::new())
            .unwrap();
        join.join().unwrap();
        assert_eq!(outcome, StreamOutcome::Cancelled);
    }

    #[test]
    fn a_callback_error_aborts_the_stream() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);

        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            producer.on_samples(&[1.0; 100]);
        });

        let result = backend.stream(
            &request(100),
            &mut |_| Err(DeviceError::unsupported("consumer is full")),
            &CancelToken::new(),
        );
        join.join().unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn close_marks_the_backend_disconnected() {
        let channel = Arc::new(SampleChannel::default());
        let mut backend = open_backend(&channel);
        channel.on_connected(None);
        assert!(backend.is_connected());
        backend.close().unwrap();
        assert!(!backend.is_connected());
        assert!(matches!(
            backend.capabilities(),
            Err(DeviceError::NotConnected)
        ));
    }

    #[test]
    fn the_channel_handle_is_shared_not_copied() {
        let channel = Arc::new(SampleChannel::default());
        let backend = StreamBackend::new(Arc::clone(&channel), clock());
        backend.channel().on_connected(None);
        assert!(channel.is_connected());
    }
}
