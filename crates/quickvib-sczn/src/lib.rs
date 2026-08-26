//! `m300-device-sim`: an M300 that speaks the vendor's **SCZN** protocol.
//!
//! The M300's transport is a framed request/response protocol over TCP in which the *host* is
//! the server and the *vibrometer* dials in. `docs/M300-NATIVE.md` §6 establishes that when
//! QuickVib runs `--backend m300` it does not own that socket at all — `m300_server_create_ex`
//! binds the port and the SDK's own accept loop runs behind it, delivering samples to a
//! callback after each `0x04` upload. So the only way to exercise that backend without
//! hardware is to be the thing that dials in and speaks SCZN back. That is this crate.
//!
//! # What this is not
//!
//! It is **not** a fake `m300_sdk.dll` (`docs/PLAN.md` D21). No vendor binary is stubbed,
//! wrapped or imitated; this crate opens a socket and writes bytes the vendor documented, and
//! the code it exercises on the far side is the real SDK doing its real job.
//!
//! It is also **not** a replacement for `m300-sim` (`quickvib-sim`), and the two are not
//! interchangeable:
//!
//! | Simulator | Wire | QuickVib backend | Runs on |
//! | --- | --- | --- | --- |
//! | `m300-sim` | bare little-endian `f32`, no framing | `--backend tcp` | anything |
//! | `m300-device-sim` | SCZN frames to the SDK's listener | `--backend m300` | Windows, with the vendor DLL |
//!
//! Pointing one at the other's backend produces a link that connects and then does nothing
//! useful, which is why both spell out what they are in `--help`.
//!
//! # Layout
//!
//! | Module | What lives there |
//! | --- | --- |
//! | [`crc`] | Both CRC variants the specification implies, and the "unchecked" zero it allows |
//! | [`packet`] | The frame: encode, decode, and a resynchronising reader for a TCP stream |
//! | [`params`] | Parameter and status ids, their widths, the code ladders, the hardware blob |
//! | [`device`] | The state machine: one request in, at most one reply out. No I/O |
//! | [`upload`] | The `0x04` payload and the tone that fills it |
//! | [`cli`] | `m300-device-sim`'s command line |
//! | [`runner`] | The socket: dial, serve, redial |
//!
//! Everything except [`runner`] is pure, so the whole command set is unit-tested on any host.
//!
//! ```
//! use quickvib_sczn::{crc::CrcMode, Command, Device, Packet};
//!
//! // What the SDK sends to start a capture, and what a device sends back.
//! let start = Packet::new(Command::START_ACQUISITION, 0x1002, Vec::new());
//! let mut device = Device::new();
//! let reply = device.handle(&start).expect("a start is always answered");
//!
//! assert_eq!(reply.command, Command::START_ACQUISITION_REPLY);
//! assert!(device.is_acquiring());
//!
//! // And then samples flow until the host says stop.
//! let block = device.upload(&[0.0, 1.0, 0.0, -1.0]).expect("acquiring");
//! assert_eq!(block.command, Command::DATA_UPLOAD);
//! assert!(!block.encode(CrcMode::Standard).is_empty());
//! ```

#![forbid(unsafe_code)]

pub mod cli;
pub mod crc;
pub mod device;
pub mod packet;
pub mod params;
pub mod runner;
pub mod upload;

pub use crc::CrcMode;
pub use device::{Device, DeviceStats};
pub use packet::{Command, DecodeError, Decoder, Packet, RESULT_FAIL, RESULT_OK};
pub use params::{DataType, HardwareInfo, ParamId, ParamStore, RunningState, StatusId};
pub use runner::{run, RunSummary, StopReason};
pub use upload::{DataUpload, Endianness, Waveform};
