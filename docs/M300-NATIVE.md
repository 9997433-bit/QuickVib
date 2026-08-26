# M300 SDK v1.2.0 — native ABI contract

> **Status: placeholder / contract skeleton.** The real SDK v1.2.0 header is not in hand, so every
> entry point, signature, and error code below is marked `TBD` and must be filled in — from the
> vendor header, not from guesswork — before `crates/quickvib-m300/src/ffi.rs` is written. This
> document is the review artifact that stands in for the part of the product CI cannot exercise
> (`docs/PLAN.md` §11, §15, §20).
>
> Nothing here is implemented yet. `crates/quickvib-m300` currently contains **scaffolding only**:
> a Windows-gated crate, a `bindgen`-gated `build.rs`, and a resolver skeleton with no real SDK
> calls. See "Current implementation status" at the end.

## 0. Ground rules

These are non-negotiable constraints from the plan; they shape everything below.

| Rule | Source | Consequence |
| --- | --- | --- |
| **No fake/stub `M300Sdk.dll`, no stub `.lib`, no binaries committed** | D21, §22 | The native path is verified by a manual Windows smoke run against real hardware, never by a fake library |
| **`unsafe` lives in exactly one crate** | D22 | `quickvib-m300` is the only crate without `#![forbid(unsafe_code)]`; it uses `#![deny(unsafe_op_in_unsafe_fn)]` and one `SAFETY:` comment per block |
| **Runtime loading, not link-time import** | §11, D18 | `libloading::Library::new` + `get::<unsafe extern "C" fn(..)>`; the shipped `quickvib.exe` has no import-library dependency on the SDK and starts fine without it (in mock mode) |
| **Excluded from `default-members`** | D2 | A plain `cargo build` / `cargo test` / `cargo clippy` on Linux never compiles this crate; when it *is* compiled (`--workspace`), everything Windows-specific is behind `#[cfg(windows)]` so it is a no-op crate on Linux |
| **Missing SDK is an error code, not a crash** | §15.6 | Library-not-found → `-241,"Hardware missing"` plus a log line naming every probed path; symbol-not-found → the same, naming the symbol |

## 1. Deployment — where the DLL must live

The SDK is installed by the vendor's installer on the test host and is **not** redistributed with
QuickVib. The loader probes, in order, and logs each candidate at `debug`:

| # | Location | Source |
| --- | --- | --- |
| 1 | Directory named by the project's `device.sdkPath` | §12 project schema |
| 2 | Directory named by the `QUICKVIB_M300_SDK` environment variable | this document |
| 3 | The directory containing `quickvib.exe` | `std::env::current_exe()` |
| 4 | The process's default DLL search path | `LoadLibraryW` semantics |

The first candidate that loads wins; the resolved full path is logged once at `info`. If every
candidate fails, the accumulated per-path error list goes into the log at `error` and the SCPI layer
reports `-241,"Hardware missing"`.

| Item | Value | Status |
| --- | --- | --- |
| DLL file name | `M300Sdk.dll` | **TBD — confirm exact name and casing with the vendor** |
| Companion files (config, firmware blobs, extra DLLs) | none assumed | **TBD** |
| Version query entry point | see §3 | **TBD** — if the ABI exposes a version string, QuickVib asserts `1.2.x` at open and logs a warning on mismatch |

## 2. Calling convention, marshalling, ownership

| Question | Working assumption | Status |
| --- | --- | --- |
| Calling convention | `extern "C"` (cdecl) — the only convention on x64 | **TBD**; if the SDK is 32-bit only (Q-C) `extern "stdcall"` becomes possible and every declaration changes |
| Bitness | x86_64 | **TBD (Q-C)** — decides the shipped target triple (D25) |
| Built with | MSVC | **TBD (Q-C)** — a MinGW-built DLL changes nothing at the plain-C ABI level but matters if CRT resources cross the boundary |
| String encoding | UTF-8, NUL-terminated `*const c_char` | **TBD** — ANSI (code-page dependent) and UTF-16 (`*const u16`) are both plausible; the wrong guess produces mojibake in `*IDN?` at best and a read past the end at worst |
| Who owns returned buffers | SDK owns; caller copies immediately and never frees | **TBD** — if the caller must free, the matching free entry point must be named here, and it must be the SDK's free, never Rust's |
| Out-parameter buffers | caller-allocated, caller-sized, length passed in and written back | **TBD** |
| Struct layout | `#[repr(C)]`, default packing | **TBD** — confirm no `#pragma pack` in the header |
| Integer widths | `c_int` = 32-bit on both Windows targets; handles are pointer-sized | **TBD** |

**Why this section is the dangerous one.** Rust FFI has no marshalling layer: a wrong signature or a
wrong ownership assumption is undefined behavior — silent memory corruption or a plausible-looking
wrong measurement — not an exception someone can catch (§20). This is why declarations are generated
by `bindgen` from the real header (§4) rather than transcribed by hand, and why every `unsafe` block
carries a `SAFETY:` comment stating exactly which of the rows above it depends on.

## 3. Entry-point table

**Exact names and signatures are unknown until the SDK header is available (Q-A).** The table below
records only the *functional roles* QuickVib needs, so that the header can be mapped onto them
mechanically. Every `TBD` is a blocker for Phase 6, not a detail to be filled in at the keyboard.

| Role | QuickVib uses it for | Native name | Signature | Blocking? | Thread affinity | Error semantics |
| --- | --- | --- | --- | --- | --- | --- |
| Library init | Once at backend open | TBD | TBD | TBD | TBD | TBD |
| Library shutdown | `Drop` / explicit `close()` | TBD | TBD | TBD | TBD | TBD |
| Open / attach device | Bind to the vibrometer | TBD | TBD | TBD | TBD | TBD |
| Close / detach device | Teardown | TBD | TBD | TBD | TBD | TBD |
| Query identity | `*IDN?` serial + firmware | TBD | TBD | TBD | TBD | TBD |
| Query capabilities | Sample rate, unit, max record time (`DeviceCapabilities`) | TBD | TBD | TBD | TBD | TBD |
| Start streaming | `INIT` / `REC:STAR` | TBD | TBD | TBD | TBD | TBD |
| Stop streaming | `ABOR`, watchdog, `Drop` | TBD | TBD | TBD | TBD | TBD |
| Last-error text | Log detail alongside the SCPI code | TBD | TBD | TBD | TBD | TBD |

Anything the header exposes that is not in this table is deliberately **not** bound: the surface
stays as small as it can be, because every declaration is a chance to get the ABI wrong.

## 4. `bindgen` — generated truth, reviewed and committed

* `build.rs` runs `bindgen` over the vendor header **only** when the non-default `bindgen` feature is
  enabled, because it needs `libclang` and the header, neither of which exists in CI.
* Header location comes from `QUICKVIB_M300_HEADER` (full path to `m300.h`) or `QUICKVIB_M300_SDK`
  (directory containing it).
* Generated output is committed as a reviewed snapshot at `src/ffi_generated.rs`, so the normal
  build needs neither `libclang` nor the vendor header.
* SDK drift is caught by a bench-side `cargo build -p quickvib-m300 --features bindgen` that
  regenerates and diffs against the snapshot; a non-empty diff fails the build.
* Committing generated *declarations* is not committing a fake DLL (D21, §22): there is no
  implementation and no binary behind them.

Regeneration, on a Windows machine that has the SDK:

```powershell
set QUICKVIB_M300_HEADER=C:\Program Files\M300\include\m300.h
cargo build -p quickvib-m300 --features bindgen
:: then review and commit the refreshed crates/quickvib-m300/src/ffi_generated.rs
```

## 5. Error mapping — native code to SCPI code

Filled in from the header's error enum (Q-A). The **fallback rule** is fixed now, so an unrecognized
code can never be swallowed: any non-zero native code with no row of its own maps to
`-240,"Hardware error"` and is logged with its numeric value and, if available, the SDK's last-error
text.

| Native code | Native meaning | SCPI code | SCPI message |
| --- | --- | --- | --- |
| `0` | success (assumed) | — | — |
| TBD | device not present / not attached | `-241` | `Hardware missing` |
| TBD | link lost mid-stream | `-240` | `Hardware error` |
| TBD | invalid argument | `-224` | `Illegal parameter value` |
| TBD | timeout | `-365` | `Time out error` |
| *(any other non-zero)* | — | `-240` | `Hardware error` |

Failures that happen **before** any native call is made are already fixed and need no header:

| Condition | SCPI code | Detail logged |
| --- | --- | --- |
| DLL not found on any probed path | `-241` | every path probed, with the OS error for each |
| DLL loaded but a required symbol is missing | `-241` | the missing symbol name and the resolved DLL path |
| Backend selected as `m300` on a non-Windows host, or in a build without the `m300` feature | startup error, exit code `2` | refused at `backend_factory`, never a silent fallback to mock |

## 6. Socket ownership — who reads the samples

**Working assumption (Q-B): QuickVib owns the transport.** QuickVib listens on `--device-port`
(default `9123`), the M300 dials in, and the little-endian `f32` stream is framed by
`quickvib-device::framer`. The SDK is used for control and identity only.

If the answer turns out to be the opposite — the SDK opens its own transport and hands samples back
through a callback or a polling read — then `M300Backend` changes shape and nothing else does:
`DeviceBackend` (§10), every platform-neutral crate, and every test stay untouched. That isolation is
the entire reason the trait exists, and it is why the framer and the inbound listener live in
`quickvib-device` (pure `std`, exhaustively tested on Linux) rather than here.

| Question | Assumption | Status |
| --- | --- | --- |
| Does the SDK open a socket itself? | No | **TBD (Q-B)** |
| Does the SDK need to be told our listen port? | Possibly, at start-streaming time | **TBD** |
| Does streaming deliver samples via callback? | No — samples arrive on our socket | **TBD**; if yes, §7 applies |
| Sample layout on the wire | contiguous little-endian `f32`, single channel, no framing header | From the brief; confirm no per-packet header |

## 7. Callback ABI, if the SDK uses one

Only relevant if Q-B resolves to "the SDK pushes samples". Rules, fixed regardless of signature:

* The Rust callback is an `extern "C"` shim whose entire body is wrapped in
  `std::panic::catch_unwind` — unwinding across an FFI boundary is undefined behavior — and which
  converts a caught panic into the SDK's error-return convention.
* Context travels through the SDK's `*mut c_void` user-data pointer, which is a pointer to a pinned
  struct owned by `M300Backend` and guaranteed to outlive the registration.
* The shim does no allocation, no locking beyond one uncontended `Mutex`, and no I/O; it copies the
  batch and returns, so a slow consumer can never stall the SDK's reader thread.
* Deregistration happens before the owning struct is dropped, and the pairing is asserted in `Drop`.

## 8. Threading rules

| Question | Assumption | Status |
| --- | --- | --- |
| Is the SDK thread-safe? | No | **TBD** — assumed hostile until documented otherwise |
| Must open/close be on the same thread? | Yes | **TBD** |
| May `stop` be called from another thread while a stream is in flight? | Yes, via an atomic flag plus socket shutdown, not via a concurrent SDK call | Design decision |

Consequence of the pessimistic assumption: `M300Backend` is `Send` but **not** `Sync`, every native
call is funnelled onto the reader thread, and `DeviceBackend::stop(&self)` (§10) does its work with
an `AtomicBool` and a cloned `TcpStream` handle rather than by calling into the SDK from the caller's
thread.

## 9. Manual Windows smoke checklist

The only verification CI cannot do (§11.8, §17.3). Run once against real hardware before v1 sign-off,
and record the date, SDK version, and QuickVib commit alongside the results.

| # | Step | Expected |
| --- | --- | --- |
| 1 | Start `quickvib.exe --project Test.proj --backend m300 --headless` with the SDK installed | Starts, banner names both ports, no error |
| 2 | Power on the M300 and let it dial in to `9123` | Connection logged with peer address |
| 3 | `SYST:DEV:CONN?` | `1` |
| 4 | `*IDN?` | Four fields; serial and firmware match the physical device |
| 5 | Capabilities read back | Sample rate and unit match the project; a mismatch is logged, not enforced (D13) |
| 6 | `CONF:REC:DUR 5.0` then `INIT`, then `REC:WAIT?` | `#REC:DONE` pushed; `REC:WAIT?` returns `1` |
| 7 | `TRAC:POIN?` | `duration × sample rate`, within one sample |
| 8 | `CALC:MEAS:ALL?` | Peak / RMS / p-p physically plausible for the excitation applied |
| 9 | `MMEM:STOR:TRAC "run001.csv"` | File written; opens in Excel; row count matches step 7 |
| 10 | Unplug the device mid-capture | `REC:STAT?` → `ABORTED`; `SYST:ERR?` → `-240,"Hardware error"`; process still alive |
| 11 | Rename the SDK DLL and rerun step 1 | `-241,"Hardware missing"` at `INIT`, a log line naming every probed path, and a clean exit — no crash |
| 12 | Rerun with the default (mock) backend on the same host | Full record → measure → export cycle passes with the SDK absent |

## 10. Current implementation status

| Item | State |
| --- | --- |
| `Cargo.toml` | Written — `libloading` as a `cfg(windows)` dependency, `bindgen` as an optional build-dependency behind the non-default `bindgen` feature |
| `build.rs` | Written — no-op in a normal build; with `--features bindgen` it regenerates from the header and fails on drift from the committed snapshot (§4) |
| `src/resolver.rs` | Written — the probe order of §1 as pure path arithmetic, plus `ResolveError`, which names every path tried and maps to `-241`. Platform-neutral and unit-tested on Linux |
| `src/ffi.rs` / `src/ffi_generated.rs` | **Not written.** Blocked on Q-A (the real header) |
| `M300Backend` (`DeviceBackend` impl) | **Not written.** Blocked on Q-A and Q-B |
| Real SDK calls | **None.** No `unsafe` block and no `extern "C"` declaration exists yet; nothing calls `libloading` |
| Fake DLL | **None, and none will be added** (D21) |
| Linux impact | None. The crate is outside `default-members`, so a plain `cargo build`/`test`/`clippy` never compiles it. Under `--workspace` it does compile and its tests pass on Linux, by design: everything that touches the OS loader is `#[cfg(windows)]`, and everything platform-neutral is tested where CI can see it |

**Before Phase 6 starts**, Q-A (ABI), Q-B (socket ownership), and Q-C (bitness and build toolchain)
from `docs/PLAN.md` §21.1 must be answered. Every `TBD` above is downstream of one of those three.
