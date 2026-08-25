//! Injected time (`docs/PLAN.md` D20).
//!
//! `Instant::now` and `SystemTime::now` are banned outside this module by the
//! `disallowed-methods` entries in `clippy.toml`. Everything that needs the time takes an
//! `Arc<dyn Clock>`, so a "5 second" capture runs in milliseconds under test.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// A source of monotonic and wall-clock time, plus the ability to wait.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// Monotonic time since this clock was created. Never goes backwards.
    fn monotonic(&self) -> Duration;

    /// Wall-clock time, for log lines and CSV preambles only.
    fn wall_clock(&self) -> SystemTime;

    /// Block for `dur`. A test clock satisfies this by advancing virtual time instead.
    fn sleep(&self, dur: Duration);
}

/// The production clock: real monotonic time from [`Instant`], real wall time from
/// [`SystemTime`], and a real [`std::thread::sleep`].
#[derive(Debug)]
pub struct SystemClock {
    origin: Instant,
}

impl SystemClock {
    /// Create a clock whose monotonic zero is now.
    #[must_use]
    #[allow(clippy::disallowed_methods)] // D20: this module is the single legal call site.
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)] // D20: this module is the single legal call site.
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }

    #[allow(clippy::disallowed_methods)] // D20: this module is the single legal call site.
    fn wall_clock(&self) -> SystemTime {
        SystemTime::now()
    }

    fn sleep(&self, dur: Duration) {
        std::thread::sleep(dur);
    }
}

/// A clock whose time only moves when a test moves it.
///
/// [`Clock::sleep`] advances virtual time immediately rather than blocking, which is what makes
/// a nominally multi-second capture finish instantly in the test suite.
#[derive(Debug)]
pub struct TestClock {
    state: Mutex<TestClockState>,
    changed: Condvar,
}

#[derive(Debug)]
struct TestClockState {
    monotonic: Duration,
    wall: SystemTime,
}

impl TestClock {
    /// A clock starting at monotonic zero and at the given wall time.
    #[must_use]
    pub fn new(wall_start: SystemTime) -> Self {
        Self {
            state: Mutex::new(TestClockState {
                monotonic: Duration::ZERO,
                wall: wall_start,
            }),
            changed: Condvar::new(),
        }
    }

    /// A clock starting at monotonic zero and at the Unix epoch.
    #[must_use]
    pub fn at_epoch() -> Self {
        Self::new(UNIX_EPOCH)
    }

    /// Move both the monotonic and the wall clock forward by `dur`.
    pub fn advance(&self, dur: Duration) {
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        guard.monotonic += dur;
        guard.wall += dur;
        drop(guard);
        self.changed.notify_all();
    }
}

impl Default for TestClock {
    fn default() -> Self {
        Self::at_epoch()
    }
}

impl Clock for TestClock {
    fn monotonic(&self) -> Duration {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .monotonic
    }

    fn wall_clock(&self) -> SystemTime {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .wall
    }

    fn sleep(&self, dur: Duration) {
        self.advance(dur);
    }
}

/// Format a wall-clock instant as `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC).
///
/// Hand-rolled rather than pulling in `chrono`/`time` (D18): the product needs exactly this one
/// conversion, and the civil-from-days algorithm below is unit-tested against known epochs.
///
/// ```
/// use std::time::{Duration, UNIX_EPOCH};
/// use quickvib_core::clock::format_iso8601;
///
/// assert_eq!(format_iso8601(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
/// assert_eq!(
///     format_iso8601(UNIX_EPOCH + Duration::from_millis(1_709_164_800_500)),
///     "2024-02-29T00:00:00.500Z"
/// );
/// ```
#[must_use]
pub fn format_iso8601(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
    let total_secs = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();

    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a proleptic Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn test_clock_starts_at_zero_and_advances() {
        let clock = TestClock::at_epoch();
        assert_eq!(clock.monotonic(), Duration::ZERO);
        clock.advance(Duration::from_millis(1500));
        assert_eq!(clock.monotonic(), Duration::from_millis(1500));
        assert_eq!(
            format_iso8601(clock.wall_clock()),
            "1970-01-01T00:00:01.500Z"
        );
    }

    #[test]
    fn test_clock_sleep_does_not_block() {
        let clock = TestClock::at_epoch();
        clock.sleep(Duration::from_secs(3600));
        assert_eq!(clock.monotonic(), Duration::from_secs(3600));
    }

    #[test]
    fn system_clock_is_monotonic() {
        let clock = SystemClock::new();
        let a = clock.monotonic();
        let b = clock.monotonic();
        assert!(b >= a);
    }

    #[test]
    fn iso8601_known_epochs() {
        let cases: &[(u64, &str)] = &[
            (0, "1970-01-01T00:00:00.000Z"),
            (1, "1970-01-01T00:00:01.000Z"),
            (86_399, "1970-01-01T23:59:59.000Z"),
            (86_400, "1970-01-02T00:00:00.000Z"),
            // Year boundary.
            (946_684_799, "1999-12-31T23:59:59.000Z"),
            (946_684_800, "2000-01-01T00:00:00.000Z"),
            // Leap day (2000 is a leap year; the century rule's exception to the exception).
            (951_782_400, "2000-02-29T00:00:00.000Z"),
            // Leap day 2024.
            (1_709_164_800, "2024-02-29T00:00:00.000Z"),
            (1_709_251_200, "2024-03-01T00:00:00.000Z"),
            // Non-leap century.
            (4_107_542_400, "2100-03-01T00:00:00.000Z"),
        ];
        for (secs, expected) in cases {
            let t = UNIX_EPOCH + Duration::from_secs(*secs);
            assert_eq!(&format_iso8601(t), expected, "at {secs}");
        }
    }

    #[test]
    fn iso8601_milliseconds_are_zero_padded() {
        let t = UNIX_EPOCH + Duration::from_millis(7);
        assert_eq!(format_iso8601(t), "1970-01-01T00:00:00.007Z");
    }
}
