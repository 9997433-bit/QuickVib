//! Windows-only interop with the M300 laser Doppler vibrometer SDK v1.2.0.
//!
//! **This crate is scaffolding.** It contains no `unsafe` code, no `extern "C"` declarations and
//! no SDK calls yet, because the vendor header for SDK v1.2.0 is not in hand — see Q-A, Q-B and
//! Q-C in `docs/PLAN.md` 21.1, which gate Phase 6. What exists today is the shape the real
//! implementation drops into: the crate boundary, the `bindgen` build hook behind a non-default
//! feature, and the library-resolution order that decides where the vendor DLL is looked for.
//!
//! Per D21 there is **no fake or stub `M300Sdk.dll` in this repository**, and none will be added.
//! The ABI is captured as a reviewed contract in `docs/M300-NATIVE.md` and verified by a manual
//! Windows smoke run against real hardware.
//!
//! # Why this crate is separate
//!
//! It is the only member of the workspace that is allowed to contain `unsafe` (D22) and the only
//! one bound to Windows. It is excluded from the workspace's `default-members` (D2), so a plain
//! `cargo build`, `cargo test` or `cargo clippy` on Linux never compiles it. It is still kept
//! compiling on every host — CI's `--workspace` lint and test jobs do build it — so a mistake here
//! surfaces in Linux CI rather than on the bench:
//!
//! * everything that touches the operating system's loader is behind `#[cfg(windows)]`;
//! * everything that is plain path arithmetic ([`resolver`]) is platform-neutral and unit-tested
//!   on Linux, because "which directory do we look in, in what order" is exactly the kind of rule
//!   that is cheap to test and annoying to debug on a test-cell machine.
//!
//! # Unsafe policy
//!
//! Every other crate is `#![forbid(unsafe_code)]`. This one is `#![deny(unsafe_op_in_unsafe_fn)]`,
//! and when FFI declarations land each `unsafe` block must carry a `SAFETY:` comment naming the
//! row of `docs/M300-NATIVE.md` 2 it depends on. Rust FFI has no marshalling safety net: a wrong
//! signature is undefined behavior, not an exception.

#![deny(unsafe_op_in_unsafe_fn)]

pub mod resolver;

pub use resolver::{Candidate, CandidateOrigin, ProbeInputs, ResolveError, ResolvedLibrary};

/// Whether this build can talk to a real M300 at all.
///
/// `false` on any non-Windows target. The binary's backend factory uses this to refuse
/// `--backend m300` at startup with a clear error instead of silently falling back to the mock
/// (`docs/PLAN.md` 7.1, 15.3).
pub const IS_SUPPORTED_TARGET: bool = cfg!(windows);

/// SDK major/minor version this crate is written against.
///
/// Asserted at open time if — and only if — the ABI turns out to expose a version query
/// (`docs/M300-NATIVE.md` 1); a mismatch is logged, never fatal.
pub const EXPECTED_SDK_VERSION: &str = "1.2";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_target_matches_cfg() {
        assert_eq!(IS_SUPPORTED_TARGET, cfg!(windows));
    }
}
