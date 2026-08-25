//! The default backend: a deterministic, pure-Rust signal generator (`docs/PLAN.md` 7.4).
//!
//! Because the peak, RMS and peak-to-peak of a synthesized sine are analytically known, the
//! mock doubles as the oracle for the measurement math. The PRNG is vendored rather than taken
//! from a crate so the byte-for-byte output is QuickVib's own guarantee — a golden-vector test
//! catches any accidental change.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, SampleUnit};

use crate::backend::{
    DeviceBackend, DeviceCapabilities, DeviceOpenOptions, SampleBatch, StreamOutcome,
    StreamRequest, BATCH_SAMPLES,
};
use crate::error::DeviceError;

/// One sine component of the synthesized signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SignalComponent {
    /// Frequency in hertz.
    pub frequency_hz: f64,
    /// Peak amplitude, in the configured sample unit.
    pub amplitude: f64,
    /// Phase offset in degrees.
    pub phase_deg: f64,
}

/// The signal the mock generates.
#[derive(Debug, Clone, PartialEq)]
pub struct MockSignalSpec {
    /// Superposed sine components.
    pub components: Vec<SignalComponent>,
    /// Standard deviation of the additive Gaussian noise. `0` disables noise entirely, which
    /// also makes the stream exactly analytic.
    pub noise_std_dev: f64,
    /// PRNG seed.
    pub seed: u64,
}

impl Default for MockSignalSpec {
    fn default() -> Self {
        Self {
            components: vec![SignalComponent {
                frequency_hz: 100.0,
                amplitude: 1.0,
                phase_deg: 0.0,
            }],
            noise_std_dev: 0.0,
            seed: 12345,
        }
    }
}

/// Deliberate misbehaviour, for negative testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MockFault {
    /// Behave normally.
    #[default]
    None,
    /// Never deliver a sample, so the run watchdog has to fire.
    Stall,
    /// Deliver this many samples, then fail hard with [`DeviceError::LinkLost`].
    LinkLostAfter(usize),
    /// Deliver this many samples, then stop cleanly but short of the requested count.
    ShortStream(usize),
}

/// A deterministic, cross-platform stand-in for the M300.
pub struct MockBackend {
    clock: Arc<dyn Clock>,
    signal: MockSignalSpec,
    fault: MockFault,
    pace: bool,
    open: bool,
    sample_rate_hz: f64,
    unit: SampleUnit,
    serial_number: String,
    stopped: Arc<AtomicBool>,
}

impl std::fmt::Debug for MockBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockBackend")
            .field("open", &self.open)
            .field("sample_rate_hz", &self.sample_rate_hz)
            .field("unit", &self.unit)
            .field("fault", &self.fault)
            .finish()
    }
}

impl MockBackend {
    /// A mock with the default signal, closed until [`DeviceBackend::open`] is called.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            signal: MockSignalSpec::default(),
            fault: MockFault::None,
            pace: false,
            open: false,
            sample_rate_hz: 0.0,
            unit: SampleUnit::VelocityUmPerSec,
            serial_number: "MOCK-0001".to_owned(),
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Use a specific signal definition.
    #[must_use]
    pub fn with_signal(mut self, signal: MockSignalSpec) -> Self {
        self.signal = signal;
        self
    }

    /// Inject a fault.
    #[must_use]
    pub fn with_fault(mut self, fault: MockFault) -> Self {
        self.fault = fault;
        self
    }

    /// Deliver samples in real time rather than as fast as possible.
    ///
    /// Off by default: with the injected [`quickvib_core::TestClock`] a paced stream still
    /// completes instantly, but with the system clock an unpaced stream is what makes the
    /// test suite fast.
    #[must_use]
    pub fn with_pacing(mut self, pace: bool) -> Self {
        self.pace = pace;
        self
    }

    /// Override the serial number reported through [`DeviceBackend::capabilities`].
    #[must_use]
    pub fn with_serial_number(mut self, serial: impl Into<String>) -> Self {
        self.serial_number = serial.into();
        self
    }

    /// Replace the fault mode on an already-constructed backend.
    pub fn set_fault(&mut self, fault: MockFault) {
        self.fault = fault;
    }

    /// How a stream that ran into its fault limit should report itself.
    ///
    /// Only called once the limit has actually been reached: a fault whose limit is beyond
    /// the requested sample count never fires, because the run finishes first.
    fn fault_outcome(&self, delivered: u64) -> Result<StreamOutcome, DeviceError> {
        match self.fault {
            MockFault::LinkLostAfter(_) => Err(DeviceError::link_lost(format!(
                "injected fault after {delivered} samples"
            ))),
            MockFault::ShortStream(_) => Ok(StreamOutcome::LinkLost),
            _ => Ok(StreamOutcome::Completed),
        }
    }

    /// Generate `count` samples starting at sample index `start`, appending to `out`.
    fn generate(&self, start: u64, count: usize, rng: &mut Xoshiro256PlusPlus, out: &mut Vec<f32>) {
        out.clear();
        let rate = if self.sample_rate_hz > 0.0 {
            self.sample_rate_hz
        } else {
            1.0
        };
        for i in 0..count {
            let t = (start + i as u64) as f64 / rate;
            let mut value = 0.0_f64;
            for component in &self.signal.components {
                let phase = component.phase_deg.to_radians();
                value += component.amplitude
                    * (std::f64::consts::TAU * component.frequency_hz * t + phase).sin();
            }
            if self.signal.noise_std_dev > 0.0 {
                value += self.signal.noise_std_dev * rng.next_standard_normal();
            }
            out.push(value as f32);
        }
    }
}

impl DeviceBackend for MockBackend {
    fn is_connected(&self) -> bool {
        self.open
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
            model: "QuickVib-Mock".to_owned(),
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
        self.stopped.store(false, Ordering::Release);

        if self.fault == MockFault::Stall {
            // Deliver nothing at all and let the run watchdog do its job.
            while !cancel.is_cancelled() && !self.stopped.load(Ordering::Acquire) {
                let _ = cancel.wait_timeout(Duration::from_millis(10));
            }
            return Ok(StreamOutcome::Cancelled);
        }

        let hard_stop = match self.fault {
            MockFault::LinkLostAfter(n) | MockFault::ShortStream(n) => Some(n as u64),
            _ => None,
        };

        let mut rng = Xoshiro256PlusPlus::seeded(self.signal.seed);
        let mut buffer: Vec<f32> = Vec::with_capacity(BATCH_SAMPLES);
        let mut delivered: u64 = 0;

        while delivered < request.expected_samples {
            if cancel.is_cancelled() || self.stopped.load(Ordering::Acquire) {
                return Ok(StreamOutcome::Cancelled);
            }

            let mut remaining = request.expected_samples - delivered;
            if let Some(limit) = hard_stop {
                if delivered >= limit {
                    return self.fault_outcome(delivered);
                }
                remaining = remaining.min(limit - delivered);
            }

            let batch_len = remaining.min(BATCH_SAMPLES as u64) as usize;
            self.generate(delivered, batch_len, &mut rng, &mut buffer);

            if self.pace && self.sample_rate_hz > 0.0 {
                self.clock.sleep(Duration::from_secs_f64(
                    batch_len as f64 / self.sample_rate_hz,
                ));
            }

            on_batch(SampleBatch {
                samples: &buffer,
                start_index: delivered,
                arrived_at: self.clock.monotonic(),
            })?;
            delivered += batch_len as u64;
        }

        // Reaching here means every requested sample was delivered, so no fault limit was
        // crossed on the way: the loop returns through `fault_outcome` when one is.
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

/// Xoshiro256++, seeded through SplitMix64.
///
/// Vendored rather than depending on `rand` (D18): the mock's output must be bit-reproducible
/// across versions and platforms, and pinning the algorithm here is a stronger guarantee than
/// relying on a dependency's stability promise.
#[derive(Debug, Clone)]
pub struct Xoshiro256PlusPlus {
    state: [u64; 4],
    spare_normal: Option<f64>,
}

impl Xoshiro256PlusPlus {
    /// Seed the generator. Any seed, including zero, produces a usable state.
    #[must_use]
    pub fn seeded(seed: u64) -> Self {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        };
        Self {
            state: [next(), next(), next(), next()],
            spare_normal: None,
        }
    }

    /// The next raw 64-bit output.
    pub fn next_u64(&mut self) -> u64 {
        let result = self.state[0]
            .wrapping_add(self.state[3])
            .rotate_left(23)
            .wrapping_add(self.state[0]);
        let t = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    /// A uniform value in `[0, 1)` using the top 53 bits, the standard construction.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// A standard normal deviate via Box–Muller, caching the second of each pair.
    pub fn next_standard_normal(&mut self) -> f64 {
        if let Some(spare) = self.spare_normal.take() {
            return spare;
        }
        // `next_f64` can return exactly 0, whose logarithm is -inf; nudge it.
        let u1 = self.next_f64().max(f64::MIN_POSITIVE);
        let u2 = self.next_f64();
        let radius = (-2.0 * u1.ln()).sqrt();
        let angle = std::f64::consts::TAU * u2;
        self.spare_normal = Some(radius * angle.sin());
        radius * angle.cos()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::TestClock;

    fn clock() -> Arc<dyn Clock> {
        Arc::new(TestClock::at_epoch())
    }

    fn open_mock(rate: f64) -> MockBackend {
        let mut backend = MockBackend::new(clock());
        backend
            .open(&DeviceOpenOptions::new(rate, SampleUnit::VelocityUmPerSec))
            .unwrap();
        backend
    }

    fn collect(backend: &mut MockBackend, expected: u64) -> (Vec<f32>, StreamOutcome) {
        let request = StreamRequest::new(expected, SampleUnit::VelocityUmPerSec, 1000.0);
        let mut samples = Vec::new();
        let outcome = backend
            .stream(
                &request,
                &mut |batch| {
                    samples.extend_from_slice(batch.samples);
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();
        (samples, outcome)
    }

    #[test]
    fn is_not_connected_until_opened() {
        let mut backend = MockBackend::new(clock());
        assert!(!backend.is_connected());
        assert!(matches!(
            backend.capabilities(),
            Err(DeviceError::NotConnected)
        ));
        backend
            .open(&DeviceOpenOptions::new(1000.0, SampleUnit::DisplacementUm))
            .unwrap();
        assert!(backend.is_connected());
        assert_eq!(
            backend.capabilities().unwrap().unit,
            SampleUnit::DisplacementUm
        );
    }

    #[test]
    fn opening_with_a_bad_rate_is_rejected() {
        let mut backend = MockBackend::new(clock());
        let mut options = DeviceOpenOptions::new(0.0, SampleUnit::VelocityUmPerSec);
        assert!(backend.open(&options).is_err());
        options.sample_rate_hz = f64::NAN;
        assert!(backend.open(&options).is_err());
    }

    #[test]
    fn delivers_exactly_the_requested_sample_count() {
        let mut backend = open_mock(1000.0);
        for expected in [1_u64, 7, 4095, 4096, 4097, 10_000] {
            let (samples, outcome) = collect(&mut backend, expected);
            assert_eq!(samples.len() as u64, expected);
            assert_eq!(outcome, StreamOutcome::Completed);
        }
    }

    #[test]
    fn multi_component_superposition_matches_the_analytic_sum() {
        let signal = MockSignalSpec {
            components: vec![
                SignalComponent {
                    frequency_hz: 10.0,
                    amplitude: 2.0,
                    phase_deg: 0.0,
                },
                SignalComponent {
                    frequency_hz: 25.0,
                    amplitude: 0.5,
                    phase_deg: 90.0,
                },
            ],
            noise_std_dev: 0.0,
            seed: 1,
        };
        let mut backend = MockBackend::new(clock()).with_signal(signal.clone());
        backend
            .open(&DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        let (samples, _) = collect(&mut backend, 500);

        for (i, actual) in samples.iter().enumerate() {
            let t = i as f64 / 1000.0;
            let expected: f64 = signal
                .components
                .iter()
                .map(|c| {
                    c.amplitude
                        * (std::f64::consts::TAU * c.frequency_hz * t + c.phase_deg.to_radians())
                            .sin()
                })
                .sum();
            assert!((f64::from(*actual) - expected).abs() < 1e-5, "sample {i}");
        }
    }

    #[test]
    fn noiseless_sine_gives_the_analytic_peak_and_rms() {
        let signal = MockSignalSpec {
            components: vec![SignalComponent {
                frequency_hz: 50.0,
                amplitude: 4.0,
                phase_deg: 0.0,
            }],
            noise_std_dev: 0.0,
            seed: 99,
        };
        let mut backend = MockBackend::new(clock()).with_signal(signal);
        backend
            .open(&DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        let (samples, _) = collect(&mut backend, 20_000);

        let peak = samples
            .iter()
            .fold(0.0_f64, |a, s| a.max(f64::from(*s).abs()));
        let rms = (samples.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
            / samples.len() as f64)
            .sqrt();
        assert!((peak - 4.0).abs() < 1e-3, "peak {peak}");
        assert!((rms - 4.0 / 2.0_f64.sqrt()).abs() < 1e-3, "rms {rms}");
    }

    #[test]
    fn output_is_deterministic_for_a_fixed_seed() {
        let signal = MockSignalSpec {
            components: vec![SignalComponent {
                frequency_hz: 120.0,
                amplitude: 250.0,
                phase_deg: 0.0,
            }],
            noise_std_dev: 2.5,
            seed: 12345,
        };
        let run = || {
            let mut backend = MockBackend::new(clock()).with_signal(signal.clone());
            backend
                .open(&DeviceOpenOptions::new(
                    100_000.0,
                    SampleUnit::VelocityUmPerSec,
                ))
                .unwrap();
            collect(&mut backend, 10_000).0
        };
        let a = run();
        let b = run();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(&b) {
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }

    #[test]
    fn prng_golden_vector() {
        // Pinning the exact Xoshiro256++/SplitMix64 output so a refactor of the generator is
        // caught rather than silently changing every recorded mock capture.
        let mut rng = Xoshiro256PlusPlus::seeded(12345);
        let got: Vec<u64> = (0..4).map(|_| rng.next_u64()).collect();
        assert_eq!(
            got,
            vec![
                10_201_931_350_592_234_856,
                3_780_764_549_115_216_544,
                1_570_246_627_180_645_737,
                3_237_956_550_421_933_520,
            ]
        );
    }

    #[test]
    fn standard_normal_has_the_right_moments() {
        let mut rng = Xoshiro256PlusPlus::seeded(7);
        let n = 200_000;
        let values: Vec<f64> = (0..n).map(|_| rng.next_standard_normal()).collect();
        let mean = values.iter().sum::<f64>() / n as f64;
        let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0);
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((variance - 1.0).abs() < 0.02, "variance {variance}");
    }

    #[test]
    fn uniform_values_stay_in_range() {
        let mut rng = Xoshiro256PlusPlus::seeded(0);
        for _ in 0..10_000 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v), "{v}");
        }
    }

    #[test]
    fn cancellation_stops_promptly() {
        let mut backend = open_mock(1000.0);
        let cancel = CancelToken::new();
        let request = StreamRequest::new(10_000_000, SampleUnit::VelocityUmPerSec, 1000.0);
        let mut delivered = 0usize;
        let outcome = backend
            .stream(
                &request,
                &mut |batch| {
                    delivered += batch.samples.len();
                    cancel.cancel();
                    Ok(())
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(outcome, StreamOutcome::Cancelled);
        assert_eq!(delivered, BATCH_SAMPLES);
    }

    #[test]
    fn stop_from_another_thread_ends_a_stall() {
        let mut backend = open_mock(1000.0);
        backend.set_fault(MockFault::Stall);
        let stopped = Arc::clone(&backend.stopped);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            stopped.store(true, Ordering::Release);
        });
        let request = StreamRequest::new(1000, SampleUnit::VelocityUmPerSec, 1000.0);
        let outcome = backend
            .stream(&request, &mut |_| Ok(()), &CancelToken::new())
            .unwrap();
        assert_eq!(outcome, StreamOutcome::Cancelled);
    }

    #[test]
    fn stall_fault_delivers_nothing_until_cancelled() {
        let mut backend = open_mock(1000.0);
        backend.set_fault(MockFault::Stall);
        let cancel = CancelToken::new();
        let token = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            token.cancel();
        });
        let request = StreamRequest::new(1000, SampleUnit::VelocityUmPerSec, 1000.0);
        let mut delivered = 0usize;
        let outcome = backend
            .stream(
                &request,
                &mut |batch| {
                    delivered += batch.samples.len();
                    Ok(())
                },
                &cancel,
            )
            .unwrap();
        assert_eq!(outcome, StreamOutcome::Cancelled);
        assert_eq!(delivered, 0);
    }

    #[test]
    fn link_lost_fault_fails_hard_after_its_limit() {
        let mut backend = open_mock(1000.0);
        backend.set_fault(MockFault::LinkLostAfter(100));
        let request = StreamRequest::new(5000, SampleUnit::VelocityUmPerSec, 1000.0);
        let mut samples = Vec::new();
        let err = backend
            .stream(
                &request,
                &mut |batch| {
                    samples.extend_from_slice(batch.samples);
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap_err();
        assert_eq!(samples.len(), 100);
        assert_eq!(err.scpi_error(), quickvib_core::ScpiError::HardwareError);
    }

    #[test]
    fn short_stream_fault_stops_early() {
        let mut backend = open_mock(1000.0);
        backend.set_fault(MockFault::ShortStream(250));
        let (samples, outcome) = collect(&mut backend, 5000);
        assert_eq!(samples.len(), 250);
        assert_eq!(outcome, StreamOutcome::LinkLost);
    }

    #[test]
    fn a_fault_limit_beyond_the_request_never_fires() {
        // Both faults are defined as "deliver this many samples, then misbehave". A run that
        // asks for fewer than the limit is satisfied in full, so it completed.
        let mut backend = open_mock(1000.0);
        for fault in [MockFault::ShortStream(5000), MockFault::LinkLostAfter(5000)] {
            backend.set_fault(fault);
            let (samples, outcome) = collect(&mut backend, 250);
            assert_eq!(samples.len(), 250, "{fault:?}");
            assert_eq!(outcome, StreamOutcome::Completed, "{fault:?}");
        }
    }

    #[test]
    fn a_callback_error_aborts_the_stream() {
        let mut backend = open_mock(1000.0);
        let request = StreamRequest::new(50_000, SampleUnit::VelocityUmPerSec, 1000.0);
        let result = backend.stream(
            &request,
            &mut |_| Err(DeviceError::unsupported("consumer is full")),
            &CancelToken::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn pacing_uses_the_injected_clock() {
        let clock = Arc::new(TestClock::at_epoch());
        let mut backend = MockBackend::new(Arc::clone(&clock) as Arc<dyn Clock>).with_pacing(true);
        backend
            .open(&DeviceOpenOptions::new(
                1000.0,
                SampleUnit::VelocityUmPerSec,
            ))
            .unwrap();
        let request = StreamRequest::new(5000, SampleUnit::VelocityUmPerSec, 1000.0);
        backend
            .stream(&request, &mut |_| Ok(()), &CancelToken::new())
            .unwrap();
        // 5000 samples at 1 kS/s is nominally five seconds of virtual time and no real time.
        assert_eq!(clock.monotonic(), Duration::from_secs(5));
    }

    #[test]
    fn close_marks_the_backend_disconnected() {
        let mut backend = open_mock(1000.0);
        backend.close().unwrap();
        assert!(!backend.is_connected());
    }
}
