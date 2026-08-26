//! The `libloading` symbol table: the one place in QuickVib that calls the vendor DLL.
//!
//! Windows only, and the only module in the workspace that dereferences a raw pointer. Every
//! `unsafe` block below names the row of `docs/M300-NATIVE.md` §2 it depends on, because Rust
//! FFI has no marshalling layer: a wrong signature or a wrong ownership assumption is undefined
//! behavior — silent memory corruption, or a plausible-looking wrong measurement — not an
//! exception anybody can catch (§20).
//!
//! Three things shape this module.
//!
//! * **Runtime loading, not link-time import** (D18). The shipped `quickvib.exe` has no import
//!   dependency on `m300_sdk.dll` and starts fine on a bench without it; the vendor ships
//!   `m300_sdk.dll.lib` and we deliberately do not use it. That also makes the two entry points
//!   no header declares (§3.1) cost nothing structural — an undeclared export resolves exactly
//!   like a declared one.
//! * **Two verdicts per call.** Every synchronous command returns an `M300Result` *and* writes a
//!   `uint16_t result_code`; [`crate::error::check`] takes both, so the wrappers here cannot
//!   accidentally accept a device-side refusal (§5).
//! * **A missing library is an error code, not a crash** (§15.6). Both failure modes —
//!   nothing loadable on any probed path, and a loadable DLL that lacks a symbol — become
//!   [`LoadError`], which maps to `-241,"Hardware missing"` and names either every path tried or
//!   the missing symbol.
//!
//! The signatures come from the reviewed `bindgen` snapshot in [`crate::ffi_generated`], whose
//! types this module reuses rather than restating. They are *types* only: nothing here
//! references the snapshot's `extern "C"` declarations, so no symbol is linked and the crate
//! still builds on a machine that has never seen the SDK.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::path::{Path, PathBuf};

use libloading::Library;
use quickvib_core::{SampleUnit, ScpiError};
use quickvib_device::DeviceError;

use crate::error::{
    check, check_result, NativeError, DEVICE_OK, RESULT_ERR_INVALID_ARG, RESULT_ERR_NETWORK,
};
use crate::ffi_generated::{
    CConnectCallback, CDataCallback, CDisconnectCallback, M300Device, M300HardwareInfo, M300Result,
    M300Server,
};
use crate::resolver::{self, Attempt, Candidate, CandidateOrigin, ProbeInputs, ResolveError};

/// SDK major version this crate is written against.
pub const EXPECTED_MAJOR: i32 = 1;

/// SDK minor version this crate is written against. The patch level is not checked: the vendor's
/// changelog keeps the ABI fixed across it.
pub const EXPECTED_MINOR: i32 = 2;

/// Largest device count [`Sdk::server_get_devices`] will ask for in one call. One `M300Server`
/// can accept many vibrometers on one port (§6); QuickVib records from one, but it still has to
/// see and release the others.
pub const MAX_DEVICES: usize = 8;

/// Bytes handed to `m300_device_get_addr`. The vendor wants at least 16 for a dotted quad; every
/// sample passes 32, so we do too.
const ADDR_BUFFER_BYTES: usize = 32;

// Signatures, transcribed from the types in `ffi_generated` so that a `bindgen` refresh that
// changes one of them stops compiling here rather than corrupting memory on the bench.
type FnResult = unsafe extern "C" fn() -> M300Result;
type FnVoid = unsafe extern "C" fn();
type FnVersion = unsafe extern "C" fn() -> *const c_char;
type FnVersionNumbers = unsafe extern "C" fn(*mut c_int, *mut c_int, *mut c_int);
type FnServerCreateEx = unsafe extern "C" fn(*const c_char, u16, *mut M300Server) -> M300Result;
type FnServerSetConnect = unsafe extern "C" fn(M300Server, CConnectCallback, *mut c_void);
type FnServerSetDisconnect = unsafe extern "C" fn(M300Server, CDisconnectCallback, *mut c_void);
type FnServerStart = unsafe extern "C" fn(M300Server) -> M300Result;
type FnServerVoid = unsafe extern "C" fn(M300Server);
type FnServerGetDevices = unsafe extern "C" fn(M300Server, *mut M300Device, u32) -> u32;
type FnServerCount = unsafe extern "C" fn(M300Server) -> u32;
type FnDeviceIsConnected = unsafe extern "C" fn(M300Device) -> c_int;
type FnDeviceVoid = unsafe extern "C" fn(M300Device);
type FnDeviceSetDataCallback = unsafe extern "C" fn(M300Device, CDataCallback, *mut c_void);
type FnDeviceCommand = unsafe extern "C" fn(M300Device, *mut u16, u32) -> M300Result;
type FnDeviceSetU8 = unsafe extern "C" fn(M300Device, u8, *mut u16, u32) -> M300Result;
type FnDeviceGetU8 = unsafe extern "C" fn(M300Device, *mut u8, *mut u16, u32) -> M300Result;
type FnDeviceGetF32 = unsafe extern "C" fn(M300Device, *mut f32, *mut u16, u32) -> M300Result;
type FnDeviceGetI16Pair =
    unsafe extern "C" fn(M300Device, *mut i16, *mut i16, *mut u16, u32) -> M300Result;
type FnHardwareInfo =
    unsafe extern "C" fn(M300Device, *mut M300HardwareInfo, *mut u16, u32) -> M300Result;
type FnDeviceGetAddr = unsafe extern "C" fn(M300Device, *mut c_char, u32) -> *const c_char;
type FnDeviceGetPort = unsafe extern "C" fn(M300Device) -> u16;

/// Resolve one required export, or fail the whole open with its name.
///
/// SAFETY (both macros): `Library::get` is unsafe because nothing checks the type it is asked
/// for against the type the DLL actually exports — that agreement is the contract of
/// `docs/M300-NATIVE.md` §2 and §3, which every `$ty` here is transcribed from. Dereferencing
/// the returned `Symbol` copies a plain function pointer out of it; the pointer stays valid for
/// as long as the `Library` is loaded, and [`Sdk`] owns that `Library` and drops it last.
macro_rules! required {
    ($library:expr, $path:expr, $ty:ty, $name:literal) => {{
        let symbol: ::libloading::Symbol<'_, $ty> =
            unsafe { $library.get(concat!($name, "\0").as_bytes()) }
                .map_err(|error| LoadError::missing_symbol($path, $name, &error))?;
        *symbol
    }};
}

/// Resolve one export whose absence costs a log line rather than the run.
macro_rules! optional {
    ($library:expr, $ty:ty, $name:literal) => {{
        let symbol: Result<::libloading::Symbol<'_, $ty>, _> =
            unsafe { $library.get(concat!($name, "\0").as_bytes()) };
        symbol.ok().map(|symbol| *symbol)
    }};
}

/// Why the SDK could not be opened. Both variants report `-241,"Hardware missing"`.
#[derive(Debug)]
#[non_exhaustive]
pub enum LoadError {
    /// No probed path held a loadable library. Carries every attempt, in probe order.
    NotFound(ResolveError),
    /// A library loaded but does not export something QuickVib needs.
    MissingSymbol {
        /// The library that was opened.
        path: PathBuf,
        /// The export that is not in it.
        symbol: String,
        /// What the loader said.
        reason: String,
    },
}

impl LoadError {
    fn missing_symbol(path: &Path, symbol: &str, error: &libloading::Error) -> Self {
        Self::MissingSymbol {
            path: path.to_path_buf(),
            symbol: symbol.to_owned(),
            reason: error.to_string(),
        }
    }

    /// The SCPI-99 error the UTS should see: always `-241`.
    #[must_use]
    pub const fn scpi_error(&self) -> ScpiError {
        ScpiError::HardwareMissing
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(error) => write!(f, "{error}"),
            Self::MissingSymbol {
                path,
                symbol,
                reason,
            } => write!(f, "{} does not export {symbol}: {reason}", path.display()),
        }
    }
}

impl std::error::Error for LoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotFound(error) => Some(error),
            Self::MissingSymbol { .. } => None,
        }
    }
}

impl From<LoadError> for DeviceError {
    /// `-241,"Hardware missing"`. The detail — every path probed, or the missing symbol — is far
    /// too useful to lose, so the caller logs [`LoadError`] itself before converting.
    fn from(_error: LoadError) -> Self {
        Self::NotConnected
    }
}

/// The exports QuickVib cannot run without (`docs/M300-NATIVE.md` §3).
struct Symbols {
    init: FnResult,
    cleanup: FnVoid,
    sdk_version: FnVersion,
    sdk_version_numbers: FnVersionNumbers,
    server_create_ex: FnServerCreateEx,
    server_set_connect_callback: FnServerSetConnect,
    server_set_disconnect_callback: FnServerSetDisconnect,
    server_start: FnServerStart,
    server_stop: FnServerVoid,
    server_destroy: FnServerVoid,
    server_device_count: FnServerCount,
    server_get_devices: FnServerGetDevices,
    device_is_connected: FnDeviceIsConnected,
    device_release_handle: FnDeviceVoid,
    device_set_data_callback: FnDeviceSetDataCallback,
    get_hardware_info: FnHardwareInfo,
    get_sample_rate: FnDeviceGetU8,
    get_data_type: FnDeviceGetU8,
    set_sample_rate: FnDeviceSetU8,
    set_data_type: FnDeviceSetU8,
    set_low_pass_filter: FnDeviceSetU8,
    set_velocity_range: FnDeviceSetU8,
    set_displacement_range: FnDeviceSetU8,
    set_acceleration_range: FnDeviceSetU8,
    start_acquisition: FnDeviceCommand,
    stop_acquisition: FnDeviceCommand,
}

/// Exports used for diagnostics only.
///
/// A bench running an older SDK that lacks one of these should lose a log field, not the ability
/// to record, so they are resolved leniently — which is exactly the opposite of the rule for
/// [`Symbols`], where a missing export means QuickVib would be guessing.
struct Diagnostics {
    device_get_addr: Option<FnDeviceGetAddr>,
    device_get_port: Option<FnDeviceGetPort>,
    get_running_status: Option<FnDeviceGetU8>,
    get_board_temp: Option<FnDeviceGetF32>,
    get_signal_strength: Option<FnDeviceGetI16Pair>,
}

/// A loaded `m300_sdk.dll`, with every entry point QuickVib uses resolved.
///
/// Shared between the SCPI threads, the reader thread and the SDK's own callback threads, which
/// the vendor documents as safe: per-device locks, replies matched by `command_id`, and locked
/// callback registration (§8).
///
/// # Handles and safety
///
/// `M300Server` and `M300Device` are opaque pointers into the SDK's own allocations, and nothing
/// in their Rust types says whether one is still alive. Every method that takes one is therefore
/// `unsafe`, with a single shared contract:
///
/// * the handle came from this `Sdk` — a server from [`Sdk::server_create`], a device from the
///   connect callback or [`Sdk::server_get_devices`];
/// * it has not been destroyed by [`Sdk::server_destroy`], released by
///   [`Sdk::device_release_handle`], or invalidated by destroying the server it belongs to; and
/// * for a device, `m300_cleanup` has not been called since it was obtained.
///
/// A handle whose *device* has merely disconnected stays safe to pass: the SDK keeps the
/// allocation alive until it is released, and a command on it fails with `-8
/// M300_ERR_DISCONNECTED` rather than misbehaving (§2). That is the whole reason the handle is
/// refcounted, and it is why [`crate::backend::M300Backend`] can park a superseded handle and
/// release it later from a thread that is allowed to block.
pub struct Sdk {
    symbols: Symbols,
    diagnostics: Diagnostics,
    version: String,
    version_numbers: (i32, i32, i32),
    path: PathBuf,
    origin: CandidateOrigin,
    timeout_ms: u32,
    /// Never read, and load-bearing anyway: it is what keeps the DLL mapped, and therefore what
    /// keeps every function pointer above valid. Declared last on purpose — fields drop in
    /// declaration order, so the library outlives them.
    #[allow(dead_code)]
    library: Library,
}

// The SDK's own threading guarantees (§8) are what make this sound; `Library` is `Send + Sync`
// and a function pointer is `Copy`. Asserted rather than assumed, because losing it would show
// up as a puzzling error message a long way from here.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Sdk>();
};

impl std::fmt::Debug for Sdk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sdk")
            .field("path", &self.path)
            .field("origin", &self.origin)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

impl Sdk {
    /// Probe for the vendor library in the order of `docs/M300-NATIVE.md` §1 and resolve every
    /// entry point QuickVib uses.
    ///
    /// The first candidate that *loads* wins; a library that loads but is missing an export is a
    /// failure of the whole open rather than a reason to try the next path, because a
    /// half-usable SDK on the search path is a configuration mistake worth reporting loudly.
    ///
    /// # Errors
    /// [`LoadError::NotFound`] naming every path tried, or [`LoadError::MissingSymbol`] naming
    /// the export and the library it is missing from. Both map to `-241`.
    pub fn open(inputs: &ProbeInputs<'_>) -> Result<Self, LoadError> {
        let mut attempts: Vec<Attempt> = Vec::new();
        for candidate in resolver::candidates(inputs) {
            // SAFETY: loading a library runs its initialisers, which is why `Library::new` is
            // unsafe. `m300_sdk.dll` is the vendor's own MSVC-built library (§1); its only
            // requirements are the VC++ 2015-2022 redistributable and the UCRT, and it has no
            // side effects beyond its own globals. A path that is not that library fails to
            // load or fails to export the symbols below, and both are handled.
            match unsafe { Library::new(&candidate.path) } {
                Ok(library) => return Self::from_library(library, candidate),
                Err(error) => attempts.push(Attempt {
                    candidate,
                    reason: error.to_string(),
                }),
            }
        }
        Err(LoadError::NotFound(ResolveError::new(attempts)))
    }

    fn from_library(library: Library, candidate: Candidate) -> Result<Self, LoadError> {
        let path = candidate.path.as_path();
        let symbols = Symbols {
            init: required!(library, path, FnResult, "m300_init"),
            cleanup: required!(library, path, FnVoid, "m300_cleanup"),
            sdk_version: required!(library, path, FnVersion, "m300_sdk_version"),
            sdk_version_numbers: required!(
                library,
                path,
                FnVersionNumbers,
                "m300_sdk_version_numbers"
            ),
            server_create_ex: required!(library, path, FnServerCreateEx, "m300_server_create_ex"),
            server_set_connect_callback: required!(
                library,
                path,
                FnServerSetConnect,
                "m300_server_set_connect_callback"
            ),
            server_set_disconnect_callback: required!(
                library,
                path,
                FnServerSetDisconnect,
                "m300_server_set_disconnect_callback"
            ),
            server_start: required!(library, path, FnServerStart, "m300_server_start"),
            server_stop: required!(library, path, FnServerVoid, "m300_server_stop"),
            server_destroy: required!(library, path, FnServerVoid, "m300_server_destroy"),
            server_device_count: required!(
                library,
                path,
                FnServerCount,
                "m300_server_device_count"
            ),
            server_get_devices: required!(
                library,
                path,
                FnServerGetDevices,
                "m300_server_get_devices"
            ),
            device_is_connected: required!(
                library,
                path,
                FnDeviceIsConnected,
                "m300_device_is_connected"
            ),
            device_release_handle: required!(
                library,
                path,
                FnDeviceVoid,
                "m300_device_release_handle"
            ),
            device_set_data_callback: required!(
                library,
                path,
                FnDeviceSetDataCallback,
                "m300_device_set_data_callback"
            ),
            get_hardware_info: required!(library, path, FnHardwareInfo, "m300_get_hardware_info"),
            get_sample_rate: required!(library, path, FnDeviceGetU8, "m300_get_sample_rate"),
            get_data_type: required!(library, path, FnDeviceGetU8, "m300_get_data_type"),
            // The two exports no vendor header declares (§3.1). They are in the export table of
            // both DLLs and in every vendor sample; the prototypes are hand-written from the C,
            // C# and LabVIEW bindings, which agree on `uint8_t`.
            set_sample_rate: required!(library, path, FnDeviceSetU8, "m300_set_sample_rate"),
            set_data_type: required!(library, path, FnDeviceSetU8, "m300_set_data_type"),
            set_low_pass_filter: required!(
                library,
                path,
                FnDeviceSetU8,
                "m300_set_low_pass_filter"
            ),
            set_velocity_range: required!(library, path, FnDeviceSetU8, "m300_set_velocity_range"),
            set_displacement_range: required!(
                library,
                path,
                FnDeviceSetU8,
                "m300_set_displacement_range"
            ),
            set_acceleration_range: required!(
                library,
                path,
                FnDeviceSetU8,
                "m300_set_acceleration_range"
            ),
            start_acquisition: required!(library, path, FnDeviceCommand, "m300_start_acquisition"),
            stop_acquisition: required!(library, path, FnDeviceCommand, "m300_stop_acquisition"),
        };
        let diagnostics = Diagnostics {
            device_get_addr: optional!(library, FnDeviceGetAddr, "m300_device_get_addr"),
            device_get_port: optional!(library, FnDeviceGetPort, "m300_device_get_port"),
            get_running_status: optional!(library, FnDeviceGetU8, "m300_get_running_status"),
            get_board_temp: optional!(library, FnDeviceGetF32, "m300_get_board_temp"),
            get_signal_strength: optional!(library, FnDeviceGetI16Pair, "m300_get_signal_strength"),
        };

        // SAFETY: §2 — `m300_sdk_version` returns a static, NUL-terminated ASCII pointer the
        // caller must not free, and cannot fail. A null pointer is not documented as possible;
        // it is still checked, because reading one would be undefined behavior.
        let version_ptr = unsafe { (symbols.sdk_version)() };
        let version = if version_ptr.is_null() {
            String::from("unknown")
        } else {
            // SAFETY: as above — the pointer is static and NUL-terminated.
            unsafe { CStr::from_ptr(version_ptr) }
                .to_string_lossy()
                .into_owned()
        };

        let (mut major, mut minor, mut patch) = (0, 0, 0);
        // SAFETY: §3 — the three out-parameters are plain `int` writes into locals we own, and
        // any of them may be null (we pass none). The call cannot fail.
        unsafe { (symbols.sdk_version_numbers)(&mut major, &mut minor, &mut patch) };

        Ok(Self {
            symbols,
            diagnostics,
            version,
            version_numbers: (major, minor, patch),
            path: candidate.path,
            origin: candidate.origin,
            timeout_ms: crate::error::DEFAULT_TIMEOUT_MS,
            library,
        })
    }

    /// Use `timeout_ms` for every device command instead of the vendor default.
    #[must_use]
    pub fn with_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// The library the loader actually opened.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Which probe-order entry won.
    #[must_use]
    pub const fn origin(&self) -> CandidateOrigin {
        self.origin
    }

    /// The SDK's own version string, e.g. `"1.2.0"`.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The SDK version as major, minor, patch.
    #[must_use]
    pub const fn version_numbers(&self) -> (i32, i32, i32) {
        self.version_numbers
    }

    /// Whether the loaded SDK is the `1.2.x` this crate was written against.
    ///
    /// A mismatch is logged rather than refused: the ABI is stable across the vendor's patch
    /// releases, and refusing to run because a bench has 1.2.1 installed would be worse than the
    /// risk it avoids. A *major* difference is still worth shouting about, which is what the
    /// caller's log line does.
    #[must_use]
    pub const fn version_is_expected(&self) -> bool {
        let (major, minor, _) = self.version_numbers;
        major == EXPECTED_MAJOR && minor == EXPECTED_MINOR
    }

    /// `m300_init` — optional and refcounted, but called explicitly so a failure is seen here
    /// rather than at the first command.
    ///
    /// # Errors
    /// [`NativeError`] when the SDK reports a non-zero result.
    pub fn init(&self) -> Result<(), NativeError> {
        // SAFETY: §3 — no arguments, no pointers, callable from any thread, idempotent.
        let code = unsafe { (self.symbols.init)() };
        check_result("m300_init", code)
    }

    /// `m300_cleanup` — pairs with [`Sdk::init`]; frees the SDK's globals at refcount zero.
    pub fn cleanup(&self) {
        // SAFETY: §3 — no arguments and no failure mode. Every server and device handle this
        // crate created has already been destroyed or released by the time it is called.
        unsafe { (self.symbols.cleanup)() };
    }

    /// `m300_server_create_ex` — bind and listen for the vibrometer.
    ///
    /// `bind` is the interface to bind to; `None` (or an empty string) binds every interface,
    /// which is what plain `m300_server_create` does.
    ///
    /// # Errors
    /// [`NativeError`] with `-7 M300_ERR_NETWORK` when the bind or listen fails — including when
    /// a second QuickVib is already on the port (§9 step 14), and
    /// [`DeviceError::IllegalParameter`] territory (`-1`) for a bind address the SDK rejects. A
    /// bind string containing an interior NUL cannot be passed to C at all and is reported the
    /// same way.
    pub fn server_create(&self, bind: Option<&str>, port: u16) -> Result<M300Server, NativeError> {
        let bind = bind.filter(|value| !value.is_empty());
        let bind = match bind {
            Some(value) => Some(CString::new(value).map_err(|_| {
                // A bind address with an interior NUL cannot be passed to C at all; the SDK's own
                // code for "illegal value" is the honest way to say so.
                NativeError::transport("m300_server_create_ex", RESULT_ERR_INVALID_ARG)
            })?),
            None => None,
        };
        let bind_ptr = bind
            .as_ref()
            .map_or(std::ptr::null(), |value| value.as_ptr());

        let mut server: M300Server = std::ptr::null_mut();
        // SAFETY: §2 — `bind_addr` is a NUL-terminated ASCII string owned by `bind`, which
        // outlives the call, and null is documented as "bind every interface". `out_server` is a
        // local the SDK writes one pointer into.
        let code = unsafe { (self.symbols.server_create_ex)(bind_ptr, port, &mut server) };
        check_result("m300_server_create_ex", code)?;
        if server.is_null() {
            // Success with a null out-pointer is not something the SDK documents; treating it as
            // a transport failure beats handing a null handle to every later call.
            return Err(NativeError::transport(
                "m300_server_create_ex",
                RESULT_ERR_NETWORK,
            ));
        }
        Ok(server)
    }

    /// `m300_server_set_connect_callback` — must be registered before [`Sdk::server_start`].
    ///
    /// # Safety
    /// `user_data` must stay valid, at a fixed address, until the callback is deregistered or
    /// the server is destroyed; the SDK hands it back to `callback` from its own accept thread
    /// (§7).
    pub unsafe fn set_connect_callback(
        &self,
        server: M300Server,
        callback: CConnectCallback,
        user_data: *mut c_void,
    ) {
        // SAFETY: registration is internally locked (§8); the caller's contract above covers the
        // lifetime of `user_data`.
        unsafe { (self.symbols.server_set_connect_callback)(server, callback, user_data) };
    }

    /// `m300_server_set_disconnect_callback` — must be registered before [`Sdk::server_start`].
    ///
    /// # Safety
    /// As [`Sdk::set_connect_callback`].
    pub unsafe fn set_disconnect_callback(
        &self,
        server: M300Server,
        callback: CDisconnectCallback,
        user_data: *mut c_void,
    ) {
        // SAFETY: as above.
        unsafe { (self.symbols.server_set_disconnect_callback)(server, callback, user_data) };
    }

    /// `m300_server_start` — returns at once; the accept loop runs on an SDK thread.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] when the SDK reports a non-zero result.
    pub unsafe fn server_start(&self, server: M300Server) -> Result<(), NativeError> {
        // SAFETY: §3 — `server` came from `m300_server_create_ex` and has not been destroyed;
        // the call is safe to make twice and from any thread.
        let code = unsafe { (self.symbols.server_start)(server) };
        check_result("m300_server_start", code)
    }

    /// `m300_server_stop` — disconnects every device; the server can be started again.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    pub unsafe fn server_stop(&self, server: M300Server) {
        // SAFETY: the caller's handle contract; §3 — cannot fail.
        unsafe { (self.symbols.server_stop)(server) };
    }

    /// `m300_server_destroy` — null-safe, and invalidates every device handle.
    ///
    /// # Safety
    /// Every handle obtained from this server must already have been released, and none may be
    /// used afterwards (§2).
    pub unsafe fn server_destroy(&self, server: M300Server) {
        // SAFETY: the caller's contract above.
        unsafe { (self.symbols.server_destroy)(server) };
    }

    /// `m300_server_device_count` — a snapshot; a handle can go stale immediately.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    #[must_use]
    pub unsafe fn server_device_count(&self, server: M300Server) -> u32 {
        // SAFETY: the caller's handle contract; §8 — internally locked, cannot fail.
        unsafe { (self.symbols.server_device_count)(server) }
    }

    /// `m300_server_get_devices` — up to [`MAX_DEVICES`] handles, each of which the caller now
    /// owns and must release.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety). Every returned handle must also be given
    /// back with [`Sdk::device_release_handle`], or the SDK leaks the `Arc` it boxed for it (§2).
    #[must_use]
    pub unsafe fn server_get_devices(&self, server: M300Server) -> Vec<M300Device> {
        let mut handles: [M300Device; MAX_DEVICES] = [std::ptr::null_mut(); MAX_DEVICES];
        // SAFETY: the caller's handle contract, and §2 — a caller-allocated, caller-sized
        // out-buffer whose element count is passed alongside it; the SDK writes at most
        // `MAX_DEVICES` entries and returns how many.
        let written = unsafe {
            (self.symbols.server_get_devices)(server, handles.as_mut_ptr(), MAX_DEVICES as u32)
        };
        let written = (written as usize).min(MAX_DEVICES);
        handles[..written]
            .iter()
            .copied()
            .filter(|handle| !handle.is_null())
            .collect()
    }

    /// `m300_device_is_connected`.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    #[must_use]
    pub unsafe fn device_is_connected(&self, device: M300Device) -> bool {
        // SAFETY: the caller's handle contract; §3 — never fails, and a handle whose device has
        // gone away answers `0` rather than misbehaving.
        unsafe { (self.symbols.device_is_connected)(device) != 0 }
    }

    /// `m300_device_release_handle` — required, or the SDK leaks the handle's boxed `Arc`.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety), and `device` must not be used again
    /// afterwards.
    pub unsafe fn device_release_handle(&self, device: M300Device) {
        // SAFETY: the caller's contract above; §2 for why this exists at all.
        unsafe { (self.symbols.device_release_handle)(device) };
    }

    /// `m300_device_set_data_callback`. Passing `None` deregisters.
    ///
    /// # Safety
    /// `user_data` must stay valid, at a fixed address, until the callback is deregistered; the
    /// SDK calls back on that device's own callback thread (§7).
    pub unsafe fn set_data_callback(
        &self,
        device: M300Device,
        callback: CDataCallback,
        user_data: *mut c_void,
    ) {
        // SAFETY: registration is internally locked (§8); the caller's contract above covers the
        // lifetime of `user_data`.
        unsafe { (self.symbols.device_set_data_callback)(device, callback, user_data) };
    }

    /// `m300_get_hardware_info` — the `*IDN?` serial and firmware.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn hardware_info(
        &self,
        device: M300Device,
    ) -> Result<M300HardwareInfo, NativeError> {
        // Written out rather than zeroed through `mem::zeroed`, which would be a second reason
        // for an `unsafe` block: the SDK compresses short replies (`has_fpga_version`), so the
        // fields it skips must start as zeros rather than as whatever was on the stack.
        let mut info = M300HardwareInfo {
            device_type: 0,
            ip: [0; 4],
            subnet: [0; 4],
            gateway: [0; 4],
            mac: [0; 6],
            serial: [0; 10],
            fpga_version: [0; 4],
            bootloader_version: [0; 3],
            app_version: [0; 3],
            server_ip: [0; 4],
            server_port: 0,
            has_fpga_version: 0,
        };
        let mut result_code: u16 = DEVICE_OK;
        // SAFETY: the caller's handle contract, and §2 — `info` is a caller-allocated
        // `#[repr(C)]` struct with the vendor's own field order and natural alignment,
        // `result_code` is a local `uint16_t`, and both outlive the call.
        let code = unsafe {
            (self.symbols.get_hardware_info)(device, &mut info, &mut result_code, self.timeout_ms)
        };
        check("m300_get_hardware_info", code, result_code)?;
        Ok(info)
    }

    /// `m300_get_sample_rate` — the enum index, not hertz (§6.1).
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn sample_rate_index(&self, device: M300Device) -> Result<u8, NativeError> {
        // SAFETY: the caller's handle contract; the out-parameters are this call's own locals.
        unsafe { self.get_u8("m300_get_sample_rate", self.symbols.get_sample_rate, device) }
    }

    /// `m300_get_data_type` — `0` velocity, `1` displacement, `2` acceleration.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn data_type(&self, device: M300Device) -> Result<u8, NativeError> {
        // SAFETY: as above.
        unsafe { self.get_u8("m300_get_data_type", self.symbols.get_data_type, device) }
    }

    /// `m300_set_sample_rate` — one of the two exports no header declares (§3.1).
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn set_sample_rate(&self, device: M300Device, index: u8) -> Result<(), NativeError> {
        // SAFETY: as above; `index` is passed by value.
        unsafe {
            self.set_u8(
                "m300_set_sample_rate",
                self.symbols.set_sample_rate,
                device,
                index,
            )
        }
    }

    /// `m300_set_data_type` — the other export no header declares (§3.1).
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn set_data_type(&self, device: M300Device, value: u8) -> Result<(), NativeError> {
        // SAFETY: as above.
        unsafe {
            self.set_u8(
                "m300_set_data_type",
                self.symbols.set_data_type,
                device,
                value,
            )
        }
    }

    /// `m300_set_low_pass_filter` — the mandatory partner of the sample rate (§6.1).
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn set_low_pass_filter(
        &self,
        device: M300Device,
        index: u8,
    ) -> Result<(), NativeError> {
        // SAFETY: as above.
        unsafe {
            self.set_u8(
                "m300_set_low_pass_filter",
                self.symbols.set_low_pass_filter,
                device,
                index,
            )
        }
    }

    /// The range setter that belongs to `unit`.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn set_range(
        &self,
        device: M300Device,
        unit: SampleUnit,
        index: u8,
    ) -> Result<(), NativeError> {
        let (command, function) = match unit {
            SampleUnit::VelocityUmPerSec => {
                ("m300_set_velocity_range", self.symbols.set_velocity_range)
            }
            SampleUnit::DisplacementUm => (
                "m300_set_displacement_range",
                self.symbols.set_displacement_range,
            ),
            SampleUnit::AccelerationMPerSec2 => (
                "m300_set_acceleration_range",
                self.symbols.set_acceleration_range,
            ),
        };
        // SAFETY: the caller's handle contract; `index` is passed by value.
        unsafe { self.set_u8(command, function, device, index) }
    }

    /// `m300_start_acquisition` — samples then arrive on the data callback until stopped.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety). The device's data callback and its
    /// `user_data` must stay valid until acquisition is stopped and the callback deregistered.
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn start_acquisition(&self, device: M300Device) -> Result<(), NativeError> {
        // SAFETY: the caller's contract above.
        unsafe {
            self.command(
                "m300_start_acquisition",
                self.symbols.start_acquisition,
                device,
            )
        }
    }

    /// `m300_stop_acquisition` — an ordinary per-device command, safe to call from another
    /// thread while a stream is in flight (§8).
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip.
    pub unsafe fn stop_acquisition(&self, device: M300Device) -> Result<(), NativeError> {
        // SAFETY: as above.
        unsafe {
            self.command(
                "m300_stop_acquisition",
                self.symbols.stop_acquisition,
                device,
            )
        }
    }

    /// The device's dotted-quad address, when the SDK exports the accessor.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    #[must_use]
    pub unsafe fn device_address(&self, device: M300Device) -> Option<String> {
        let function = self.diagnostics.device_get_addr?;
        let mut buffer = [0 as c_char; ADDR_BUFFER_BYTES];
        // SAFETY: the caller's handle contract, and §2 — a caller-allocated, caller-sized
        // buffer; the vendor wants at least 16 bytes for a dotted quad and every sample passes
        // 32. The returned pointer aliases the same buffer, so the buffer is what gets read.
        let returned = unsafe { (function)(device, buffer.as_mut_ptr(), ADDR_BUFFER_BYTES as u32) };
        if returned.is_null() {
            return None;
        }
        // Scanned rather than handed to `CStr::from_ptr`: a reply that filled the buffer without
        // a terminator would otherwise be read past its end.
        let bytes: Vec<u8> = buffer.iter().map(|byte| *byte as u8).collect();
        let end = bytes.iter().position(|byte| *byte == 0)?;
        Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    /// The device's port, when the SDK exports the accessor.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    #[must_use]
    pub unsafe fn device_port(&self, device: M300Device) -> Option<u16> {
        let function = self.diagnostics.device_get_port?;
        // SAFETY: the caller's handle contract; §3 — returns a plain integer and cannot fail.
        Some(unsafe { (function)(device) })
    }

    /// `m300_get_running_status` — `0` idle, `1` acquiring, `2` upgrading, `3` error.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip. `Ok(None)` when the SDK does not
    /// export the accessor.
    pub unsafe fn running_status(&self, device: M300Device) -> Result<Option<u8>, NativeError> {
        match self.diagnostics.get_running_status {
            // SAFETY: the caller's handle contract.
            Some(function) => {
                unsafe { self.get_u8("m300_get_running_status", function, device) }.map(Some)
            }
            None => Ok(None),
        }
    }

    /// `m300_get_board_temp`, in degrees Celsius.
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip. `Ok(None)` when the SDK does not
    /// export the accessor.
    pub unsafe fn board_temperature(&self, device: M300Device) -> Result<Option<f32>, NativeError> {
        let Some(function) = self.diagnostics.get_board_temp else {
            return Ok(None);
        };
        let mut value: f32 = 0.0;
        let mut result_code: u16 = DEVICE_OK;
        // SAFETY: the caller's handle contract, and §2 — two caller-owned locals written by the
        // SDK, both outliving the call.
        let code = unsafe { (function)(device, &mut value, &mut result_code, self.timeout_ms) };
        check("m300_get_board_temp", code, result_code)?;
        Ok(Some(value))
    }

    /// `m300_get_signal_strength` — the I and Q amplitudes of the returned laser light.
    ///
    /// The number worth having when a device accepts `m300_start_acquisition` and then delivers
    /// nothing: a weak or unlocked return reads near zero, which distinguishes "aimed at nothing"
    /// from "the SDK is not calling us back".
    ///
    /// # Safety
    /// [Live handle contract](Self#handles-and-safety).
    ///
    /// # Errors
    /// [`NativeError`] from either verdict of the round trip. `Ok(None)` when the SDK does not
    /// export the accessor.
    pub unsafe fn signal_strength(
        &self,
        device: M300Device,
    ) -> Result<Option<(i16, i16)>, NativeError> {
        let Some(function) = self.diagnostics.get_signal_strength else {
            return Ok(None);
        };
        let (mut i, mut q) = (0i16, 0i16);
        let mut result_code: u16 = DEVICE_OK;
        // SAFETY: the caller's handle contract, and §2 — three caller-owned locals written by
        // the SDK, all outliving the call.
        let code = unsafe { (function)(device, &mut i, &mut q, &mut result_code, self.timeout_ms) };
        check("m300_get_signal_strength", code, result_code)?;
        Ok(Some((i, q)))
    }

    /// # Safety
    /// [Live handle contract](Self#handles-and-safety), and `function` must be one of the
    /// `get_*` entry points, which all share this shape.
    unsafe fn get_u8(
        &self,
        command: &'static str,
        function: FnDeviceGetU8,
        device: M300Device,
    ) -> Result<u8, NativeError> {
        let mut value: u8 = 0;
        let mut result_code: u16 = DEVICE_OK;
        // SAFETY: the caller's contract above, and §2 — `value` and `result_code` are
        // caller-owned locals of exactly the widths the vendor declares (`uint8_t`, `uint16_t`),
        // and both outlive the call.
        let code = unsafe { (function)(device, &mut value, &mut result_code, self.timeout_ms) };
        check(command, code, result_code)?;
        Ok(value)
    }

    /// # Safety
    /// [Live handle contract](Self#handles-and-safety), and `function` must be one of the
    /// `set_*` entry points, which all share this shape.
    unsafe fn set_u8(
        &self,
        command: &'static str,
        function: FnDeviceSetU8,
        device: M300Device,
        value: u8,
    ) -> Result<(), NativeError> {
        let mut result_code: u16 = DEVICE_OK;
        // SAFETY: the caller's contract above, and §2 and §3.1 — the value is passed by value as
        // `uint8_t` and `result_code` is a caller-owned local that outlives the call.
        let code = unsafe { (function)(device, value, &mut result_code, self.timeout_ms) };
        check(command, code, result_code)
    }

    /// # Safety
    /// [Live handle contract](Self#handles-and-safety), and `function` must be an argumentless
    /// per-device command.
    unsafe fn command(
        &self,
        command: &'static str,
        function: FnDeviceCommand,
        device: M300Device,
    ) -> Result<(), NativeError> {
        let mut result_code: u16 = DEVICE_OK;
        // SAFETY: the caller's contract above, and §2 — `result_code` is a caller-owned local
        // that outlives the call.
        let code = unsafe { (function)(device, &mut result_code, self.timeout_ms) };
        check(command, code, result_code)
    }
}
