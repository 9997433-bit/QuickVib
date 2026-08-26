//! The device half of the SCZN conversation: one request in, at most one reply out.
//!
//! Deliberately pure. [`Device::handle`] takes a decoded [`Packet`] and returns the [`Packet`]
//! a real M300 would send back, touching no socket and no clock, so the whole command set —
//! including the state machine that decides whether samples are flowing — is exercised by
//! ordinary unit tests on any host. The transport lives in [`crate::runner`].
//!
//! ```
//! use quickvib_sczn::{Command, Device, Packet, RESULT_OK};
//!
//! let mut device = Device::new();
//! assert!(!device.is_acquiring());
//!
//! let reply = device
//!     .handle(&Packet::new(Command::START_ACQUISITION, 0x1002, Vec::new()))
//!     .expect("the device replies to a start");
//! assert_eq!(reply.command, Command::START_ACQUISITION_REPLY);
//! assert_eq!(reply.command_id, 0x1002, "the reply echoes the request id");
//! assert_eq!(reply.payload, RESULT_OK.to_be_bytes());
//! assert!(device.is_acquiring());
//! ```

use crate::packet::{Command, Packet, RESULT_FAIL, RESULT_OK};
use crate::params::{DataType, ParamId, ParamStore, RunningState, StatusId};
use crate::upload::{encode_into, Endianness};

/// What one device has done since it was created, for the shutdown summary and for tests that
/// want to assert on behaviour rather than on bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceStats {
    /// Requests decoded.
    pub requests: u64,
    /// Replies produced.
    pub replies: u64,
    /// Start-acquisition commands honoured.
    pub starts: u64,
    /// Stop-acquisition commands honoured.
    pub stops: u64,
    /// Parameters written, counted individually rather than per request.
    pub params_written: u64,
    /// DC-removal commands honoured.
    pub dc_removals: u64,
    /// Firmware-upgrade commands refused.
    pub upgrades_refused: u64,
    /// Commands with no entry in the specification's table, which are ignored.
    pub unknown_commands: u64,
    /// Sample blocks handed out by [`Device::upload`].
    pub blocks_uploaded: u64,
    /// Samples across those blocks.
    pub samples_uploaded: u64,
}

/// A simulated M300, as seen from the protocol.
#[derive(Debug, Clone)]
pub struct Device {
    /// The parameters the host can read and write.
    params: ParamStore,
    /// Idle, acquiring, or one of the two states this simulator never enters.
    state: RunningState,
    /// Byte order for the upload prefix.
    prefix: Endianness,
    /// The id the next upload block will carry.
    next_upload_id: u16,
    /// Running tally.
    stats: DeviceStats,
}

impl Device {
    /// A device in its power-on state: idle, with [`ParamStore::default`] loaded.
    #[must_use]
    pub fn new() -> Self {
        Self {
            params: ParamStore::default(),
            state: RunningState::Idle,
            prefix: Endianness::default(),
            next_upload_id: 0x0001,
            stats: DeviceStats::default(),
        }
    }

    /// The same, with a specific parameter set.
    #[must_use]
    pub fn with_params(params: ParamStore) -> Self {
        Self {
            params,
            ..Self::new()
        }
    }

    /// Choose the byte order for the upload payload's prefix words.
    #[must_use]
    pub const fn with_prefix_endianness(mut self, prefix: Endianness) -> Self {
        self.prefix = prefix;
        self
    }

    /// The parameters, for inspection.
    #[must_use]
    pub const fn params(&self) -> &ParamStore {
        &self.params
    }

    /// The parameters, for a caller setting up a device before it dials in.
    pub fn params_mut(&mut self) -> &mut ParamStore {
        &mut self.params
    }

    /// The running state the host would read back under [`StatusId::RUNNING_STATE`].
    #[must_use]
    pub const fn state(&self) -> RunningState {
        self.state
    }

    /// Whether the device is streaming, i.e. whether the runner should be sending `0x04`.
    #[must_use]
    pub fn is_acquiring(&self) -> bool {
        self.state == RunningState::Acquiring
    }

    /// The tally so far.
    #[must_use]
    pub const fn stats(&self) -> DeviceStats {
        self.stats
    }

    /// Which quantity the host has asked for.
    #[must_use]
    pub fn data_type(&self) -> DataType {
        self.params.data_type()
    }

    /// Forget that a link ever started acquiring, as a power cycle or a dropped TCP connection
    /// would. The parameters survive — they live in the device's flash, not in the session.
    pub fn reset_link(&mut self) {
        self.state = RunningState::Idle;
    }

    /// Handle one request, returning the reply to send.
    ///
    /// `None` means "a real device would say nothing": an unknown command, or one of the
    /// device-to-host commands arriving in the wrong direction. The protocol has no generic
    /// negative acknowledgement, so inventing one would be worse than silence.
    pub fn handle(&mut self, request: &Packet) -> Option<Packet> {
        self.stats.requests += 1;
        let id = request.command_id;
        let reply = match request.command {
            Command::START_ACQUISITION => Some(self.start(id)),
            Command::STOP_ACQUISITION => Some(self.stop(id)),
            Command::WRITE_PARAMS => Some(self.write_params(id, &request.payload)),
            Command::READ_PARAMS => Some(self.read_params(id, &request.payload)),
            Command::PARAM_LIST => Some(self.param_list(id)),
            Command::DEVICE_STATUS => Some(self.device_status(id, &request.payload)),
            Command::REMOVE_DC => Some(self.remove_dc(id)),
            // No pulse generator behind this simulator, and the specification's only failure
            // channel is the result field, so it says so rather than pretending.
            Command::START_PULSE => {
                Some(Packet::result(Command::START_PULSE_REPLY, id, RESULT_FAIL))
            }
            Command::STOP_PULSE => Some(Packet::result(Command::STOP_PULSE_REPLY, id, RESULT_FAIL)),
            command if command.is_upgrade() => self.refuse_upgrade(command, id),
            _ => {
                self.stats.unknown_commands += 1;
                None
            }
        };
        if reply.is_some() {
            self.stats.replies += 1;
        }
        reply
    }

    /// `0x00` → `0x01`. Starting an already-started device is a no-op that still succeeds,
    /// mirroring `m300_server_start`, which the vendor documents as safe to call twice.
    fn start(&mut self, id: u16) -> Packet {
        if self.state == RunningState::Idle {
            self.state = RunningState::Acquiring;
            self.next_upload_id = id.wrapping_add(1);
        }
        self.stats.starts += 1;
        Packet::result(Command::START_ACQUISITION_REPLY, id, RESULT_OK)
    }

    /// `0x02` → `0x03`. Stopping an idle device also succeeds: the host asked for a state, and
    /// the device is in it.
    fn stop(&mut self, id: u16) -> Packet {
        self.state = RunningState::Idle;
        self.stats.stops += 1;
        Packet::result(Command::STOP_ACQUISITION_REPLY, id, RESULT_OK)
    }

    /// `0x06` → `0x07`. One result for the whole batch, as the specification defines it: if any
    /// entry is malformed or names a parameter this device does not have, nothing is stored.
    fn write_params(&mut self, id: u16, payload: &[u8]) -> Packet {
        let result = match parse_write_entries(payload) {
            Some(entries) => {
                // Validate the whole batch before storing any of it; a half-applied
                // configuration is the one outcome a caller cannot recover from. A device only
                // accepts writes to parameters it already carries — its table is fixed in
                // firmware, and a host cannot add a row to it.
                let acceptable = entries.iter().all(|(param, value)| {
                    self.params.get(*param).is_some() && param.accepts_width(value.len())
                });
                if acceptable {
                    for (param, value) in &entries {
                        if self.params.set(*param, value).is_ok() {
                            self.stats.params_written += 1;
                        }
                    }
                    RESULT_OK
                } else {
                    RESULT_FAIL
                }
            }
            None => RESULT_FAIL,
        };
        Packet::result(Command::WRITE_PARAMS_REPLY, id, result)
    }

    /// `0x08` → `0x09`. Values for every requested id the device holds; the result field is
    /// non-zero if any requested id was missing, which is the only way the reply's shape lets a
    /// device say "not all of them".
    fn read_params(&mut self, id: u16, payload: &[u8]) -> Packet {
        let Some(requested) = parse_id_list(payload) else {
            return Packet::result(Command::READ_PARAMS_REPLY, id, RESULT_FAIL);
        };
        let mut found: Vec<(ParamId, Vec<u8>)> = Vec::with_capacity(requested.len());
        let mut complete = true;
        for param in requested {
            match self.params.get(ParamId(param)) {
                Some(value) => found.push((ParamId(param), value.to_vec())),
                None => complete = false,
            }
        }

        let mut payload = Vec::new();
        payload.extend_from_slice(&if complete { RESULT_OK } else { RESULT_FAIL }.to_be_bytes());
        payload.extend_from_slice(&(found.len() as u32).to_be_bytes());
        for (param, value) in &found {
            payload.extend_from_slice(&param.0.to_be_bytes());
            payload.extend_from_slice(&(value.len() as u32).to_be_bytes());
            payload.extend_from_slice(value);
        }
        Packet::new(Command::READ_PARAMS_REPLY, id, payload)
    }

    /// `0x0A` → `0x0B`. Every id this device carries, ascending.
    fn param_list(&mut self, id: u16) -> Packet {
        let ids = self.params.ids();
        let mut payload = Vec::with_capacity(6 + ids.len() * 4);
        payload.extend_from_slice(&RESULT_OK.to_be_bytes());
        payload.extend_from_slice(&(ids.len() as u32).to_be_bytes());
        for param in ids {
            payload.extend_from_slice(&param.0.to_be_bytes());
        }
        Packet::new(Command::PARAM_LIST_REPLY, id, payload)
    }

    /// `0x0C` → `0x0D`. Status entries carry **no** length field, so only ids whose width the
    /// specification defines can be answered — an unknown id would leave the reader unable to
    /// find the next entry.
    fn device_status(&mut self, id: u16, payload: &[u8]) -> Packet {
        let Some(requested) = parse_id_list(payload) else {
            return Packet::result(Command::DEVICE_STATUS_REPLY, id, RESULT_FAIL);
        };
        let mut entries: Vec<(StatusId, Vec<u8>)> = Vec::with_capacity(requested.len());
        let mut complete = true;
        for raw in requested {
            let status = StatusId(raw);
            match self.status_value(status) {
                Some(value) => entries.push((status, value)),
                None => complete = false,
            }
        }

        let mut payload = Vec::new();
        payload.extend_from_slice(&if complete { RESULT_OK } else { RESULT_FAIL }.to_be_bytes());
        payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (status, value) in &entries {
            payload.extend_from_slice(&status.0.to_be_bytes());
            payload.extend_from_slice(value);
        }
        Packet::new(Command::DEVICE_STATUS_REPLY, id, payload)
    }

    /// The bytes for one status id, or `None` for an id whose width is undefined.
    ///
    /// The three analogue readings are invented but stable: a simulator has no board to measure,
    /// and a value that drifted would make a test flaky for no gain in fidelity.
    fn status_value(&self, status: StatusId) -> Option<Vec<u8>> {
        match status {
            StatusId::RUNNING_STATE => Some(vec![self.state.code()]),
            StatusId::TEC_NTTC => Some(10_000i32.to_be_bytes().to_vec()),
            StatusId::BOARD_TEMPERATURE => Some(30.125f32.to_be_bytes().to_vec()),
            StatusId::PD_CURRENT => Some(5.5f32.to_be_bytes().to_vec()),
            StatusId::SIGNAL_STRENGTH => Some(0x4000_4000u32.to_be_bytes().to_vec()),
            _ => None,
        }
    }

    /// `0x0E` → `0x0F`. The synthesized tone has no DC component to remove, so this records the
    /// request, flips the DC-removal parameter, and succeeds.
    fn remove_dc(&mut self, id: u16) -> Packet {
        let _ = self.params.set_byte(ParamId::REMOVE_DC_SWITCH, 1);
        self.stats.dc_removals += 1;
        Packet::result(Command::REMOVE_DC_REPLY, id, RESULT_OK)
    }

    /// Refuse a firmware upgrade in the shape the specification gives each step.
    ///
    /// There is no firmware here to replace, and a stand-in that accepted an upgrade would leave
    /// the host waiting for a reboot that never comes. Refusing in the documented reply shape
    /// lets the host's own error handling run, which is the interesting path to exercise.
    fn refuse_upgrade(&mut self, command: Command, id: u16) -> Option<Packet> {
        self.stats.upgrades_refused += 1;
        match command {
            Command::UPGRADE_START => Some(Packet::result(
                Command::UPGRADE_START_REPLY,
                id,
                RESULT_FAIL,
            )),
            Command::UPGRADE_PARAMS => Some(Packet::result(
                Command::UPGRADE_PARAMS_REPLY,
                id,
                RESULT_FAIL,
            )),
            Command::UPGRADE_CHUNK => {
                // The chunk acknowledgement echoes the packet id and length before the result,
                // so a refusal has to carry them too or the host cannot match it up.
                let mut payload = vec![0u8; 10];
                payload[..8].copy_from_slice(&[0u8; 8]);
                payload[8..].copy_from_slice(&RESULT_FAIL.to_be_bytes());
                Some(Packet::new(Command::UPGRADE_CHUNK_REPLY, id, payload))
            }
            // 0xFA and 0xFF are device-to-host; the host never sends them and a device that
            // answered them would be talking to itself.
            _ => None,
        }
    }

    /// Build one `0x04` block from `samples`, or `None` when the device is not acquiring.
    ///
    /// Returning `None` rather than an empty block is deliberate: a device that is not
    /// acquiring sends nothing at all, and a caller that ignored the distinction would put an
    /// empty upload on the wire after every stop.
    pub fn upload(&mut self, samples: &[f32]) -> Option<Packet> {
        let mut payload = Vec::new();
        self.upload_into(samples, &mut payload)?;
        let id = self.next_upload_id;
        self.next_upload_id = self.next_upload_id.wrapping_add(1);
        Some(Packet::new(Command::DATA_UPLOAD, id, payload))
    }

    /// As [`Device::upload`], but encoding into a buffer the caller reuses, and leaving the
    /// command id to [`Device::take_upload_id`]. `None` when the device is not acquiring.
    pub fn upload_into(&mut self, samples: &[f32], out: &mut Vec<u8>) -> Option<()> {
        if !self.is_acquiring() {
            return None;
        }
        encode_into(self.params.data_type(), samples, self.prefix, out);
        self.stats.blocks_uploaded += 1;
        self.stats.samples_uploaded += samples.len() as u64;
        Some(())
    }

    /// Consume the command id for the block just encoded by [`Device::upload_into`].
    ///
    /// Upload ids run on from the start-acquisition request's id, so a host correlating a
    /// stream back to the command that began it can, and two runs of the same script produce
    /// the same ids.
    pub fn take_upload_id(&mut self) -> u16 {
        let id = self.next_upload_id;
        self.next_upload_id = self.next_upload_id.wrapping_add(1);
        id
    }
}

impl Default for Device {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse a `count, [id, length, value]…` write body.
///
/// `None` for anything that does not parse exactly: a partially-read parameter write is how a
/// device ends up configured differently from what the host believes.
fn parse_write_entries(payload: &[u8]) -> Option<Vec<(ParamId, Vec<u8>)>> {
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32()?;
    let mut entries = Vec::with_capacity(count.min(64) as usize);
    for _ in 0..count {
        let id = cursor.u32()?;
        let length = cursor.u32()?;
        let value = cursor.bytes(usize::try_from(length).ok()?)?;
        entries.push((ParamId(id), value.to_vec()));
    }
    cursor.finished().then_some(entries)
}

/// Parse a `count, [id]…` request body, used by both read-parameters and device-status.
fn parse_id_list(payload: &[u8]) -> Option<Vec<u32>> {
    let mut cursor = Cursor::new(payload);
    let count = cursor.u32()?;
    let mut ids = Vec::with_capacity(count.min(64) as usize);
    for _ in 0..count {
        ids.push(cursor.u32()?);
    }
    cursor.finished().then_some(ids)
}

/// A bounds-checked reader over a payload. Every field is big-endian, like the header.
struct Cursor<'a> {
    /// The payload.
    bytes: &'a [u8],
    /// How far in.
    at: usize,
}

impl<'a> Cursor<'a> {
    /// A cursor at the start of `bytes`.
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The next big-endian `u16`, or `None` at the end.
    fn u16(&mut self) -> Option<u16> {
        let word = self.bytes(2)?;
        Some(u16::from_be_bytes([word[0], word[1]]))
    }

    /// The next big-endian `u32`, or `None` at the end.
    fn u32(&mut self) -> Option<u32> {
        let word = self.bytes(4)?;
        Some(u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
    }

    /// The next `len` bytes, or `None` when fewer remain.
    fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(len)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(slice)
    }

    /// Whether the payload was consumed exactly.
    const fn finished(&self) -> bool {
        self.at == self.bytes.len()
    }
}

/// Build the body of a `0x06` write-parameters request. Host-side, so the tests and any bench
/// script can drive a simulated device without hand-assembling bytes.
#[must_use]
pub fn write_params_body(entries: &[(ParamId, Vec<u8>)]) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    for (id, value) in entries {
        payload.extend_from_slice(&id.0.to_be_bytes());
        payload.extend_from_slice(&(value.len() as u32).to_be_bytes());
        payload.extend_from_slice(value);
    }
    payload
}

/// Build the body of a `0x08` read-parameters or `0x0C` device-status request.
#[must_use]
pub fn id_list_body(ids: &[u32]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(4 + ids.len() * 4);
    payload.extend_from_slice(&(ids.len() as u32).to_be_bytes());
    for id in ids {
        payload.extend_from_slice(&id.to_be_bytes());
    }
    payload
}

/// One entry of a decoded `0x09` read-parameters reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamReadback {
    /// Which parameter.
    pub id: ParamId,
    /// Its value.
    pub value: Vec<u8>,
}

/// Decode a `0x09` reply into its result field and entries, for a host-side caller.
///
/// `None` when the payload does not parse.
#[must_use]
pub fn parse_read_params_reply(payload: &[u8]) -> Option<(u16, Vec<ParamReadback>)> {
    let mut cursor = Cursor::new(payload);
    let result = cursor.u16()?;
    let count = cursor.u32()?;
    let mut entries = Vec::with_capacity(count.min(64) as usize);
    for _ in 0..count {
        let id = ParamId(cursor.u32()?);
        let length = usize::try_from(cursor.u32()?).ok()?;
        entries.push(ParamReadback {
            id,
            value: cursor.bytes(length)?.to_vec(),
        });
    }
    cursor.finished().then_some((result, entries))
}

/// Decode a `0x0B` parameter-id-list reply into its result field and ids.
#[must_use]
pub fn parse_param_list_reply(payload: &[u8]) -> Option<(u16, Vec<ParamId>)> {
    let mut cursor = Cursor::new(payload);
    let result = cursor.u16()?;
    let count = cursor.u32()?;
    let mut ids = Vec::with_capacity(count.min(64) as usize);
    for _ in 0..count {
        ids.push(ParamId(cursor.u32()?));
    }
    cursor.finished().then_some((result, ids))
}

/// One entry of a decoded `0x0D` device-status reply, and the reply's result field alongside.
pub type StatusReadback = (u16, Vec<(StatusId, Vec<u8>)>);

/// Decode a `0x0D` device-status reply, using [`StatusId::width`] to find each entry's end.
///
/// `None` when the payload does not parse, which includes a status id whose width this build
/// does not know — there is no length field to skip past it with.
#[must_use]
pub fn parse_device_status_reply(payload: &[u8]) -> Option<StatusReadback> {
    let mut cursor = Cursor::new(payload);
    let result = cursor.u16()?;
    let count = cursor.u32()?;
    let mut entries = Vec::with_capacity(count.min(64) as usize);
    for _ in 0..count {
        let status = StatusId(cursor.u32()?);
        let width = status.width()?;
        entries.push((status, cursor.bytes(width)?.to_vec()));
    }
    cursor.finished().then_some((result, entries))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::params::{HardwareInfo, SAMPLE_RATES_HZ};
    use crate::upload::DataUpload;

    fn request(command: Command, payload: Vec<u8>) -> Packet {
        Packet::new(command, 0x0001, payload)
    }

    #[test]
    fn a_new_device_is_idle_with_its_power_on_parameters() {
        let device = Device::new();
        assert!(!device.is_acquiring());
        assert_eq!(device.state(), RunningState::Idle);
        assert_eq!(device.params().sample_rate_hz(), Some(100_000.0));
        assert_eq!(device.stats(), DeviceStats::default());
        assert_eq!(Device::default().state(), RunningState::Idle);
    }

    #[test]
    fn start_then_stop_walks_the_state_machine_and_replies_to_both() {
        let mut device = Device::new();

        let started = device.handle(&request(Command::START_ACQUISITION, Vec::new()));
        let started = started.unwrap();
        assert_eq!(started.command, Command::START_ACQUISITION_REPLY);
        assert_eq!(started.payload, RESULT_OK.to_be_bytes());
        assert!(device.is_acquiring());
        assert_eq!(device.state(), RunningState::Acquiring);

        let stopped = device
            .handle(&request(Command::STOP_ACQUISITION, Vec::new()))
            .unwrap();
        assert_eq!(stopped.command, Command::STOP_ACQUISITION_REPLY);
        assert_eq!(stopped.payload, RESULT_OK.to_be_bytes());
        assert!(!device.is_acquiring());

        let stats = device.stats();
        assert_eq!((stats.starts, stats.stops, stats.replies), (1, 1, 2));
    }

    #[test]
    fn every_reply_echoes_the_requests_command_id() {
        let mut device = Device::new();
        for (command, payload) in [
            (Command::START_ACQUISITION, Vec::new()),
            (Command::STOP_ACQUISITION, Vec::new()),
            (Command::WRITE_PARAMS, write_params_body(&[])),
            (Command::READ_PARAMS, id_list_body(&[0])),
            (Command::PARAM_LIST, Vec::new()),
            (Command::DEVICE_STATUS, id_list_body(&[0])),
            (Command::REMOVE_DC, Vec::new()),
            (Command::START_PULSE, Vec::new()),
            (Command::STOP_PULSE, Vec::new()),
            (Command::UPGRADE_START, Vec::new()),
        ] {
            let reply = device
                .handle(&Packet::new(command, 0xBEEF, payload))
                .unwrap_or_else(|| panic!("{command} went unanswered"));
            assert_eq!(reply.command_id, 0xBEEF, "for {command}");
        }
    }

    #[test]
    fn starting_twice_is_idempotent_rather_than_an_error() {
        let mut device = Device::new();
        for _ in 0..3 {
            let reply = device
                .handle(&request(Command::START_ACQUISITION, Vec::new()))
                .unwrap();
            assert_eq!(reply.payload, RESULT_OK.to_be_bytes());
        }
        assert!(device.is_acquiring());
    }

    #[test]
    fn stopping_an_idle_device_still_succeeds() {
        let mut device = Device::new();
        let reply = device
            .handle(&request(Command::STOP_ACQUISITION, Vec::new()))
            .unwrap();
        assert_eq!(reply.payload, RESULT_OK.to_be_bytes());
        assert!(!device.is_acquiring());
    }

    #[test]
    fn a_dropped_link_returns_the_device_to_idle_without_losing_parameters() {
        let mut device = Device::new();
        device.handle(&request(
            Command::WRITE_PARAMS,
            write_params_body(&[(ParamId::SAMPLE_RATE, vec![0x02])]),
        ));
        device.handle(&request(Command::START_ACQUISITION, Vec::new()));
        assert!(device.is_acquiring());

        device.reset_link();
        assert!(!device.is_acquiring());
        assert_eq!(device.params().sample_rate_hz(), Some(10_000.0));
    }

    #[test]
    fn a_parameter_write_is_applied_and_reads_back() {
        let mut device = Device::new();
        let body = write_params_body(&[
            (ParamId::SAMPLE_RATE, vec![0x02]),
            (ParamId::DATA_TYPE, vec![DataType::Displacement.code()]),
        ]);
        let reply = device
            .handle(&request(Command::WRITE_PARAMS, body))
            .unwrap();
        assert_eq!(reply.command, Command::WRITE_PARAMS_REPLY);
        assert_eq!(reply.payload, RESULT_OK.to_be_bytes());
        assert_eq!(device.params().sample_rate_hz(), Some(10_000.0));
        assert_eq!(device.data_type(), DataType::Displacement);
        assert_eq!(device.stats().params_written, 2);

        let read = device
            .handle(&request(
                Command::READ_PARAMS,
                id_list_body(&[ParamId::SAMPLE_RATE.0, ParamId::DATA_TYPE.0]),
            ))
            .unwrap();
        assert_eq!(read.command, Command::READ_PARAMS_REPLY);
        let (result, entries) = parse_read_params_reply(&read.payload).unwrap();
        assert_eq!(result, RESULT_OK);
        assert_eq!(
            entries,
            vec![
                ParamReadback {
                    id: ParamId::SAMPLE_RATE,
                    value: vec![0x02]
                },
                ParamReadback {
                    id: ParamId::DATA_TYPE,
                    value: vec![DataType::Displacement.code()]
                },
            ]
        );
    }

    #[test]
    fn a_write_batch_with_one_bad_entry_applies_none_of_it() {
        let mut device = Device::new();
        let before = device.params().clone();
        let body = write_params_body(&[
            (ParamId::SAMPLE_RATE, vec![0x02]),
            (ParamId::DATA_TYPE, vec![0x00, 0x01]), // two bytes for a one-byte parameter
        ]);
        let reply = device
            .handle(&request(Command::WRITE_PARAMS, body))
            .unwrap();
        assert_eq!(reply.payload, RESULT_FAIL.to_be_bytes());
        assert_eq!(device.params(), &before, "nothing was half-applied");
        assert_eq!(device.stats().params_written, 0);
    }

    #[test]
    fn a_write_to_an_unknown_parameter_is_refused() {
        let mut device = Device::new();
        let body = write_params_body(&[(ParamId(0x0BAD_0BAD), vec![0x01])]);
        let reply = device
            .handle(&request(Command::WRITE_PARAMS, body))
            .unwrap();
        assert_eq!(reply.payload, RESULT_FAIL.to_be_bytes());
    }

    #[test]
    fn a_truncated_write_body_is_refused_rather_than_partially_read() {
        let mut device = Device::new();
        let mut body = write_params_body(&[(ParamId::SAMPLE_RATE, vec![0x02])]);
        body.pop();
        let reply = device
            .handle(&request(Command::WRITE_PARAMS, body))
            .unwrap();
        assert_eq!(reply.payload, RESULT_FAIL.to_be_bytes());

        // And a count that promises more entries than the body carries.
        let mut lying = Vec::new();
        lying.extend_from_slice(&7u32.to_be_bytes());
        let reply = device
            .handle(&request(Command::WRITE_PARAMS, lying))
            .unwrap();
        assert_eq!(reply.payload, RESULT_FAIL.to_be_bytes());
    }

    #[test]
    fn reading_a_parameter_the_device_lacks_reports_failure_and_returns_the_rest() {
        let mut device = Device::new();
        let reply = device
            .handle(&request(
                Command::READ_PARAMS,
                id_list_body(&[ParamId::SAMPLE_RATE.0, 0x0BAD_0BAD]),
            ))
            .unwrap();
        let (result, entries) = parse_read_params_reply(&reply.payload).unwrap();
        assert_eq!(result, RESULT_FAIL);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, ParamId::SAMPLE_RATE);
    }

    #[test]
    fn reading_the_hardware_blob_returns_forty_one_decodable_bytes() {
        let mut device = Device::new();
        let reply = device
            .handle(&request(
                Command::READ_PARAMS,
                id_list_body(&[ParamId::HARDWARE_INFO.0]),
            ))
            .unwrap();
        let (result, entries) = parse_read_params_reply(&reply.payload).unwrap();
        assert_eq!(result, RESULT_OK);
        assert_eq!(entries[0].value.len(), HardwareInfo::LEN);
        let info = HardwareInfo::decode(&entries[0].value).unwrap();
        assert_eq!(info.device_type, HardwareInfo::DEVICE_TYPE_M300);
        assert_eq!(info.serial_text(), "SIM-000001");
    }

    #[test]
    fn the_parameter_list_names_every_parameter_that_can_then_be_read() {
        let mut device = Device::new();
        let listed = device
            .handle(&request(Command::PARAM_LIST, Vec::new()))
            .unwrap();
        assert_eq!(listed.command, Command::PARAM_LIST_REPLY);
        let (result, ids) = parse_param_list_reply(&listed.payload).unwrap();
        assert_eq!(result, RESULT_OK);
        assert_eq!(ids.len(), device.params().len());
        assert!(ids.contains(&ParamId::SAMPLE_RATE));
        assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "ascending");

        let raw: Vec<u32> = ids.iter().map(|id| id.0).collect();
        let read = device
            .handle(&request(Command::READ_PARAMS, id_list_body(&raw)))
            .unwrap();
        let (result, entries) = parse_read_params_reply(&read.payload).unwrap();
        assert_eq!(result, RESULT_OK, "everything listed must be readable");
        assert_eq!(entries.len(), ids.len());
    }

    #[test]
    fn device_status_reports_the_running_state_and_tracks_it() {
        let mut device = Device::new();
        let body = id_list_body(&[StatusId::RUNNING_STATE.0, StatusId::BOARD_TEMPERATURE.0]);

        let idle = device
            .handle(&request(Command::DEVICE_STATUS, body.clone()))
            .unwrap();
        assert_eq!(idle.command, Command::DEVICE_STATUS_REPLY);
        let (result, entries) = parse_device_status_reply(&idle.payload).unwrap();
        assert_eq!(result, RESULT_OK);
        assert_eq!(entries[0], (StatusId::RUNNING_STATE, vec![0x00]));
        assert_eq!(entries[1].1.len(), 4, "temperature is four bytes");

        device.handle(&request(Command::START_ACQUISITION, Vec::new()));
        let busy = device
            .handle(&request(Command::DEVICE_STATUS, body))
            .unwrap();
        let (_, entries) = parse_device_status_reply(&busy.payload).unwrap();
        assert_eq!(entries[0], (StatusId::RUNNING_STATE, vec![0x01]));
    }

    #[test]
    fn an_unknown_status_id_is_omitted_and_flagged() {
        let mut device = Device::new();
        let reply = device
            .handle(&request(
                Command::DEVICE_STATUS,
                id_list_body(&[StatusId::RUNNING_STATE.0, 0x0BAD_0BAD]),
            ))
            .unwrap();
        let (result, entries) = parse_device_status_reply(&reply.payload).unwrap();
        assert_eq!(result, RESULT_FAIL);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn dc_removal_succeeds_and_flips_the_parameter() {
        let mut device = Device::new();
        assert_eq!(device.params().get_byte(ParamId::REMOVE_DC_SWITCH), Some(0));
        let reply = device
            .handle(&request(Command::REMOVE_DC, Vec::new()))
            .unwrap();
        assert_eq!(reply.command, Command::REMOVE_DC_REPLY);
        assert_eq!(reply.payload, RESULT_OK.to_be_bytes());
        assert_eq!(device.params().get_byte(ParamId::REMOVE_DC_SWITCH), Some(1));
        assert_eq!(device.stats().dc_removals, 1);
    }

    #[test]
    fn firmware_upgrade_is_refused_in_the_documented_reply_shape() {
        let mut device = Device::new();
        for (sent, expected) in [
            (Command::UPGRADE_START, Command::UPGRADE_START_REPLY),
            (Command::UPGRADE_PARAMS, Command::UPGRADE_PARAMS_REPLY),
        ] {
            let reply = device.handle(&request(sent, Vec::new())).unwrap();
            assert_eq!(reply.command, expected);
            assert_eq!(reply.payload, RESULT_FAIL.to_be_bytes(), "for {sent}");
        }

        let chunk = device
            .handle(&request(Command::UPGRADE_CHUNK, vec![0; 1288]))
            .unwrap();
        assert_eq!(chunk.command, Command::UPGRADE_CHUNK_REPLY);
        assert_eq!(chunk.payload.len(), 10, "id, length, then the result");
        assert_eq!(&chunk.payload[8..], &RESULT_FAIL.to_be_bytes());
        assert_eq!(device.stats().upgrades_refused, 3);
    }

    #[test]
    fn a_device_to_host_upgrade_command_gets_no_reply() {
        let mut device = Device::new();
        for command in [Command(0xFA), Command(0xFF)] {
            assert_eq!(device.handle(&request(command, Vec::new())), None);
        }
        assert_eq!(device.stats().replies, 0);
    }

    #[test]
    fn pulse_output_is_refused_because_this_simulator_has_no_pulse_generator() {
        let mut device = Device::new();
        for (sent, expected) in [
            (Command::START_PULSE, Command::START_PULSE_REPLY),
            (Command::STOP_PULSE, Command::STOP_PULSE_REPLY),
        ] {
            let reply = device.handle(&request(sent, vec![0; 12])).unwrap();
            assert_eq!(reply.command, expected);
            assert_eq!(reply.payload, RESULT_FAIL.to_be_bytes());
        }
    }

    #[test]
    fn an_unknown_command_is_ignored_rather_than_answered_with_an_invented_error() {
        let mut device = Device::new();
        for command in [Command(0x10), Command(0x55), Command(0xC0)] {
            assert_eq!(device.handle(&request(command, Vec::new())), None);
        }
        assert_eq!(device.stats().unknown_commands, 3);
        assert_eq!(device.stats().replies, 0);
    }

    #[test]
    fn a_device_to_host_command_arriving_from_the_host_is_ignored() {
        let mut device = Device::new();
        for command in [
            Command::START_ACQUISITION_REPLY,
            Command::DATA_UPLOAD,
            Command::READ_PARAMS_REPLY,
        ] {
            assert_eq!(device.handle(&request(command, Vec::new())), None);
        }
    }

    #[test]
    fn an_idle_device_uploads_nothing() {
        let mut device = Device::new();
        assert_eq!(device.upload(&[1.0, 2.0]), None);
        assert_eq!(device.stats().blocks_uploaded, 0);
    }

    #[test]
    fn an_acquiring_device_uploads_blocks_the_host_can_decode() {
        let mut device = Device::new();
        device.handle(&Packet::new(Command::START_ACQUISITION, 0x1002, Vec::new()));

        let samples = [0.0, 1.5, -1.5, 250.0];
        let block = device.upload(&samples).unwrap();
        assert_eq!(block.command, Command::DATA_UPLOAD);
        assert_eq!(
            block.command_id, 0x1003,
            "ids run on from the start command"
        );

        let decoded = DataUpload::decode(&block.payload, Endianness::Big).unwrap();
        assert_eq!(decoded.data_type, u32::from(DataType::Velocity.code()));
        assert_eq!(decoded.samples, samples);

        let next = device.upload(&samples).unwrap();
        assert_eq!(next.command_id, 0x1004);
        assert_eq!(device.stats().blocks_uploaded, 2);
        assert_eq!(device.stats().samples_uploaded, 8);
    }

    #[test]
    fn the_upload_prefix_order_is_the_one_the_device_was_built_with() {
        let mut device = Device::new().with_prefix_endianness(Endianness::Little);
        device.handle(&request(Command::START_ACQUISITION, Vec::new()));
        let block = device.upload(&[1.0]).unwrap();
        assert_eq!(&block.payload[4..8], &1u32.to_le_bytes());
    }

    #[test]
    fn uploads_carry_whatever_data_type_the_host_last_wrote() {
        let mut device = Device::new();
        device.handle(&request(
            Command::WRITE_PARAMS,
            write_params_body(&[(ParamId::DATA_TYPE, vec![DataType::Acceleration.code()])]),
        ));
        device.handle(&request(Command::START_ACQUISITION, Vec::new()));
        let block = device.upload(&[1.0]).unwrap();
        let decoded = DataUpload::decode(&block.payload, Endianness::Big).unwrap();
        assert_eq!(decoded.data_type, u32::from(DataType::Acceleration.code()));
    }

    #[test]
    fn stopping_mid_stream_ends_the_uploads_immediately() {
        let mut device = Device::new();
        device.handle(&request(Command::START_ACQUISITION, Vec::new()));
        assert!(device.upload(&[1.0]).is_some());
        device.handle(&request(Command::STOP_ACQUISITION, Vec::new()));
        assert!(device.upload(&[1.0]).is_none());
    }

    #[test]
    fn upload_ids_wrap_rather_than_overflow() {
        let mut device = Device::new();
        device.handle(&Packet::new(Command::START_ACQUISITION, 0xFFFF, Vec::new()));
        assert_eq!(device.take_upload_id(), 0x0000);
        assert_eq!(device.take_upload_id(), 0x0001);
    }

    #[test]
    fn a_device_can_be_built_with_a_non_default_parameter_set() {
        let mut params = ParamStore::default();
        params.set_byte(ParamId::SAMPLE_RATE, 0x0E).unwrap();
        let device = Device::with_params(params);
        assert_eq!(
            device.params().sample_rate_hz(),
            Some(SAMPLE_RATES_HZ[0x0E])
        );
    }

    #[test]
    fn host_side_body_builders_round_trip_through_the_device() {
        // The builders exist so tests and bench scripts do not hand-assemble bytes; if they
        // and the parsers ever disagree, every other test here is testing the wrong thing.
        let entries = vec![(ParamId::SAMPLE_RATE, vec![0x03u8])];
        let body = write_params_body(&entries);
        assert_eq!(parse_write_entries(&body), Some(entries));
        assert_eq!(
            parse_id_list(&id_list_body(&[1, 2, 3])),
            Some(vec![1, 2, 3])
        );
        assert_eq!(parse_id_list(&[]), None);
        assert_eq!(parse_read_params_reply(&[0x00]), None);
        assert_eq!(parse_param_list_reply(&[0x00]), None);
        assert_eq!(parse_device_status_reply(&[0x00]), None);
    }
}
