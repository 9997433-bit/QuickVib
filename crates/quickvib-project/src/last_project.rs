//! The auto-load-last record (`docs/PLAN.md` 7.5).
//!
//! The path of the most recently loaded or saved project is remembered in a tiny JSON file so
//! `MMEM:LOAD:AUTO` and startup can reopen it. The location is
//! `%LOCALAPPDATA%\QuickVib\` on Windows and `$XDG_STATE_HOME/quickvib/` (falling back to
//! `~/.local/state/quickvib/`) elsewhere — two branches of `std::env`, no `dirs` crate.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ProjectError;

/// Environment variable that overrides the state directory outright.
///
/// Primarily for tests and for locked-down UTS hosts where the per-user profile is not
/// writable.
pub const STATE_DIR_ENV: &str = "QUICKVIB_STATE_DIR";

/// File name of the record inside the state directory.
pub const RECORD_FILE_NAME: &str = "last-project.json";

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    path: PathBuf,
}

/// Resolve the directory QuickVib keeps its per-user state in.
///
/// Returns `None` when neither the override nor the platform variables are set, which is
/// treated as "auto-load is unavailable" rather than as an error.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(STATE_DIR_ENV) {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }

    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|v| PathBuf::from(v).join("QuickVib"))
    } else {
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            return Some(PathBuf::from(xdg).join("quickvib"));
        }
        std::env::var_os("HOME").filter(|v| !v.is_empty()).map(|v| {
            PathBuf::from(v)
                .join(".local")
                .join("state")
                .join("quickvib")
        })
    }
}

/// Reads and writes the last-project record in a specific directory.
#[derive(Debug, Clone)]
pub struct LastProjectStore {
    dir: PathBuf,
}

impl LastProjectStore {
    /// A store rooted at an explicit directory.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// A store rooted at the platform state directory, if one can be resolved.
    #[must_use]
    pub fn discover() -> Option<Self> {
        state_dir().map(Self::new)
    }

    /// The full path of the record file.
    #[must_use]
    pub fn record_path(&self) -> PathBuf {
        self.dir.join(RECORD_FILE_NAME)
    }

    /// Remember `project_path` as the most recently used project.
    ///
    /// The path is canonicalized when possible so a later run from a different working
    /// directory still finds it.
    ///
    /// # Errors
    /// [`ProjectError::Io`] if the directory cannot be created or the record cannot be written.
    pub fn record(&self, project_path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let path = project_path.as_ref();
        let absolute = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        fs::create_dir_all(&self.dir).map_err(|e| ProjectError::Io {
            path: self.dir.clone(),
            source: e,
        })?;
        let record = Record { path: absolute };
        let text = serde_json::to_string_pretty(&record).map_err(|e| ProjectError::Parse {
            path: Some(self.record_path()),
            message: e.to_string(),
        })?;
        fs::write(self.record_path(), text).map_err(|e| ProjectError::Io {
            path: self.record_path(),
            source: e,
        })
    }

    /// The recorded path, if a record exists and parses.
    ///
    /// A missing, unreadable or malformed record is reported as `None` rather than as an error:
    /// auto-load is a convenience and must never be fatal at startup.
    #[must_use]
    pub fn read(&self) -> Option<PathBuf> {
        let text = fs::read_to_string(self.record_path()).ok()?;
        let record: Record = serde_json::from_str(&text).ok()?;
        Some(record.path)
    }

    /// The recorded path, but only when the file it names still exists.
    #[must_use]
    pub fn read_existing(&self) -> Option<PathBuf> {
        self.read().filter(|p| p.is_file())
    }

    /// Delete the record, if any. Missing files are not an error.
    ///
    /// # Errors
    /// [`ProjectError::Io`] if the file exists but cannot be removed.
    pub fn clear(&self) -> Result<(), ProjectError> {
        match fs::remove_file(self.record_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ProjectError::Io {
                path: self.record_path(),
                source: e,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn record_then_read_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("Test.proj");
        fs::write(&project, "{}").unwrap();

        let store = LastProjectStore::new(dir.path().join("state"));
        assert!(store.read().is_none());
        store.record(&project).unwrap();

        let read = store.read().unwrap();
        assert!(read.ends_with("Test.proj"), "{read:?}");
        assert!(store.read_existing().is_some());
    }

    #[test]
    fn stale_recorded_path_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("Gone.proj");
        fs::write(&project, "{}").unwrap();

        let store = LastProjectStore::new(dir.path().join("state"));
        store.record(&project).unwrap();
        fs::remove_file(&project).unwrap();

        assert!(store.read().is_some());
        assert!(store.read_existing().is_none());
    }

    #[test]
    fn malformed_record_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = LastProjectStore::new(dir.path());
        fs::write(store.record_path(), "not json at all").unwrap();
        assert!(store.read().is_none());
    }

    #[test]
    fn clear_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = LastProjectStore::new(dir.path());
        store.clear().unwrap();
        fs::write(store.record_path(), "{\"path\":\"/tmp/x\"}").unwrap();
        store.clear().unwrap();
        assert!(store.read().is_none());
    }

    #[test]
    fn record_creates_the_state_directory() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("a").join("b").join("c");
        let store = LastProjectStore::new(&state);
        store.record(dir.path().join("x.proj")).unwrap();
        assert!(state.is_dir());
    }
}
