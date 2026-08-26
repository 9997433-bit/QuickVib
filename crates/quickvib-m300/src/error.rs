//! Native status codes on one side, [`DeviceError`] and the SCPI catalogue on the other
//! (`docs/M300-NATIVE.md` §5).
//!
//! Every synchronous SDK command returns **two** independent verdicts: the `M300Result` the
//! transport layer produced, and a `uint16_t result_code` the device itself wrote into an
//! out-parameter. Checking only the return value silently accepts a device-side refusal, so
//! [`check`] takes both and neither can be forgotten — the `result_code` is a required argument,
//! not an option a call site can leave off.
//!
//! The fallback is fixed: any non-zero code with no row of its own becomes `-240,"Hardware
//! error"` carrying its numeric value, so a code the vendor adds in a later SDK cannot be
//! swallowed. There is no `m300_get_last_error` in this SDK — the numbers below are the entire
//! diagnostic surface, which is why both of them always reach the log verbatim.

use std::fmt;

use quickvib_core::ScpiError;
use quickvib_device::DeviceError;

use crate::ffi_generated::{
    M300Result, M300Result_M300_RESULT_ERR_BAD_HEADER, M300Result_M300_RESULT_ERR_BAD_PACKET,
    M300Result_M300_RESULT_ERR_BUFFER_SMALL, M300Result_M300_RESULT_ERR_CRC_MISMATCH,
    M300Result_M300_RESULT_ERR_DISCONNECTED, M300Result_M300_RESULT_ERR_INVALID_ARG,
    M300Result_M300_RESULT_ERR_NETWORK, M300Result_M300_RESULT_ERR_TIMEOUT,
    M300Result_M300_RESULT_SUCCESS,
};

// The generated snapshot spells these `M300Result_M300_RESULT_*`, which is what `bindgen` makes
// of a C enum and is neither readable nor matchable without a lint exception. Re-exported under
// the vendor's own names, from the snapshot rather than from a second transcription, so a
// regenerated snapshot that renumbers one of them stops compiling here.
/// `M300_SUCCESS`.
pub const RESULT_SUCCESS: M300Result = M300Result_M300_RESULT_SUCCESS;
/// `M300_ERR_INVALID_ARG` — null pointer or illegal value.
pub const RESULT_ERR_INVALID_ARG: M300Result = M300Result_M300_RESULT_ERR_INVALID_ARG;
/// `M300_ERR_BUFFER_SMALL` — our out-buffer was too small.
pub const RESULT_ERR_BUFFER_SMALL: M300Result = M300Result_M300_RESULT_ERR_BUFFER_SMALL;
/// `M300_ERR_BAD_HEADER` — the reply header was not `"SCZN"`.
pub const RESULT_ERR_BAD_HEADER: M300Result = M300Result_M300_RESULT_ERR_BAD_HEADER;
/// `M300_ERR_CRC_MISMATCH` — CRC32 failed.
pub const RESULT_ERR_CRC_MISMATCH: M300Result = M300Result_M300_RESULT_ERR_CRC_MISMATCH;
/// `M300_ERR_BAD_PACKET` — malformed or short packet.
pub const RESULT_ERR_BAD_PACKET: M300Result = M300Result_M300_RESULT_ERR_BAD_PACKET;
/// `M300_ERR_TIMEOUT` — no reply within `timeout_ms`.
pub const RESULT_ERR_TIMEOUT: M300Result = M300Result_M300_RESULT_ERR_TIMEOUT;
/// `M300_ERR_NETWORK` — socket, bind or send failed.
pub const RESULT_ERR_NETWORK: M300Result = M300Result_M300_RESULT_ERR_NETWORK;
/// `M300_ERR_DISCONNECTED` — device not connected, or the link dropped.
pub const RESULT_ERR_DISCONNECTED: M300Result = M300Result_M300_RESULT_ERR_DISCONNECTED;

/// The device executed the command (`M300_OK`).
pub const DEVICE_OK: u16 = 0;

/// The device refused the command (`M300_FAIL`).
pub const DEVICE_FAIL: u16 = 1;

/// Per-command timeout the vendor's own samples and documentation use everywhere.
pub const DEFAULT_TIMEOUT_MS: u32 = 3_000;

/// Which of the two verdicts failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeFailure {
    /// The SDK's own `M300Result`: the command did not reach the device, or its reply did not
    /// come back intact.
    Transport(M300Result),
    /// The device answered, and refused. The value is the `result_code` out-parameter.
    Device(u16),
}

/// A failed SDK call, with everything the log needs to be actionable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeError {
    /// The native entry point that failed, for the log line.
    pub command: &'static str,
    /// Which verdict failed, and with what value.
    pub failure: NativeFailure,
}

impl NativeError {
    /// A transport-layer failure: `m300_<verb>` returned a non-zero `M300Result`.
    #[must_use]
    pub const fn transport(command: &'static str, code: M300Result) -> Self {
        Self {
            command,
            failure: NativeFailure::Transport(code),
        }
    }

    /// A device-side refusal: the call succeeded but `result_code` was not `M300_OK`.
    #[must_use]
    pub const fn device(command: &'static str, result_code: u16) -> Self {
        Self {
            command,
            failure: NativeFailure::Device(result_code),
        }
    }

    /// The SCPI-99 error the UTS should see, per the table in `docs/M300-NATIVE.md` §5.
    #[must_use]
    pub const fn scpi_error(&self) -> ScpiError {
        match self.failure {
            NativeFailure::Transport(code) => scpi_error_for_result(code),
            // A device that answers "no" is a hardware error whatever it was asked; the command
            // name and the raw value go into the log so it is clear which "no" it was.
            NativeFailure::Device(_) => ScpiError::HardwareError,
        }
    }

    /// The numeric value that failed, as it will appear in the log.
    #[must_use]
    pub const fn code(&self) -> i32 {
        match self.failure {
            NativeFailure::Transport(code) => code,
            NativeFailure::Device(result_code) => result_code as i32,
        }
    }
}

impl fmt::Display for NativeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.failure {
            NativeFailure::Transport(code) => write!(
                f,
                "{}() returned {code} ({})",
                self.command,
                result_name(code)
            ),
            NativeFailure::Device(result_code) => write!(
                f,
                "the device refused {}(): result_code={result_code:#06x}",
                self.command
            ),
        }
    }
}

impl std::error::Error for NativeError {}

impl From<NativeError> for DeviceError {
    fn from(error: NativeError) -> Self {
        let detail = error.to_string();
        match error.scpi_error() {
            ScpiError::HardwareMissing => Self::NotConnected,
            ScpiError::TimeoutError => Self::timeout(detail),
            ScpiError::IllegalParameterValue => Self::illegal_parameter(detail),
            // The fallback of docs/M300-NATIVE.md 5: anything without a row of its own is a
            // hardware error carrying its numeric value.
            _ => Self::Native {
                code: error.code(),
                message: detail,
            },
        }
    }
}

/// Both verdicts of one SDK call, checked together.
///
/// # Errors
/// [`NativeError`] when the transport code is non-zero, or when it is zero and the device wrote
/// anything other than [`DEVICE_OK`]. The transport verdict is checked first: when a command
/// never reached the device, whatever is in `result_code` is uninitialised noise.
pub fn check(command: &'static str, code: M300Result, result_code: u16) -> Result<(), NativeError> {
    check_result(command, code)?;
    if result_code == DEVICE_OK {
        Ok(())
    } else {
        Err(NativeError::device(command, result_code))
    }
}

/// The transport verdict alone, for the handful of calls that have no `result_code`
/// out-parameter (`m300_init`, `m300_server_create_ex`, `m300_server_start`).
///
/// # Errors
/// [`NativeError`] when `code` is non-zero.
pub fn check_result(command: &'static str, code: M300Result) -> Result<(), NativeError> {
    if code == RESULT_SUCCESS {
        Ok(())
    } else {
        Err(NativeError::transport(command, code))
    }
}

/// The SCPI-99 error one `M300Result` maps to (`docs/M300-NATIVE.md` §5).
///
/// **Not** for `M300NcResult`: the net-configuration family uses the same numbers with `-6` and
/// `-7` swapped, deliberately, to stay byte-compatible with the legacy C SDK. Mapping those
/// codes through here would report a timeout as a network error and vice versa. QuickVib binds
/// no `m300_nc_*` entry point, and this comment is why that must stay true.
#[must_use]
pub const fn scpi_error_for_result(code: M300Result) -> ScpiError {
    match code {
        RESULT_SUCCESS => ScpiError::NoError,
        // A null pointer or an out-of-range enum index: QuickVib asked for something illegal.
        RESULT_ERR_INVALID_ARG => ScpiError::IllegalParameterValue,
        RESULT_ERR_TIMEOUT => ScpiError::TimeoutError,
        // The link is down: the same answer the UTS gets when nothing has dialled in at all.
        RESULT_ERR_DISCONNECTED => ScpiError::HardwareMissing,
        _ => ScpiError::HardwareError,
    }
}

/// The vendor's own name for a result code, for the log line.
#[must_use]
pub const fn result_name(code: M300Result) -> &'static str {
    match code {
        RESULT_SUCCESS => "M300_SUCCESS",
        RESULT_ERR_INVALID_ARG => "M300_ERR_INVALID_ARG",
        RESULT_ERR_BUFFER_SMALL => "M300_ERR_BUFFER_SMALL",
        RESULT_ERR_BAD_HEADER => "M300_ERR_BAD_HEADER",
        RESULT_ERR_CRC_MISMATCH => "M300_ERR_CRC_MISMATCH",
        RESULT_ERR_BAD_PACKET => "M300_ERR_BAD_PACKET",
        RESULT_ERR_TIMEOUT => "M300_ERR_TIMEOUT",
        RESULT_ERR_NETWORK => "M300_ERR_NETWORK",
        RESULT_ERR_DISCONNECTED => "M300_ERR_DISCONNECTED",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn success_on_both_verdicts_is_success() {
        assert!(check("m300_set_sample_rate", 0, DEVICE_OK).is_ok());
        assert!(check_result("m300_init", 0).is_ok());
    }

    #[test]
    fn the_mapping_matches_the_abi_contract_table() {
        let expected = [
            (0, ScpiError::NoError),
            (-1, ScpiError::IllegalParameterValue),
            (-2, ScpiError::HardwareError),
            (-3, ScpiError::HardwareError),
            (-4, ScpiError::HardwareError),
            (-5, ScpiError::HardwareError),
            (-6, ScpiError::TimeoutError),
            (-7, ScpiError::HardwareError),
            (-8, ScpiError::HardwareMissing),
        ];
        for (code, scpi) in expected {
            assert_eq!(scpi_error_for_result(code), scpi, "code {code}");
        }
    }

    #[test]
    fn the_contract_table_maps_onto_the_scpi_codes_the_document_names() {
        assert_eq!(scpi_error_for_result(-1).code(), -224);
        assert_eq!(scpi_error_for_result(-6).code(), -365);
        assert_eq!(scpi_error_for_result(-8).code(), -241);
        assert_eq!(scpi_error_for_result(-7).code(), -240);
    }

    #[test]
    fn an_unrecognised_code_falls_back_to_hardware_error_and_keeps_its_value() {
        let error = check("m300_start_acquisition", -99, DEVICE_OK).unwrap_err();
        assert_eq!(error.scpi_error(), ScpiError::HardwareError);
        assert_eq!(error.code(), -99);
        assert!(error.to_string().contains("-99"), "{error}");
        assert!(error.to_string().contains("unknown"), "{error}");
    }

    #[test]
    fn a_device_refusal_is_not_swallowed_by_a_successful_return() {
        let error = check("m300_set_low_pass_filter", 0, DEVICE_FAIL).unwrap_err();
        assert_eq!(error.failure, NativeFailure::Device(DEVICE_FAIL));
        assert_eq!(error.scpi_error(), ScpiError::HardwareError);
        assert!(
            error.to_string().contains("m300_set_low_pass_filter"),
            "the log line has to name the command: {error}"
        );
    }

    #[test]
    fn the_transport_verdict_is_checked_before_the_device_one() {
        // `result_code` is only meaningful once a reply actually came back; a transport failure
        // leaves whatever was in the caller's variable.
        let error = check("m300_get_sample_rate", -6, 0xBEEF).unwrap_err();
        assert_eq!(error.failure, NativeFailure::Transport(-6));
    }

    #[test]
    fn native_errors_become_the_device_error_the_scpi_layer_expects() {
        let timeout: DeviceError = NativeError::transport("m300_get_data_type", -6).into();
        assert_eq!(timeout.scpi_error(), ScpiError::TimeoutError);

        let missing: DeviceError = NativeError::transport("m300_start_acquisition", -8).into();
        assert!(matches!(missing, DeviceError::NotConnected));
        assert_eq!(missing.scpi_error(), ScpiError::HardwareMissing);

        let illegal: DeviceError = NativeError::transport("m300_set_sample_rate", -1).into();
        assert_eq!(illegal.scpi_error(), ScpiError::IllegalParameterValue);

        let hardware: DeviceError = NativeError::transport("m300_server_start", -7).into();
        assert_eq!(hardware.scpi_error(), ScpiError::HardwareError);
        assert!(matches!(hardware, DeviceError::Native { code: -7, .. }));

        let refused: DeviceError = NativeError::device("m300_set_data_type", DEVICE_FAIL).into();
        assert_eq!(refused.scpi_error(), ScpiError::HardwareError);
    }

    #[test]
    fn every_failure_keeps_its_numbers_in_the_message() {
        let error: DeviceError = NativeError::transport("m300_get_hardware_info", -4).into();
        let rendered = error.to_string();
        assert!(rendered.contains("m300_get_hardware_info"), "{rendered}");
        assert!(rendered.contains("-4"), "{rendered}");
        assert!(rendered.contains("M300_ERR_CRC_MISMATCH"), "{rendered}");
    }

    #[test]
    fn every_documented_code_has_a_vendor_name() {
        for code in -8..=0 {
            assert_ne!(result_name(code), "unknown", "code {code}");
        }
        assert_eq!(result_name(-9), "unknown");
    }
}
