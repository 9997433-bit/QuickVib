//! The `0x04` data-upload payload, and the tone a simulated device puts in it.
//!
//! ```text
//! 0            4              8                    8 + 4·N
//! +------------+--------------+------- ... --------+
//! | data_type  | point_count  |  N × f32 samples   |
//! +------------+--------------+------- ... --------+
//! ```
//!
//! **The two prefix words and the samples do not share a byte order.** The prefix follows the
//! header, which the specification packs big-endian; the samples are little-endian `f32`,
//! because that is what the SDK hands its `M300DataCallback` (`docs/M300-NATIVE.md` §6) and the
//! SDK is passing the payload bytes straight through. The asymmetry is the specification's, not
//! ours — but it is also the one thing here nothing in the vendor material states outright, so
//! [`Endianness`] makes the prefix switchable and `m300-device-sim --prefix-endian` exposes it
//! for a bench run to settle. See `docs/SCZN-PROTOCOL.md` §4.

use crate::params::DataType;

/// Bytes per sample on the wire.
pub const BYTES_PER_SAMPLE: usize = 4;

/// Bytes of prefix before the samples: `data_type` and `point_count`.
pub const PREFIX_LEN: usize = 8;

/// Byte order for the upload payload's two prefix words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Endianness {
    /// Big-endian, matching the frame header. The default.
    #[default]
    Big,
    /// Little-endian, matching the samples and the sibling UDP protocol.
    Little,
}

impl Endianness {
    /// Encode a word.
    #[must_use]
    pub const fn encode(self, value: u32) -> [u8; 4] {
        match self {
            Self::Big => value.to_be_bytes(),
            Self::Little => value.to_le_bytes(),
        }
    }

    /// Decode a word.
    #[must_use]
    pub const fn decode(self, bytes: [u8; 4]) -> u32 {
        match self {
            Self::Big => u32::from_be_bytes(bytes),
            Self::Little => u32::from_le_bytes(bytes),
        }
    }

    /// The command-line spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Big => "big",
            Self::Little => "little",
        }
    }

    /// Parse a command-line spelling.
    #[must_use]
    pub fn from_str_opt(text: &str) -> Option<Self> {
        match text {
            "big" | "be" => Some(Self::Big),
            "little" | "le" => Some(Self::Little),
            _ => None,
        }
    }
}

impl std::fmt::Display for Endianness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One decoded `0x04` payload.
#[derive(Debug, Clone, PartialEq)]
pub struct DataUpload {
    /// The `data_type` word, widened from the one-byte parameter code.
    pub data_type: u32,
    /// The samples, already decoded from little-endian `f32`.
    pub samples: Vec<f32>,
}

impl DataUpload {
    /// Encode the payload for `samples` of `kind`.
    #[must_use]
    pub fn encode(kind: DataType, samples: &[f32], prefix: Endianness) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PREFIX_LEN + samples.len() * BYTES_PER_SAMPLE);
        encode_into(kind, samples, prefix, &mut bytes);
        bytes
    }

    /// Decode a payload.
    ///
    /// # Errors
    /// [`UploadError`] when the payload is short, when `point_count` disagrees with the byte
    /// count, or when the trailing bytes are not a whole number of samples. The SDK
    /// cross-checks `data_len == point_count * 4` before it reads the buffer
    /// (`docs/M300-NATIVE.md` §7); a stand-in that emitted payloads failing that check would
    /// look fine here and abort a real run, so the same check is enforced on the way in.
    pub fn decode(payload: &[u8], prefix: Endianness) -> Result<Self, UploadError> {
        if payload.len() < PREFIX_LEN {
            return Err(UploadError::Short { len: payload.len() });
        }
        let data_type = prefix.decode([payload[0], payload[1], payload[2], payload[3]]);
        let declared = prefix.decode([payload[4], payload[5], payload[6], payload[7]]);
        let body = &payload[PREFIX_LEN..];
        if body.len() % BYTES_PER_SAMPLE != 0 {
            return Err(UploadError::Ragged { bytes: body.len() });
        }
        let actual = body.len() / BYTES_PER_SAMPLE;
        if u64::from(declared) != actual as u64 {
            return Err(UploadError::CountMismatch { declared, actual });
        }
        let samples = body
            .chunks_exact(BYTES_PER_SAMPLE)
            .map(|word| f32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect();
        Ok(Self { data_type, samples })
    }
}

/// As [`DataUpload::encode`], but appending to a buffer the caller reuses across blocks.
pub fn encode_into(kind: DataType, samples: &[f32], prefix: Endianness, out: &mut Vec<u8>) {
    out.clear();
    out.reserve(PREFIX_LEN + samples.len() * BYTES_PER_SAMPLE);
    out.extend_from_slice(&prefix.encode(u32::from(kind.code())));
    out.extend_from_slice(&prefix.encode(samples.len() as u32));
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
}

/// Why an upload payload could not be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadError {
    /// Fewer than [`PREFIX_LEN`] bytes: not even the prefix is there.
    Short {
        /// How many bytes arrived.
        len: usize,
    },
    /// The sample region is not a whole number of four-byte samples.
    Ragged {
        /// How many bytes followed the prefix.
        bytes: usize,
    },
    /// `point_count` and the byte count disagree — the check the SDK makes before reading.
    CountMismatch {
        /// What the prefix claimed.
        declared: u32,
        /// What the payload actually holds.
        actual: usize,
    },
}

impl std::fmt::Display for UploadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Short { len } => {
                write!(
                    f,
                    "upload payload of {len} bytes is shorter than its prefix"
                )
            }
            Self::Ragged { bytes } => write!(f, "{bytes} sample bytes is not a multiple of 4"),
            Self::CountMismatch { declared, actual } => {
                write!(f, "point_count says {declared}, payload holds {actual}")
            }
        }
    }
}

impl std::error::Error for UploadError {}

/// A pure sine indexed by absolute sample number, so phase is continuous however the stream is
/// chunked.
///
/// Deliberately the same generator, with the same defaults, as `m300-sim`'s: the two simulators
/// speak different wire protocols but should be indistinguishable as *signal sources*, so a
/// measurement taken through one can be compared against the same measurement taken through the
/// other. A test in this crate asserts the two produce bit-identical samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waveform {
    /// Samples per second.
    pub sample_rate_hz: f64,
    /// Peak amplitude, in the unit the configured data type implies.
    pub amplitude: f64,
    /// Tone frequency in hertz. Zero produces a flat line.
    pub frequency_hz: f64,
}

impl Waveform {
    /// A waveform with these parameters.
    #[must_use]
    pub const fn new(sample_rate_hz: f64, amplitude: f64, frequency_hz: f64) -> Self {
        Self {
            sample_rate_hz,
            amplitude,
            frequency_hz,
        }
    }

    /// The sample at absolute index `index`.
    #[must_use]
    pub fn value_at(&self, index: u64) -> f32 {
        let rate = if self.sample_rate_hz > 0.0 {
            self.sample_rate_hz
        } else {
            1.0
        };
        let t = index as f64 / rate;
        (self.amplitude * (std::f64::consts::TAU * self.frequency_hz * t).sin()) as f32
    }

    /// `count` samples starting at `start`.
    #[must_use]
    pub fn samples(&self, start: u64, count: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(count);
        self.samples_into(start, count, &mut out);
        out
    }

    /// As [`Waveform::samples`], but into a buffer the caller reuses.
    pub fn samples_into(&self, start: u64, count: usize, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(count);
        for offset in 0..count as u64 {
            out.push(self.value_at(start + offset));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_prefix_is_big_endian_and_the_samples_are_little_endian() {
        let bytes = DataUpload::encode(DataType::Acceleration, &[1.0], Endianness::Big);
        assert_eq!(&bytes[0..4], &[0, 0, 0, 0x02], "data_type, big-endian");
        assert_eq!(&bytes[4..8], &[0, 0, 0, 0x01], "point_count, big-endian");
        assert_eq!(
            &bytes[8..12],
            &1.0f32.to_le_bytes(),
            "sample, little-endian"
        );
    }

    #[test]
    fn the_prefix_order_is_switchable_and_the_sample_order_is_not() {
        let bytes = DataUpload::encode(DataType::Acceleration, &[1.0], Endianness::Little);
        assert_eq!(&bytes[0..4], &[0x02, 0, 0, 0]);
        assert_eq!(&bytes[4..8], &[0x01, 0, 0, 0]);
        assert_eq!(&bytes[8..12], &1.0f32.to_le_bytes(), "still little-endian");
    }

    #[test]
    fn an_upload_round_trips_under_either_prefix_order() {
        let samples: Vec<f32> = (0..64).map(|i| i as f32 * 0.25 - 8.0).collect();
        for prefix in [Endianness::Big, Endianness::Little] {
            for kind in DataType::ALL {
                let payload = DataUpload::encode(kind, &samples, prefix);
                assert_eq!(payload.len(), PREFIX_LEN + samples.len() * 4);
                let decoded = DataUpload::decode(&payload, prefix).unwrap();
                assert_eq!(decoded.data_type, u32::from(kind.code()));
                assert_eq!(decoded.samples, samples);
            }
        }
    }

    #[test]
    fn an_empty_block_is_a_prefix_and_nothing_else() {
        let payload = DataUpload::encode(DataType::Velocity, &[], Endianness::Big);
        assert_eq!(payload.len(), PREFIX_LEN);
        let decoded = DataUpload::decode(&payload, Endianness::Big).unwrap();
        assert!(decoded.samples.is_empty());
    }

    #[test]
    fn the_count_the_sdk_cross_checks_is_enforced_on_the_way_in() {
        let mut payload = DataUpload::encode(DataType::Velocity, &[1.0, 2.0], Endianness::Big);
        payload[4..8].copy_from_slice(&99u32.to_be_bytes());
        assert_eq!(
            DataUpload::decode(&payload, Endianness::Big),
            Err(UploadError::CountMismatch {
                declared: 99,
                actual: 2
            })
        );
    }

    #[test]
    fn a_truncated_or_ragged_payload_is_refused() {
        let payload = DataUpload::encode(DataType::Velocity, &[1.0, 2.0], Endianness::Big);
        assert_eq!(
            DataUpload::decode(&payload[..7], Endianness::Big),
            Err(UploadError::Short { len: 7 })
        );
        assert_eq!(
            DataUpload::decode(&payload[..payload.len() - 1], Endianness::Big),
            Err(UploadError::Ragged { bytes: 7 })
        );
    }

    #[test]
    fn reading_a_payload_with_the_wrong_prefix_order_is_caught_not_silently_wrong() {
        // 2 points read the other way round is 33554432, which cannot match the byte count.
        let payload = DataUpload::encode(DataType::Velocity, &[1.0, 2.0], Endianness::Big);
        assert!(matches!(
            DataUpload::decode(&payload, Endianness::Little),
            Err(UploadError::CountMismatch { .. })
        ));
    }

    #[test]
    fn errors_say_what_went_wrong() {
        assert!(UploadError::Short { len: 3 }
            .to_string()
            .contains("3 bytes"));
        assert!(UploadError::Ragged { bytes: 7 }
            .to_string()
            .contains("multiple of 4"));
        assert!(UploadError::CountMismatch {
            declared: 9,
            actual: 2
        }
        .to_string()
        .contains("point_count says 9"));
    }

    #[test]
    fn the_waveform_is_a_sine_of_the_requested_amplitude() {
        // A quarter period at 1 Hz / 4 S/s lands exactly on the peak.
        let wave = Waveform::new(4.0, 3.0, 1.0);
        assert!(wave.value_at(0).abs() < 1e-5);
        assert!((wave.value_at(1) - 3.0).abs() < 1e-5);
        assert!((wave.value_at(3) + 3.0).abs() < 1e-5);
    }

    #[test]
    fn phase_is_continuous_across_block_boundaries() {
        let wave = Waveform::new(1000.0, 1.0, 50.0);
        let whole = wave.samples(0, 40);
        let mut split = wave.samples(0, 17);
        split.extend(wave.samples(17, 23));
        assert_eq!(whole, split);
    }

    #[test]
    fn a_zero_rate_waveform_does_not_divide_by_zero() {
        assert!(Waveform::new(0.0, 1.0, 1.0).value_at(7).is_finite());
    }

    /// The two simulators must be interchangeable as signal sources; only their wire formats
    /// differ. If this ever fails, one of the two generators has drifted.
    #[test]
    fn the_tone_is_bit_identical_to_the_one_m300_sim_transmits() {
        let ours = Waveform::new(100_000.0, 250.0, 120.0);
        let theirs = quickvib_sim::Waveform::new(100_000.0, 250.0, 120.0);
        let mine: Vec<u8> = ours
            .samples(0, 500)
            .iter()
            .flat_map(|sample| sample.to_le_bytes())
            .collect();
        assert_eq!(mine, theirs.encode(0, 500));
    }
}
