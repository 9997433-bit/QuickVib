//! Windows-only interop with the M300 laser Doppler vibrometer SDK v1.2.0.
//!
//! [`M300Backend`] is the `DeviceBackend` that drives real hardware: it loads the vendor DLL at
//! run time, lets the SDK own the listening socket, and turns the samples the SDK pushes at it
//! into the batches the engine pulls. The ABI it is written against — every signature, every
//! ownership rule, every enum index — is documented in `docs/M300-NATIVE.md`, read out of the
//! vendor's own headers, PDFs and sample code.
//!
//! Per D21 there is **no fake or stub `m300_sdk.dll` in this repository**, and none will be
//! added; the vendor's real DLL is not committed either. That means the last mile is verified by
//! the manual Windows smoke run of `docs/M300-NATIVE.md` §9 against real hardware, and everything
//! upstream of it is arranged so that as little as possible depends on that run.
//!
//! # Why this crate is separate
//!
//! It is the only member of the workspace that is allowed to contain `unsafe` (D22) and the only
//! one bound to Windows. It is excluded from the workspace's `default-members` (D2), so a plain
//! `cargo build`, `cargo test` or `cargo clippy` on Linux never compiles it. It is still kept
//! compiling and tested on every host — CI's `--workspace` lint and test jobs do build it — so a
//! mistake surfaces in Linux CI rather than on the bench:
//!
//! | Module | Platform | What it holds |
//! | --- | --- | --- |
//! | [`resolver`] | any | Which directory the DLL is looked for in, in what order |
//! | [`maps`] | any | The vendor's enum indices against QuickVib's hertz, units and ranges |
//! | [`error`] | any | Native status codes against `DeviceError` and the SCPI catalogue |
//! | [`batch`] | any | What the data callback may believe about the buffer it was handed |
//! | [`shim`] | any | The `extern "C"` callbacks, the queue they fill and the drain loop |
//! | [`ffi_generated`] | any | The reviewed `bindgen` declaration snapshot |
//! | `ffi` | Windows | The `libloading` symbol table: every SDK call there is |
//! | `backend` | Windows | [`M300Backend`] itself |
//!
//! Only the last two need the operating system's loader, and they are the only two a bench can
//! break without Linux CI noticing.
//!
//! # Unsafe policy
//!
//! Every other crate is `#![forbid(unsafe_code)]`. This one is `#![deny(unsafe_op_in_unsafe_fn)]`
//! and every `unsafe` block carries a `SAFETY:` comment naming the row of `docs/M300-NATIVE.md`
//! §2 it depends on. Rust FFI has no marshalling safety net: a wrong signature is undefined
//! behavior — silent memory corruption, or a plausible-looking wrong measurement — not an
//! exception anybody can catch. Declaring a prototype is not calling it: [`ffi_generated`] is
//! inert, and nothing links a single vendor symbol.

#![deny(unsafe_op_in_unsafe_fn)]

/// `bindgen` output for the vendor headers `m300.h` + `m300_macros.h`, committed as a reviewed
/// snapshot (`docs/M300-NATIVE.md` §4).
///
/// Declarations only: type aliases, `#[repr(C)]` structs, callback function types and `extern "C"`
/// prototypes. Nothing in this module is called yet, and no symbol here is linked — the DLL is
/// opened at runtime through `libloading`, so these prototypes exist to give
/// `Library::get::<unsafe extern "C" fn(..)>` a signature to check against.
#[allow(non_camel_case_types, non_upper_case_globals, missing_docs)]
pub mod ffi_generated;

pub mod batch;
pub mod error;
pub mod maps;
pub mod resolver;
pub mod shim;

#[cfg(windows)]
pub mod backend;
#[cfg(windows)]
pub mod ffi;

pub use batch::BatchFault;
pub use error::{NativeError, NativeFailure};
pub use maps::{DeviceIdentity, FilterChoice, RangeSpan, RateChoice};
pub use resolver::{Candidate, CandidateOrigin, ProbeInputs, ResolveError, ResolvedLibrary};
pub use shim::{Drain, Handle, Shared};

#[cfg(windows)]
pub use backend::{M300Backend, DEFAULT_STALL_TIMEOUT};
#[cfg(windows)]
pub use ffi::{LoadError, Sdk};

/// Whether this build can talk to a real M300 at all.
///
/// `false` on any non-Windows target. The binary's backend factory uses this to refuse
/// `--backend m300` at startup with a clear error instead of silently falling back to the mock
/// (`docs/PLAN.md` 7.1, 15.3).
pub const IS_SUPPORTED_TARGET: bool = cfg!(windows);

/// SDK major/minor version this crate is written against.
///
/// Checked at open time against `m300_sdk_version_numbers` (`docs/M300-NATIVE.md` §1). A
/// mismatch is logged, never fatal: the ABI is stable across the vendor's patch releases, and
/// refusing to record because a bench has 1.2.1 installed would be worse than the risk it
/// avoids.
pub const EXPECTED_SDK_VERSION: &str = "1.2";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_target_matches_cfg() {
        assert_eq!(IS_SUPPORTED_TARGET, cfg!(windows));
    }

    #[test]
    fn the_expected_version_is_the_one_the_symbol_table_checks_for() {
        // Two spellings of the same fact — the constant the binary logs and the numbers `ffi`
        // compares against — kept from drifting apart.
        assert_eq!(
            EXPECTED_SDK_VERSION,
            format!(
                "{}.{}",
                ffi_generated::VERSION_MAJOR,
                ffi_generated::VERSION_MINOR
            )
        );
    }
}
