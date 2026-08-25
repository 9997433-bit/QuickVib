//! Peak, RMS and peak-to-peak over a capture buffer (`docs/PLAN.md` 7.7).
//!
//! One pass, accumulating in `f64` even though the samples are `f32` (D15), so a
//! multi-hundred-thousand-sample capture does not lose precision in the sum of squares.

/// A capture buffer that is statically known to hold at least one sample.
///
/// A completed run always has `expected_samples > 0`, so the empty case is made
/// unrepresentable rather than left untested (`docs/PLAN.md` 7.7).
#[derive(Debug, Clone, Copy)]
pub struct NonEmptyCapture<'a> {
    samples: &'a [f32],
}

impl<'a> NonEmptyCapture<'a> {
    /// Wrap a slice, returning `None` when it is empty.
    #[must_use]
    pub const fn new(samples: &'a [f32]) -> Option<Self> {
        if samples.is_empty() {
            None
        } else {
            Some(Self { samples })
        }
    }

    /// The underlying samples.
    #[must_use]
    pub const fn as_slice(self) -> &'a [f32] {
        self.samples
    }

    /// Number of samples. Always at least one.
    #[must_use]
    pub const fn len(self) -> usize {
        self.samples.len()
    }

    /// Always `false`; present so clippy's `len_without_is_empty` stays satisfied and callers
    /// can use the type interchangeably with a slice.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        false
    }
}

/// Options controlling how the statistics are derived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MeasureOptions {
    /// Subtract the arithmetic mean before computing peak and RMS.
    ///
    /// Peak-to-peak is unaffected by definition, so it is computed from the raw samples either
    /// way.
    pub remove_dc: bool,
}

/// The three scalar measurements QuickVib reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasurementSet {
    /// `max(abs(x))`, after optional DC removal.
    pub peak: f64,
    /// `sqrt(mean(x^2))`, after optional DC removal.
    pub rms: f64,
    /// `max(x) - min(x)`, always on the raw samples.
    pub peak_to_peak: f64,
    /// Arithmetic mean of the raw samples. Reported for logging, not over SCPI.
    pub mean: f64,
    /// Number of samples the measurements were derived from.
    pub sample_count: usize,
    /// Whether any sample was `NaN` or infinite. Logged once per run as a warning.
    pub has_non_finite: bool,
}

impl MeasurementSet {
    /// `CALC:MEAS:ALL?` wire form: peak, RMS and peak-to-peak, comma separated.
    #[must_use]
    pub fn all_wire_form(&self, decimals: u8) -> String {
        format!(
            "{},{},{}",
            format_fixed(self.peak, decimals),
            format_fixed(self.rms, decimals),
            format_fixed(self.peak_to_peak, decimals)
        )
    }
}

/// Compute the measurement set for a capture.
///
/// `NaN` samples propagate: if any sample is `NaN`, every derived value is `NaN`. That is a
/// documented choice rather than an accident of which `f32` method was reached for — silently
/// dropping them would report a plausible-looking wrong answer.
#[must_use]
pub fn compute(capture: NonEmptyCapture<'_>, options: MeasureOptions) -> MeasurementSet {
    let samples = capture.as_slice();
    let count = samples.len();

    let mut sum = 0.0_f64;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut saw_nan = false;
    let mut saw_infinite = false;

    for &raw in samples {
        let value = f64::from(raw);
        if value.is_nan() {
            saw_nan = true;
            continue;
        }
        if value.is_infinite() {
            saw_infinite = true;
        }
        sum += value;
        if value < min {
            min = value;
        }
        if value > max {
            max = value;
        }
    }

    let has_non_finite = saw_nan || saw_infinite;

    if saw_nan {
        return MeasurementSet {
            peak: f64::NAN,
            rms: f64::NAN,
            peak_to_peak: f64::NAN,
            mean: f64::NAN,
            sample_count: count,
            has_non_finite,
        };
    }

    let mean = sum / count as f64;
    let offset = if options.remove_dc { mean } else { 0.0 };

    // Second pass for peak and RMS so DC removal uses the exact mean rather than a running
    // estimate. At 500 000 samples this is still well under a millisecond.
    let mut peak = 0.0_f64;
    let mut sum_squares = 0.0_f64;
    for &raw in samples {
        let value = f64::from(raw) - offset;
        let magnitude = value.abs();
        if magnitude > peak {
            peak = magnitude;
        }
        sum_squares += value * value;
    }

    MeasurementSet {
        peak,
        rms: (sum_squares / count as f64).sqrt(),
        peak_to_peak: max - min,
        mean,
        sample_count: count,
        has_non_finite,
    }
}

/// Format a measurement with a fixed number of decimals, locale-independently (D16).
///
/// ```
/// use quickvib_measure::format_fixed;
/// assert_eq!(format_fixed(12.34, 4), "12.3400");
/// assert_eq!(format_fixed(-1.5, 1), "-1.5");
/// ```
#[must_use]
pub fn format_fixed(value: f64, decimals: u8) -> String {
    format!("{value:.*}", usize::from(decimals))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::f64::consts::TAU;

    fn assert_close(a: f64, b: f64, tol: f64) {
        assert!(
            (a - b).abs() <= tol,
            "expected {b}, got {a} (tolerance {tol})"
        );
    }

    fn sine(amplitude: f64, cycles: usize, samples_per_cycle: usize, dc: f64) -> Vec<f32> {
        (0..cycles * samples_per_cycle)
            .map(|i| {
                let phase = TAU * (i % samples_per_cycle) as f64 / samples_per_cycle as f64;
                (dc + amplitude * phase.sin()) as f32
            })
            .collect()
    }

    #[test]
    fn empty_capture_is_unrepresentable() {
        assert!(NonEmptyCapture::new(&[]).is_none());
        assert!(NonEmptyCapture::new(&[0.0]).is_some());
    }

    #[test]
    fn pure_sine_matches_the_analytic_oracle() {
        let samples = sine(3.0, 10, 1024, 0.0);
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert_close(m.peak, 3.0, 1e-3);
        assert_close(m.rms, 3.0 / 2.0_f64.sqrt(), 1e-3);
        assert_close(m.peak_to_peak, 6.0, 1e-3);
        assert_close(m.mean, 0.0, 1e-6);
        assert!(!m.has_non_finite);
        assert_eq!(m.sample_count, 10 * 1024);
    }

    #[test]
    fn dc_offset_without_removal_shifts_peak_and_rms() {
        let samples = sine(1.0, 8, 512, 5.0);
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert_close(m.peak, 6.0, 1e-3);
        assert_close(m.rms, (25.0 + 0.5_f64).sqrt(), 1e-3);
        assert_close(m.peak_to_peak, 2.0, 1e-3);
        assert_close(m.mean, 5.0, 1e-3);
    }

    #[test]
    fn dc_removal_recovers_the_ac_measurements() {
        let samples = sine(1.0, 8, 512, 5.0);
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions { remove_dc: true },
        );
        assert_close(m.peak, 1.0, 1e-3);
        assert_close(m.rms, 1.0 / 2.0_f64.sqrt(), 1e-3);
        // Peak-to-peak is unaffected by DC removal by definition.
        assert_close(m.peak_to_peak, 2.0, 1e-3);
    }

    #[test]
    fn constant_signal() {
        let samples = vec![2.5_f32; 100];
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert_close(m.peak, 2.5, 1e-6);
        assert_close(m.rms, 2.5, 1e-6);
        assert_close(m.peak_to_peak, 0.0, 1e-12);
    }

    #[test]
    fn single_sample_buffer() {
        let samples = [-4.0_f32];
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert_close(m.peak, 4.0, 1e-6);
        assert_close(m.rms, 4.0, 1e-6);
        assert_close(m.peak_to_peak, 0.0, 1e-12);
        assert_eq!(m.sample_count, 1);
    }

    #[test]
    fn f64_accumulation_beats_f32_on_a_long_buffer() {
        // 400 000 samples of 1000.0: an f32 running sum of squares saturates badly here.
        let samples = vec![1000.0_f32; 400_000];
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert_close(m.rms, 1000.0, 1e-6);

        // Compare the mean against a Kahan-summed reference.
        let mut sum = 0.0_f64;
        let mut compensation = 0.0_f64;
        for &s in &samples {
            let y = f64::from(s) - compensation;
            let t = sum + y;
            compensation = (t - sum) - y;
            sum = t;
        }
        assert_close(m.mean, sum / samples.len() as f64, 1e-9);
    }

    #[test]
    fn nan_propagates_to_every_measurement() {
        let samples = [1.0_f32, f32::NAN, -1.0];
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert!(m.peak.is_nan());
        assert!(m.rms.is_nan());
        assert!(m.peak_to_peak.is_nan());
        assert!(m.has_non_finite);
    }

    #[test]
    fn infinity_propagates_and_is_flagged() {
        let samples = [1.0_f32, f32::INFINITY];
        let m = compute(
            NonEmptyCapture::new(&samples).unwrap(),
            MeasureOptions::default(),
        );
        assert!(m.peak.is_infinite());
        assert!(m.peak_to_peak.is_infinite());
        assert!(m.has_non_finite);
    }

    #[test]
    fn fixed_formatting_is_locale_independent_and_padded() {
        assert_eq!(format_fixed(12.34, 4), "12.3400");
        assert_eq!(format_fixed(0.0, 0), "0");
        assert_eq!(format_fixed(1.0 / 3.0, 9), "0.333333333");
        assert!(!format_fixed(1234.5, 2).contains(','));
    }

    #[test]
    fn all_wire_form_is_three_fields() {
        let m = MeasurementSet {
            peak: 12.34,
            rms: 4.56,
            peak_to_peak: 24.68,
            mean: 0.0,
            sample_count: 1,
            has_non_finite: false,
        };
        assert_eq!(m.all_wire_form(4), "12.3400,4.5600,24.6800");
    }
}
