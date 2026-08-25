//! Reading and writing project files.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use crate::error::ProjectError;
use crate::schema::Project;

/// Stateless helpers for moving a [`Project`] between memory and disk.
#[derive(Debug, Clone, Copy)]
pub struct ProjectStore;

impl ProjectStore {
    /// Load and validate a project from `path`.
    ///
    /// # Errors
    /// [`ProjectError::NotFound`] when the file is missing, [`ProjectError::Io`] for any other
    /// read failure, [`ProjectError::Parse`] for malformed JSON, and the validation errors from
    /// [`crate::validate::validate`].
    pub fn load(path: impl AsRef<Path>) -> Result<Project, ProjectError> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).map_err(|e| match e.kind() {
            ErrorKind::NotFound => ProjectError::NotFound {
                path: path.to_path_buf(),
            },
            _ => ProjectError::Io {
                path: path.to_path_buf(),
                source: e,
            },
        })?;

        let project: Project = serde_json::from_str(&text).map_err(|e| ProjectError::Parse {
            path: Some(path.to_path_buf()),
            message: e.to_string(),
        })?;
        crate::validate::validate(&project)?;
        Ok(project)
    }

    /// Write `project` to `path` as pretty JSON, creating parent directories as needed.
    ///
    /// # Errors
    /// [`ProjectError::Io`] if the directory cannot be created or the file cannot be written.
    pub fn save(project: &Project, path: impl AsRef<Path>) -> Result<(), ProjectError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| ProjectError::Io {
                    path: parent.to_path_buf(),
                    source: e,
                })?;
            }
        }
        let mut text = project.to_json_string()?;
        text.push('\n');
        fs::write(path, text).map_err(|e| ProjectError::Io {
            path: path.to_path_buf(),
            source: e,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use quickvib_core::ScpiError;

    const SAMPLE: &str = r#"{
        "schemaVersion": 1,
        "name": "Store",
        "device": { "sampleRateHz": 2000.0, "unit": "acceleration_m_s2" },
        "recording": { "durationSeconds": 0.25 }
    }"#;

    #[test]
    fn missing_file_is_file_name_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = ProjectStore::load(dir.path().join("nope.proj")).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::FileNameNotFound);
    }

    #[test]
    fn malformed_json_is_illegal_parameter_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.proj");
        fs::write(&path, "{ not json").unwrap();
        let err = ProjectStore::load(&path).unwrap_err();
        assert_eq!(err.scpi_error(), ScpiError::IllegalParameterValue);
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("round.proj");
        let project = Project::from_json_str(SAMPLE).unwrap();
        ProjectStore::save(&project, &path).unwrap();
        assert_eq!(ProjectStore::load(&path).unwrap(), project);
    }

    #[test]
    fn saved_file_is_pretty_and_newline_terminated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pretty.proj");
        ProjectStore::save(&Project::from_json_str(SAMPLE).unwrap(), &path).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("\n  \"name\": \"Store\""), "{text}");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn runtime_overrides_are_persisted_by_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("override.proj");
        let mut project = Project::from_json_str(SAMPLE).unwrap();
        project.recording.duration_seconds = 0.75;
        project.export.format = quickvib_core::ExportFormat::Txt;
        ProjectStore::save(&project, &path).unwrap();
        let reloaded = ProjectStore::load(&path).unwrap();
        assert_eq!(reloaded.recording.duration_seconds, 0.75);
        assert_eq!(reloaded.export.format, quickvib_core::ExportFormat::Txt);
    }
}
