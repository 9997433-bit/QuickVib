//! The hand-off between the inbound device link and a socket-fed backend
//! (`docs/PLAN.md` 7.3, 7.4).
//!
//! The listener thread frames bytes into `f32` samples and the reader thread inside
//! [`crate::StreamBackend`] consumes them; [`SampleChannel`] is the one place they meet. It is
//! a bounded queue rather than an `std::sync::mpsc` channel for two reasons: a live instrument
//! keeps streaming whether or not anybody is recording, so the buffer has to drop the *oldest*
//! samples instead of growing without limit, and the consumer needs to distinguish "nothing
//! yet" from "the device hung up" without closing the producer's half.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// Samples held while nobody is recording, before the oldest are dropped. At 100 kS/s this is
/// roughly ten seconds of slack, which is far more than the gap between `INIT` and the first
/// batch, and still only 4 MiB.
pub const DEFAULT_CHANNEL_SAMPLES: usize = 1_048_576;

/// Where the device link delivers what it framed.
///
/// Implemented by [`SampleChannel`]; the trait exists so the device server can stay ignorant of
/// which backend, if any, is listening.
pub trait SampleSink: Send + Sync {
    /// A device link came up.
    fn on_connected(&self, peer: Option<SocketAddr>);

    /// One framed batch. Called on the link thread, so it must not block for long.
    fn on_samples(&self, samples: &[f32]);

    /// The device link went away, for any reason.
    fn on_disconnected(&self);
}

/// What a call to [`SampleChannel::take`] produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    /// Samples were appended to the caller's buffer.
    Samples,
    /// Nothing arrived before the deadline, and the link is still up.
    Idle,
    /// The link is down and the buffer is drained.
    Disconnected,
}

#[derive(Debug, Default)]
struct ChannelState {
    samples: VecDeque<f32>,
    connected: bool,
}

/// The bounded queue between the link thread and the reader thread.
#[derive(Debug)]
pub struct SampleChannel {
    state: Mutex<ChannelState>,
    changed: Condvar,
    capacity: usize,
    dropped: AtomicU64,
    received: AtomicU64,
}

impl Default for SampleChannel {
    fn default() -> Self {
        Self::new(DEFAULT_CHANNEL_SAMPLES)
    }
}

impl SampleChannel {
    /// A channel holding at most `capacity` samples. A capacity of zero is rounded up to one.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(ChannelState::default()),
            changed: Condvar::new(),
            capacity: capacity.max(1),
            dropped: AtomicU64::new(0),
            received: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ChannelState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a device link is currently up. Backs `SYST:DEV:CONN?` for the `tcp` backend.
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.lock().connected
    }

    /// Samples accepted since startup, dropped ones excluded.
    #[must_use]
    pub fn received(&self) -> u64 {
        self.received.load(Ordering::Relaxed)
    }

    /// Samples discarded because the buffer was full while nobody was recording.
    #[must_use]
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// How many samples are queued right now.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().samples.len()
    }

    /// Whether nothing is queued right now.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Discard everything queued, so a run starts from "now" rather than from whatever the
    /// device pushed while the instrument was idle.
    pub fn reset(&self) {
        self.lock().samples.clear();
    }

    /// Block until a device link is up or `timeout` elapses. Returns the connected state.
    #[must_use]
    pub fn wait_connected(&self, timeout: Duration) -> bool {
        let guard = self.lock();
        let (guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |s| !s.connected)
            .unwrap_or_else(PoisonError::into_inner);
        guard.connected
    }

    /// Move up to `max` queued samples into `out`, waiting up to `timeout` for the first one.
    ///
    /// `out` is appended to, not cleared, so the caller can reuse one buffer across calls.
    pub fn take(&self, out: &mut Vec<f32>, max: usize, timeout: Duration) -> Taken {
        let guard = self.lock();
        let (mut guard, _) = self
            .changed
            .wait_timeout_while(guard, timeout, |s| s.samples.is_empty() && s.connected)
            .unwrap_or_else(PoisonError::into_inner);

        if guard.samples.is_empty() {
            return if guard.connected {
                Taken::Idle
            } else {
                Taken::Disconnected
            };
        }

        let take = max.min(guard.samples.len());
        out.extend(guard.samples.drain(..take));
        Taken::Samples
    }
}

impl SampleSink for SampleChannel {
    fn on_connected(&self, _peer: Option<SocketAddr>) {
        let mut guard = self.lock();
        guard.samples.clear();
        guard.connected = true;
        drop(guard);
        self.changed.notify_all();
    }

    fn on_samples(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut guard = self.lock();
        guard.samples.extend(samples.iter().copied());
        let overflow = guard.samples.len().saturating_sub(self.capacity);
        if overflow != 0 {
            guard.samples.drain(..overflow);
            self.dropped.fetch_add(overflow as u64, Ordering::Relaxed);
        }
        drop(guard);
        self.received
            .fetch_add(samples.len() as u64, Ordering::Relaxed);
        self.changed.notify_all();
    }

    fn on_disconnected(&self) {
        let mut guard = self.lock();
        guard.connected = false;
        drop(guard);
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use std::sync::Arc;

    const INSTANT: Duration = Duration::from_millis(10);

    fn connected_channel(capacity: usize) -> SampleChannel {
        let channel = SampleChannel::new(capacity);
        channel.on_connected(None);
        channel
    }

    #[test]
    fn a_fresh_channel_is_disconnected_and_empty() {
        let channel = SampleChannel::default();
        assert!(!channel.is_connected());
        assert!(channel.is_empty());
        assert_eq!(channel.received(), 0);
        assert_eq!(channel.dropped(), 0);
    }

    #[test]
    fn samples_come_back_in_order() {
        let channel = connected_channel(64);
        channel.on_samples(&[1.0, 2.0, 3.0]);
        let mut out = Vec::new();
        assert_eq!(channel.take(&mut out, 8, INSTANT), Taken::Samples);
        assert_eq!(out, vec![1.0, 2.0, 3.0]);
        assert_eq!(channel.received(), 3);
    }

    #[test]
    fn take_respects_its_maximum_and_leaves_the_rest() {
        let channel = connected_channel(64);
        channel.on_samples(&[1.0, 2.0, 3.0, 4.0]);
        let mut out = Vec::new();
        assert_eq!(channel.take(&mut out, 2, INSTANT), Taken::Samples);
        assert_eq!(out, vec![1.0, 2.0]);
        assert_eq!(channel.len(), 2);
    }

    #[test]
    fn an_idle_link_reports_idle_rather_than_disconnected() {
        let channel = connected_channel(64);
        let mut out = Vec::new();
        assert_eq!(channel.take(&mut out, 8, INSTANT), Taken::Idle);
        assert!(out.is_empty());
    }

    #[test]
    fn a_dropped_link_drains_before_it_reports_disconnected() {
        let channel = connected_channel(64);
        channel.on_samples(&[7.0, 8.0]);
        channel.on_disconnected();

        let mut out = Vec::new();
        assert_eq!(channel.take(&mut out, 8, INSTANT), Taken::Samples);
        assert_eq!(out, vec![7.0, 8.0]);
        assert_eq!(channel.take(&mut out, 8, INSTANT), Taken::Disconnected);
    }

    #[test]
    fn overflow_drops_the_oldest_samples() {
        let channel = connected_channel(4);
        channel.on_samples(&[1.0, 2.0, 3.0]);
        channel.on_samples(&[4.0, 5.0, 6.0]);
        assert_eq!(channel.dropped(), 2);

        let mut out = Vec::new();
        assert_eq!(channel.take(&mut out, 16, INSTANT), Taken::Samples);
        assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn reset_discards_what_arrived_while_idle() {
        let channel = connected_channel(64);
        channel.on_samples(&[1.0, 2.0]);
        channel.reset();
        assert!(channel.is_empty());
        // The lifetime counter is unaffected: those samples really did arrive.
        assert_eq!(channel.received(), 2);
    }

    #[test]
    fn reconnecting_clears_whatever_the_previous_link_left() {
        let channel = connected_channel(64);
        channel.on_samples(&[1.0]);
        channel.on_disconnected();
        channel.on_connected(None);
        assert!(channel.is_empty());
        assert!(channel.is_connected());
    }

    #[test]
    fn a_waiting_consumer_is_woken_by_a_push() {
        let channel = Arc::new(connected_channel(64));
        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            producer.on_samples(&[42.0]);
        });

        let mut out = Vec::new();
        assert_eq!(
            channel.take(&mut out, 8, Duration::from_secs(5)),
            Taken::Samples
        );
        assert_eq!(out, vec![42.0]);
        join.join().unwrap();
    }

    #[test]
    fn a_waiting_consumer_is_woken_by_a_disconnect() {
        let channel = Arc::new(connected_channel(64));
        let producer = Arc::clone(&channel);
        let join = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            producer.on_disconnected();
        });

        let mut out = Vec::new();
        assert_eq!(
            channel.take(&mut out, 8, Duration::from_secs(5)),
            Taken::Disconnected
        );
        join.join().unwrap();
    }

    #[test]
    fn an_empty_batch_is_ignored() {
        let channel = connected_channel(64);
        channel.on_samples(&[]);
        assert_eq!(channel.received(), 0);
    }

    #[test]
    fn wait_connected_times_out_when_nobody_dials_in() {
        let channel = SampleChannel::default();
        assert!(!channel.wait_connected(Duration::from_millis(20)));
        channel.on_connected(None);
        assert!(channel.wait_connected(Duration::from_millis(20)));
    }

    #[test]
    fn a_zero_capacity_channel_still_holds_one_sample() {
        let channel = connected_channel(0);
        channel.on_samples(&[1.0, 2.0]);
        let mut out = Vec::new();
        assert_eq!(channel.take(&mut out, 8, INSTANT), Taken::Samples);
        assert_eq!(out, vec![2.0]);
    }
}
