//! The SCZN frame: magic, version, command, command id, length, payload, checksum.
//!
//! ```text
//! 0      4    5    6        8              12                12+N
//! +------+----+----+--------+--------------+------ ... ------+--------+
//! | SCZN | ver| cmd| cmd_id | data_length  |     payload     |  crc32 |
//! +------+----+----+--------+--------------+------ ... ------+--------+
//! ```
//!
//! **Everything outside the payload is big-endian.** The specification never says so in prose,
//! but its only executable statement of the layout is a Python `struct.pack(">4s B B H I")`,
//! and the magic itself is quoted as the number `0x53435A4E` — which is the ASCII of `SCZN`
//! read big-endian. The sibling UDP configuration protocol is little-endian and has a
//! different header (it carries a MAC address), so the two do not constrain each other. See
//! `docs/SCZN-PROTOCOL.md` §2.
//!
//! ```
//! use quickvib_sczn::{crc::CrcMode, Command, Packet};
//!
//! let packet = Packet::new(Command::STOP_ACQUISITION, 0x0001, Vec::new());
//! let bytes = packet.encode(CrcMode::Standard);
//! assert_eq!(&bytes[..4], b"SCZN");
//! assert_eq!(Packet::decode(&bytes).unwrap(), packet);
//! ```

use crate::crc::{self, CrcMode};

/// The four magic bytes, `"SCZN"`, which read big-endian are `0x53435A4E`.
pub const MAGIC: [u8; 4] = *b"SCZN";

/// The only protocol version the specification defines.
pub const VERSION: u8 = 0x00;

/// Bytes before the payload: magic, version, command, command id, data length.
pub const HEADER_LEN: usize = 12;

/// Bytes after the payload: the checksum.
pub const TRAILER_LEN: usize = 4;

/// Longest payload this codec will accept.
///
/// The specification sets no ceiling, but a decoder that trusts a 32-bit length off the wire
/// will happily try to reserve four gigabytes for one corrupt frame. The largest legitimate
/// payload in the whole protocol is a data upload, and even a 64 k-sample block is 256 KiB.
pub const MAX_PAYLOAD_LEN: usize = 4 * 1024 * 1024;

/// A command byte.
///
/// A newtype rather than an enum because the specification's table is open — `0x10` through
/// `0xCF` are simply unassigned, and a simulator that could not *represent* an unknown command
/// could not reply "unsupported" to one either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Command(pub u8);

impl Command {
    /// `0x00` host → device: begin acquiring.
    pub const START_ACQUISITION: Self = Self(0x00);
    /// `0x01` device → host: reply to [`Command::START_ACQUISITION`].
    pub const START_ACQUISITION_REPLY: Self = Self(0x01);
    /// `0x02` host → device: stop acquiring.
    pub const STOP_ACQUISITION: Self = Self(0x02);
    /// `0x03` device → host: reply to [`Command::STOP_ACQUISITION`].
    pub const STOP_ACQUISITION_REPLY: Self = Self(0x03);
    /// `0x04` device → host: one block of samples.
    pub const DATA_UPLOAD: Self = Self(0x04);
    /// `0x06` host → device: write parameters.
    pub const WRITE_PARAMS: Self = Self(0x06);
    /// `0x07` device → host: reply to [`Command::WRITE_PARAMS`].
    pub const WRITE_PARAMS_REPLY: Self = Self(0x07);
    /// `0x08` host → device: read parameters.
    pub const READ_PARAMS: Self = Self(0x08);
    /// `0x09` device → host: reply to [`Command::READ_PARAMS`], values included.
    pub const READ_PARAMS_REPLY: Self = Self(0x09);
    /// `0x0A` host → device: which parameter ids does this device have?
    pub const PARAM_LIST: Self = Self(0x0A);
    /// `0x0B` device → host: reply to [`Command::PARAM_LIST`].
    pub const PARAM_LIST_REPLY: Self = Self(0x0B);
    /// `0x0C` host → device: read device status values.
    pub const DEVICE_STATUS: Self = Self(0x0C);
    /// `0x0D` device → host: reply to [`Command::DEVICE_STATUS`].
    pub const DEVICE_STATUS_REPLY: Self = Self(0x0D);
    /// `0x0E` host → device: remove the DC component.
    pub const REMOVE_DC: Self = Self(0x0E);
    /// `0x0F` device → host: reply to [`Command::REMOVE_DC`].
    pub const REMOVE_DC_REPLY: Self = Self(0x0F);
    /// `0xD0` host → device: start pulse output.
    pub const START_PULSE: Self = Self(0xD0);
    /// `0xD1` device → host: reply to [`Command::START_PULSE`].
    pub const START_PULSE_REPLY: Self = Self(0xD1);
    /// `0xD2` host → device: stop pulse output.
    pub const STOP_PULSE: Self = Self(0xD2);
    /// `0xD3` device → host: reply to [`Command::STOP_PULSE`].
    pub const STOP_PULSE_REPLY: Self = Self(0xD3);
    /// `0xF8` host → device: begin a firmware upgrade.
    pub const UPGRADE_START: Self = Self(0xF8);
    /// `0xF9` device → host: reply to [`Command::UPGRADE_START`].
    pub const UPGRADE_START_REPLY: Self = Self(0xF9);
    /// `0xFB` host → device: firmware parameters.
    pub const UPGRADE_PARAMS: Self = Self(0xFB);
    /// `0xFC` device → host: reply to [`Command::UPGRADE_PARAMS`].
    pub const UPGRADE_PARAMS_REPLY: Self = Self(0xFC);
    /// `0xFD` host → device: one firmware chunk.
    pub const UPGRADE_CHUNK: Self = Self(0xFD);
    /// `0xFE` device → host: reply to [`Command::UPGRADE_CHUNK`].
    pub const UPGRADE_CHUNK_REPLY: Self = Self(0xFE);

    /// Whether this command is part of the firmware-upgrade family (`0xF8`–`0xFF`).
    #[must_use]
    pub const fn is_upgrade(self) -> bool {
        self.0 >= 0xF8
    }

    /// The name the specification gives this command, for log lines.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::START_ACQUISITION => "startAcquisition",
            Self::START_ACQUISITION_REPLY => "startAcquisitionReply",
            Self::STOP_ACQUISITION => "stopAcquisition",
            Self::STOP_ACQUISITION_REPLY => "stopAcquisitionReply",
            Self::DATA_UPLOAD => "dataUpload",
            Self::WRITE_PARAMS => "writeParams",
            Self::WRITE_PARAMS_REPLY => "writeParamsReply",
            Self::READ_PARAMS => "readParams",
            Self::READ_PARAMS_REPLY => "readParamsReply",
            Self::PARAM_LIST => "paramList",
            Self::PARAM_LIST_REPLY => "paramListReply",
            Self::DEVICE_STATUS => "deviceStatus",
            Self::DEVICE_STATUS_REPLY => "deviceStatusReply",
            Self::REMOVE_DC => "removeDc",
            Self::REMOVE_DC_REPLY => "removeDcReply",
            Self::START_PULSE => "startPulse",
            Self::START_PULSE_REPLY => "startPulseReply",
            Self::STOP_PULSE => "stopPulse",
            Self::STOP_PULSE_REPLY => "stopPulseReply",
            Self::UPGRADE_START => "upgradeStart",
            Self::UPGRADE_START_REPLY => "upgradeStartReply",
            Self::UPGRADE_PARAMS => "upgradeParams",
            Self::UPGRADE_PARAMS_REPLY => "upgradeParamsReply",
            Self::UPGRADE_CHUNK => "upgradeChunk",
            Self::UPGRADE_CHUNK_REPLY => "upgradeChunkReply",
            _ => "unknown",
        }
    }
}

impl std::fmt::Display for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}(0x{:02X})", self.name(), self.0)
    }
}

/// The specification's two-byte result field: `0x0000` succeeded, anything else failed.
pub const RESULT_OK: u16 = 0x0000;

/// The only failure code the specification assigns; everything else is "待定" (to be decided).
pub const RESULT_FAIL: u16 = 0x0001;

/// One decoded frame. The checksum is not kept: it is a property of the bytes, not of the
/// message, and re-encoding recomputes it under whichever [`CrcMode`] the sender wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Protocol version. Always [`VERSION`] in practice, but preserved so a mismatch is
    /// visible to the caller rather than silently normalised.
    pub version: u8,
    /// Which command this frame carries.
    pub command: Command,
    /// The host's correlation id. A device reply must echo it unchanged.
    pub command_id: u16,
    /// The data region, exactly `data_length` bytes.
    pub payload: Vec<u8>,
}

impl Packet {
    /// A version-[`VERSION`] frame.
    #[must_use]
    pub const fn new(command: Command, command_id: u16, payload: Vec<u8>) -> Self {
        Self {
            version: VERSION,
            command,
            command_id,
            payload,
        }
    }

    /// A frame whose payload is just the specification's two-byte result field.
    #[must_use]
    pub fn result(command: Command, command_id: u16, result: u16) -> Self {
        Self::new(command, command_id, result.to_be_bytes().to_vec())
    }

    /// Total wire size of this frame.
    #[must_use]
    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + self.payload.len() + TRAILER_LEN
    }

    /// Serialise, computing the trailer under `crc`.
    #[must_use]
    pub fn encode(&self, crc: CrcMode) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.encoded_len());
        self.encode_into(crc, &mut bytes);
        bytes
    }

    /// As [`Packet::encode`], but appending to a buffer the caller reuses.
    pub fn encode_into(&self, crc: CrcMode, out: &mut Vec<u8>) {
        encode_parts_into(
            self.version,
            self.command,
            self.command_id,
            &self.payload,
            crc,
            out,
        );
    }

    /// Decode exactly one frame that fills `bytes`.
    ///
    /// # Errors
    /// [`DecodeError`] when the magic, length or checksum do not hold up, or when `bytes` is
    /// not exactly one frame — use [`Decoder`] for a byte stream.
    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let (packet, consumed) = Self::decode_prefix(bytes)?;
        if consumed != bytes.len() {
            return Err(DecodeError::TrailingBytes {
                extra: bytes.len() - consumed,
            });
        }
        Ok(packet)
    }

    /// Decode the frame at the start of `bytes`, returning it and how many bytes it used.
    ///
    /// # Errors
    /// [`DecodeError::Incomplete`] when more bytes are needed, or a hard error when what is
    /// there cannot be a frame.
    pub fn decode_prefix(bytes: &[u8]) -> Result<(Self, usize), DecodeError> {
        if bytes.len() < HEADER_LEN {
            return Err(DecodeError::Incomplete {
                needed: HEADER_LEN - bytes.len(),
            });
        }
        if bytes[..4] != MAGIC {
            return Err(DecodeError::BadMagic {
                found: [bytes[0], bytes[1], bytes[2], bytes[3]],
            });
        }
        let version = bytes[4];
        let command = Command(bytes[5]);
        let command_id = u16::from_be_bytes([bytes[6], bytes[7]]);
        let declared = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
        let length = usize::try_from(declared).unwrap_or(usize::MAX);
        if length > MAX_PAYLOAD_LEN {
            return Err(DecodeError::PayloadTooLong { declared });
        }

        let total = HEADER_LEN + length + TRAILER_LEN;
        if bytes.len() < total {
            return Err(DecodeError::Incomplete {
                needed: total - bytes.len(),
            });
        }

        let claimed = u32::from_be_bytes([
            bytes[HEADER_LEN + length],
            bytes[HEADER_LEN + length + 1],
            bytes[HEADER_LEN + length + 2],
            bytes[HEADER_LEN + length + 3],
        ]);
        if !crc::accepts(&bytes[..HEADER_LEN + length], claimed) {
            return Err(DecodeError::BadChecksum {
                claimed,
                standard: crc::crc32(&bytes[..HEADER_LEN + length]),
            });
        }

        Ok((
            Self {
                version,
                command,
                command_id,
                payload: bytes[HEADER_LEN..HEADER_LEN + length].to_vec(),
            },
            total,
        ))
    }
}

/// Encode a frame from its parts, appending to `out`.
///
/// The streaming path uses this rather than [`Packet::encode_into`] so it can keep one payload
/// buffer alive across every upload block instead of moving a fresh `Vec` into a [`Packet`] and
/// back out again for each one.
pub fn encode_parts_into(
    version: u8,
    command: Command,
    command_id: u16,
    payload: &[u8],
    crc: CrcMode,
    out: &mut Vec<u8>,
) {
    let start = out.len();
    out.reserve(HEADER_LEN + payload.len() + TRAILER_LEN);
    out.extend_from_slice(&MAGIC);
    out.push(version);
    out.push(command.0);
    out.extend_from_slice(&command_id.to_be_bytes());
    // A payload longer than u32::MAX cannot be built on a 32-bit host and would be a
    // programming error on a 64-bit one; saturating keeps this total rather than panicking in a
    // crate that denies `unwrap`.
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(payload);
    let checksum = crc.checksum(&out[start..]);
    out.extend_from_slice(&checksum.to_be_bytes());
}

/// Why a byte sequence is not a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// More bytes are needed before this can be decided. Not a failure on a stream.
    Incomplete {
        /// How many more bytes the decoder wants before it can make progress.
        needed: usize,
    },
    /// The first four bytes are not `"SCZN"`.
    BadMagic {
        /// What was there instead.
        found: [u8; 4],
    },
    /// The declared payload length exceeds [`MAX_PAYLOAD_LEN`].
    PayloadTooLong {
        /// The length off the wire.
        declared: u32,
    },
    /// The trailer matched neither checksum variant, nor was it the permitted zero.
    BadChecksum {
        /// What the sender wrote.
        claimed: u32,
        /// What standard CRC-32 says it should have been, for the log line.
        standard: u32,
    },
    /// A whole frame was decoded but bytes were left over.
    TrailingBytes {
        /// How many.
        extra: usize,
    },
}

impl DecodeError {
    /// Whether this only means "come back with more bytes".
    #[must_use]
    pub const fn is_incomplete(self) -> bool {
        matches!(self, Self::Incomplete { .. })
    }
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Incomplete { needed } => write!(f, "incomplete frame, {needed} more bytes"),
            Self::BadMagic { found } => write!(
                f,
                "expected magic 'SCZN', found {:02X}{:02X}{:02X}{:02X}",
                found[0], found[1], found[2], found[3]
            ),
            Self::PayloadTooLong { declared } => write!(
                f,
                "declared payload of {declared} bytes exceeds the {MAX_PAYLOAD_LEN}-byte ceiling"
            ),
            Self::BadChecksum { claimed, standard } => write!(
                f,
                "checksum {claimed:#010X} matches neither CRC-32 ({standard:#010X}) nor CRC-32C nor zero"
            ),
            Self::TrailingBytes { extra } => write!(f, "{extra} bytes after the frame"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// A resynchronising frame reader for a TCP byte stream.
///
/// TCP delivers a byte stream, not messages: one `read` may return half a frame or three, and
/// nothing about the socket says where a frame begins. The decoder therefore buffers, hands
/// back whole frames as they complete, and — on a magic mismatch — scans forward to the next
/// plausible start rather than giving up on the connection. A real device that lost framing
/// would have to do the same thing.
#[derive(Debug, Default)]
pub struct Decoder {
    /// Bytes received and not yet consumed by a complete frame.
    buffer: Vec<u8>,
    /// How many bytes have been discarded while hunting for the magic.
    resynchronised: u64,
}

impl Decoder {
    /// An empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Add freshly read bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Bytes buffered and not yet part of a returned frame.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// How many bytes have been skipped to regain framing since this decoder was created.
    #[must_use]
    pub const fn resynchronised(&self) -> u64 {
        self.resynchronised
    }

    /// The next complete frame, `Ok(None)` when more bytes are needed.
    ///
    /// # Errors
    /// [`DecodeError::BadChecksum`] or [`DecodeError::PayloadTooLong`] for a frame that started
    /// with valid magic but cannot be trusted. The offending frame is consumed first, so the
    /// caller may log and carry on.
    pub fn next_packet(&mut self) -> Result<Option<Packet>, DecodeError> {
        loop {
            if self.buffer.len() < HEADER_LEN {
                return Ok(None);
            }
            match Packet::decode_prefix(&self.buffer) {
                Ok((packet, consumed)) => {
                    self.buffer.drain(..consumed);
                    return Ok(Some(packet));
                }
                Err(DecodeError::Incomplete { .. }) => return Ok(None),
                Err(DecodeError::BadMagic { .. }) => {
                    // Drop one byte and look again. Cheap, and the only way back into sync
                    // without assuming where the next frame starts.
                    self.buffer.drain(..1);
                    self.resynchronised += 1;
                }
                Err(other) => {
                    // The magic was right, so the length field is the best guess available for
                    // where this frame ends; skip past the header at minimum so the same bad
                    // frame is not reported forever.
                    let skip = match other {
                        DecodeError::PayloadTooLong { .. } => HEADER_LEN,
                        _ => {
                            let declared = u32::from_be_bytes([
                                self.buffer[8],
                                self.buffer[9],
                                self.buffer[10],
                                self.buffer[11],
                            ]) as usize;
                            (HEADER_LEN + declared + TRAILER_LEN).min(self.buffer.len())
                        }
                    };
                    self.buffer.drain(..skip);
                    self.resynchronised += skip as u64;
                    return Err(other);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn the_header_matches_the_specifications_worked_example_byte_for_byte() {
        // "构建的数据包: 53435A4E0001000100000000A57105A2" — the whole frame, with the Python
        // sample's CRC-32C trailer.
        let packet = Packet::new(Command::START_ACQUISITION_REPLY, 0x0001, Vec::new());
        let bytes = packet.encode(CrcMode::Castagnoli);
        assert_eq!(
            bytes,
            hex("53435A4E0001000100000000A57105A2"),
            "got {}",
            hex_string(&bytes)
        );
    }

    #[test]
    fn the_same_frame_under_the_specifications_c_table() {
        let packet = Packet::new(Command::START_ACQUISITION_REPLY, 0x0001, Vec::new());
        assert_eq!(
            packet.encode(CrcMode::Standard),
            hex("53435A4E00010001000000005CEBDD6D")
        );
        assert_eq!(
            packet.encode(CrcMode::Zero),
            hex("53435A4E000100010000000000000000")
        );
    }

    #[test]
    fn multi_byte_header_fields_are_big_endian() {
        let packet = Packet::new(Command(0x06), 0x1234, vec![0xAA; 3]);
        let bytes = packet.encode(CrcMode::Zero);
        assert_eq!(&bytes[6..8], &[0x12, 0x34], "command id");
        assert_eq!(&bytes[8..12], &[0x00, 0x00, 0x00, 0x03], "data length");
    }

    #[test]
    fn every_frame_round_trips_under_every_checksum_mode() {
        let cases = [
            Packet::new(Command::START_ACQUISITION, 0x0001, Vec::new()),
            Packet::result(Command::STOP_ACQUISITION_REPLY, 0xFFFF, RESULT_OK),
            Packet::new(Command::DATA_UPLOAD, 0x1003, vec![7; 1024]),
            Packet::new(Command(0x7F), 0x0000, vec![0, 255, 128]),
        ];
        for mode in [CrcMode::Standard, CrcMode::Castagnoli, CrcMode::Zero] {
            for packet in &cases {
                let bytes = packet.encode(mode);
                assert_eq!(bytes.len(), packet.encoded_len());
                assert_eq!(
                    &Packet::decode(&bytes).unwrap(),
                    packet,
                    "{mode} {packet:?}"
                );
            }
        }
    }

    #[test]
    fn a_reply_carries_the_two_byte_result_big_endian() {
        let packet = Packet::result(Command::WRITE_PARAMS_REPLY, 0x0001, RESULT_FAIL);
        assert_eq!(packet.payload, vec![0x00, 0x01]);
    }

    #[test]
    fn wrong_magic_is_rejected_rather_than_guessed_at() {
        let mut bytes = Packet::new(Command::REMOVE_DC, 1, Vec::new()).encode(CrcMode::Standard);
        bytes[0] = b'X';
        assert!(matches!(
            Packet::decode(&bytes),
            Err(DecodeError::BadMagic { .. })
        ));
    }

    #[test]
    fn a_corrupt_payload_fails_the_checksum() {
        let mut bytes =
            Packet::new(Command::DATA_UPLOAD, 1, vec![1, 2, 3, 4]).encode(CrcMode::Standard);
        bytes[13] ^= 0xFF;
        assert!(matches!(
            Packet::decode(&bytes),
            Err(DecodeError::BadChecksum { .. })
        ));
    }

    #[test]
    fn a_zero_checksum_is_accepted_because_the_specification_permits_it() {
        let bytes = Packet::new(Command::DATA_UPLOAD, 1, vec![1, 2, 3, 4]).encode(CrcMode::Zero);
        assert_eq!(&bytes[bytes.len() - 4..], &[0, 0, 0, 0]);
        assert!(Packet::decode(&bytes).is_ok());
    }

    #[test]
    fn a_short_read_asks_for_more_rather_than_failing() {
        let bytes = Packet::new(Command::DATA_UPLOAD, 1, vec![9; 40]).encode(CrcMode::Standard);
        for cut in [0, 1, 11, HEADER_LEN, HEADER_LEN + 20, bytes.len() - 1] {
            let error = Packet::decode_prefix(&bytes[..cut]).unwrap_err();
            assert!(error.is_incomplete(), "at {cut}: {error}");
        }
        assert!(Packet::decode_prefix(&bytes).is_ok());
    }

    #[test]
    fn an_absurd_length_is_refused_before_anything_is_allocated() {
        let mut bytes = Packet::new(Command::DATA_UPLOAD, 1, Vec::new()).encode(CrcMode::Zero);
        bytes[8..12].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(matches!(
            Packet::decode(&bytes),
            Err(DecodeError::PayloadTooLong { declared: u32::MAX })
        ));
    }

    #[test]
    fn trailing_bytes_are_an_error_for_the_one_shot_decoder() {
        let mut bytes = Packet::new(Command::REMOVE_DC, 1, Vec::new()).encode(CrcMode::Standard);
        bytes.push(0);
        assert!(matches!(
            Packet::decode(&bytes),
            Err(DecodeError::TrailingBytes { extra: 1 })
        ));
    }

    #[test]
    fn the_stream_decoder_reassembles_frames_split_across_reads() {
        let first = Packet::new(Command::START_ACQUISITION, 0x0001, Vec::new());
        let second = Packet::new(Command::WRITE_PARAMS, 0x0002, vec![1, 2, 3, 4, 5]);
        let mut wire = first.encode(CrcMode::Standard);
        wire.extend_from_slice(&second.encode(CrcMode::Castagnoli));

        let mut decoder = Decoder::new();
        let mut seen = Vec::new();
        for byte in &wire {
            decoder.push(std::slice::from_ref(byte));
            while let Some(packet) = decoder.next_packet().unwrap() {
                seen.push(packet);
            }
        }
        assert_eq!(seen, vec![first, second]);
        assert_eq!(decoder.buffered(), 0);
        assert_eq!(decoder.resynchronised(), 0);
    }

    #[test]
    fn the_stream_decoder_handles_several_frames_in_one_read() {
        let mut wire = Vec::new();
        for id in 0..5u16 {
            wire.extend_from_slice(
                &Packet::new(Command::DATA_UPLOAD, id, vec![id as u8; 8]).encode(CrcMode::Standard),
            );
        }
        let mut decoder = Decoder::new();
        decoder.push(&wire);
        for id in 0..5u16 {
            let packet = decoder.next_packet().unwrap().unwrap();
            assert_eq!(packet.command_id, id);
        }
        assert!(decoder.next_packet().unwrap().is_none());
    }

    #[test]
    fn the_stream_decoder_resynchronises_after_junk() {
        let packet = Packet::new(Command::START_ACQUISITION, 0x0042, Vec::new());
        let mut wire = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00];
        wire.extend_from_slice(&packet.encode(CrcMode::Standard));

        let mut decoder = Decoder::new();
        decoder.push(&wire);
        assert_eq!(decoder.next_packet().unwrap(), Some(packet));
        assert_eq!(decoder.resynchronised(), 5);
    }

    #[test]
    fn the_stream_decoder_survives_a_corrupt_frame_and_keeps_going() {
        let mut corrupt =
            Packet::new(Command::DATA_UPLOAD, 1, vec![1, 2, 3, 4]).encode(CrcMode::Standard);
        corrupt[12] ^= 0xFF;
        let good = Packet::new(Command::STOP_ACQUISITION, 2, Vec::new());

        let mut decoder = Decoder::new();
        decoder.push(&corrupt);
        decoder.push(&good.encode(CrcMode::Standard));
        assert!(matches!(
            decoder.next_packet(),
            Err(DecodeError::BadChecksum { .. })
        ));
        assert_eq!(decoder.next_packet().unwrap(), Some(good));
    }

    #[test]
    fn commands_describe_themselves_for_the_log() {
        assert_eq!(
            Command::START_ACQUISITION.to_string(),
            "startAcquisition(0x00)"
        );
        assert_eq!(Command(0x55).name(), "unknown");
        assert!(Command::UPGRADE_START.is_upgrade());
        assert!(Command(0xFF).is_upgrade());
        assert!(!Command::DATA_UPLOAD.is_upgrade());
        assert!(!Command::STOP_PULSE.is_upgrade());
    }

    fn hex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap_or(0))
            .collect()
    }

    fn hex_string(bytes: &[u8]) -> String {
        use std::fmt::Write as _;
        bytes.iter().fold(String::new(), |mut text, byte| {
            let _ = write!(text, "{byte:02X}");
            text
        })
    }
}
