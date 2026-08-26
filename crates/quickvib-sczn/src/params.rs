//! Device parameters: the ids, their widths, the values a simulated M300 holds, and the
//! hardware-information blob.
//!
//! The specification's parameter table is a flat address space of 32-bit ids, each with a
//! documented width — mostly one byte, occasionally four, and one (`0x00000004`, hardware
//! information) that is a 41- or 45-byte structure. Width matters: read-parameter replies carry
//! an explicit length, but *status* replies do not, so a reader has to know from the id alone
//! how many bytes to take.
//!
//! Every code value here is an index into a vendor table, never a physical quantity — `0x05`
//! means "100 kHz", not "5 Hz". `crates/quickvib-m300/src/maps.rs` already carries those
//! ladders for the SDK backend, and [`sample_rate_hz`] mirrors the two this crate needs so the
//! simulator can be told `--rate 100000` and pick the right code without depending on a
//! Windows-only crate.

use std::fmt;

/// A 32-bit parameter id from the specification's table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamId(pub u32);

impl ParamId {
    /// `0x00000000`, 1 byte: sample rate, as an index into the device's rate ladder.
    pub const SAMPLE_RATE: Self = Self(0x0000_0000);
    /// `0x00000001`, 1 byte: which quantity the device uploads.
    pub const DATA_TYPE: Self = Self(0x0000_0001);
    /// `0x00000002`, 1 byte: indicator-laser level, 0–10.
    pub const LASER_LEVEL: Self = Self(0x0000_0002);
    /// `0x00000003`, 1 byte: signal strength.
    pub const SIGNAL_STRENGTH: Self = Self(0x0000_0003);
    /// `0x00000004`, 41 or 45 bytes: MAC, serial, addresses and firmware versions.
    pub const HARDWARE_INFO: Self = Self(0x0000_0004);
    /// `0x00000005`, 1 byte: low-pass filter band.
    pub const LOW_PASS: Self = Self(0x0000_0005);
    /// `0x00000006`, 1 byte: high-pass filter band.
    pub const HIGH_PASS: Self = Self(0x0000_0006);
    /// `0x00000007`, 1 byte: velocity measuring range.
    pub const VELOCITY_RANGE: Self = Self(0x0000_0007);
    /// `0x00000008`, 1 byte: displacement measuring range.
    pub const DISPLACEMENT_RANGE: Self = Self(0x0000_0008);
    /// `0x00000009`, 1 byte: acceleration measuring range.
    pub const ACCELERATION_RANGE: Self = Self(0x0000_0009);
    /// `0x0000000E`, 1 byte: trigger type — free, software, hardware, custom.
    pub const TRIGGER_TYPE: Self = Self(0x0000_000E);
    /// `0x00000012`, 4 bytes: triggered-capture length.
    pub const TRIGGER_LENGTH: Self = Self(0x0000_0012);
    /// `0x00000013`, 4 bytes: samples retained before the trigger.
    pub const TRIGGER_PRETRIGGER: Self = Self(0x0000_0013);
    /// `0x10000003`, 1 byte: DC-removal switch, `0` off, `1` on.
    pub const REMOVE_DC_SWITCH: Self = Self(0x1000_0003);

    /// The width the specification gives this parameter, or `None` for one it does not define.
    ///
    /// Hardware information reports [`HardwareInfo::LEN`], the 41-byte form — the 45-byte form
    /// adds an FPGA version the M300 does not report.
    #[must_use]
    pub const fn width(self) -> Option<usize> {
        match self {
            Self::HARDWARE_INFO => Some(HardwareInfo::LEN),
            Self::SAMPLE_RATE
            | Self::DATA_TYPE
            | Self::LASER_LEVEL
            | Self::SIGNAL_STRENGTH
            | Self::LOW_PASS
            | Self::HIGH_PASS
            | Self::VELOCITY_RANGE
            | Self::DISPLACEMENT_RANGE
            | Self::ACCELERATION_RANGE
            | Self::TRIGGER_TYPE
            | Self::REMOVE_DC_SWITCH => Some(1),
            Self::TRIGGER_LENGTH | Self::TRIGGER_PRETRIGGER => Some(4),
            _ => None,
        }
    }

    /// Whether a value of `len` bytes is the right size for this parameter.
    ///
    /// Hardware information is the one field with two legal widths; everything else is exact.
    #[must_use]
    pub const fn accepts_width(self, len: usize) -> bool {
        match self.width() {
            Some(width) => {
                len == width
                    || (self.0 == Self::HARDWARE_INFO.0 && len == HardwareInfo::LEN_WITH_FPGA)
            }
            None => false,
        }
    }

    /// The name the specification gives this parameter, for log lines.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::SAMPLE_RATE => "sampleRate",
            Self::DATA_TYPE => "dataType",
            Self::LASER_LEVEL => "laserLevel",
            Self::SIGNAL_STRENGTH => "signalStrength",
            Self::HARDWARE_INFO => "hardwareInfo",
            Self::LOW_PASS => "lowPass",
            Self::HIGH_PASS => "highPass",
            Self::VELOCITY_RANGE => "velocityRange",
            Self::DISPLACEMENT_RANGE => "displacementRange",
            Self::ACCELERATION_RANGE => "accelerationRange",
            Self::TRIGGER_TYPE => "triggerType",
            Self::TRIGGER_LENGTH => "triggerLength",
            Self::TRIGGER_PRETRIGGER => "triggerPretrigger",
            Self::REMOVE_DC_SWITCH => "removeDcSwitch",
            _ => "unknown",
        }
    }
}

impl fmt::Display for ParamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(0x{:08X})", self.name(), self.0)
    }
}

/// A 32-bit device-status id.
///
/// Status values have no length field on the wire — the reply is `id, value, id, value` — so
/// [`StatusId::width`] is load-bearing rather than merely informative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StatusId(pub u32);

impl StatusId {
    /// `0x00000000`, 1 byte: see [`RunningState`].
    pub const RUNNING_STATE: Self = Self(0x0000_0000);
    /// `0x00000001`, 4 bytes: TEC NTTC resistance, `i32` ohms.
    pub const TEC_NTTC: Self = Self(0x0000_0001);
    /// `0x00000002`, 4 bytes: board temperature, `f32` degrees Celsius.
    pub const BOARD_TEMPERATURE: Self = Self(0x0000_0002);
    /// `0x00000003`, 4 bytes: photodiode current, `f32` milliamps.
    pub const PD_CURRENT: Self = Self(0x0000_0003);
    /// `0x00000004`, 4 bytes: signal strength, I in the low half and Q in the high half.
    pub const SIGNAL_STRENGTH: Self = Self(0x0000_0004);

    /// The width the specification gives this status value, or `None` for an unknown id.
    #[must_use]
    pub const fn width(self) -> Option<usize> {
        match self {
            Self::RUNNING_STATE => Some(1),
            Self::TEC_NTTC | Self::BOARD_TEMPERATURE | Self::PD_CURRENT | Self::SIGNAL_STRENGTH => {
                Some(4)
            }
            _ => None,
        }
    }

    /// The name the specification gives this status, for log lines.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RUNNING_STATE => "runningState",
            Self::TEC_NTTC => "tecNttc",
            Self::BOARD_TEMPERATURE => "boardTemperature",
            Self::PD_CURRENT => "pdCurrent",
            Self::SIGNAL_STRENGTH => "signalStrength",
            _ => "unknown",
        }
    }
}

impl fmt::Display for StatusId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(0x{:08X})", self.name(), self.0)
    }
}

/// The running-state byte reported under [`StatusId::RUNNING_STATE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunningState {
    /// `0x00` — healthy, not uploading.
    #[default]
    Idle,
    /// `0x01` — healthy, uploading samples.
    Acquiring,
    /// `0x02` — firmware upgrade in progress.
    Upgrading,
    /// `0x03` — fault.
    Fault,
}

impl RunningState {
    /// The byte the specification assigns.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Idle => 0x00,
            Self::Acquiring => 0x01,
            Self::Upgrading => 0x02,
            Self::Fault => 0x03,
        }
    }
}

/// Upload data types (`ParamId::DATA_TYPE`), matching `quickvib_m300::maps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DataType {
    /// `0x00` — velocity, μm/s.
    #[default]
    Velocity,
    /// `0x01` — displacement, μm.
    Displacement,
    /// `0x02` — acceleration, m/s².
    Acceleration,
    /// `0x03` — raw I/Q pairs. The simulator can name it but does not synthesize it: an I/Q
    /// block is two 16-bit lanes per 4-byte word, not an `f32`, and inventing plausible-looking
    /// quadrature data would be a fiction about the instrument rather than about the wire.
    IqPair,
}

impl DataType {
    /// Every data type, in code order.
    pub const ALL: [Self; 4] = [
        Self::Velocity,
        Self::Displacement,
        Self::Acceleration,
        Self::IqPair,
    ];

    /// The byte the specification assigns.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Velocity => 0x00,
            Self::Displacement => 0x01,
            Self::Acceleration => 0x02,
            Self::IqPair => 0x03,
        }
    }

    /// The type a code means, or `None` for one the specification does not define.
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0x00 => Some(Self::Velocity),
            0x01 => Some(Self::Displacement),
            0x02 => Some(Self::Acceleration),
            0x03 => Some(Self::IqPair),
            _ => None,
        }
    }

    /// The command-line spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Velocity => "velocity",
            Self::Displacement => "displacement",
            Self::Acceleration => "acceleration",
            Self::IqPair => "iq",
        }
    }

    /// Parse a command-line spelling.
    #[must_use]
    pub fn from_str_opt(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }

    /// The unit samples of this type are expressed in, for the banner line.
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Velocity => "um/s",
            Self::Displacement => "um",
            Self::Acceleration => "m/s^2",
            Self::IqPair => "iq",
        }
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Sample rates the M300 offers, indexed by the [`ParamId::SAMPLE_RATE`] code
/// (`device_type = 0x01`). The same ladder as `quickvib_m300::maps::SAMPLE_RATES_HZ`.
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

/// Low-pass cutoffs, indexed by the [`ParamId::LOW_PASS`] code (`device_type = 0x01`). The same
/// ladder as `quickvib_m300::maps::LOW_PASS_HZ`.
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

/// The rate a [`ParamId::SAMPLE_RATE`] code means, or `None` for a code off the ladder.
#[must_use]
pub fn sample_rate_hz(code: u8) -> Option<f64> {
    SAMPLE_RATES_HZ.get(usize::from(code)).copied()
}

/// The code for a rate in hertz, choosing the nearest rung in log space when the request is not
/// on the ladder. The M300 cannot be set to an arbitrary rate, so `--rate 96000` has to become
/// *something*; the same ratio-based choice `quickvib_m300::maps::nearest_sample_rate` makes.
#[must_use]
pub fn nearest_sample_rate_code(hz: f64) -> u8 {
    if !hz.is_finite() || hz <= 0.0 {
        return 0;
    }
    let mut best = 0u8;
    let mut best_distance = f64::INFINITY;
    for (index, candidate) in SAMPLE_RATES_HZ.iter().enumerate() {
        let distance = (candidate.ln() - hz.ln()).abs();
        if distance < best_distance {
            best_distance = distance;
            best = index as u8;
        }
    }
    best
}

/// The highest low-pass band that does not exceed `hz`, which is the band the vendor insists be
/// set together with the sample rate. Rounding down is the conservative direction: a filter
/// above the requested cutoff passes energy the caller asked to reject.
#[must_use]
pub fn low_pass_code_for(hz: f64) -> u8 {
    if !hz.is_finite() || hz <= 0.0 {
        return 0;
    }
    let mut best = 0u8;
    for (index, candidate) in LOW_PASS_HZ.iter().enumerate() {
        if *candidate <= hz {
            best = index as u8;
        }
    }
    best
}

/// The 41-byte hardware-information blob returned under [`ParamId::HARDWARE_INFO`].
///
/// Field order and widths come straight from the specification's table. The 45-byte variant
/// inserts a four-byte FPGA version between the serial and the bootloader version; the M300
/// (`device_type = 0x01`) reports the short form, so that is what this encodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareInfo {
    /// `0x01` for the M300.
    pub device_type: u8,
    /// The device's own address.
    pub ip: [u8; 4],
    /// Subnet mask.
    pub subnet: [u8; 4],
    /// Default gateway.
    pub gateway: [u8; 4],
    /// MAC address.
    pub mac: [u8; 6],
    /// Serial number, ten ASCII bytes, **not** necessarily NUL-terminated.
    pub serial: [u8; 10],
    /// Bootloader version, `[major, minor, patch]`.
    pub bootloader_version: [u8; 3],
    /// Application firmware version, `[major, minor, patch]`.
    pub app_version: [u8; 3],
    /// Address of the host the device dials.
    pub server_ip: [u8; 4],
    /// Port on that host.
    pub server_port: u16,
}

impl HardwareInfo {
    /// `device_type` for the M300.
    pub const DEVICE_TYPE_M300: u8 = 0x01;

    /// Encoded size of the short form: 1 + 4 + 4 + 4 + 6 + 10 + 3 + 3 + 4 + 2.
    pub const LEN: usize = 41;

    /// Encoded size of the long form, which adds a four-byte FPGA version.
    pub const LEN_WITH_FPGA: usize = 45;

    /// The blob, in the specification's field order.
    ///
    /// Addresses go out in network byte order, which for a dotted quad stored as four octets
    /// means simply "as written". `server_port` is big-endian for consistency with every other
    /// multi-byte field in this protocol — note that the *UDP* configuration protocol states
    /// the opposite for its own port fields, so this is one to confirm on a bench.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(Self::LEN);
        bytes.push(self.device_type);
        bytes.extend_from_slice(&self.ip);
        bytes.extend_from_slice(&self.subnet);
        bytes.extend_from_slice(&self.gateway);
        bytes.extend_from_slice(&self.mac);
        bytes.extend_from_slice(&self.serial);
        bytes.extend_from_slice(&self.bootloader_version);
        bytes.extend_from_slice(&self.app_version);
        bytes.extend_from_slice(&self.server_ip);
        bytes.extend_from_slice(&self.server_port.to_be_bytes());
        bytes
    }

    /// Decode the short form.
    ///
    /// # Errors
    /// `()` when `bytes` is not exactly [`HardwareInfo::LEN`] long. There is nothing more to
    /// say about the failure: every field is fixed-width, so length is the only thing that can
    /// be wrong.
    #[allow(clippy::result_unit_err)]
    pub fn decode(bytes: &[u8]) -> Result<Self, ()> {
        if bytes.len() != Self::LEN {
            return Err(());
        }
        let take = |start: usize, len: usize| &bytes[start..start + len];
        let mut ip = [0u8; 4];
        let mut subnet = [0u8; 4];
        let mut gateway = [0u8; 4];
        let mut mac = [0u8; 6];
        let mut serial = [0u8; 10];
        let mut bootloader_version = [0u8; 3];
        let mut app_version = [0u8; 3];
        let mut server_ip = [0u8; 4];
        ip.copy_from_slice(take(1, 4));
        subnet.copy_from_slice(take(5, 4));
        gateway.copy_from_slice(take(9, 4));
        mac.copy_from_slice(take(13, 6));
        serial.copy_from_slice(take(19, 10));
        bootloader_version.copy_from_slice(take(29, 3));
        app_version.copy_from_slice(take(32, 3));
        server_ip.copy_from_slice(take(35, 4));
        Ok(Self {
            device_type: bytes[0],
            ip,
            subnet,
            gateway,
            mac,
            serial,
            bootloader_version,
            app_version,
            server_ip,
            server_port: u16::from_be_bytes([bytes[39], bytes[40]]),
        })
    }

    /// Overwrite the serial number from a string, padding with NULs and truncating at ten
    /// bytes — which is what the field does on real hardware, terminator or no terminator.
    pub fn set_serial(&mut self, serial: &str) {
        self.serial = [0u8; 10];
        for (slot, byte) in self.serial.iter_mut().zip(serial.bytes()) {
            *slot = byte;
        }
    }

    /// The serial as text, stopping at the first NUL.
    #[must_use]
    pub fn serial_text(&self) -> String {
        let end = self
            .serial
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(self.serial.len());
        String::from_utf8_lossy(&self.serial[..end]).into_owned()
    }
}

impl Default for HardwareInfo {
    /// A plausible bench device: `192.168.1.50` on a `/24`, dialling `192.168.1.100:9123`.
    fn default() -> Self {
        let mut info = Self {
            device_type: Self::DEVICE_TYPE_M300,
            ip: [192, 168, 1, 50],
            subnet: [255, 255, 255, 0],
            gateway: [192, 168, 1, 1],
            mac: [0x02, 0x00, 0x4D, 0x33, 0x00, 0x01],
            serial: [0u8; 10],
            bootloader_version: [1, 0, 0],
            app_version: [1, 2, 0],
            server_ip: [192, 168, 1, 100],
            server_port: 9123,
        };
        info.set_serial("SIM-000001");
        info
    }
}

/// Why a parameter write was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamError {
    /// No such id in the specification's table.
    UnknownId(ParamId),
    /// The id exists but the value is the wrong width for it.
    WrongWidth {
        /// Which parameter.
        id: ParamId,
        /// What the specification says.
        expected: usize,
        /// What arrived.
        found: usize,
    },
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownId(id) => write!(f, "no parameter {id}"),
            Self::WrongWidth {
                id,
                expected,
                found,
            } => write!(f, "parameter {id} is {expected} bytes, got {found}"),
        }
    }
}

impl std::error::Error for ParamError {}

/// The parameters a simulated device holds, in id order.
///
/// A sorted `Vec` rather than a map: the set is a dozen entries, the parameter-id-list reply has
/// to be ordered and stable anyway, and a `BTreeMap` would buy nothing but an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamStore {
    /// `(id, value)` sorted by id.
    entries: Vec<(ParamId, Vec<u8>)>,
}

impl ParamStore {
    /// An empty store. [`ParamStore::default`] is the populated one.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// The stored value for `id`, or `None` when the device does not carry that parameter.
    #[must_use]
    pub fn get(&self, id: ParamId) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, value)| value.as_slice())
    }

    /// The stored value for `id` as a single byte, for the many one-byte parameters.
    #[must_use]
    pub fn get_byte(&self, id: ParamId) -> Option<u8> {
        match self.get(id) {
            Some([byte]) => Some(*byte),
            _ => None,
        }
    }

    /// Store `value` against `id`.
    ///
    /// # Errors
    /// [`ParamError`] when the id is not in the specification's table or the value is the wrong
    /// width for it. A device that accepted a two-byte sample rate would be a worse stand-in
    /// than one that refuses.
    pub fn set(&mut self, id: ParamId, value: &[u8]) -> Result<(), ParamError> {
        let expected = id.width().ok_or(ParamError::UnknownId(id))?;
        if !id.accepts_width(value.len()) {
            return Err(ParamError::WrongWidth {
                id,
                expected,
                found: value.len(),
            });
        }
        match self
            .entries
            .binary_search_by_key(&id.0, |(candidate, _)| candidate.0)
        {
            Ok(position) => self.entries[position].1 = value.to_vec(),
            Err(position) => self.entries.insert(position, (id, value.to_vec())),
        }
        Ok(())
    }

    /// Store a one-byte parameter.
    ///
    /// # Errors
    /// As [`ParamStore::set`].
    pub fn set_byte(&mut self, id: ParamId, value: u8) -> Result<(), ParamError> {
        self.set(id, &[value])
    }

    /// Every id this device carries, ascending — the body of a parameter-list reply.
    #[must_use]
    pub fn ids(&self) -> Vec<ParamId> {
        self.entries.iter().map(|(id, _)| *id).collect()
    }

    /// How many parameters are stored.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the store holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The configured sample rate in hertz, or `None` when the code is off the ladder.
    #[must_use]
    pub fn sample_rate_hz(&self) -> Option<f64> {
        self.get_byte(ParamId::SAMPLE_RATE).and_then(sample_rate_hz)
    }

    /// The configured upload data type, defaulting to velocity if the stored code is one the
    /// specification does not define.
    #[must_use]
    pub fn data_type(&self) -> DataType {
        self.get_byte(ParamId::DATA_TYPE)
            .and_then(DataType::from_code)
            .unwrap_or(DataType::Velocity)
    }

    /// The hardware-information blob, decoded.
    #[must_use]
    pub fn hardware_info(&self) -> Option<HardwareInfo> {
        HardwareInfo::decode(self.get(ParamId::HARDWARE_INFO)?).ok()
    }
}

impl Default for ParamStore {
    /// The power-on parameter set of a simulated M300: 100 kS/s velocity with the matching
    /// 100 kHz low-pass band, the widest ranges, free-running trigger, DC removal off.
    fn default() -> Self {
        let mut store = Self::empty();
        // Every id below has a width in the specification's table, so none of these can fail.
        let mut set = |id: ParamId, value: Vec<u8>| {
            let _ = store.set(id, &value);
        };
        set(ParamId::SAMPLE_RATE, vec![0x05]); // 100 kHz
        set(ParamId::DATA_TYPE, vec![DataType::Velocity.code()]);
        set(ParamId::LASER_LEVEL, vec![0x05]);
        set(ParamId::SIGNAL_STRENGTH, vec![0x00]);
        set(ParamId::HARDWARE_INFO, HardwareInfo::default().encode());
        set(ParamId::LOW_PASS, vec![0x09]); // 100 kHz, matching the sample rate
        set(ParamId::HIGH_PASS, vec![0x00]);
        set(ParamId::VELOCITY_RANGE, vec![0x10]);
        set(ParamId::DISPLACEMENT_RANGE, vec![0x12]);
        set(ParamId::ACCELERATION_RANGE, vec![0x0B]);
        set(ParamId::TRIGGER_TYPE, vec![0x00]); // free-running
        set(ParamId::TRIGGER_LENGTH, vec![0, 0, 0, 0]);
        set(ParamId::TRIGGER_PRETRIGGER, vec![0, 0, 0, 0]);
        set(ParamId::REMOVE_DC_SWITCH, vec![0x00]);
        store
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_default_store_is_a_plausible_power_on_state() {
        let store = ParamStore::default();
        assert_eq!(store.len(), 14);
        assert!(!store.is_empty());
        assert_eq!(store.sample_rate_hz(), Some(100_000.0));
        assert_eq!(store.data_type(), DataType::Velocity);
        assert_eq!(store.get_byte(ParamId::LOW_PASS), Some(0x09));
    }

    #[test]
    fn the_low_pass_band_matches_the_sample_rate_the_vendor_insists_on_pairing_it_with() {
        let store = ParamStore::default();
        let rate = store.sample_rate_hz().unwrap();
        let band = LOW_PASS_HZ[usize::from(store.get_byte(ParamId::LOW_PASS).unwrap())];
        assert_eq!(band, rate);
    }

    #[test]
    fn ids_come_back_sorted_however_they_were_inserted() {
        let mut store = ParamStore::empty();
        store.set_byte(ParamId::HIGH_PASS, 1).unwrap();
        store.set_byte(ParamId::SAMPLE_RATE, 2).unwrap();
        store.set_byte(ParamId::REMOVE_DC_SWITCH, 1).unwrap();
        store.set_byte(ParamId::LOW_PASS, 3).unwrap();
        assert_eq!(
            store.ids(),
            vec![
                ParamId::SAMPLE_RATE,
                ParamId::LOW_PASS,
                ParamId::HIGH_PASS,
                ParamId::REMOVE_DC_SWITCH,
            ]
        );
    }

    #[test]
    fn writing_a_parameter_twice_replaces_it_rather_than_duplicating_it() {
        let mut store = ParamStore::empty();
        store.set_byte(ParamId::SAMPLE_RATE, 0x02).unwrap();
        store.set_byte(ParamId::SAMPLE_RATE, 0x05).unwrap();
        assert_eq!(store.len(), 1);
        assert_eq!(store.get_byte(ParamId::SAMPLE_RATE), Some(0x05));
    }

    #[test]
    fn a_value_of_the_wrong_width_is_refused() {
        let mut store = ParamStore::empty();
        assert_eq!(
            store.set(ParamId::SAMPLE_RATE, &[1, 2]),
            Err(ParamError::WrongWidth {
                id: ParamId::SAMPLE_RATE,
                expected: 1,
                found: 2,
            })
        );
        assert_eq!(
            store.set(ParamId::TRIGGER_LENGTH, &[1]),
            Err(ParamError::WrongWidth {
                id: ParamId::TRIGGER_LENGTH,
                expected: 4,
                found: 1,
            })
        );
        assert!(store.is_empty());
    }

    #[test]
    fn an_unknown_id_is_refused_rather_than_invented() {
        let mut store = ParamStore::empty();
        let id = ParamId(0x0BAD_0BAD);
        assert_eq!(store.set(id, &[0]), Err(ParamError::UnknownId(id)));
        assert!(store.get(id).is_none());
        assert_eq!(id.width(), None);
    }

    #[test]
    fn hardware_info_accepts_both_documented_widths() {
        let mut store = ParamStore::empty();
        assert!(store
            .set(ParamId::HARDWARE_INFO, &[0u8; HardwareInfo::LEN])
            .is_ok());
        assert!(store
            .set(ParamId::HARDWARE_INFO, &[0u8; HardwareInfo::LEN_WITH_FPGA])
            .is_ok());
        assert!(store.set(ParamId::HARDWARE_INFO, &[0u8; 40]).is_err());
    }

    #[test]
    fn the_hardware_blob_is_forty_one_bytes_in_the_documented_field_order() {
        let info = HardwareInfo::default();
        let bytes = info.encode();
        assert_eq!(bytes.len(), HardwareInfo::LEN);
        assert_eq!(bytes[0], HardwareInfo::DEVICE_TYPE_M300);
        assert_eq!(&bytes[1..5], &[192, 168, 1, 50], "ip");
        assert_eq!(&bytes[5..9], &[255, 255, 255, 0], "subnet");
        assert_eq!(&bytes[9..13], &[192, 168, 1, 1], "gateway");
        assert_eq!(&bytes[13..19], &info.mac, "mac");
        assert_eq!(&bytes[19..29], b"SIM-000001", "serial");
        assert_eq!(&bytes[29..32], &[1, 0, 0], "bootloader");
        assert_eq!(&bytes[32..35], &[1, 2, 0], "app");
        assert_eq!(&bytes[35..39], &[192, 168, 1, 100], "server ip");
        assert_eq!(&bytes[39..41], &9123u16.to_be_bytes(), "server port");
    }

    #[test]
    fn the_hardware_blob_round_trips() {
        let info = HardwareInfo::default();
        assert_eq!(HardwareInfo::decode(&info.encode()), Ok(info));
        assert!(HardwareInfo::decode(&[0u8; 40]).is_err());
        assert!(HardwareInfo::decode(&[0u8; 42]).is_err());
    }

    #[test]
    fn a_serial_that_fills_the_field_has_no_room_for_a_terminator() {
        let mut info = HardwareInfo::default();
        info.set_serial("M300123456");
        assert_eq!(&info.serial, b"M300123456");
        assert_eq!(info.serial_text(), "M300123456");

        info.set_serial("M300123456789");
        assert_eq!(&info.serial, b"M300123456", "truncated at ten bytes");

        info.set_serial("SN1");
        assert_eq!(info.serial_text(), "SN1");
        assert_eq!(info.serial[3], 0, "padded with NULs");
    }

    #[test]
    fn the_default_store_carries_a_decodable_hardware_blob() {
        let info = ParamStore::default().hardware_info().unwrap();
        assert_eq!(info.serial_text(), "SIM-000001");
        assert_eq!(info.device_type, HardwareInfo::DEVICE_TYPE_M300);
    }

    #[test]
    fn the_rate_ladder_matches_the_one_the_sdk_backend_uses() {
        assert_eq!(sample_rate_hz(0x00), Some(2_000.0));
        assert_eq!(sample_rate_hz(0x05), Some(100_000.0));
        assert_eq!(sample_rate_hz(0x0E), Some(20_000_000.0));
        assert_eq!(sample_rate_hz(0x0F), None);
        for (index, hz) in SAMPLE_RATES_HZ.iter().enumerate() {
            assert_eq!(nearest_sample_rate_code(*hz), index as u8);
        }
    }

    #[test]
    fn a_rate_off_the_ladder_picks_the_nearest_rung_by_ratio() {
        // 3 kHz is 1.5x the 2 kHz rung and a third of the 5 kHz one.
        assert_eq!(nearest_sample_rate_code(3_000.0), 0x00);
        assert_eq!(nearest_sample_rate_code(4_000.0), 0x01);
        assert_eq!(nearest_sample_rate_code(96_000.0), 0x05);
        assert_eq!(nearest_sample_rate_code(1.0), 0x00);
        assert_eq!(nearest_sample_rate_code(1e12), 0x0E);
        for nonsense in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(nearest_sample_rate_code(nonsense), 0x00);
        }
    }

    #[test]
    fn the_low_pass_band_rounds_down_to_the_rate() {
        assert_eq!(low_pass_code_for(100_000.0), 0x09);
        assert_eq!(low_pass_code_for(50_000.0), 0x07, "no 50 kHz band; 40 kHz");
        assert_eq!(low_pass_code_for(1.0), 0x00, "below every band");
        assert_eq!(low_pass_code_for(f64::NAN), 0x00);
        for hz in SAMPLE_RATES_HZ {
            assert!(LOW_PASS_HZ[usize::from(low_pass_code_for(hz))] <= hz);
        }
    }

    #[test]
    fn data_types_round_trip_through_codes_and_spellings() {
        for kind in DataType::ALL {
            assert_eq!(DataType::from_code(kind.code()), Some(kind));
            assert_eq!(DataType::from_str_opt(kind.as_str()), Some(kind));
            assert_eq!(kind.to_string(), kind.as_str());
            assert!(!kind.unit().is_empty());
        }
        assert_eq!(DataType::from_code(0x04), None);
        assert_eq!(DataType::from_str_opt("pressure"), None);
        assert_eq!(DataType::default(), DataType::Velocity);
    }

    #[test]
    fn running_states_carry_the_documented_codes() {
        assert_eq!(RunningState::Idle.code(), 0x00);
        assert_eq!(RunningState::Acquiring.code(), 0x01);
        assert_eq!(RunningState::Upgrading.code(), 0x02);
        assert_eq!(RunningState::Fault.code(), 0x03);
        assert_eq!(RunningState::default(), RunningState::Idle);
    }

    #[test]
    fn status_widths_are_known_because_the_reply_has_no_length_field() {
        assert_eq!(StatusId::RUNNING_STATE.width(), Some(1));
        assert_eq!(StatusId::TEC_NTTC.width(), Some(4));
        assert_eq!(StatusId::BOARD_TEMPERATURE.width(), Some(4));
        assert_eq!(StatusId::PD_CURRENT.width(), Some(4));
        assert_eq!(StatusId::SIGNAL_STRENGTH.width(), Some(4));
        assert_eq!(StatusId(0x99).width(), None);
    }

    #[test]
    fn ids_describe_themselves_for_the_log() {
        assert_eq!(ParamId::SAMPLE_RATE.to_string(), "sampleRate(0x00000000)");
        assert_eq!(ParamId(0x1234).name(), "unknown");
        assert_eq!(
            StatusId::RUNNING_STATE.to_string(),
            "runningState(0x00000000)"
        );
        assert_eq!(StatusId(0x1234).name(), "unknown");
        assert!(ParamError::UnknownId(ParamId(1))
            .to_string()
            .contains("no parameter"));
        assert!(ParamError::WrongWidth {
            id: ParamId::SAMPLE_RATE,
            expected: 1,
            found: 4,
        }
        .to_string()
        .contains("1 bytes"));
    }
}
