//! What the data callback is allowed to believe about the buffer it was handed
//! (`docs/M300-NATIVE.md` §7).
//!
//! The SDK hands the shim a raw pointer, a point count and a byte length, and the shim has one
//! chance to decide whether reading that memory is sound: `data` dies when the callback returns,
//! there is no error channel back to the SDK, and a wrong length is a buffer overrun rather than
//! an exception. So the two numbers are cross-checked against each other before the pointer is
//! touched at all, and the unit is cross-checked against the one the run asked for.
//!
//! Splitting those checks out here is what lets them be tested: [`check_batch`] and
//! [`decode_le_f32`] are ordinary functions over ordinary values, exercised by Linux CI, and the
//! `unsafe` in the shim reduces to "build a slice of the length this module already agreed to".

/// Bytes per sample on the wire: contiguous IEEE-754 little-endian `f32`, single channel, no
/// per-packet header at this level (`docs/M300-NATIVE.md` §6).
pub const BYTES_PER_SAMPLE: u32 = 4;

/// Sentinel for [`check_batch`]'s `expected_data_type` meaning "no run is in flight, so any unit
/// is acceptable". No real `data_type` can collide with it: the SDK documents `0`, `1` and `2`.
pub const ANY_DATA_TYPE: u32 = u32::MAX;

/// A batch that cannot be trusted, and therefore aborts the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchFault {
    /// `data_len` and `point_count` disagree. One of the two numbers is wrong and there is no
    /// way to tell which, so neither is used.
    LengthMismatch {
        /// Samples the SDK said the batch holds.
        point_count: u32,
        /// Bytes the SDK said the batch holds.
        data_len: u32,
    },
    /// The device is sending a different quantity than the run asked for, which means it was
    /// reconfigured behind QuickVib's back. Relabelling the samples would produce a plausible
    /// measurement in the wrong unit.
    UnexpectedDataType {
        /// The `m300_set_data_type` value the run configured.
        expected: u32,
        /// The value the callback reported.
        got: u32,
    },
    /// A non-empty batch arrived with a null pointer.
    NullData {
        /// Bytes the SDK said the batch holds.
        data_len: u32,
    },
}

impl std::fmt::Display for BatchFault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LengthMismatch {
                point_count,
                data_len,
            } => write!(
                f,
                "M300 data callback: data_len={data_len} is not point_count={point_count} x {BYTES_PER_SAMPLE}"
            ),
            Self::UnexpectedDataType { expected, got } => write!(
                f,
                "M300 data callback: device is sending data_type={got}, the run configured {expected}"
            ),
            Self::NullData { data_len } => {
                write!(f, "M300 data callback: null data pointer with data_len={data_len}")
            }
        }
    }
}

impl std::error::Error for BatchFault {}

/// Validate one callback's arguments and return how many bytes may be read.
///
/// # Errors
/// [`BatchFault`] when the two length fields disagree, when the pointer is null for a non-empty
/// batch, or when the device changed unit mid-run.
pub fn check_batch(
    expected_data_type: u32,
    data_type: u32,
    point_count: u32,
    data_len: u32,
    data_is_null: bool,
) -> Result<usize, BatchFault> {
    if expected_data_type != ANY_DATA_TYPE && data_type != expected_data_type {
        return Err(BatchFault::UnexpectedDataType {
            expected: expected_data_type,
            got: data_type,
        });
    }
    if point_count.checked_mul(BYTES_PER_SAMPLE) != Some(data_len) {
        return Err(BatchFault::LengthMismatch {
            point_count,
            data_len,
        });
    }
    if data_len == 0 {
        return Ok(0);
    }
    if data_is_null {
        return Err(BatchFault::NullData { data_len });
    }
    Ok(data_len as usize)
}

/// Append `bytes` to `out` as little-endian `f32`.
///
/// Trailing bytes that do not fill a whole sample are dropped; [`check_batch`] has already
/// refused any batch whose length is not a multiple of [`BYTES_PER_SAMPLE`], so there are none.
pub fn decode_le_f32(bytes: &[u8], out: &mut Vec<f32>) {
    out.reserve(bytes.len() / BYTES_PER_SAMPLE as usize);
    out.extend(
        bytes
            .chunks_exact(BYTES_PER_SAMPLE as usize)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])),
    );
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_consistent_batch_yields_its_byte_length() {
        assert_eq!(check_batch(0, 0, 1024, 4096, false), Ok(4096));
    }

    #[test]
    fn a_length_mismatch_is_refused_rather_than_trusting_either_number() {
        let fault = check_batch(0, 0, 1024, 4095, false).unwrap_err();
        assert_eq!(
            fault,
            BatchFault::LengthMismatch {
                point_count: 1024,
                data_len: 4095
            }
        );
        assert!(check_batch(0, 0, 1024, 8192, false).is_err());
        assert!(check_batch(0, 0, 0, 4, false).is_err());
    }

    #[test]
    fn a_point_count_that_would_overflow_the_byte_length_is_refused() {
        // `point_count * 4` wraps in 32 bits; the check must not wrap with it.
        assert!(check_batch(0, 0, u32::MAX, 4, false).is_err());
        assert!(check_batch(0, 0, 0x4000_0000, 0, false).is_err());
    }

    #[test]
    fn an_empty_batch_is_allowed_and_reads_nothing() {
        assert_eq!(check_batch(0, 0, 0, 0, false), Ok(0));
        // A null pointer is only a fault when there was supposed to be something behind it.
        assert_eq!(check_batch(0, 0, 0, 0, true), Ok(0));
    }

    #[test]
    fn a_null_pointer_with_samples_behind_it_is_refused() {
        let fault = check_batch(0, 0, 2, 8, true).unwrap_err();
        assert_eq!(fault, BatchFault::NullData { data_len: 8 });
    }

    #[test]
    fn a_unit_change_mid_run_aborts_rather_than_relabelling() {
        let fault = check_batch(0, 2, 4, 16, false).unwrap_err();
        assert_eq!(
            fault,
            BatchFault::UnexpectedDataType {
                expected: 0,
                got: 2
            }
        );
        assert!(fault.to_string().contains("data_type=2"), "{fault}");
    }

    #[test]
    fn between_runs_any_unit_is_accepted() {
        assert_eq!(check_batch(ANY_DATA_TYPE, 2, 4, 16, false), Ok(16));
    }

    #[test]
    fn samples_decode_as_little_endian_f32() {
        let mut out = Vec::new();
        let bytes: Vec<u8> = [1.0f32, -2.5, 1e-9]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        decode_le_f32(&bytes, &mut out);
        assert_eq!(out, vec![1.0f32, -2.5, 1e-9]);
    }

    #[test]
    fn decoding_appends_rather_than_replacing() {
        let mut out = vec![7.0f32];
        decode_le_f32(&3.0f32.to_le_bytes(), &mut out);
        assert_eq!(out, vec![7.0, 3.0]);
    }

    #[test]
    fn decoding_is_byte_order_explicit_not_host_order() {
        let mut out = Vec::new();
        decode_le_f32(&[0x00, 0x00, 0x80, 0x3f], &mut out);
        assert_eq!(out, vec![1.0f32]);
    }

    #[test]
    fn a_ragged_tail_is_dropped_rather_than_read_past() {
        let mut out = Vec::new();
        decode_le_f32(&[0, 0, 0x80, 0x3f, 0xff], &mut out);
        assert_eq!(out, vec![1.0f32]);
    }

    #[test]
    fn nothing_decodes_to_nothing() {
        let mut out = Vec::new();
        decode_le_f32(&[], &mut out);
        assert!(out.is_empty());
    }
}
