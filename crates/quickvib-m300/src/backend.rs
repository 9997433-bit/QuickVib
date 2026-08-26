//! [`M300Backend`]: the [`DeviceBackend`] that drives a real vibrometer through the vendor SDK.
//!
//! Windows only, and the only backend that loads anything. Everything platform-neutral about
//! this path — buffering, duration enforcement, measurement, export — lives above the trait in
//! crates Linux CI exercises, and the callback/queue seam below it lives in [`crate::shim`],
//! which Linux CI also exercises. What is left here is the part that genuinely needs the SDK:
//! loading it, bringing its server up, applying the project's configuration, and taking it all
//! down again in an order that cannot leave a callback pointing at freed memory.
//!
//! # Lifecycle
//!
//! | Step | What happens |
//! | --- | --- |
//! | [`DeviceBackend::open`] | Resolve and load the DLL, `m300_init`, `m300_server_create_ex(bind, port)`, register the link callbacks, `m300_server_start`. It does **not** wait for a device: a vibrometer that has not been switched on yet must leave the instrument answering `SYST:DEV:CONN? -> 0` and `INIT -> -241`, not stall startup |
//! | Device dials in | The connect shim records the handle and flips the connection flag, and nothing slow happens on the SDK's accept thread |
//! | First use after that | The project's rate, filter, unit and range are applied, the configuration is read back, and the data callback is registered |
//! | [`DeviceBackend::stream`] | `m300_start_acquisition`, drain the queue until the requested sample count, cancellation, a fault or a stall, then `m300_stop_acquisition` |
//! | [`DeviceBackend::close`] / `Drop` | Deregister every callback, release every handle, stop and destroy the server, `m300_cleanup`, and only then give back the `Arc` the SDK was holding |
//!
//! Configuration is applied from QuickVib's own thread rather than from the connect callback on
//! purpose: the vendor documents that long work on the accept thread stalls accepts
//! (`docs/M300-NATIVE.md` §7), and applying settings is half a dozen blocking round trips.
//!
//! With this backend the **SDK owns the listening socket**, so QuickVib must not bind the device
//! port itself — the two would fight over it and the loser reports `-7 M300_ERR_NETWORK` at open
//! (§6). `--device-port` and `--bind` are the arguments to `m300_server_create_ex`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use quickvib_core::{CancelToken, Clock, Level, Logger, NullLogger, SampleUnit};
use quickvib_device::{
    DeviceBackend, DeviceCapabilities, DeviceError, DeviceOpenOptions, SampleBatch, StreamOutcome,
    StreamRequest, DEFAULT_CHANNEL_SAMPLES,
};

use crate::error::{NativeError, DEFAULT_TIMEOUT_MS};
use crate::ffi::{self, Sdk};
use crate::maps;
use crate::resolver::{self, ProbeInputs};
use crate::shim::{self, Drain, Handle, Shared};

/// Component tag on every log line this backend emits.
const LOG: &str = "device";

/// Model string reported through `*IDN?` field 2.
const MODEL: &str = "M300";

/// Serial reported when the device does not have one. Matches the `tcp` backend, where the same
/// question has the same answer: `*IDN?` falls back to the project's `identity.serialNumber`.
const UNKNOWN_SERIAL: &str = "0";

/// Longest capture this backend claims to sustain, in seconds. The engine's own duration guard
/// is the real limit.
const MAX_RECORD_SECONDS: f64 = 3600.0;

/// How long a started acquisition may deliver nothing before [`StreamOutcome::TimedOut`].
///
/// This is the backend's own stall guard, not the run watchdog: the engine still enforces the
/// project's `timeoutMultiplier` above it. It exists because a device that accepts
/// `m300_start_acquisition` and then goes quiet — laser unlocked, target out of range — would
/// otherwise be indistinguishable from a very slow capture.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(5);

/// What the project asked for, remembered from [`DeviceBackend::open`].
#[derive(Debug, Clone, Copy)]
struct Requested {
    sample_rate_hz: f64,
    unit: SampleUnit,
}

/// The state that exists only between `open` and `close`.
struct Live {
    sdk: Arc<Sdk>,
    server: Handle,
    shared: Arc<Shared>,
    /// The `Arc<Shared>` pointer the SDK holds. Reclaimed in `close`, never before.
    user_data: Handle,
    /// The generation the current device configuration belongs to.
    configured: Option<u64>,
    /// The device the data callback is registered on, so the pairing can be undone.
    callback_device: Option<Handle>,
}

/// A [`DeviceBackend`] backed by the vendor's M300 SDK.
///
/// Constructing one loads nothing and binds nothing: everything happens in
/// [`DeviceBackend::open`], so building one on a machine without the SDK installed is harmless.
pub struct M300Backend {
    clock: Arc<dyn Clock>,
    logger: Arc<dyn Logger>,
    bind_address: Option<String>,
    library_name: Option<String>,
    low_pass_hz: Option<f64>,
    range: Option<f64>,
    timeout_ms: u32,
    stall_timeout: Duration,
    queue_samples: usize,
    requested: Option<Requested>,
    live: Option<Live>,
    stopped: Arc<AtomicBool>,
}

impl std::fmt::Debug for M300Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("M300Backend")
            .field("open", &self.live.is_some())
            .field("connected", &self.is_connected())
            .field("sdk", &self.library_path())
            .finish_non_exhaustive()
    }
}

impl M300Backend {
    /// A backend that has not loaded anything yet.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            clock,
            logger: Arc::new(NullLogger),
            bind_address: None,
            library_name: None,
            low_pass_hz: None,
            range: None,
            timeout_ms: DEFAULT_TIMEOUT_MS,
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            queue_samples: DEFAULT_CHANNEL_SAMPLES,
            requested: None,
            live: None,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Send the resolved DLL path, the SDK version, the applied configuration and every
    /// substitution to `logger`.
    ///
    /// Worth wiring up: a `-241` carries the probe list only in the log, because the SCPI
    /// response cannot hold it (§5).
    #[must_use]
    pub fn with_logger(mut self, logger: Arc<dyn Logger>) -> Self {
        self.logger = logger;
        self
    }

    /// Bind the SDK's listening socket to one interface instead of all of them. This is the
    /// `--bind`/`bindHost` value.
    #[must_use]
    pub fn with_bind_address(mut self, bind: Option<String>) -> Self {
        self.bind_address = bind;
        self
    }

    /// Look for a library other than `m300_sdk.dll` — the vendor's MinGW package names it
    /// `libm300_sdk.dll` (§1).
    #[must_use]
    pub fn with_library_name(mut self, name: Option<String>) -> Self {
        self.library_name = name;
        self
    }

    /// Pin the low-pass cutoff instead of pairing it with the sample rate.
    ///
    /// This is the project's `device.lpfHz`. Left unset, the filter band is chosen to match the
    /// rate, which is the pairing the vendor insists on (§6.1).
    #[must_use]
    pub fn with_low_pass_hz(mut self, hz: Option<f64>) -> Self {
        self.low_pass_hz = hz;
        self
    }

    /// Apply the project's measuring range for the configured unit.
    ///
    /// Only the two ends of each range ladder are documented, so a value between them is logged
    /// and the device keeps its own range rather than being sent a guessed index — see
    /// [`crate::maps::range_index`].
    #[must_use]
    pub fn with_range(mut self, range: Option<f64>) -> Self {
        self.range = range;
        self
    }

    /// Per-command timeout in milliseconds. The vendor's samples and docs use `3000` everywhere.
    #[must_use]
    pub fn with_timeout_ms(mut self, timeout_ms: u32) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// How long an acquisition may deliver nothing before [`StreamOutcome::TimedOut`].
    #[must_use]
    pub fn with_stall_timeout(mut self, timeout: Duration) -> Self {
        self.stall_timeout = timeout;
        self
    }

    /// Capacity of the queue between the SDK's callback threads and the reader thread, in
    /// samples.
    #[must_use]
    pub fn with_queue_samples(mut self, samples: usize) -> Self {
        self.queue_samples = samples;
        self
    }

    /// The resolved library path, once [`DeviceBackend::open`] has found one.
    #[must_use]
    pub fn library_path(&self) -> Option<PathBuf> {
        self.live.as_ref().map(|live| live.sdk.path().to_path_buf())
    }

    /// The SDK's own version string, once the library is loaded.
    #[must_use]
    pub fn sdk_version(&self) -> Option<String> {
        self.live.as_ref().map(|live| live.sdk.version().to_owned())
    }

    /// Samples the queue dropped because the consumer fell behind, since the backend opened.
    #[must_use]
    pub fn dropped_samples(&self) -> u64 {
        self.live
            .as_ref()
            .map_or(0, |live| live.shared.dropped_samples())
    }

    fn log(&self, level: Level, message: impl FnOnce() -> String) {
        if self.logger.enabled(level) {
            self.logger.log(level, LOG, &message());
        }
    }

    /// Load the DLL, bring the SDK's server up, and register the link callbacks.
    fn start(&mut self, options: &DeviceOpenOptions) -> Result<Live, DeviceError> {
        let executable_dir = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(std::path::Path::to_path_buf));
        let env_sdk_path = std::env::var_os(resolver::SDK_PATH_ENV).map(PathBuf::from);
        let inputs = ProbeInputs {
            project_sdk_path: options.sdk_path.as_deref(),
            env_sdk_path: env_sdk_path.as_deref(),
            executable_dir: executable_dir.as_deref(),
            library_name: self.library_name.as_deref(),
        };
        for candidate in resolver::candidates(&inputs) {
            self.log(Level::Debug, || {
                format!(
                    "m300 sdk probe origin=\"{}\" path={}",
                    candidate.origin,
                    candidate.path.display()
                )
            });
        }

        let sdk = match Sdk::open(&inputs) {
            Ok(sdk) => Arc::new(sdk.with_timeout_ms(self.timeout_ms)),
            Err(error) => {
                // The probe list, or the missing symbol, is the whole diagnostic value of a
                // `-241`; the SCPI response cannot carry it, so the log must (§5).
                self.log(Level::Error, || format!("m300 sdk not loaded: {error}"));
                return Err(error.into());
            }
        };
        self.log(Level::Info, || {
            format!(
                "m300 sdk loaded path={} origin=\"{}\" version={}",
                sdk.path().display(),
                sdk.origin(),
                sdk.version()
            )
        });
        if !sdk.version_is_expected() {
            let (major, minor, patch) = sdk.version_numbers();
            self.log(Level::Warn, || {
                format!(
                    "m300 sdk is {major}.{minor}.{patch}; this build was written against {}.{}.x \
                     and is continuing anyway",
                    ffi::EXPECTED_MAJOR,
                    ffi::EXPECTED_MINOR
                )
            });
        }

        sdk.init()?;

        let shared = Arc::new(Shared::new(self.queue_samples));
        // A stable address that outlives every registration; `close` reclaims it once no
        // callback can fire again (§7).
        let user_data = shim::into_user_data(&shared);

        let server = match sdk.server_create(self.bind_address.as_deref(), options.device_port) {
            Ok(server) => Handle(server),
            Err(error) => {
                self.log(Level::Error, || {
                    format!(
                        "m300 server could not bind {}:{}: {error}",
                        self.bind_display(),
                        options.device_port
                    )
                });
                // SAFETY: nothing was ever registered, so no callback can be holding it.
                unsafe { shim::release_user_data(user_data) };
                sdk.cleanup();
                return Err(error.into());
            }
        };

        // Both must be registered before `m300_server_start` (§3).
        // SAFETY: `user_data` points at the `Arc<Shared>` above, which is reclaimed only after
        // both callbacks have been deregistered and the server destroyed.
        unsafe {
            sdk.set_connect_callback(server.0, Some(shim::on_connect), user_data.0);
            sdk.set_disconnect_callback(server.0, Some(shim::on_disconnect), user_data.0);
        }

        // SAFETY: `server` came from `m300_server_create_ex` above and has not been destroyed.
        if let Err(error) = unsafe { sdk.server_start(server.0) } {
            self.log(Level::Error, || {
                format!("m300 server did not start: {error}")
            });
            // SAFETY: no device handle has been handed out yet, and destroying the server tears
            // the two callbacks registered above down with it, so nothing can read `user_data`
            // after this returns.
            unsafe {
                sdk.server_destroy(server.0);
                shim::release_user_data(user_data);
            }
            sdk.cleanup();
            return Err(error.into());
        }

        self.log(Level::Info, || {
            format!(
                "m300 server listening bind={} port={}",
                self.bind_display(),
                options.device_port
            )
        });

        Ok(Live {
            sdk,
            server,
            shared,
            user_data,
            configured: None,
            callback_device: None,
        })
    }

    fn bind_display(&self) -> &str {
        self.bind_address.as_deref().unwrap_or("0.0.0.0")
    }

    /// Bring the backend's view of the device up to date: release what a reconnection retired,
    /// adopt the current handle, apply the project's settings if this device has not had them
    /// yet, and register the data callback.
    fn synchronise(&mut self) -> Result<(Arc<Sdk>, Handle), DeviceError> {
        let (sdk, shared, server, user_data, configured) = {
            let live = self.live.as_ref().ok_or(DeviceError::NotConnected)?;
            (
                Arc::clone(&live.sdk),
                Arc::clone(&live.shared),
                live.server,
                live.user_data,
                live.configured,
            )
        };

        // Handles a reconnection superseded. This thread is allowed to block; the accept thread
        // that retired them is not, which is why they were parked rather than released there.
        for stale in shared.take_stale() {
            // SAFETY: a handle that was replaced is used nowhere else, and releasing it is
            // required or the SDK leaks the `Arc` it boxed for it (§2).
            unsafe { sdk.device_release_handle(stale.0) };
        }

        if !shared.is_connected() {
            return Err(DeviceError::NotConnected);
        }

        let device = match shared.device() {
            Some(device) => device,
            None => {
                Self::adopt_from_server(&sdk, &shared, server).ok_or(DeviceError::NotConnected)?
            }
        };

        let generation = shared.generation();
        if configured == Some(generation) {
            return Ok((sdk, device));
        }

        let capabilities = self.configure(&sdk, device)?;
        shared.set_capabilities(capabilities);

        // SAFETY: `user_data` points at the live `Arc<Shared>`; `close` deregisters this
        // callback before reclaiming it (§7).
        unsafe { sdk.set_data_callback(device.0, Some(shim::on_data), user_data.0) };

        if let Some(live) = self.live.as_mut() {
            live.configured = Some(generation);
            live.callback_device = Some(device);
        }
        Ok((sdk, device))
    }

    /// Ask the server for a handle, for the case where no connect callback was seen — a device
    /// that was already dialled in when QuickVib took the port over, for instance.
    fn adopt_from_server(sdk: &Sdk, shared: &Shared, server: Handle) -> Option<Handle> {
        // SAFETY: every handle this returns is owned by us; the one kept is released in `close`
        // and the rest immediately below (§2).
        let mut devices = unsafe { sdk.server_get_devices(server.0) }.into_iter();
        let first = devices.next()?;
        // QuickVib records from one vibrometer; the others are given straight back rather than
        // leaked.
        for extra in devices {
            // SAFETY: as above — a handle we own and will not use.
            unsafe { sdk.device_release_handle(extra) };
        }
        shared.adopt(Handle(first));
        shared.set_connected();
        Some(Handle(first))
    }

    /// Apply the project's sample rate, filter, unit and range, then read back what the device
    /// actually has.
    fn configure(&self, sdk: &Sdk, device: Handle) -> Result<DeviceCapabilities, DeviceError> {
        let requested = self
            .requested
            .ok_or_else(|| DeviceError::unsupported("the M300 backend has not been opened"))?;

        // SAFETY (every call in this method): `device` is a handle this backend owns and has not
        // released, and the server it came from is still alive. A device that has disconnected
        // in the meantime answers `-8 M300_ERR_DISCONNECTED` rather than misbehaving (§2).
        if let Some(address) = unsafe { sdk.device_address(device.0) } {
            let port = unsafe { sdk.device_port(device.0) }.unwrap_or_default();
            self.log(Level::Info, || {
                format!("m300 device connected peer={address}:{port}")
            });
        }

        let rate = maps::nearest_sample_rate(requested.sample_rate_hz);
        if !rate.is_exact() {
            // D13: a project/device mismatch is logged, not enforced — but an index still has to
            // be chosen, so the substitution is spelled out rather than left to be inferred.
            self.log(Level::Warn, || {
                format!(
                    "m300 has no {} Hz sample rate; using {} Hz (index {:#04x})",
                    requested.sample_rate_hz, rate.hz, rate.index
                )
            });
        }
        unsafe { sdk.set_sample_rate(device.0, rate.index) }?;

        // The vendor insists the filter is set together with the rate, at a matching band (§6.1).
        let filter = match self.low_pass_hz {
            Some(hz) => maps::nearest_low_pass(hz),
            None => maps::low_pass_for_sample_rate(rate.index),
        };
        unsafe { sdk.set_low_pass_filter(device.0, filter.index) }?;
        self.log(Level::Info, || {
            format!(
                "m300 configured rate={} Hz (index {:#04x}) lpf={} Hz (index {:#04x}) unit={}",
                rate.hz, rate.index, filter.hz, filter.index, requested.unit
            )
        });

        let data_type = maps::data_type_for_unit(requested.unit);
        unsafe { sdk.set_data_type(device.0, data_type) }?;

        if let Some(range) = self.range {
            match maps::range_index(requested.unit, range) {
                Some(index) => unsafe { sdk.set_range(device.0, requested.unit, index) }?,
                None => self.log(Level::Warn, || {
                    format!(
                        "m300 range {range} {} lies between the two documented ends of the \
                         ladder, which does not give an index; the device keeps its current range",
                        requested.unit.label()
                    )
                }),
            }
        }

        // Read back rather than assume. Two of the setters above are the entry points no vendor
        // header declares (§3.1), so this round trip is also what proves they took.
        let actual_rate_index = unsafe { sdk.sample_rate_index(device.0) }?;
        let actual_data_type = unsafe { sdk.data_type(device.0) }?;
        let actual_hz = maps::sample_rate_hz(actual_rate_index).unwrap_or(rate.hz);
        let actual_unit = maps::unit_for_data_type(actual_data_type).unwrap_or(requested.unit);
        if actual_rate_index != rate.index || actual_data_type != data_type {
            self.log(Level::Warn, || {
                format!(
                    "m300 read back rate index {actual_rate_index:#04x} ({actual_hz} Hz) and \
                     data type {actual_data_type} ({actual_unit}) after setting {:#04x} and \
                     {data_type}; reporting what the device says",
                    rate.index
                )
            });
        }

        let info = unsafe { sdk.hardware_info(device.0) }?;
        let identity = maps::device_identity(&info);
        self.log(Level::Info, || {
            format!(
                "m300 identity serial={} firmware={} ip={}",
                identity.serial_number, identity.firmware_version, identity.ip
            )
        });

        Ok(DeviceCapabilities {
            model: MODEL.to_owned(),
            serial_number: if identity.serial_number.is_empty() {
                UNKNOWN_SERIAL.to_owned()
            } else {
                identity.serial_number
            },
            firmware_version: identity.firmware_version,
            sample_rate_hz: actual_hz,
            unit: actual_unit,
            max_record_seconds: MAX_RECORD_SECONDS,
        })
    }

    /// Say what the device thinks it is doing, after it accepted a start and then delivered
    /// nothing.
    ///
    /// Best effort, and the failures are as informative as the values: a running status of `1`
    /// with a signal strength near zero is a laser aimed at nothing, whereas three refusals in a
    /// row is a device that has stopped answering altogether. `-365` on its own says neither.
    fn log_stall(&self, sdk: &Sdk, device: Handle) {
        if !self.logger.enabled(Level::Warn) {
            return;
        }
        // SAFETY (all three): the handle the run used, which only `close` releases.
        let status = describe(unsafe { sdk.running_status(device.0) });
        let signal = describe(
            unsafe { sdk.signal_strength(device.0) }
                .map(|pair| pair.map(|(i, q)| format!("i={i} q={q}"))),
        );
        let temperature = describe(unsafe { sdk.board_temperature(device.0) });
        self.log(Level::Warn, || {
            format!(
                "m300 delivered nothing for {:?}: running_status={status} signal={signal} \
                 board_temp={temperature}",
                self.stall_timeout
            )
        });
    }

    /// Tear everything down in the one order that is safe: callbacks first, then handles, then
    /// the server, then the library's globals, and only then the state the callbacks pointed at.
    fn shutdown(&self, live: Live) {
        let Live {
            sdk,
            server,
            shared,
            user_data,
            configured: _,
            callback_device,
        } = live;

        if let Some(device) = callback_device {
            // Deregistration before the owning struct is dropped is the pairing §7 asks for, and
            // it is what makes reclaiming the `Arc` below sound.
            // SAFETY: `None` deregisters; after this the SDK holds no pointer to `Shared` for
            // this device.
            unsafe { sdk.set_data_callback(device.0, None, std::ptr::null_mut()) };
        }
        if let Some(device) = shared.take_device() {
            // SAFETY: a handle this backend owns and has not released yet.
            if let Err(error) = unsafe { sdk.stop_acquisition(device.0) } {
                self.log(Level::Debug, || {
                    format!("m300 stop on close reported {error}")
                });
            }
            // SAFETY: the handle is not used again, and releasing it is required (§2).
            unsafe { sdk.device_release_handle(device.0) };
        }
        for stale in shared.take_stale() {
            // SAFETY: as above.
            unsafe { sdk.device_release_handle(stale.0) };
        }

        // SAFETY: `None` deregisters both link callbacks, so no SDK thread can reach `Shared`
        // once these return.
        unsafe {
            sdk.set_connect_callback(server.0, None, std::ptr::null_mut());
            sdk.set_disconnect_callback(server.0, None, std::ptr::null_mut());
        }
        // SAFETY: every handle from this server has been released above and none is used
        // afterwards, which is what `m300_server_destroy` requires (§2).
        unsafe {
            sdk.server_stop(server.0);
            sdk.server_destroy(server.0);
        }
        sdk.cleanup();

        shared.set_disconnected();
        // SAFETY: every callback that could read this pointer has been deregistered and the
        // server destroyed, and this is the only reclaim.
        unsafe { shim::release_user_data(user_data) };
        self.log(Level::Info, || "m300 backend closed".to_owned());
    }
}

/// Render one optional diagnostic for a log line, failure included.
fn describe<T: std::fmt::Display>(value: Result<Option<T>, NativeError>) -> String {
    match value {
        Ok(Some(value)) => value.to_string(),
        Ok(None) => "unavailable".to_owned(),
        Err(error) => format!("<{error}>"),
    }
}

impl DeviceBackend for M300Backend {
    fn is_connected(&self) -> bool {
        self.live
            .as_ref()
            .is_some_and(|live| live.shared.is_connected())
    }

    fn open(&mut self, options: &DeviceOpenOptions) -> Result<(), DeviceError> {
        if options.sample_rate_hz <= 0.0 || !options.sample_rate_hz.is_finite() {
            return Err(DeviceError::unsupported(format!(
                "sample rate {} is not a positive finite number",
                options.sample_rate_hz
            )));
        }
        self.requested = Some(Requested {
            sample_rate_hz: options.sample_rate_hz,
            unit: options.unit,
        });
        if self.live.is_some() {
            return Ok(());
        }

        // The application resolves `--bind`/`server.bindHost` into the options, so that is where
        // the bind address comes from unless an embedder pinned one with
        // [`M300Backend::with_bind_address`]. Empty means "every interface", which is also what
        // a null `bind_addr` means to `m300_server_create_ex`.
        if self.bind_address.is_none() && !options.bind_host.is_empty() {
            self.bind_address = Some(options.bind_host.clone());
        }

        self.stopped.store(false, Ordering::Release);
        self.live = Some(self.start(options)?);
        Ok(())
    }

    fn capabilities(&self) -> Result<DeviceCapabilities, DeviceError> {
        self.live
            .as_ref()
            .and_then(|live| live.shared.capabilities())
            .ok_or(DeviceError::NotConnected)
    }

    fn stream(
        &mut self,
        request: &StreamRequest,
        on_batch: &mut dyn FnMut(SampleBatch<'_>) -> Result<(), DeviceError>,
        cancel: &CancelToken,
    ) -> Result<StreamOutcome, DeviceError> {
        let (sdk, device) = self.synchronise()?;
        let shared = {
            let live = self.live.as_ref().ok_or(DeviceError::NotConnected)?;
            Arc::clone(&live.shared)
        };

        if let Some(stale) = shared.take_fault() {
            self.log(Level::Debug, || {
                format!("m300 discarding a fault recorded before this run: {stale}")
            });
        }
        self.stopped.store(false, Ordering::Release);
        shared.expect_data_type(maps::data_type_for_unit(request.unit));
        // A run captures from `INIT` onwards; whatever the device pushed while the instrument
        // sat idle is not part of it.
        shared.reset_queue();

        // SAFETY: `synchronise` returned this handle, so it is one we own and have not released,
        // and it registered the data callback against the `Arc<Shared>` `close` reclaims last.
        if let Err(error) = unsafe { sdk.start_acquisition(device.0) } {
            shared.expect_any_data_type();
            return Err(error.into());
        }

        let outcome = Drain {
            clock: self.clock.as_ref(),
            stopped: self.stopped.as_ref(),
            stall_timeout: self.stall_timeout,
        }
        .run(&shared, request, on_batch, cancel);

        shared.expect_any_data_type();
        if matches!(outcome, Ok(StreamOutcome::TimedOut)) {
            self.log_stall(&sdk, device);
        }
        // SAFETY: as above — the handle is still ours; `close` is the only thing that releases it.
        if let Err(error) = unsafe { sdk.stop_acquisition(device.0) } {
            // The run is over either way, and a device that has already gone away cannot be
            // stopped politely; saying so must not overwrite the outcome.
            self.log(Level::Warn, || {
                format!("m300 stop after the run reported {error}")
            });
        }
        let dropped = shared.dropped_samples();
        if dropped != 0 {
            self.log(Level::Warn, || {
                format!("m300 queue overran: {dropped} sample(s) dropped since open")
            });
        }
        outcome
    }

    fn stop(&self) -> Result<(), DeviceError> {
        self.stopped.store(true, Ordering::Release);
        let Some(live) = self.live.as_ref() else {
            return Ok(());
        };
        let Some(device) = live.shared.device() else {
            return Ok(());
        };
        // §8: an ordinary per-device command, documented as safe to call from another thread
        // while a stream is in flight. The flag above has already ended the run, so a device
        // that refuses — or has vanished, which is the common case for `ABOR` — is logged rather
        // than turned into an `ABOR` failure the UTS has to interpret.
        // SAFETY: `shared.device()` hands back a handle the backend still owns; only `close`
        // releases it, and `close` takes `&mut self` so it cannot be running concurrently.
        if let Err(error) = unsafe { live.sdk.stop_acquisition(device.0) } {
            self.log(Level::Warn, || format!("m300 stop reported {error}"));
        }
        Ok(())
    }

    fn stop_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
        let stopped = Arc::clone(&self.stopped);
        // The flag and nothing else: the handle is called from the watchdog and from `ABOR`,
        // must not block, and `m300_stop_acquisition` is a round trip with a three-second
        // timeout. `stream` clears the flag on entry, so the next run on the same open
        // transport is unaffected, which is the third rule of the `stop_handle` contract.
        Some(Arc::new(move || stopped.store(true, Ordering::Release)))
    }

    fn close(&mut self) -> Result<(), DeviceError> {
        if let Some(live) = self.live.take() {
            self.shutdown(live);
        }
        self.stopped.store(false, Ordering::Release);
        Ok(())
    }
}

impl Drop for M300Backend {
    fn drop(&mut self) {
        if let Some(live) = self.live.take() {
            self.shutdown(live);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    // Everything here holds without an SDK: the states a backend can be in before anything is
    // loaded. The parts that need a device are the manual Windows smoke checklist
    // (`docs/M300-NATIVE.md` §9), because no fake DLL exists in this repository and none will
    // (D21).
    use super::*;
    use quickvib_core::TestClock;

    fn backend() -> M300Backend {
        M300Backend::new(Arc::new(TestClock::at_epoch()))
    }

    #[test]
    fn a_backend_that_has_not_opened_is_disconnected_and_has_no_capabilities() {
        let backend = backend();
        assert!(!backend.is_connected());
        assert!(matches!(
            backend.capabilities(),
            Err(DeviceError::NotConnected)
        ));
        assert!(backend.library_path().is_none());
        assert!(backend.sdk_version().is_none());
        assert_eq!(backend.dropped_samples(), 0);
    }

    #[test]
    fn opening_with_a_bad_rate_is_rejected_before_anything_is_loaded() {
        let mut backend = backend();
        let mut options = DeviceOpenOptions::new(0.0, SampleUnit::VelocityUmPerSec);
        assert!(matches!(
            backend.open(&options),
            Err(DeviceError::Unsupported { .. })
        ));
        options.sample_rate_hz = f64::INFINITY;
        assert!(backend.open(&options).is_err());
        assert!(backend.library_path().is_none());
    }

    #[test]
    fn stopping_and_closing_an_unopened_backend_are_no_ops() {
        let mut backend = backend();
        assert!(backend.stop().is_ok());
        assert!(backend.close().is_ok());
    }

    #[test]
    fn the_stop_handle_is_idempotent_and_does_not_need_the_transport() {
        let backend = backend();
        let handle = backend
            .stop_handle()
            .expect("the M300 backend hands one out");
        handle();
        handle();
        assert!(backend.stopped.load(Ordering::Acquire));
    }

    #[test]
    fn streaming_without_a_device_reports_not_connected() {
        let mut backend = backend();
        let request = StreamRequest::new(10, SampleUnit::VelocityUmPerSec, 1000.0);
        let result = backend.stream(&request, &mut |_| Ok(()), &CancelToken::new());
        assert!(matches!(result, Err(DeviceError::NotConnected)));
    }

    #[test]
    fn the_builders_are_all_optional() {
        let backend = backend()
            .with_bind_address(Some("192.168.1.100".to_owned()))
            .with_library_name(Some("libm300_sdk.dll".to_owned()))
            .with_low_pass_hz(Some(20_000.0))
            .with_range(Some(1_000.0))
            .with_timeout_ms(5_000)
            .with_stall_timeout(Duration::from_secs(1))
            .with_queue_samples(4_096);
        assert_eq!(backend.bind_display(), "192.168.1.100");
        assert_eq!(backend.timeout_ms, 5_000);
        assert_eq!(backend.queue_samples, 4_096);
    }

    #[test]
    fn the_open_options_supply_the_bind_address_unless_one_was_pinned() {
        // `--bind` reaches the SDK through the options, because on this backend the SDK is the
        // listener and there is no `DeviceServer` of ours for the flag to configure instead.
        // The open still fails — there is no DLL on a test machine — but by then the address
        // has been adopted.
        let mut options = DeviceOpenOptions::new(1000.0, SampleUnit::VelocityUmPerSec);
        options.bind_host = "127.0.0.1".to_owned();

        let mut from_options = backend();
        let _ = from_options.open(&options);
        assert_eq!(from_options.bind_display(), "127.0.0.1");

        let mut pinned = backend().with_bind_address(Some("10.0.0.4".to_owned()));
        let _ = pinned.open(&options);
        assert_eq!(pinned.bind_display(), "10.0.0.4");

        // An empty host is "every interface", which is a null `bind_addr` rather than a bind to
        // the literal empty string.
        options.bind_host = String::new();
        let mut wildcard = backend();
        let _ = wildcard.open(&options);
        assert!(wildcard.bind_address.is_none());
    }
}
