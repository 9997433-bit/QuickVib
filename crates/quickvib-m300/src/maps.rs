//! The vendor's enum indices on one side, QuickVib's physical values on the other.
//!
//! Every configuration knob on the M300 is a `uint8_t` index into a table the vendor publishes
//! in prose, not a quantity in hertz or micrometres per second
//! (`docs/M300-NATIVE.md` §6.1). Reading the sample rate back gives an index too, so
//! `DeviceCapabilities.sample_rate_hz` needs the table in both directions.
//!
//! This module is the whole of that arithmetic and it is deliberately platform-neutral: getting
//! an index wrong means a capture at the wrong rate that still looks plausible, which is exactly
//! the class of mistake worth catching in Linux CI rather than on a test cell. Nothing here
//! calls the SDK or touches a pointer.

use quickvib_core::SampleUnit;

use crate::ffi_generated::M300HardwareInfo;

/// Sample rates the device offers, indexed by the value `m300_set_sample_rate` takes
/// (`docs/M300-NATIVE.md` §6.1).
pub const SAMPLE_RATES_HZ: [f64; 15] = [
    2_000.0,
    5_000.0,
    10_000.0,
    20_000.0,
    50_000.0,
    100_000.0,
    200_000.0,
    400_000.0,
    800_000.0,
    1_000_000.0,
    2_000_000.0,
    4_000_000.0,
    5_000_000.0,
    10_000_000.0,
    20_000_000.0,
];

/// Low-pass filter cutoffs, indexed by the value `m300_set_low_pass_filter` takes
/// (`docs/M300-NATIVE.md` §6.1).
pub const LOW_PASS_HZ: [f64; 15] = [
    100.0,
    500.0,
    1_000.0,
    2_000.0,
    5_000.0,
    10_000.0,
    20_000.0,
    40_000.0,
    80_000.0,
    100_000.0,
    160_000.0,
    320_000.0,
    500_000.0,
    1_000_000.0,
    3_000_000.0,
];

/// `m300_set_data_type` / `m300_get_data_type` value for velocity, in μm/s.
pub const DATA_TYPE_VELOCITY: u8 = 0;
/// `m300_set_data_type` / `m300_get_data_type` value for displacement, in μm.
pub const DATA_TYPE_DISPLACEMENT: u8 = 1;
/// `m300_set_data_type` / `m300_get_data_type` value for acceleration, in m/s².
pub const DATA_TYPE_ACCELERATION: u8 = 2;

/// Relative tolerance for calling a requested value "the same as" a table entry. Project files
/// carry rates as JSON doubles, so `100000.0` and `1e5` must land on the same index.
const MATCH_TOLERANCE: f64 = 1e-9;

/// The rate that will actually be set, and whether it is the one that was asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateChoice {
    /// Index to hand `m300_set_sample_rate`.
    pub index: u8,
    /// The rate that index means, in hertz.
    pub hz: f64,
    /// The rate the project asked for, in hertz.
    pub requested_hz: f64,
}

impl RateChoice {
    /// Whether the device can be set to exactly what the project asked for.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        close(self.hz, self.requested_hz)
    }
}

/// The filter band that will actually be set, and how it was arrived at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterChoice {
    /// Index to hand `m300_set_low_pass_filter`.
    pub index: u8,
    /// The cutoff that index means, in hertz.
    pub hz: f64,
    /// Whether a band matching the request exactly exists.
    pub is_exact: bool,
}

/// The rate `index` selects, or `None` when the device has no such index.
#[must_use]
pub fn sample_rate_hz(index: u8) -> Option<f64> {
    SAMPLE_RATES_HZ.get(usize::from(index)).copied()
}

/// The index for exactly `hz`, or `None` when the device cannot be set to that rate.
#[must_use]
pub fn sample_rate_index(hz: f64) -> Option<u8> {
    SAMPLE_RATES_HZ
        .iter()
        .position(|candidate| close(*candidate, hz))
        .map(|index| index as u8)
}

/// The legal rate closest to `hz`, for a project that names a rate the hardware does not have.
///
/// D13 says a device/project mismatch is logged rather than enforced, but "logged, not enforced"
/// cannot mean "send an index the device will reject": something has to be chosen. Closeness is
/// measured in ratio rather than in hertz, because the table spans four decades and 3 kHz is far
/// nearer to 2 kHz than to 5 kHz even though the absolute gaps are equal. Ties go to the lower
/// rate: over-sampling costs disk, under-filtering costs correctness.
#[must_use]
pub fn nearest_sample_rate(hz: f64) -> RateChoice {
    let lowest = RateChoice {
        index: 0,
        hz: SAMPLE_RATES_HZ[0],
        requested_hz: hz,
    };
    if !hz.is_finite() || hz <= 0.0 {
        return lowest;
    }

    let mut best = lowest;
    let mut best_distance = f64::INFINITY;
    for (index, candidate) in SAMPLE_RATES_HZ.iter().enumerate() {
        let distance = (candidate.ln() - hz.ln()).abs();
        if distance < best_distance {
            best_distance = distance;
            best = RateChoice {
                index: index as u8,
                hz: *candidate,
                requested_hz: hz,
            };
        }
    }
    best
}

/// The cutoff `index` selects, or `None` when the device has no such index.
#[must_use]
pub fn low_pass_hz(index: u8) -> Option<f64> {
    LOW_PASS_HZ.get(usize::from(index)).copied()
}

/// The filter band for a requested cutoff: the exact band when one exists, otherwise the highest
/// band that does not exceed it, and the lowest band when the request is under all of them.
///
/// Rounding down rather than to the nearest band is the conservative direction — a filter set
/// above the requested cutoff passes energy the project asked to reject.
#[must_use]
pub fn nearest_low_pass(hz: f64) -> FilterChoice {
    let lowest = FilterChoice {
        index: 0,
        hz: LOW_PASS_HZ[0],
        is_exact: close(LOW_PASS_HZ[0], hz),
    };
    if !hz.is_finite() || hz <= 0.0 {
        return lowest;
    }

    let mut best = lowest;
    for (index, candidate) in LOW_PASS_HZ.iter().enumerate() {
        if close(*candidate, hz) {
            return FilterChoice {
                index: index as u8,
                hz: *candidate,
                is_exact: true,
            };
        }
        if *candidate < hz {
            best = FilterChoice {
                index: index as u8,
                hz: *candidate,
                is_exact: false,
            };
        }
    }
    best
}

/// The filter band that must accompany sample-rate index `rate_index`.
///
/// The vendor repeats in every document that the low-pass filter has to be set together with the
/// sample rate at a matching band — "10 kHz sampling with the 10 kHz filter" — or "data may be
/// abnormal or the device may refuse" (`docs/M300-NATIVE.md` §6.1). Five of the fifteen rates
/// have no band of the same name (there is no 50 kHz or 200 kHz filter), and for those the
/// highest band below the rate is chosen. **Bench check:** step 13 of the smoke list settles
/// whether the device refuses a mismatched pair outright or merely degrades.
#[must_use]
pub fn low_pass_for_sample_rate(rate_index: u8) -> FilterChoice {
    match sample_rate_hz(rate_index) {
        Some(hz) => nearest_low_pass(hz),
        None => nearest_low_pass(SAMPLE_RATES_HZ[0]),
    }
}

/// The `m300_set_data_type` value for `unit`.
///
/// No conversion is involved: the SDK already delivers μm/s, μm and m/s², which are QuickVib's
/// own units (`docs/M300-NATIVE.md` §6).
#[must_use]
pub const fn data_type_for_unit(unit: SampleUnit) -> u8 {
    match unit {
        SampleUnit::VelocityUmPerSec => DATA_TYPE_VELOCITY,
        SampleUnit::DisplacementUm => DATA_TYPE_DISPLACEMENT,
        SampleUnit::AccelerationMPerSec2 => DATA_TYPE_ACCELERATION,
    }
}

/// The unit a `m300_get_data_type` value means, or `None` for a value the SDK does not document.
#[must_use]
pub const fn unit_for_data_type(value: u8) -> Option<SampleUnit> {
    match value {
        DATA_TYPE_VELOCITY => Some(SampleUnit::VelocityUmPerSec),
        DATA_TYPE_DISPLACEMENT => Some(SampleUnit::DisplacementUm),
        DATA_TYPE_ACCELERATION => Some(SampleUnit::AccelerationMPerSec2),
        _ => None,
    }
}

/// What the vendor documents about one unit's measuring-range ladder.
///
/// Only the two ends are documented. The vendor's PDFs give the index count and the first and
/// last entry of each ladder and nothing in between, so [`range_index`] can honour a project's
/// range only when it names one of the ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RangeSpan {
    /// Index of the smallest range.
    pub min_index: u8,
    /// Index of the largest range.
    pub max_index: u8,
    /// The smallest range, in the unit's own scale.
    pub min_value: f64,
    /// The largest range, in the unit's own scale.
    pub max_value: f64,
}

/// The documented range ladder for `unit` (`docs/M300-NATIVE.md` §6.1).
///
/// The last velocity and displacement entries are the two additions v1.2.0 made, so they are
/// also the first thing that will be missing if a bench turns out to have v1.1.0 installed.
#[must_use]
pub const fn range_span(unit: SampleUnit) -> RangeSpan {
    match unit {
        // ±2.45 μm/s … ±7 m/s.
        SampleUnit::VelocityUmPerSec => RangeSpan {
            min_index: 0x00,
            max_index: 0x10,
            min_value: 2.45,
            max_value: 7_000_000.0,
        },
        // ±0.245 μm … ±0.5 m.
        SampleUnit::DisplacementUm => RangeSpan {
            min_index: 0x00,
            max_index: 0x12,
            min_value: 0.245,
            max_value: 500_000.0,
        },
        // ±1.225 m/s² … ±612500 m/s².
        SampleUnit::AccelerationMPerSec2 => RangeSpan {
            min_index: 0x00,
            max_index: 0x0B,
            min_value: 1.225,
            max_value: 612_500.0,
        },
    }
}

/// The range index for `value`, or `None` when the documented table cannot decide.
///
/// A value at or below the smallest range takes the smallest index and a value at or above the
/// largest takes the largest, because both ends are documented. Everything between them is
/// **not**: the vendor publishes the endpoints and the index count but not the ladder, and a
/// guessed geometric progression would silently capture at the wrong full scale — the exact
/// failure mode `docs/M300-NATIVE.md` §2 warns about. `None` means "leave the device's range
/// alone and say so in the log", which is recoverable; a wrong index is not.
///
/// **Bench check:** read `m300_get_velocity_range` back on hardware and fill the ladder in.
#[must_use]
pub fn range_index(unit: SampleUnit, value: f64) -> Option<u8> {
    let span = range_span(unit);
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    if value <= span.min_value || close(value, span.min_value) {
        return Some(span.min_index);
    }
    if value >= span.max_value || close(value, span.max_value) {
        return Some(span.max_index);
    }
    None
}

/// The device's serial number, decoded from [`M300HardwareInfo::serial`].
///
/// The field is ten ASCII bytes and the vendor's own header warns it **may not be
/// NUL-terminated** (`docs/M300-NATIVE.md` §3), so a plain `CStr::from_ptr` would run off the
/// end of the struct. Anything after the first NUL is dropped, non-printable bytes are dropped
/// with it, and the result is trimmed; an empty string means the device did not report one.
#[must_use]
pub fn serial_number(serial: &[u8; 10]) -> String {
    let end = serial.iter().position(|b| *b == 0).unwrap_or(serial.len());
    serial[..end]
        .iter()
        .copied()
        .filter(|b| b.is_ascii_graphic() || *b == b' ')
        .map(char::from)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// A dotted firmware version from one of the SDK's three-byte version triples.
#[must_use]
pub fn firmware_version(triple: &[u8; 3]) -> String {
    format!("{}.{}.{}", triple[0], triple[1], triple[2])
}

/// A dotted-quad from one of the SDK's four-byte address fields.
#[must_use]
pub fn ipv4(octets: &[u8; 4]) -> String {
    format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3])
}

/// The device identity QuickVib reports, decoded from one `m300_get_hardware_info` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    /// Serial number, or empty when the device did not report one.
    pub serial_number: String,
    /// Application firmware version, `major.minor.patch`.
    pub firmware_version: String,
    /// The device's own IPv4 address, for the connection log line.
    pub ip: String,
}

/// Decode the fields of `M300HardwareInfo` that QuickVib reports through `*IDN?`.
#[must_use]
pub fn device_identity(info: &M300HardwareInfo) -> DeviceIdentity {
    DeviceIdentity {
        serial_number: serial_number(&info.serial),
        firmware_version: firmware_version(&info.app_version),
        ip: ipv4(&info.ip),
    }
}

/// Whether two quantities are the same value written two ways.
fn close(a: f64, b: f64) -> bool {
    if a == b {
        return true;
    }
    // An infinite or NaN request has no tolerance band: scaling by infinity would make every
    // table entry "close" to it.
    if !a.is_finite() || !b.is_finite() {
        return false;
    }
    let scale = a.abs().max(b.abs());
    (a - b).abs() <= scale * MATCH_TOLERANCE
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_rate_table_is_the_one_in_the_abi_contract() {
        // Spot-checks against docs/M300-NATIVE.md 6.1, including both ends and the one row
        // whose index and value are easiest to transpose.
        assert_eq!(sample_rate_hz(0x00), Some(2_000.0));
        assert_eq!(sample_rate_hz(0x05), Some(100_000.0));
        assert_eq!(sample_rate_hz(0x0A), Some(2_000_000.0));
        assert_eq!(sample_rate_hz(0x0E), Some(20_000_000.0));
        assert_eq!(sample_rate_hz(0x0F), None);
    }

    #[test]
    fn every_rate_round_trips_through_its_index() {
        for (index, hz) in SAMPLE_RATES_HZ.iter().enumerate() {
            let index = index as u8;
            assert_eq!(sample_rate_index(*hz), Some(index));
            assert_eq!(sample_rate_hz(index), Some(*hz));
            let choice = nearest_sample_rate(*hz);
            assert_eq!(choice.index, index);
            assert!(choice.is_exact());
        }
    }

    #[test]
    fn the_rate_table_is_strictly_increasing() {
        assert!(SAMPLE_RATES_HZ.windows(2).all(|w| w[0] < w[1]));
        assert!(LOW_PASS_HZ.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_rate_the_hardware_does_not_have_is_substituted_not_rejected() {
        let choice = nearest_sample_rate(3_000.0);
        assert!(!choice.is_exact());
        // Ratio, not hertz: 3 kHz is 1.5x the 2 kHz entry and a third of the 5 kHz one.
        assert_eq!(choice.hz, 2_000.0);
        assert_eq!(choice.requested_hz, 3_000.0);

        assert_eq!(nearest_sample_rate(4_000.0).hz, 5_000.0);
        assert_eq!(nearest_sample_rate(99_000.0).hz, 100_000.0);
    }

    #[test]
    fn rates_outside_the_table_clamp_to_an_end() {
        assert_eq!(nearest_sample_rate(1.0).hz, 2_000.0);
        assert_eq!(nearest_sample_rate(1e12).hz, 20_000_000.0);
    }

    #[test]
    fn a_nonsense_rate_still_yields_a_legal_index() {
        for hz in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let choice = nearest_sample_rate(hz);
            assert!(sample_rate_hz(choice.index).is_some());
            assert!(!choice.is_exact());
        }
    }

    #[test]
    fn a_rate_written_two_ways_lands_on_one_index() {
        assert_eq!(sample_rate_index(1e5), Some(0x05));
        assert_eq!(sample_rate_index(100_000.000_000_01), Some(0x05));
        assert_eq!(sample_rate_index(100_001.0), None);
    }

    #[test]
    fn the_filter_table_is_the_one_in_the_abi_contract() {
        assert_eq!(low_pass_hz(0), Some(100.0));
        assert_eq!(low_pass_hz(5), Some(10_000.0));
        assert_eq!(low_pass_hz(14), Some(3_000_000.0));
        assert_eq!(low_pass_hz(15), None);
    }

    #[test]
    fn a_rate_with_a_matching_band_pairs_exactly() {
        // 10 kHz sampling with the 10 kHz filter, the vendor's own example.
        let paired = low_pass_for_sample_rate(0x02);
        assert!(paired.is_exact);
        assert_eq!(paired.hz, 10_000.0);
        assert_eq!(paired.index, 5);

        for rate_index in [0x00, 0x01, 0x02, 0x03, 0x05, 0x09] {
            let paired = low_pass_for_sample_rate(rate_index);
            assert!(paired.is_exact, "rate index {rate_index:#04x}");
            assert_eq!(Some(paired.hz), sample_rate_hz(rate_index));
        }
    }

    #[test]
    fn a_rate_with_no_matching_band_pairs_downwards() {
        // There is no 50 kHz filter; 40 kHz is the highest band that does not exceed the rate.
        let paired = low_pass_for_sample_rate(0x04);
        assert!(!paired.is_exact);
        assert_eq!(paired.hz, 40_000.0);

        assert_eq!(low_pass_for_sample_rate(0x06).hz, 160_000.0);
        assert_eq!(low_pass_for_sample_rate(0x0E).hz, 3_000_000.0);
    }

    #[test]
    fn every_rate_pairs_with_a_band_no_higher_than_itself() {
        for index in 0..SAMPLE_RATES_HZ.len() as u8 {
            let rate = sample_rate_hz(index).unwrap();
            let paired = low_pass_for_sample_rate(index);
            assert!(paired.hz <= rate, "rate index {index}");
            assert!(low_pass_hz(paired.index).is_some());
        }
    }

    #[test]
    fn an_unknown_rate_index_still_pairs_with_a_legal_band() {
        let paired = low_pass_for_sample_rate(0xFF);
        assert!(low_pass_hz(paired.index).is_some());
    }

    #[test]
    fn a_requested_cutoff_rounds_down_to_a_band() {
        assert_eq!(nearest_low_pass(30_000.0).hz, 20_000.0);
        assert_eq!(nearest_low_pass(20_000.0).hz, 20_000.0);
        assert!(nearest_low_pass(20_000.0).is_exact);
        assert_eq!(nearest_low_pass(50.0).hz, 100.0, "below the lowest band");
        assert_eq!(nearest_low_pass(1e12).hz, 3_000_000.0);
    }

    #[test]
    fn data_types_round_trip_for_every_unit() {
        for unit in SampleUnit::all() {
            let value = data_type_for_unit(*unit);
            assert_eq!(unit_for_data_type(value), Some(*unit));
        }
        assert_eq!(data_type_for_unit(SampleUnit::VelocityUmPerSec), 0);
        assert_eq!(data_type_for_unit(SampleUnit::DisplacementUm), 1);
        assert_eq!(data_type_for_unit(SampleUnit::AccelerationMPerSec2), 2);
        assert_eq!(unit_for_data_type(3), None);
    }

    #[test]
    fn range_ends_map_to_the_documented_indices() {
        assert_eq!(range_index(SampleUnit::VelocityUmPerSec, 2.45), Some(0x00));
        assert_eq!(
            range_index(SampleUnit::VelocityUmPerSec, 7_000_000.0),
            Some(0x10)
        );
        assert_eq!(range_index(SampleUnit::DisplacementUm, 0.245), Some(0x00));
        assert_eq!(
            range_index(SampleUnit::DisplacementUm, 500_000.0),
            Some(0x12)
        );
        assert_eq!(
            range_index(SampleUnit::AccelerationMPerSec2, 1.225),
            Some(0x00)
        );
        assert_eq!(
            range_index(SampleUnit::AccelerationMPerSec2, 612_500.0),
            Some(0x0B)
        );
    }

    #[test]
    fn a_range_outside_the_ladder_clamps_to_the_nearest_end() {
        assert_eq!(range_index(SampleUnit::VelocityUmPerSec, 0.1), Some(0x00));
        assert_eq!(range_index(SampleUnit::VelocityUmPerSec, 1e9), Some(0x10));
    }

    #[test]
    fn a_range_between_the_documented_ends_is_refused_rather_than_guessed() {
        // The default project range. Nothing in the vendor documentation says which index
        // 1000 um/s is, so the backend logs it and leaves the device's range alone.
        assert_eq!(range_index(SampleUnit::VelocityUmPerSec, 1_000.0), None);
        assert_eq!(range_index(SampleUnit::DisplacementUm, 1_000.0), None);
        assert_eq!(range_index(SampleUnit::AccelerationMPerSec2, 100.0), None);
    }

    #[test]
    fn a_nonsense_range_is_refused() {
        for value in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(range_index(SampleUnit::VelocityUmPerSec, value), None);
        }
    }

    #[test]
    fn a_serial_that_fills_the_field_has_no_nul_to_stop_at() {
        // The header's own warning: ten characters, no terminator (docs/M300-NATIVE.md 3).
        let serial = *b"M300123456";
        assert_eq!(serial_number(&serial), "M300123456");
    }

    #[test]
    fn a_serial_stops_at_the_first_nul() {
        let serial = *b"M30012\0\0\0\0";
        assert_eq!(serial_number(&serial), "M30012");
    }

    #[test]
    fn a_serial_is_trimmed_and_stripped_of_junk() {
        let serial = *b"  M300 1  ";
        assert_eq!(serial_number(&serial), "M300 1");

        // Control and high bytes are dropped rather than rendered as replacement characters.
        let noisy = *b"M300\x01\x02\xff5\x7f6";
        assert_eq!(serial_number(&noisy), "M30056");
    }

    #[test]
    fn a_blank_serial_decodes_to_nothing_rather_than_junk() {
        assert_eq!(serial_number(&[0u8; 10]), "");
        assert_eq!(serial_number(&[b' '; 10]), "");
    }

    #[test]
    fn versions_and_addresses_decode_for_the_idn_and_the_log() {
        assert_eq!(firmware_version(&[1, 2, 3]), "1.2.3");
        assert_eq!(ipv4(&[192, 168, 1, 100]), "192.168.1.100");
    }

    #[test]
    fn a_hardware_info_reply_decodes_into_an_identity() {
        let info = M300HardwareInfo {
            device_type: 1,
            ip: [192, 168, 1, 50],
            subnet: [255, 255, 255, 0],
            gateway: [192, 168, 1, 1],
            mac: [0, 1, 2, 3, 4, 5],
            serial: *b"M300000042",
            fpga_version: [1, 0, 0, 0],
            bootloader_version: [1, 0, 0],
            app_version: [1, 4, 7],
            server_ip: [192, 168, 1, 100],
            server_port: 9123,
            has_fpga_version: 1,
        };
        let identity = device_identity(&info);
        assert_eq!(identity.serial_number, "M300000042");
        assert_eq!(identity.firmware_version, "1.4.7");
        assert_eq!(identity.ip, "192.168.1.50");
    }
}
