//! Response formatting.
//!
//! Query responses within one compound message are joined with `;` on a single line
//! (`docs/PLAN.md` 7.2). Sample data is written straight through the caller's writer rather
//! than materialized as one multi-megabyte `String` (7.9).

use std::io::Write;
use std::sync::Arc;

/// Out-of-band notification pushed when a recording completes (D17).
pub const NOTIFY_RECORD_DONE: &str = "#REC:DONE";

/// Out-of-band notification pushed when a recording aborts or times out.
pub const NOTIFY_RECORD_ABORTED: &str = "#REC:ABORT";

/// Line terminator for every response.
pub const RESPONSE_TERMINATOR: u8 = b'\n';

/// What one dispatched command produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    /// A command with no response.
    None,
    /// A single-line textual response.
    Text(String),
    /// The capture buffer, formatted as comma-separated ASCII.
    ///
    /// Held behind an [`Arc`] so the engine lock can be released before the — potentially
    /// multi-megabyte — write begins.
    Samples(Arc<Vec<f32>>),
}

impl Response {
    /// A textual response from anything displayable.
    #[must_use]
    pub fn text(value: impl std::fmt::Display) -> Self {
        Self::Text(value.to_string())
    }

    /// Whether this response contributes a field to the response line.
    #[must_use]
    pub const fn is_some(&self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Write the responses for one input line, joined with `;` and terminated with `\n`.
///
/// Writes nothing at all when no part produced a response, which is the correct behaviour for
/// a line of pure commands.
///
/// # Errors
/// Propagates any error from `writer`.
pub fn write_response_line<W: Write>(
    writer: &mut W,
    responses: &[Response],
) -> std::io::Result<()> {
    if !responses.iter().any(Response::is_some) {
        return Ok(());
    }

    let mut first = true;
    for response in responses.iter().filter(|r| r.is_some()) {
        if !first {
            writer.write_all(b";")?;
        }
        first = false;
        match response {
            Response::None => {}
            Response::Text(text) => writer.write_all(text.as_bytes())?,
            Response::Samples(samples) => write_samples(writer, samples)?,
        }
    }
    writer.write_all(&[RESPONSE_TERMINATOR])
}

/// Write samples as comma-separated ASCII, using the shortest representation that round-trips
/// back to the same `f32`.
///
/// # Errors
/// Propagates any error from `writer`.
pub fn write_samples<W: Write>(writer: &mut W, samples: &[f32]) -> std::io::Result<()> {
    let mut first = true;
    // One reusable scratch string keeps this allocation-free per sample.
    let mut scratch = String::with_capacity(24);
    for value in samples {
        if !first {
            writer.write_all(b",")?;
        }
        first = false;
        scratch.clear();
        use std::fmt::Write as _;
        let _ = write!(scratch, "{value}");
        writer.write_all(scratch.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn render(responses: &[Response]) -> String {
        let mut out = Vec::new();
        write_response_line(&mut out, responses).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn a_single_response_is_newline_terminated() {
        assert_eq!(
            render(&[Response::text("QuickVib,M300-SCPI,0,1.0.0")]),
            "QuickVib,M300-SCPI,0,1.0.0\n"
        );
    }

    #[test]
    fn commands_with_no_response_write_nothing() {
        assert_eq!(render(&[Response::None, Response::None]), "");
    }

    #[test]
    fn compound_responses_are_joined_with_semicolons() {
        let responses = [
            Response::None,
            Response::text("1"),
            Response::None,
            Response::text("CSV"),
        ];
        assert_eq!(render(&responses), "1;CSV\n");
    }

    #[test]
    fn samples_are_comma_separated() {
        let samples = Arc::new(vec![1.0_f32, -2.5, 0.125]);
        assert_eq!(render(&[Response::Samples(samples)]), "1,-2.5,0.125\n");
    }

    #[test]
    fn sample_formatting_round_trips_bit_exactly() {
        let samples: Vec<f32> = (0..5000).map(|i| (i as f32).sin() * 1e-7).collect();
        let mut out = Vec::new();
        write_samples(&mut out, &samples).unwrap();
        let text = String::from_utf8(out).unwrap();
        let parsed: Vec<f32> = text.split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(parsed.len(), samples.len());
        for (a, b) in parsed.iter().zip(&samples) {
            assert_eq!(a.to_bits(), b.to_bits());
        }
    }

    #[test]
    fn an_empty_capture_writes_an_empty_line() {
        assert_eq!(render(&[Response::Samples(Arc::new(Vec::new()))]), "\n");
    }

    #[test]
    fn notification_prefixes_are_out_of_band() {
        assert!(NOTIFY_RECORD_DONE.starts_with('#'));
        assert!(NOTIFY_RECORD_ABORTED.starts_with('#'));
    }
}
