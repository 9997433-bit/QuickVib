//! Where the SDK's threads and QuickVib's threads meet.
//!
//! `DeviceBackend::stream` is a **pull** API on the reader thread; the SDK is a **push** source
//! that calls back on its own threads (`docs/M300-NATIVE.md` §6). This module is the seam: the
//! `extern "C"` shims the SDK calls, the pinned state they write into, the bounded queue they
//! fill, and the drain loop that empties it. Nothing here loads a library or calls an SDK
//! function, so — unlike [`crate::backend`], which does both — all of it compiles and is tested
//! on Linux, which is where the hand-off deserves to be exercised: it is concurrent, it runs on
//! threads QuickVib does not own, and it is the one place a mistake shows up as missing samples
//! rather than as a compile error.
//!
//! The rules the shims hold to, all from §7:
//!
//! * The whole body of every shim is inside `catch_unwind`. Unwinding across an FFI boundary is
//!   undefined behavior, and since the callback ABI returns `void` there is no way to report the
//!   panic back to the SDK — so it is recorded in [`Shared`] and surfaced on the next
//!   [`Drain::run`] poll, which aborts the run from QuickVib's side.
//! * Context travels through the SDK's `*mut c_void` user-data pointer, which points at a
//!   [`Shared`] owned by the backend and guaranteed to outlive the registration.
//! * The shim copies (it must — the SDK's buffer dies when the callback returns), does no I/O,
//!   and holds one uncontended mutex at a time, so a slow consumer can never stall the SDK's
//!   reader thread. An overrun drops the oldest samples and counts them.
//! * `point_count` and `data_len` are cross-checked before the buffer is read, and the unit is
//!   cross-checked against the one the run asked for. Either mismatch aborts the run instead of
//!   trusting a number or relabelling a measurement.

use std::cell::RefCell;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use quickvib_core::{CancelToken, Clock};
use quickvib_device::sink::Taken;
use quickvib_device::{
    DeviceCapabilities, DeviceError, SampleBatch, SampleChannel, SampleSink, StreamOutcome,
    StreamRequest, BATCH_SAMPLES,
};

use crate::batch::{self, ANY_DATA_TYPE};
use crate::ffi_generated::{M300Device, M300Server};

/// How long a queue drain parks before the loop re-checks cancellation. Short enough that `ABOR`
/// feels instant, long enough that an idle link costs nothing.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

thread_local! {
    /// Scratch space for one callback's worth of samples, one buffer per SDK callback thread.
    ///
    /// The shim must copy, but it must not allocate on every batch either, so the buffer is kept
    /// and reused. It belongs to the calling thread rather than to [`Shared`] precisely so that
    /// two devices' callback threads never contend for it.
    static DECODE_BUFFER: RefCell<Vec<f32>> = const { RefCell::new(Vec::new()) };
}

/// A raw SDK handle that may cross threads.
///
/// The vendor documents the SDK as thread-safe — per-device locks, replies matched by
/// `command_id`, locked callback registration (`docs/M300-NATIVE.md` §8) — and QuickVib's own
/// use is narrower still: a handle is adopted once, used from the SCPI and reader threads, and
/// released once. Rust cannot see any of that through a `*mut c_void`, so it is asserted here,
/// in one place, rather than at each of the dozen call sites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Handle(
    /// The SDK's own pointer. This crate never dereferences it; it only hands it back.
    pub *mut c_void,
);

// SAFETY: the paragraph above — §8's threading contract is what makes moving and sharing these
// pointers between QuickVib's threads sound.
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

/// Everything the SDK's callback threads and QuickVib's own threads both touch.
///
/// This is the pinned struct the `*mut c_void` user-data pointer points at: the backend
/// allocates it once, hands the SDK a raw `Arc` pointer to it, and reclaims that pointer only
/// after every callback has been deregistered.
#[derive(Debug)]
pub struct Shared {
    channel: SampleChannel,
    connected: AtomicBool,
    generation: AtomicU64,
    device: Mutex<Option<Handle>>,
    stale: Mutex<Vec<Handle>>,
    expected_data_type: AtomicU32,
    fault: Mutex<Option<String>>,
    capabilities: Mutex<Option<DeviceCapabilities>>,
}

impl Shared {
    /// Shared state with a queue holding at most `queue_samples` samples before the oldest are
    /// dropped.
    #[must_use]
    pub fn new(queue_samples: usize) -> Self {
        Self {
            channel: SampleChannel::new(queue_samples),
            connected: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            device: Mutex::new(None),
            stale: Mutex::new(Vec::new()),
            expected_data_type: AtomicU32::new(ANY_DATA_TYPE),
            fault: Mutex::new(None),
            capabilities: Mutex::new(None),
        }
    }

    fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a device is currently dialled in. Backs `SYST:DEV:CONN?`.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// How many times a device has dialled in, so the backend can tell "still the same device"
    /// from "a different one, which needs configuring again".
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The handle of the device currently dialled in, if there is one.
    #[must_use]
    pub fn device(&self) -> Option<Handle> {
        *Self::lock(&self.device)
    }

    /// Samples the queue dropped because the consumer fell behind.
    #[must_use]
    pub fn dropped_samples(&self) -> u64 {
        self.channel.dropped()
    }

    /// Take ownership of a device handle, retiring whatever it replaced.
    ///
    /// The replaced handle is parked in the stale list rather than released here: this runs on
    /// the SDK's accept thread, where the vendor asks for no long work (§7), and it lets the
    /// release happen on a thread that is allowed to block.
    pub fn adopt(&self, device: Handle) {
        let mut guard = Self::lock(&self.device);
        if let Some(previous) = guard.replace(device) {
            if previous != device {
                Self::lock(&self.stale).push(previous);
            }
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Give up the current handle, so the caller can release it.
    pub fn take_device(&self) -> Option<Handle> {
        Self::lock(&self.device).take()
    }

    /// Every handle a reconnection superseded, for the caller to release.
    pub fn take_stale(&self) -> Vec<Handle> {
        std::mem::take(&mut *Self::lock(&self.stale))
    }

    /// Publish a connection, discarding whatever the previous link left queued.
    pub fn set_connected(&self) {
        self.connected.store(true, Ordering::Release);
        self.channel.on_connected(None);
    }

    /// Publish a disconnection. The queue drains before it reports the link is gone, so samples
    /// already delivered are still part of the run.
    pub fn set_disconnected(&self) {
        self.connected.store(false, Ordering::Release);
        self.channel.on_disconnected();
    }

    /// Declare the `data_type` a run expects. Any other value from the device aborts it.
    pub fn expect_data_type(&self, data_type: u8) {
        self.expected_data_type
            .store(u32::from(data_type), Ordering::Release);
    }

    /// Accept any `data_type` again, between runs.
    pub fn expect_any_data_type(&self) {
        self.expected_data_type
            .store(ANY_DATA_TYPE, Ordering::Release);
    }

    /// Discard whatever arrived while the instrument was idle, so a run captures from `INIT`
    /// onwards.
    pub fn reset_queue(&self) {
        self.channel.reset();
    }

    /// Hand a batch to the queue as if it had come from the SDK. The link half of the callback
    /// path, for tests and for anything that needs to prime the queue.
    pub fn push_samples(&self, samples: &[f32]) {
        self.channel.on_samples(samples);
    }

    /// Record a reason to abort the run. The first one wins: it is the one that explains the
    /// rest.
    pub fn set_fault(&self, detail: impl Into<String>) {
        let mut guard = Self::lock(&self.fault);
        if guard.is_none() {
            *guard = Some(detail.into());
        }
    }

    /// Take the recorded fault, if any.
    pub fn take_fault(&self) -> Option<String> {
        Self::lock(&self.fault).take()
    }

    /// The capabilities read back from the device when it was configured.
    #[must_use]
    pub fn capabilities(&self) -> Option<DeviceCapabilities> {
        Self::lock(&self.capabilities).clone()
    }

    /// Publish the capabilities read back from the device.
    pub fn set_capabilities(&self, capabilities: DeviceCapabilities) {
        *Self::lock(&self.capabilities) = Some(capabilities);
    }

    /// Copy one callback's batch into the queue.
    ///
    /// # Safety
    /// `data` must point at `data_len` initialised bytes that stay valid for the duration of the
    /// call — exactly what the SDK guarantees for the lifetime of the callback
    /// (`docs/M300-NATIVE.md` §2).
    pub unsafe fn deliver(&self, data_type: u32, point_count: u32, data: *const u8, data_len: u32) {
        let expected = self.expected_data_type.load(Ordering::Acquire);
        let length =
            match batch::check_batch(expected, data_type, point_count, data_len, data.is_null()) {
                Ok(0) => return,
                Ok(length) => length,
                Err(fault) => {
                    self.set_fault(fault.to_string());
                    return;
                }
            };

        // SAFETY: the caller's contract above, narrowed by `check_batch`, which has already
        // agreed that `data_len` is `point_count * 4` and that the pointer is not null.
        let bytes = unsafe { std::slice::from_raw_parts(data, length) };
        DECODE_BUFFER.with(|cell| {
            // A borrow can only fail if the SDK re-entered this thread's callback, which it does
            // not do; dropping the batch still beats panicking across an FFI boundary.
            let Ok(mut buffer) = cell.try_borrow_mut() else {
                self.set_fault("re-entrant M300 data callback on one thread");
                return;
            };
            buffer.clear();
            batch::decode_le_f32(bytes, &mut buffer);
            self.channel.on_samples(&buffer);
        });
    }
}

/// The SDK's data callback (`docs/M300-NATIVE.md` §7). Runs on the device's callback thread.
///
/// # Safety
/// `user_data` must be null or the `Arc::into_raw` pointer to the [`Shared`] the backend
/// registered, and `data` must be valid for `data_len` bytes for the duration of the call.
pub unsafe extern "C" fn on_data(
    _device: M300Device,
    data_type: u32,
    point_count: u32,
    data: *const u8,
    data_len: u32,
    user_data: *mut c_void,
) {
    let Some(shared) = (unsafe { shared_from(user_data) }) else {
        return;
    };
    // `AssertUnwindSafe`: the shared state is atomics and mutexes whose invariants do not depend
    // on a panic-free body — a panic mid-batch leaves a queue that is merely missing samples,
    // and the fault recorded below aborts the run anyway.
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: the caller's contract above.
        unsafe { shared.deliver(data_type, point_count, data, data_len) };
    }));
    if outcome.is_err() {
        shared.set_fault("a panic escaped the M300 data callback; the run is aborted");
    }
}

/// The SDK's connect callback. Runs on the accept thread, so it does the least it can: long work
/// here stalls every other device's accept (§7).
///
/// # Safety
/// `user_data` must be null or the `Arc::into_raw` pointer to the [`Shared`] the backend
/// registered.
pub unsafe extern "C" fn on_connect(
    _server: M300Server,
    device: M300Device,
    user_data: *mut c_void,
) {
    let Some(shared) = (unsafe { shared_from(user_data) }) else {
        return;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // The handle is ours to keep and ours to release (§2); the configuration round trips it
        // needs happen on QuickVib's own thread, not here.
        shared.adopt(Handle(device));
        shared.set_connected();
    }));
    if outcome.is_err() {
        shared.set_fault("a panic escaped the M300 connect callback");
    }
}

/// The SDK's disconnect callback. Runs on that device's callback thread.
///
/// # Safety
/// As [`on_connect`].
pub unsafe extern "C" fn on_disconnect(
    _server: M300Server,
    _device: M300Device,
    user_data: *mut c_void,
) {
    let Some(shared) = (unsafe { shared_from(user_data) }) else {
        return;
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| shared.set_disconnected()));
    if outcome.is_err() {
        shared.set_fault("a panic escaped the M300 disconnect callback");
    }
}

/// Borrow the [`Shared`] behind a user-data pointer.
///
/// # Safety
/// `user_data` must be null or the `Arc::into_raw` pointer the backend registered, which it
/// reclaims only after every callback using it has been deregistered.
unsafe fn shared_from<'a>(user_data: *mut c_void) -> Option<&'a Shared> {
    if user_data.is_null() {
        return None;
    }
    // SAFETY: the caller's contract above. Borrowing rather than reconstructing the `Arc` is
    // deliberate: an `Arc::from_raw` here would drop a strong count the SDK still owns.
    Some(unsafe { &*(user_data as *const Shared) })
}

/// Hand a [`Shared`] to the SDK as a raw user-data pointer.
///
/// The returned pointer keeps the allocation alive on its own; it must be given back to
/// [`release_user_data`] exactly once, after the callbacks that use it are deregistered.
#[must_use]
pub fn into_user_data(shared: &Arc<Shared>) -> Handle {
    Handle(Arc::into_raw(Arc::clone(shared)) as *mut c_void)
}

/// Reclaim a pointer from [`into_user_data`].
///
/// # Safety
/// The pointer must have come from [`into_user_data`], must not have been reclaimed already, and
/// no callback that could read it may still be registered.
pub unsafe fn release_user_data(user_data: Handle) {
    // SAFETY: the caller's contract above.
    drop(unsafe { Arc::from_raw(user_data.0 as *const Shared) });
}

/// The reader thread's half: empty the queue the shim fills until the run is over.
pub struct Drain<'a> {
    /// Injected time, for batch timestamps and the stall guard (D20).
    pub clock: &'a dyn Clock,
    /// The backend's stop flag, set by `ABOR`, the watchdog and [`Drain`]-external callers.
    pub stopped: &'a AtomicBool,
    /// How long the device may deliver nothing before [`StreamOutcome::TimedOut`].
    pub stall_timeout: Duration,
}

impl Drain<'_> {
    /// Deliver batches to `on_batch` until the requested sample count arrives, the run is
    /// cancelled or stopped, the shim records a fault, the link drops, or the device goes quiet
    /// for longer than [`Drain::stall_timeout`].
    ///
    /// # Errors
    /// [`DeviceError::LinkLost`] carrying whatever the shim recorded, or any error `on_batch`
    /// returns.
    pub fn run(
        &self,
        shared: &Shared,
        request: &StreamRequest,
        on_batch: &mut dyn FnMut(SampleBatch<'_>) -> Result<(), DeviceError>,
        cancel: &CancelToken,
    ) -> Result<StreamOutcome, DeviceError> {
        let mut buffer: Vec<f32> = Vec::with_capacity(BATCH_SAMPLES);
        let mut delivered: u64 = 0;
        let mut last_progress = self.clock.monotonic();

        while delivered < request.expected_samples {
            // The callback ABI has no error channel, so this is where a bad batch or a caught
            // panic finally becomes a failure the SCPI layer can report (§7).
            if let Some(fault) = shared.take_fault() {
                return Err(DeviceError::link_lost(fault));
            }
            if cancel.is_cancelled() || self.stopped.load(Ordering::Acquire) {
                return Ok(StreamOutcome::Cancelled);
            }

            let remaining = (request.expected_samples - delivered).min(BATCH_SAMPLES as u64);
            buffer.clear();
            match shared
                .channel
                .take(&mut buffer, remaining as usize, POLL_INTERVAL)
            {
                Taken::Samples => {}
                Taken::Idle => {
                    if self.clock.monotonic().saturating_sub(last_progress) >= self.stall_timeout {
                        return Ok(StreamOutcome::TimedOut);
                    }
                    continue;
                }
                Taken::Disconnected => return Ok(StreamOutcome::LinkLost),
            }

            on_batch(SampleBatch {
                samples: &buffer,
                start_index: delivered,
                arrived_at: self.clock.monotonic(),
            })?;
            delivered += buffer.len() as u64;
            last_progress = self.clock.monotonic();
        }

        Ok(StreamOutcome::Completed)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::{SampleUnit, TestClock};

    fn handle(value: usize) -> Handle {
        Handle(value as *mut c_void)
    }

    fn drain_for<'a>(clock: &'a dyn Clock, stopped: &'a AtomicBool) -> Drain<'a> {
        Drain {
            clock,
            stopped,
            // Long enough that only a test that asks for a stall gets one.
            stall_timeout: Duration::from_secs(3600),
        }
    }

    fn request(samples: u64) -> StreamRequest {
        StreamRequest::new(samples, SampleUnit::VelocityUmPerSec, 1000.0)
    }

    #[test]
    fn fresh_state_is_disconnected_and_empty() {
        let shared = Shared::new(64);
        assert!(!shared.is_connected());
        assert_eq!(shared.generation(), 0);
        assert!(shared.device().is_none());
        assert!(shared.take_fault().is_none());
        assert!(shared.capabilities().is_none());
    }

    #[test]
    fn only_the_first_fault_is_kept() {
        let shared = Shared::new(64);
        shared.set_fault("first");
        shared.set_fault("second");
        assert_eq!(shared.take_fault().as_deref(), Some("first"));
        assert!(shared.take_fault().is_none());
    }

    #[test]
    fn adopting_a_new_handle_retires_the_old_one_and_bumps_the_generation() {
        let shared = Shared::new(64);
        shared.adopt(handle(1));
        assert_eq!(shared.generation(), 1);
        assert_eq!(shared.device(), Some(handle(1)));
        assert!(shared.take_stale().is_empty());

        shared.adopt(handle(2));
        assert_eq!(shared.generation(), 2);
        assert_eq!(shared.device(), Some(handle(2)));
        assert_eq!(shared.take_stale(), vec![handle(1)]);
        assert!(shared.take_stale().is_empty(), "taken once, not twice");
    }

    #[test]
    fn adopting_the_same_handle_twice_does_not_retire_it() {
        let shared = Shared::new(64);
        shared.adopt(handle(1));
        shared.adopt(handle(1));
        assert!(
            shared.take_stale().is_empty(),
            "releasing a handle still in use would be a use-after-free"
        );
    }

    #[test]
    fn a_reconnection_looks_different_from_the_same_device_still_being_there() {
        let shared = Shared::new(64);
        shared.adopt(handle(1));
        let first = shared.generation();
        shared.adopt(handle(2));
        assert_ne!(
            shared.generation(),
            first,
            "the backend reconfigures on a new generation"
        );
    }

    #[test]
    fn the_shim_queues_what_it_is_given() {
        let shared = Shared::new(1024);
        shared.set_connected();
        let bytes: Vec<u8> = [1.0f32, 2.0, 3.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();

        // SAFETY: `bytes` outlives the call, which is the guarantee the SDK gives the real shim.
        unsafe { shared.deliver(0, 3, bytes.as_ptr(), bytes.len() as u32) };

        let mut out = Vec::new();
        assert_eq!(
            shared.channel.take(&mut out, 8, Duration::from_millis(10)),
            Taken::Samples
        );
        assert_eq!(out, vec![1.0, 2.0, 3.0]);
        assert!(shared.take_fault().is_none());
    }

    #[test]
    fn the_shim_refuses_a_batch_whose_two_lengths_disagree() {
        let shared = Shared::new(1024);
        shared.set_connected();
        let bytes = [0u8; 12];

        // SAFETY: the buffer is valid; it is the SDK's arithmetic that is not.
        unsafe { shared.deliver(0, 4, bytes.as_ptr(), 12) };

        assert!(shared.channel.is_empty());
        let fault = shared.take_fault().expect("the run has to be aborted");
        assert!(fault.contains("data_len=12"), "{fault}");
    }

    #[test]
    fn the_shim_refuses_a_unit_the_run_did_not_ask_for() {
        let shared = Shared::new(1024);
        shared.set_connected();
        shared.expect_data_type(crate::maps::DATA_TYPE_VELOCITY);
        let bytes = [0u8; 8];

        // SAFETY: the buffer is valid for the length passed.
        unsafe {
            shared.deliver(
                u32::from(crate::maps::DATA_TYPE_ACCELERATION),
                2,
                bytes.as_ptr(),
                8,
            );
        }

        assert!(shared.channel.is_empty());
        assert!(shared.take_fault().is_some());
    }

    #[test]
    fn between_runs_the_shim_accepts_any_unit() {
        let shared = Shared::new(1024);
        shared.set_connected();
        shared.expect_data_type(crate::maps::DATA_TYPE_VELOCITY);
        shared.expect_any_data_type();
        let bytes = [0u8; 8];

        // SAFETY: the buffer is valid for the length passed.
        unsafe { shared.deliver(2, 2, bytes.as_ptr(), 8) };
        assert!(shared.take_fault().is_none());
    }

    #[test]
    fn the_shim_tolerates_an_empty_batch() {
        let shared = Shared::new(1024);
        shared.set_connected();
        // SAFETY: a null pointer with a zero length is never dereferenced.
        unsafe { shared.deliver(0, 0, std::ptr::null(), 0) };
        assert!(shared.channel.is_empty());
        assert!(shared.take_fault().is_none());
    }

    #[test]
    fn the_shim_refuses_a_null_pointer_that_claims_to_hold_samples() {
        let shared = Shared::new(1024);
        shared.set_connected();
        // SAFETY: `check_batch` rejects the null pointer before anything reads it.
        unsafe { shared.deliver(0, 2, std::ptr::null(), 8) };
        assert!(shared.take_fault().is_some());
    }

    #[test]
    fn a_null_user_data_pointer_is_ignored_rather_than_dereferenced() {
        // The SDK should never do this; if it does, the callback must not be what brings the
        // process down.
        // SAFETY: every shim checks for null before it touches the pointer, which is exactly
        // what this exercises.
        unsafe {
            on_data(
                std::ptr::null_mut(),
                0,
                0,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
            );
            on_connect(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            on_disconnect(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
        }
    }

    #[test]
    fn the_callbacks_reach_the_state_behind_the_user_data_pointer() {
        let shared = Arc::new(Shared::new(1024));
        let user_data = into_user_data(&shared);
        let device = handle(42);
        let bytes: Vec<u8> = [4.0f32, 5.0].iter().flat_map(|v| v.to_le_bytes()).collect();

        // SAFETY: `user_data` is live for the whole test and reclaimed once at the end, which is
        // the same discipline `M300Backend::open` and `close` hold to.
        unsafe {
            on_connect(std::ptr::null_mut(), device.0, user_data.0);
            assert!(shared.is_connected());
            assert_eq!(shared.device(), Some(device));

            on_data(device.0, 0, 2, bytes.as_ptr(), 8, user_data.0);

            on_disconnect(std::ptr::null_mut(), device.0, user_data.0);
        }
        assert!(!shared.is_connected());

        let mut out = Vec::new();
        assert_eq!(
            shared.channel.take(&mut out, 8, Duration::from_millis(10)),
            Taken::Samples
        );
        assert_eq!(out, vec![4.0, 5.0]);

        // SAFETY: no callback is registered any more, and this is the only reclaim.
        unsafe { release_user_data(user_data) };
        assert_eq!(Arc::strong_count(&shared), 1);
    }

    #[test]
    fn a_reconnection_clears_what_the_previous_link_left_queued() {
        let shared = Shared::new(1024);
        shared.set_connected();
        shared.push_samples(&[1.0, 2.0]);
        shared.set_disconnected();
        shared.set_connected();
        assert!(shared.channel.is_empty());
    }

    #[test]
    fn an_overrun_drops_the_oldest_samples_rather_than_blocking_the_sdk() {
        let shared = Shared::new(4);
        shared.set_connected();
        shared.push_samples(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(shared.dropped_samples(), 2);

        let mut out = Vec::new();
        assert_eq!(
            shared.channel.take(&mut out, 16, Duration::from_millis(10)),
            Taken::Samples
        );
        assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn a_drain_delivers_exactly_what_was_asked_for() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(1024);
        shared.set_connected();
        shared.push_samples(&[1.0, 2.0, 3.0, 4.0, 5.0]);

        let mut collected = Vec::new();
        let outcome = drain_for(&clock, &stopped)
            .run(
                &shared,
                &request(4),
                &mut |batch| {
                    assert_eq!(batch.start_index, collected.len() as u64);
                    collected.extend_from_slice(batch.samples);
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();

        assert_eq!(outcome, StreamOutcome::Completed);
        assert_eq!(collected, vec![1.0, 2.0, 3.0, 4.0]);
        // The device keeps streaming after the run ends; the surplus is simply still queued.
        assert!(!shared.channel.is_empty());
    }

    #[test]
    fn a_drain_aborts_on_a_fault_the_shim_recorded() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(64);
        shared.set_connected();
        shared.set_fault("data_len mismatch");

        let error = drain_for(&clock, &stopped)
            .run(&shared, &request(4), &mut |_| Ok(()), &CancelToken::new())
            .unwrap_err();
        assert!(matches!(error, DeviceError::LinkLost { .. }));
        assert!(error.to_string().contains("data_len mismatch"));
    }

    #[test]
    fn a_drain_reports_a_dropped_link() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(64);
        shared.set_connected();
        shared.push_samples(&[1.0]);
        shared.set_disconnected();

        let mut delivered = 0usize;
        let outcome = drain_for(&clock, &stopped)
            .run(
                &shared,
                &request(64),
                &mut |batch| {
                    delivered += batch.samples.len();
                    Ok(())
                },
                &CancelToken::new(),
            )
            .unwrap();
        assert_eq!(outcome, StreamOutcome::LinkLost);
        assert_eq!(
            delivered, 1,
            "the queue drains before the link is declared lost"
        );
    }

    #[test]
    fn a_drain_observes_cancellation_and_the_stop_flag() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(64);
        shared.set_connected();

        let cancel = CancelToken::new();
        cancel.cancel();
        assert_eq!(
            drain_for(&clock, &stopped)
                .run(&shared, &request(4), &mut |_| Ok(()), &cancel)
                .unwrap(),
            StreamOutcome::Cancelled
        );

        stopped.store(true, Ordering::Release);
        assert_eq!(
            drain_for(&clock, &stopped)
                .run(&shared, &request(4), &mut |_| Ok(()), &CancelToken::new())
                .unwrap(),
            StreamOutcome::Cancelled
        );
    }

    #[test]
    fn a_device_that_goes_quiet_times_out_rather_than_hanging() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(64);
        shared.set_connected();

        let mut drain = drain_for(&clock, &stopped);
        drain.stall_timeout = Duration::ZERO;
        let outcome = drain
            .run(&shared, &request(4), &mut |_| Ok(()), &CancelToken::new())
            .unwrap();
        assert_eq!(outcome, StreamOutcome::TimedOut);
    }

    #[test]
    fn a_consumer_error_aborts_the_drain() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(64);
        shared.set_connected();
        shared.push_samples(&[1.0; 8]);

        let result = drain_for(&clock, &stopped).run(
            &shared,
            &request(8),
            &mut |_| Err(DeviceError::unsupported("the capture buffer is full")),
            &CancelToken::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_zero_sample_request_completes_without_touching_the_queue() {
        let clock = TestClock::at_epoch();
        let stopped = AtomicBool::new(false);
        let shared = Shared::new(64);
        let outcome = drain_for(&clock, &stopped)
            .run(&shared, &request(0), &mut |_| Ok(()), &CancelToken::new())
            .unwrap();
        assert_eq!(outcome, StreamOutcome::Completed);
    }
}
