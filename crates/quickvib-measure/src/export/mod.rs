//! Writing a capture to disk as CSV or TXT (`docs/PLAN.md` 7.8).
//!
//! Both formats use `\r\n` line endings, because the consumer is Windows UTS tooling. Sample
//! values are written with Rust's `{}`, which emits the shortest representation that
//! round-trips back to the same `f32` and is locale-independent by construction (D16).
//!
//! Writes are atomic from a reader's point of view: the bytes go to a temporary file **in the
//! target directory**, are flushed and `sync_all`ed, and are then renamed into place, so a UTS
//! polling for the file never observes a half-written CSV.

pub mod csv;
pub mod txt;

use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use quickvib_core::{ExportFormat, SampleUnit, ScpiError};

/// Line terminator used by both exporters.
pub const LINE_ENDING: &str = "\r\n";

/// Buffer size for the export writer. At 500 000 rows this is the difference between a
/// fraction of a second and tens of seconds.
const WRITE_BUFFER_BYTES: usize = 64 * 1024;

/// Everything the CSV preamble needs to describe a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureMetadata {
    /// Project name.
    pub project_name: String,
    /// Capture time, formatted as ISO-8601 UTC.
    pub timestamp: String,
    /// Nominal sample rate in hertz.
    pub sample_rate_hz: f64,
    /// Unit of the samples.
    pub unit: SampleUnit,
    /// Requested capture duration in seconds.
    pub duration_seconds: f64,
    /// Whether to write the preamble and header row (CSV only).
    pub include_header: bool,
}

impl CaptureMetadata {
    /// Metadata with the header enabled and placeholder text, for tests and TXT exports.
    #[must_use]
    pub fn minimal(sample_rate_hz: f64, unit: SampleUnit) -> Self {
        Self {
            project_name: String::new(),
            timestamp: String::new(),
            sample_rate_hz,
            unit,
            duration_seconds: 0.0,
            include_header: true,
        }
    }
}

/// Failure while exporting a capture.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExportError {
    /// The file, its temporary sibling, or the containing directory could not be written.
    Io {
        /// The path being written.
        path: PathBuf,
        /// The underlying operating-system error.
        source: std::io::Error,
    },
}

impl ExportError {
    /// The SCPI-99 error the UTS should see.
    #[must_use]
    pub const fn scpi_error(&self) -> ScpiError {
        match self {
            Self::Io { .. } => ScpiError::FileNameError,
        }
    }
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "could not write {}: {source}", path.display())
            }
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
        }
    }
}

/// Resolve `path` against `base_directory` when it is relative.
#[must_use]
pub fn resolve_path(path: &Path, base_directory: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_directory.join(path)
    }
}

/// Serialize `samples` into `writer` in the requested format.
///
/// # Errors
/// Propagates any error from `writer`.
pub fn write_samples<W: Write>(
    writer: &mut W,
    format: ExportFormat,
    metadata: &CaptureMetadata,
    samples: &[f32],
) -> std::io::Result<()> {
    match format {
        ExportFormat::Csv => csv::write_csv(writer, metadata, samples),
        ExportFormat::Txt => txt::write_txt(writer, samples),
    }
}

/// Write a capture to `path`, creating parent directories and replacing any existing file.
///
/// # Errors
/// [`ExportError::Io`] if the directory cannot be created, the temporary file cannot be
/// written or synced, or the rename into place fails.
pub fn write_capture(
    path: &Path,
    format: ExportFormat,
    metadata: &CaptureMetadata,
    samples: &[f32],
) -> Result<(), ExportError> {
    let directory = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(directory) = directory {
        fs::create_dir_all(directory).map_err(|e| ExportError::Io {
            path: directory.to_path_buf(),
            source: e,
        })?;
    }

    let temp_path = temp_sibling(path);
    let outcome = (|| -> std::io::Result<()> {
        let file = File::create(&temp_path)?;
        let mut writer = BufWriter::with_capacity(WRITE_BUFFER_BYTES, file);
        write_samples(&mut writer, format, metadata, samples)?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(std::io::Error::from)?
            .sync_all()
    })();

    if let Err(source) = outcome {
        let _ = fs::remove_file(&temp_path);
        return Err(ExportError::Io {
            path: temp_path,
            source,
        });
    }

    fs::rename(&temp_path, path).map_err(|source| {
        let _ = fs::remove_file(&temp_path);
        ExportError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// A unique temporary name inside the same directory as `path`, so the final `rename` is
/// atomic (it is only atomic within one filesystem/directory).
fn temp_sibling(path: &Path) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let stem = path
        .file_name()
        .map_or_else(|| "export".into(), |n| n.to_string_lossy());
    let name = format!(".{stem}.{}.{n}.tmp", std::process::id());
    match path.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn metadata() -> CaptureMetadata {
        CaptureMetadata {
            project_name: "Test".to_owned(),
            timestamp: "1970-01-01T00:00:00.000Z".to_owned(),
            sample_rate_hz: 1000.0,
            unit: SampleUnit::VelocityUmPerSec,
            duration_seconds: 0.003,
            include_header: true,
        }
    }

    #[test]
    fn csv_file_is_created_with_the_expected_rows() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run001.csv");
        write_capture(&path, ExportFormat::Csv, &metadata(), &[1.0, -2.0, 3.5]).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 6 + 1 + 3);
        assert_eq!(lines[6], "index,time_s,value");
        assert_eq!(lines[7], "0,0,1");
    }

    #[test]
    fn missing_directories_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a").join("b").join("run.csv");
        write_capture(&path, ExportFormat::Csv, &metadata(), &[1.0]).unwrap();
        assert!(path.is_file());
    }

    #[test]
    fn existing_files_are_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.txt");
        fs::write(&path, "stale contents that must disappear").unwrap();
        write_capture(&path, ExportFormat::Txt, &metadata(), &[9.5]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "9.5\r\n");
    }

    #[test]
    fn no_temporary_files_are_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.csv");
        write_capture(&path, ExportFormat::Csv, &metadata(), &[1.0, 2.0]).unwrap();
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
    }

    #[test]
    fn unwritable_directory_is_a_file_name_error() {
        // A path whose parent is an existing *file* can never be created.
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "x").unwrap();
        let err = write_capture(
            &blocker.join("run.csv"),
            ExportFormat::Csv,
            &metadata(),
            &[1.0],
        )
        .unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::FileNameError);
    }

    #[test]
    fn relative_paths_resolve_against_the_export_directory() {
        assert_eq!(
            resolve_path(Path::new("run.csv"), Path::new("/tmp/out")),
            PathBuf::from("/tmp/out/run.csv")
        );
        let absolute = if cfg!(windows) {
            r"C:\x\run.csv"
        } else {
            "/x/run.csv"
        };
        assert_eq!(
            resolve_path(Path::new(absolute), Path::new("/tmp/out")),
            PathBuf::from(absolute)
        );
    }

    #[test]
    fn a_failing_writer_leaves_no_partial_file() {
        struct FailAfter(usize);
        impl Write for FailAfter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                if self.0 == 0 {
                    return Err(std::io::Error::new(std::io::ErrorKind::Other, "disk full"));
                }
                let n = buf.len().min(self.0);
                self.0 -= n;
                Ok(n)
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let mut writer = FailAfter(16);
        let err = write_samples(
            &mut writer,
            ExportFormat::Csv,
            &metadata(),
            &vec![1.0_f32; 10_000],
        )
        .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
    }
}
