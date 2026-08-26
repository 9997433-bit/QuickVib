//! Where the M300 SDK library is looked for, in what order, and what a failure looks like.
//!
//! This is deliberately plain path arithmetic with no operating-system loader in it, so the probe
//! order — the thing a field engineer actually needs to reason about when a test cell reports
//! `-241,"Hardware missing"` — is unit-tested on Linux like any other pure function. Phase 6 adds
//! the Windows half: hand [`Candidate::path`] to `libloading::Library::new` in order, and turn the
//! accumulated failures into a [`ResolveError`].
//!
//! The order is fixed by `docs/M300-NATIVE.md` 1:
//!
//! 1. the project's `device.sdkPath`,
//! 2. the [`SDK_PATH_ENV`] environment variable,
//! 3. the directory containing `quickvib.exe`,
//! 4. the process's default DLL search path (the bare file name, resolved by the OS loader).

use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};

/// File name of the vendor library, as it ships in `M300SDK_v1.2.0` under `x64\bin`.
///
/// All lowercase, no version suffix (`docs/M300-NATIVE.md` §1). A bench that has the vendor's
/// MinGW-packaged build instead has `libm300_sdk.dll`, which is why the name is overridable.
pub const DEFAULT_LIBRARY_NAME: &str = "m300_sdk.dll";

/// Environment variable naming a directory to probe for the vendor library.
pub const SDK_PATH_ENV: &str = "QUICKVIB_M300_SDK";

/// SCPI error a failed resolution is reported as.
///
/// `-241,"Hardware missing"` — a missing SDK is an error code the UTS can act on, never a process
/// that refuses to start (`docs/PLAN.md` 15.6).
pub const SCPI_HARDWARE_MISSING: i16 = -241;

/// Why a particular directory was probed, so the log line says where the path came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CandidateOrigin {
    /// The loaded project's `device.sdkPath`.
    ProjectSdkPath,
    /// The [`SDK_PATH_ENV`] environment variable.
    Environment,
    /// The directory containing the running executable.
    ExecutableDirectory,
    /// The operating system's default library search path.
    SystemSearchPath,
}

impl CandidateOrigin {
    /// Short stable label for logs and error messages.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProjectSdkPath => "project device.sdkPath",
            Self::Environment => SDK_PATH_ENV,
            Self::ExecutableDirectory => "executable directory",
            Self::SystemSearchPath => "system search path",
        }
    }
}

impl fmt::Display for CandidateOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One location the loader will try, in probe order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Full path to try, or a bare file name for [`CandidateOrigin::SystemSearchPath`].
    pub path: PathBuf,
    /// Where this candidate came from.
    pub origin: CandidateOrigin,
}

/// Everything the probe order depends on, gathered by the caller.
///
/// Taking these as parameters rather than reading the environment inside [`candidates`] is what
/// makes the order testable; [`ProbeInputs::from_environment`] is the convenience wrapper that
/// does the reading.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProbeInputs<'a> {
    /// The loaded project's `device.sdkPath`, if it set one.
    pub project_sdk_path: Option<&'a Path>,
    /// Value of [`SDK_PATH_ENV`], if set.
    pub env_sdk_path: Option<&'a Path>,
    /// Directory containing the running executable, if it could be determined.
    pub executable_dir: Option<&'a Path>,
    /// Library file name; [`DEFAULT_LIBRARY_NAME`] when `None`.
    pub library_name: Option<&'a str>,
}

impl<'a> ProbeInputs<'a> {
    /// Build inputs from the process environment, with `project_sdk_path` supplied by the caller.
    ///
    /// Reads [`SDK_PATH_ENV`] and `std::env::current_exe`. A missing or unreadable value is simply
    /// one fewer candidate, never an error: the point of the probe list is that the next entry
    /// gets a turn.
    pub fn from_environment(
        project_sdk_path: Option<&'a Path>,
        env_sdk_path: Option<&'a Path>,
        executable_dir: Option<&'a Path>,
    ) -> Self {
        Self {
            project_sdk_path,
            env_sdk_path,
            executable_dir,
            library_name: None,
        }
    }

    /// The library file name these inputs select.
    pub fn library_name(&self) -> &str {
        self.library_name.unwrap_or(DEFAULT_LIBRARY_NAME)
    }
}

/// The locations to try, in order, with duplicates removed.
///
/// The final entry is always the bare library name, which lets the operating system's default
/// search path have the last word — that is how a vendor installer that put the DLL in
/// `System32` or on `PATH` still works without any configuration.
pub fn candidates(inputs: &ProbeInputs<'_>) -> Vec<Candidate> {
    let name = inputs.library_name();
    let dirs = [
        (inputs.project_sdk_path, CandidateOrigin::ProjectSdkPath),
        (inputs.env_sdk_path, CandidateOrigin::Environment),
        (inputs.executable_dir, CandidateOrigin::ExecutableDirectory),
    ];

    let mut out: Vec<Candidate> = Vec::with_capacity(dirs.len() + 1);
    for (dir, origin) in dirs {
        if let Some(dir) = dir {
            push_unique(
                &mut out,
                Candidate {
                    path: dir.join(name),
                    origin,
                },
            );
        }
    }
    push_unique(
        &mut out,
        Candidate {
            path: PathBuf::from(name),
            origin: CandidateOrigin::SystemSearchPath,
        },
    );
    out
}

fn push_unique(out: &mut Vec<Candidate>, candidate: Candidate) {
    if !out.iter().any(|existing| existing.path == candidate.path) {
        out.push(candidate);
    }
}

/// A library that was found and opened, and where it was found.
///
/// Phase 6 gives this an owning handle to the loaded library; today it records only the outcome,
/// which is what gets logged once at `info` after a successful open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLibrary {
    /// Path the loader actually opened.
    pub path: PathBuf,
    /// Which probe-order entry won.
    pub origin: CandidateOrigin,
}

/// One failed attempt: what was tried and what the operating system said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attempt {
    /// The location that was tried.
    pub candidate: Candidate,
    /// The loader's error, rendered as text at the point of failure.
    pub reason: String,
}

/// Every probed location failed.
///
/// Carries all attempts rather than just the last one, because "it could not find the DLL" is
/// useless on a locked-down test host and "it looked in these four places, and here is what each
/// said" is actionable. Maps to [`SCPI_HARDWARE_MISSING`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveError {
    attempts: Vec<Attempt>,
}

impl ResolveError {
    /// Build the error from the attempts made, in probe order.
    pub fn new(attempts: Vec<Attempt>) -> Self {
        Self { attempts }
    }

    /// The attempts made, in probe order.
    pub fn attempts(&self) -> &[Attempt] {
        &self.attempts
    }

    /// The SCPI error code this maps to: always [`SCPI_HARDWARE_MISSING`].
    pub const fn scpi_code(&self) -> i16 {
        SCPI_HARDWARE_MISSING
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "M300 SDK library not found; probed {} location(s)",
            self.attempts.len()
        )?;
        for attempt in &self.attempts {
            write!(
                f,
                "; [{}] {}: {}",
                attempt.candidate.origin,
                attempt.candidate.path.display(),
                attempt.reason
            )?;
        }
        Ok(())
    }
}

impl Error for ResolveError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(project: &'a str, env: &'a str, exe: &'a str) -> ProbeInputs<'a> {
        ProbeInputs {
            project_sdk_path: Some(Path::new(project)),
            env_sdk_path: Some(Path::new(env)),
            executable_dir: Some(Path::new(exe)),
            library_name: None,
        }
    }

    #[test]
    fn probe_order_is_project_then_env_then_exe_then_system() {
        let found = candidates(&inputs("/proj", "/env", "/exe"));
        let origins: Vec<CandidateOrigin> = found.iter().map(|c| c.origin).collect();
        assert_eq!(
            origins,
            vec![
                CandidateOrigin::ProjectSdkPath,
                CandidateOrigin::Environment,
                CandidateOrigin::ExecutableDirectory,
                CandidateOrigin::SystemSearchPath,
            ]
        );
        assert_eq!(found[0].path, Path::new("/proj").join(DEFAULT_LIBRARY_NAME));
        assert_eq!(found[3].path, Path::new(DEFAULT_LIBRARY_NAME));
    }

    #[test]
    fn absent_inputs_are_skipped_but_the_system_path_always_remains() {
        let found = candidates(&ProbeInputs::default());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].origin, CandidateOrigin::SystemSearchPath);
    }

    #[test]
    fn only_the_inputs_that_are_present_are_probed() {
        let exe = PathBuf::from("/opt/quickvib");
        let found = candidates(&ProbeInputs {
            executable_dir: Some(&exe),
            ..ProbeInputs::default()
        });
        let origins: Vec<CandidateOrigin> = found.iter().map(|c| c.origin).collect();
        assert_eq!(
            origins,
            vec![
                CandidateOrigin::ExecutableDirectory,
                CandidateOrigin::SystemSearchPath,
            ]
        );
    }

    #[test]
    fn duplicate_directories_are_probed_once() {
        let found = candidates(&inputs("/same", "/same", "/same"));
        assert_eq!(found.len(), 2, "one directory plus the system search path");
        assert_eq!(found[0].origin, CandidateOrigin::ProjectSdkPath);
    }

    #[test]
    fn library_name_is_overridable_for_a_renamed_vendor_dll() {
        let found = candidates(&ProbeInputs {
            project_sdk_path: Some(Path::new("/proj")),
            library_name: Some("libm300_sdk.dll"),
            ..ProbeInputs::default()
        });
        assert_eq!(found[0].path, Path::new("/proj/libm300_sdk.dll"));
        assert_eq!(found[1].path, Path::new("libm300_sdk.dll"));
    }

    #[test]
    fn from_environment_defaults_the_library_name() {
        let probe = ProbeInputs::from_environment(None, None, None);
        assert_eq!(probe.library_name(), DEFAULT_LIBRARY_NAME);
    }

    #[test]
    fn resolve_error_names_every_path_it_tried() {
        let attempts: Vec<Attempt> = candidates(&inputs("/proj", "/env", "/exe"))
            .into_iter()
            .map(|candidate| Attempt {
                candidate,
                reason: "no such file".to_owned(),
            })
            .collect();
        let err = ResolveError::new(attempts);

        assert_eq!(err.scpi_code(), SCPI_HARDWARE_MISSING);
        assert_eq!(err.attempts().len(), 4);

        let rendered = err.to_string();
        assert!(rendered.contains("probed 4 location(s)"), "{rendered}");
        assert!(rendered.contains("/proj"), "{rendered}");
        assert!(rendered.contains("/env"), "{rendered}");
        assert!(rendered.contains("/exe"), "{rendered}");
        assert!(rendered.contains(SDK_PATH_ENV), "{rendered}");
    }
}
