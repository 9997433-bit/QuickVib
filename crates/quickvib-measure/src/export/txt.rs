//! TXT export: one value per line, no header. The minimal form for scripts that just want
//! numbers.

use std::io::Write;

use super::LINE_ENDING;

/// Write one sample per line.
///
/// # Errors
/// Propagates any error from `writer`.
pub fn write_txt<W: Write>(writer: &mut W, samples: &[f32]) -> std::io::Result<()> {
    for value in samples {
        write!(writer, "{value}{LINE_ENDING}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn render(samples: &[f32]) -> String {
        let mut out = Vec::new();
        write_txt(&mut out, samples).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn one_value_per_line_with_no_header() {
        assert_eq!(render(&[1.0, -2.25, 0.5]), "1\r\n-2.25\r\n0.5\r\n");
    }

    #[test]
    fn empty_capture_writes_nothing() {
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn values_round_trip_bit_exactly() {
        let samples: Vec<f32> = (0..500).map(|i| 1.0 / (i as f32 + 0.5)).collect();
        let parsed: Vec<f32> = render(&samples)
            .split(LINE_ENDING)
            .filter(|l| !l.is_empty())
            .map(|l| l.parse::<f32>().unwrap())
            .collect();
        for (a, b) in parsed.iter().zip(&samples) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }
}
