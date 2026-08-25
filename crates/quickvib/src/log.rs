//! The structured line logger (`docs/PLAN.md` 16).
//!
//! One line per event, `timestamp level component message key=value…`, UTC. `warn` and
//! `error` go to stderr, everything else to stdout, and each line is written under a mutex so
//! concurrent threads never interleave mid-line.

use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};

use quickvib_core::clock::format_iso8601;
use quickvib_core::{Clock, Level, Logger};

/// A [`Logger`] that writes the fixed QuickVib line format.
pub struct LineLogger {
    minimum: Level,
    clock: Arc<dyn Clock>,
    out: Mutex<Box<dyn Write + Send>>,
    err: Mutex<Box<dyn Write + Send>>,
}

impl std::fmt::Debug for LineLogger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LineLogger").field("minimum", &self.minimum).finish()
    }
}

impl LineLogger {
    /// A logger writing to the process's stdout and stderr.
    #[must_use]
    pub fn stdio(minimum: Level, clock: Arc<dyn Clock>) -> Self {
        Self::with_writers(
            minimum,
            clock,
            Box::new(std::io::stdout()),
            Box::new(std::io::stderr()),
        )
    }

    /// A logger writing to arbitrary sinks, so tests can capture the lines.
    #[must_use]
    pub fn with_writers(
        minimum: Level,
        clock: Arc<dyn Clock>,
        out: Box<dyn Write + Send>,
        err: Box<dyn Write + Send>,
    ) -> Self {
        Self { minimum, clock, out: Mutex::new(out), err: Mutex::new(err) }
    }

    /// Format one record exactly as it is written.
    #[must_use]
    pub fn format_record(
        timestamp: &str,
        level: Level,
        component: &str,
        message: &str,
    ) -> String {
        format!("{timestamp} {:<5} {component:<7} {message}\n", level.as_str())
    }
}

impl Logger for LineLogger {
    fn log(&self, level: Level, component: &str, message: &str) {
        if level < self.minimum {
            return;
        }
        let line = Self::format_record(
            &format_iso8601(self.clock.wall_clock()),
            level,
            component,
            message,
        );
        let sink = if level.is_diagnostic() { &self.err } else { &self.out };
        let mut guard = sink.lock().unwrap_or_else(PoisonError::into_inner);
        // A logger that cannot write must not take the instrument down with it.
        let _ = guard.write_all(line.as_bytes());
        let _ = guard.flush();
    }

    fn enabled(&self, level: Level) -> bool {
        level >= self.minimum
    }
}

/// A writer that appends every line into a shared buffer, for tests.
#[derive(Debug, Clone, Default)]
pub struct CaptureWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    /// A writer with an empty buffer.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything written so far, split into lines.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let guard = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&guard)
            .lines()
            .map(ToOwned::to_owned)
            .collect()
    }
}

impl Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut guard = self.buffer.lock().unwrap_or_else(PoisonError::into_inner);
        guard.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::TestClock;
    use std::time::Duration;

    fn logger(minimum: Level) -> (LineLogger, CaptureWriter, CaptureWriter) {
        let clock = Arc::new(TestClock::at_epoch());
        clock.advance(Duration::from_millis(1_767_225_600_123));
        let out = CaptureWriter::new();
        let err = CaptureWriter::new();
        let logger = LineLogger::with_writers(
            minimum,
            clock,
            Box::new(out.clone()),
            Box::new(err.clone()),
        );
        (logger, out, err)
    }

    #[test]
    fn the_line_format_matches_the_reference() {
        let (logger, out, _) = logger(Level::Info);
        logger.log(Level::Info, "scpi", "listening port=5025");
        assert_eq!(out.lines(), vec!["2026-01-01T00:00:00.123Z INFO  scpi    listening port=5025"]);
    }

    #[test]
    fn warnings_and_errors_go_to_stderr() {
        let (logger, out, err) = logger(Level::Trace);
        logger.log(Level::Info, "rec", "started");
        logger.log(Level::Warn, "rec", "rateDrift expected=100000 actual=99762");
        logger.log(Level::Error, "dev", "link lost");
        assert_eq!(out.lines().len(), 1);
        assert_eq!(err.lines().len(), 2);
        assert!(err.lines()[0].contains("WARN"));
        assert!(err.lines()[1].contains("ERROR"));
    }

    #[test]
    fn records_below_the_threshold_are_dropped() {
        let (logger, out, _) = logger(Level::Warn);
        logger.log(Level::Info, "scpi", "chatty");
        logger.log(Level::Debug, "scpi", "chattier");
        assert!(out.lines().is_empty());
        assert!(!logger.enabled(Level::Info));
        assert!(logger.enabled(Level::Warn));
    }

    #[test]
    fn concurrent_writers_never_interleave_within_a_line() {
        let (logger, out, _) = logger(Level::Trace);
        let logger = Arc::new(logger);
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let logger = Arc::clone(&logger);
                std::thread::spawn(move || {
                    for n in 0..50 {
                        logger.log(Level::Info, "test", &format!("thread={i} n={n}"));
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let lines = out.lines();
        assert_eq!(lines.len(), 400);
        for line in lines {
            assert_eq!(line.matches("thread=").count(), 1, "{line}");
            assert!(line.starts_with("2026-"), "{line}");
        }
    }

    #[test]
    fn components_are_padded_so_columns_line_up() {
        let (logger, out, _) = logger(Level::Trace);
        logger.log(Level::Info, "scpi", "a");
        logger.log(Level::Info, "device", "b");
        let lines = out.lines();
        let first = lines[0].find(" a").unwrap();
        let second = lines[1].find(" b").unwrap();
        assert_eq!(first, second);
    }
}
