//! CSV export: optional metadata preamble, optional header row, one row per sample.

use std::io::Write;

use super::{CaptureMetadata, LINE_ENDING};

/// The fixed header row.
pub const HEADER_ROW: &str = "index,time_s,value";

/// Write the capture as CSV.
///
/// With `metadata.include_header` the output is a `#`-prefixed preamble, the header row, then
/// `index,time_s,value` rows. Without it, only the rows are written.
///
/// # Errors
/// Propagates any error from `writer`.
pub fn write_csv<W: Write>(
    writer: &mut W,
    metadata: &CaptureMetadata,
    samples: &[f32],
) -> std::io::Result<()> {
    if metadata.include_header {
        write!(writer, "# project={}{LINE_ENDING}", metadata.project_name)?;
        write!(writer, "# timestamp={}{LINE_ENDING}", metadata.timestamp)?;
        write!(
            writer,
            "# sampleRateHz={}{LINE_ENDING}",
            metadata.sample_rate_hz
        )?;
        write!(writer, "# unit={}{LINE_ENDING}", metadata.unit.label())?;
        write!(writer, "# samples={}{LINE_ENDING}", samples.len())?;
        write!(
            writer,
            "# durationSeconds={}{LINE_ENDING}",
            metadata.duration_seconds
        )?;
        write!(writer, "{HEADER_ROW}{LINE_ENDING}")?;
    }

    let rate = metadata.sample_rate_hz;
    for (index, value) in samples.iter().enumerate() {
        let time_s = if rate > 0.0 { index as f64 / rate } else { 0.0 };
        write!(writer, "{index},{time_s},{value}{LINE_ENDING}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::SampleUnit;

    fn metadata(include_header: bool) -> CaptureMetadata {
        CaptureMetadata {
            project_name: "Test".to_owned(),
            timestamp: "2026-01-01T12:00:00.000Z".to_owned(),
            sample_rate_hz: 4.0,
            unit: SampleUnit::AccelerationMPerSec2,
            duration_seconds: 1.0,
            include_header,
        }
    }

    fn render(include_header: bool, samples: &[f32]) -> String {
        let mut out = Vec::new();
        write_csv(&mut out, &metadata(include_header), samples).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn preamble_and_header_are_written() {
        let text = render(true, &[0.5, -0.5]);
        let lines: Vec<&str> = text.split(LINE_ENDING).collect();
        assert_eq!(lines[0], "# project=Test");
        assert_eq!(lines[1], "# timestamp=2026-01-01T12:00:00.000Z");
        assert_eq!(lines[2], "# sampleRateHz=4");
        assert_eq!(lines[3], "# unit=m/s^2");
        assert_eq!(lines[4], "# samples=2");
        assert_eq!(lines[5], "# durationSeconds=1");
        assert_eq!(lines[6], HEADER_ROW);
        assert_eq!(lines[7], "0,0,0.5");
        assert_eq!(lines[8], "1,0.25,-0.5");
        assert_eq!(lines[9], "");
    }

    #[test]
    fn include_header_false_suppresses_everything_but_rows() {
        let text = render(false, &[1.0]);
        assert_eq!(text, "0,0,1\r\n");
    }

    #[test]
    fn line_endings_are_crlf() {
        let text = render(false, &[1.0, 2.0]);
        assert_eq!(text.matches("\r\n").count(), 2);
        assert!(!text.contains("\n\n"));
    }

    #[test]
    fn values_round_trip_bit_exactly() {
        let samples: Vec<f32> = (0..2000)
            .map(|i| (i as f32) * 0.1234567 - 61.7)
            .chain([f32::MIN_POSITIVE, -0.0, 1.0e-30, 3.402_823_5e38])
            .collect();
        let text = render(false, &samples);
        let parsed: Vec<f32> = text
            .split(LINE_ENDING)
            .filter(|l| !l.is_empty())
            .map(|l| l.rsplit(',').next().unwrap().parse::<f32>().unwrap())
            .collect();
        assert_eq!(parsed.len(), samples.len());
        for (a, b) in parsed.iter().zip(&samples) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    #[test]
    fn row_count_matches_sample_count() {
        let samples = vec![1.0_f32; 1000];
        let text = render(true, &samples);
        let rows = text.split(LINE_ENDING).filter(|l| !l.is_empty()).count();
        assert_eq!(rows, 6 + 1 + 1000);
    }
}
