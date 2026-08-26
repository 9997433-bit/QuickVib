//! The SCZN checksum, and the one place the vendor specification contradicts itself.
//!
//! `docs/SCZN-PROTOCOL.md` §7 has the detail; the short version is that the specification
//! prints a C table for **standard CRC-32** (reflected polynomial `0xEDB88320`) and, a few
//! pages later, a worked Python example whose expected output can only have come from
//! **CRC-32C** (Castagnoli, `0x82F63B78`) — the example imports `crc32c` and calls it. Both
//! are implemented here, and [`accepts`] treats either as valid on receive, because the same
//! specification also says a device short of compute may skip the checksum entirely and send
//! `0x00000000`. A simulator that rejected a packet on this basis would be asserting a fact
//! about the wire that the vendor has not actually stated.
//!
//! ```
//! use quickvib_sczn::crc;
//!
//! // The specification's own worked example, byte for byte.
//! let packet = b"SCZN\x00\x01\x00\x01\x00\x00\x00\x00";
//! assert_eq!(crc::crc32(packet), 0x5CEB_DD6D);
//! assert_eq!(crc::crc32c(packet), 0xA571_05A2); // what the Python sample prints
//! ```

/// Reflected polynomial of standard CRC-32, the one the specification's C table encodes.
const STANDARD_POLYNOMIAL: u32 = 0xEDB8_8320;

/// Reflected polynomial of CRC-32C, the one the specification's Python example uses.
const CASTAGNOLI_POLYNOMIAL: u32 = 0x82F6_3B78;

/// The "no checksum computed" value the specification permits in place of a real one.
pub const UNCHECKED: u32 = 0x0000_0000;

/// The 256-entry lookup table for a reflected polynomial.
///
/// Built at compile time rather than transcribed, which is both shorter than the vendor's
/// literal table and impossible to get wrong by a digit; the tests check it against the
/// published `"123456789"` check values for both polynomials.
const fn table(polynomial: u32) -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0usize;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ polynomial
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

/// Table for [`crc32`].
static STANDARD_TABLE: [u32; 256] = table(STANDARD_POLYNOMIAL);

/// Table for [`crc32c`].
static CASTAGNOLI_TABLE: [u32; 256] = table(CASTAGNOLI_POLYNOMIAL);

/// Run one reflected CRC to completion: init all-ones, table-driven, final complement. This is
/// the loop the specification prints verbatim, with the table as a parameter.
fn digest(table: &[u32; 256], bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        let index = usize::from(((crc ^ u32::from(*byte)) & 0xFF) as u8);
        crc = table[index] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Standard CRC-32 of `bytes` — the specification's C table.
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    digest(&STANDARD_TABLE, bytes)
}

/// CRC-32C (Castagnoli) of `bytes` — the specification's Python example.
#[must_use]
pub fn crc32c(bytes: &[u8]) -> u32 {
    digest(&CASTAGNOLI_TABLE, bytes)
}

/// Which checksum a sender writes into the trailer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CrcMode {
    /// Standard CRC-32, as the specification's C table computes it. The default, because the
    /// table is the normative part of the document and the Python listing is illustrative.
    #[default]
    Standard,
    /// CRC-32C, as the specification's Python example computes it.
    Castagnoli,
    /// Send [`UNCHECKED`], which the specification explicitly allows for a device short of
    /// compute. Useful on the bench for deciding whether a peer verifies at all.
    Zero,
}

impl CrcMode {
    /// The checksum this mode puts in the trailer for a packet whose leading bytes are `bytes`.
    #[must_use]
    pub fn checksum(self, bytes: &[u8]) -> u32 {
        match self {
            Self::Standard => crc32(bytes),
            Self::Castagnoli => crc32c(bytes),
            Self::Zero => UNCHECKED,
        }
    }

    /// The spelling accepted on the command line, and the one [`std::fmt::Display`] produces.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Castagnoli => "castagnoli",
            Self::Zero => "zero",
        }
    }

    /// Parse a command-line spelling.
    #[must_use]
    pub fn from_str_opt(text: &str) -> Option<Self> {
        match text {
            "standard" | "crc32" => Some(Self::Standard),
            "castagnoli" | "crc32c" => Some(Self::Castagnoli),
            "zero" | "none" | "off" => Some(Self::Zero),
            _ => None,
        }
    }
}

impl std::fmt::Display for CrcMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether `claimed` is a checksum this receiver will accept over `bytes`.
///
/// Liberal on purpose — see the module docs. Being strict here would mean picking a winner in
/// the specification's own disagreement, and picking wrong turns a working link into a silent
/// one with no way to tell which end is at fault.
#[must_use]
pub fn accepts(bytes: &[u8], claimed: u32) -> bool {
    claimed == UNCHECKED || claimed == crc32(bytes) || claimed == crc32c(bytes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// The check value every CRC catalogue publishes for these two polynomials. If the
    /// compile-time table were wrong by one entry these would not both hold.
    #[test]
    fn both_polynomials_match_the_published_check_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32c(b"123456789"), 0xE306_9283);
    }

    #[test]
    fn the_specifications_own_example_packet_pins_both_variants() {
        // "构建的数据包: 53435A4E0001000100000000A57105A2" — a stop-acquisition frame whose
        // printed checksum is CRC-32C, while the table printed two pages earlier is CRC-32.
        let packet = b"SCZN\x00\x01\x00\x01\x00\x00\x00\x00";
        assert_eq!(crc32(packet), 0x5CEB_DD6D);
        assert_eq!(crc32c(packet), 0xA571_05A2);
        assert_ne!(crc32(packet), crc32c(packet), "the two really do disagree");
    }

    #[test]
    fn an_empty_input_is_zero_under_both() {
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32c(b""), 0);
    }

    #[test]
    fn every_variant_the_specification_permits_is_accepted() {
        let packet = b"SCZN\x00\x01\x00\x01\x00\x00\x00\x00";
        assert!(accepts(packet, UNCHECKED), "the spec allows skipping it");
        assert!(accepts(packet, crc32(packet)));
        assert!(accepts(packet, crc32c(packet)));
        assert!(!accepts(packet, 0xDEAD_BEEF));
    }

    #[test]
    fn a_single_flipped_bit_changes_the_checksum() {
        let mut packet = *b"SCZN\x00\x01\x00\x01\x00\x00\x00\x00";
        let before = crc32(&packet);
        packet[7] ^= 0x01;
        assert_ne!(crc32(&packet), before);
    }

    #[test]
    fn modes_round_trip_through_their_command_line_spelling() {
        for mode in [CrcMode::Standard, CrcMode::Castagnoli, CrcMode::Zero] {
            assert_eq!(CrcMode::from_str_opt(mode.as_str()), Some(mode));
            assert_eq!(mode.to_string(), mode.as_str());
        }
        assert_eq!(CrcMode::from_str_opt("crc32"), Some(CrcMode::Standard));
        assert_eq!(CrcMode::from_str_opt("crc32c"), Some(CrcMode::Castagnoli));
        assert_eq!(CrcMode::from_str_opt("off"), Some(CrcMode::Zero));
        assert_eq!(CrcMode::from_str_opt("sha256"), None);
        assert_eq!(CrcMode::default(), CrcMode::Standard);
    }

    #[test]
    fn each_mode_writes_the_checksum_it_names() {
        let packet = b"SCZN\x00\x01\x00\x01\x00\x00\x00\x00";
        assert_eq!(CrcMode::Standard.checksum(packet), crc32(packet));
        assert_eq!(CrcMode::Castagnoli.checksum(packet), crc32c(packet));
        assert_eq!(CrcMode::Zero.checksum(packet), UNCHECKED);
    }
}
