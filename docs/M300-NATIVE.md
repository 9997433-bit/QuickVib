# M300 SDK v1.2.0 — native ABI contract

> **Status: filled in from the real SDK v1.2.0 distribution, and now implemented against.** Q-A
> (ABI), Q-B (socket ownership) and Q-C (bitness and toolchain) from `docs/PLAN.md` §21.1 are
> answered below, from the vendor's own headers, PDFs and sample code — not from guesswork. What is
> left for Phase 6 is the bench run of §9: the rows marked **bench** are the ones only real hardware
> can settle.
>
> Everything here was read out of the vendor archive `M300SDK_v1.2.0.zip` (27 MB), which was
> unpacked **outside** the repository and is not committed, in keeping with D21/§22. Sources:
> `include/m300.h`, `include/m300_macros.h`, `include/m300_net_conf.h`, `README.txt`,
> `CHANGELOG.md`, `doc/API参考手册.pdf` (API reference), `doc/使用说明.pdf` (usage guide),
> `doc/网络配置协议.pdf` (UDP configuration protocol), the C / C++ / C# / Python / LabVIEW samples,
> and the PE export tables of both shipped DLLs.
>
> `crates/quickvib-m300` now holds the whole backend: the reviewed `bindgen` snapshot
> (`src/ffi_generated.rs`), the resolver, the platform-neutral maps/errors/queue, the `libloading`
> symbol table and `M300Backend` itself. There is still no binary and no fake DLL. See "Current
> implementation status" at the end for what that does and does not mean.

## 0. Ground rules

These are non-negotiable constraints from the plan; they shape everything below.

| Rule | Source | Consequence |
| --- | --- | --- |
| **No fake/stub SDK DLL, no stub `.lib`, no binaries committed** | D21, §22 | The native path is verified by a manual Windows smoke run against real hardware, never by a fake library. The vendor's real `m300_sdk.dll` is not committed either: it is the vendor's binary, and the loader finds it on the bench |
| **`unsafe` lives in exactly one crate** | D22 | `quickvib-m300` is the only crate without `#![forbid(unsafe_code)]`; it uses `#![deny(unsafe_op_in_unsafe_fn)]` and one `SAFETY:` comment per block |
| **Runtime loading, not link-time import** | §11, D18 | `libloading::Library::new` + `get::<unsafe extern "C" fn(..)>`; the shipped `quickvib.exe` has no import-library dependency on the SDK and starts fine without it (in mock mode). The vendor ships `m300_sdk.dll.lib`, and we deliberately do not use it |
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
| DLL file name | **`m300_sdk.dll`** — all lowercase, no version suffix | Confirmed (archive + PE export table) |
| Vendor layout | `include/`, `x64/bin/m300_sdk.dll`, `x64/lib/{m300_sdk.dll.lib, m300_sdk.lib}`, and the same under `x86/` | Confirmed |
| Companion files | None from the vendor for the MSVC build. The DLL needs the **VC++ 2015–2022 redistributable** (`VCRUNTIME140.dll`) and the UCRT on the host | Confirmed from the import table |
| Version query entry point | `m300_sdk_version() -> *const c_char` returns a static `"1.2.0"`; `m300_sdk_version_numbers(*mut c_int × 3)` returns 1 / 2 / 0 | Confirmed |

A MinGW-built variant of this SDK also exists (the vendor's `scripts\pack_sdk.ps1 -Compiler MinGW`
default, named `libm300_sdk.dll`); it drags in `libgcc_s_*.dll`, `libstdc++-6.dll` and
`libwinpthread-1.dll`. **The archive we were given is the MSVC build**, and the resolver expects
`m300_sdk.dll`. If a bench ever has the MinGW package instead, the file name differs and the whole
MinGW `bin` directory must be kept together — worth checking before blaming the loader.

## 2. Calling convention, marshalling, ownership

| Question | Answer | Status |
| --- | --- | --- |
| Calling convention | `extern "C"` (cdecl) on **both** x64 and x86 | Confirmed: the x86 export table carries plain undecorated names (`m300_init`, not `_m300_init@0`), so it is cdecl, not stdcall |
| Bitness (Q-C) | Both shipped: `x64/bin` is PE32+ x86-64, `x86/bin` is PE32 i386. The export name set is **identical** across the two | Confirmed; QuickVib stays on `x86_64-pc-windows-msvc` (D25) |
| Built with (Q-C) | **MSVC** — linker version 14.41 (VS 2022), imports `VCRUNTIME140.dll` + `api-ms-win-crt-*` (UCRT), `ws2_32`, `bcryptprimitives`, `ntdll` | Confirmed from the PE headers |
| What is behind the ABI | v1.0.1 onwards the SDK is internally a **Rust** rewrite of the old C++ SDK, exposing the same C ABI (`README.txt`, `CHANGELOG.md`) | Confirmed; it means the library is threading-hardened, and it explains the Chinese doc comments in the generated header |
| String encoding | ASCII/UTF-8 `*const c_char`, NUL-terminated. There are no wide-string entry points at all. Only four places take or return strings: `m300_sdk_version` (static, ASCII), `m300_device_get_addr` (dotted-quad IPv4 into a caller buffer), `m300_server_create_ex(bind_addr)`, `m300_nc_create_ex(bind_ip)` | Confirmed |
| Who owns returned buffers | The SDK. `m300_sdk_version` returns a **static** pointer that must never be freed; the callback's `data` pointer is **valid only for the duration of the callback** and must be copied | Confirmed (`m300.h` doc comment; API reference §7.1 and appendix D) |
| Out-parameter buffers | Caller-allocated, caller-sized, count written back: `m300_server_get_devices(.., devices, max_count) -> written`, `m300_read_params(.., out_params, max_params, out_count, ..)`, `m300_nc_discover(.., out_devices, max_devices, out_count)`. `m300_device_get_addr` wants ≥ 16 bytes (we pass 32, as every sample does) | Confirmed |
| Handle ownership | Device handles come from the connect callback or `m300_server_get_devices`. The Rust implementation boxes an `Arc` per handle, so each one **must** be given back with `m300_device_release_handle`, or it leaks — this is new in v1.0.1 and has no equivalent in the legacy C SDK. All handles become invalid after `m300_server_destroy` | Confirmed (`m300.h` doc comment, §5) |
| Struct layout | `#[repr(C)]`, natural alignment, for `M300HardwareInfo`, `M300ParamEntry`, `NetConfig`/`M300NcDeviceInfo`, `NetworkConfig`, `ServerConfig`. The **only** packed struct is `M300NcMqttConfig` (`#pragma pack(1)`, 397 bytes on the wire) and QuickVib does not touch MQTT | Confirmed |
| Integer widths | `M300Result` and `M300NcResult` are `int32_t`; the device result code is a `uint16_t` out-parameter; every timeout is `uint32_t` milliseconds; handles are `void *` | Confirmed |
| Two-level results | Every synchronous command returns **both** an `M300Result` (transport/SDK layer) **and** writes a `uint16_t result_code` (the device's own verdict: `0x0000` = `M300_OK`, `0x0001` = `M300_FAIL`). Checking only the return value silently accepts a device-side refusal | Confirmed; §5 maps both |

**Why this section is the dangerous one.** Rust FFI has no marshalling layer: a wrong signature or a
wrong ownership assumption is undefined behavior — silent memory corruption or a plausible-looking
wrong measurement — not an exception someone can catch (§20). This is why declarations are generated
by `bindgen` from the real header (§4) rather than transcribed by hand, and why every `unsafe` block
carries a `SAFETY:` comment stating exactly which of the rows above it depends on.

## 3. Entry-point table

The SDK exports **105** functions; the headers declare **103** of them. QuickVib binds the small
subset below and nothing else, because every declaration is a chance to get the ABI wrong.

All device commands share one shape:

```c
M300Result m300_<verb>(M300Device device, /* args… */, uint16_t *result_code, uint32_t timeout_ms);
```

`timeout_ms` is per call; the vendor's samples and docs use `3000` everywhere (`5000` for upgrade).

| Role | QuickVib uses it for | Native name | Signature (abridged) | Blocking? | Thread affinity | Error semantics |
| --- | --- | --- | --- | --- | --- | --- |
| Library init | Once at backend open | `m300_init` | `() -> M300Result` | No | Any | Idempotent, refcounted; optional but we call it explicitly |
| Library shutdown | `Drop` / explicit `close()` | `m300_cleanup` | `() -> ()` | No | Any | Pairs with `m300_init`; frees globals at refcount zero |
| Version query | Startup log + `1.2.x` assertion | `m300_sdk_version`, `m300_sdk_version_numbers` | `() -> *const c_char`, `(*mut c_int × 3)` | No | Any | Cannot fail; pointer is static |
| Open transport | Bind and listen for the vibrometer | `m300_server_create_ex` | `(bind_addr: *const c_char, port: u16, out: *mut M300Server) -> M300Result` | No | Any | `-1` on a null out-pointer, `-7` when socket/bind/listen fails → maps to a startup error, not `-241` |
| Register link callbacks | Learn when the device dials in / drops | `m300_server_set_connect_callback`, `m300_server_set_disconnect_callback` | `(server, cb, *mut c_void)` | No | Before `start` | Must be registered **before** `m300_server_start` |
| Start listening | Backend open | `m300_server_start` | `(server) -> M300Result` | No — returns at once, accept loop runs on an SDK thread | Any | Safe to call twice |
| Stop listening | Backend close | `m300_server_stop` | `(server) -> ()` | No | Any | Disconnects all devices; the server can be started again |
| Close transport | `Drop` | `m300_server_destroy` | `(server) -> ()` | No | Any | Null-safe. Invalidates every device handle |
| Enumerate devices | `SYST:DEV:CONN?`, picking the one device | `m300_server_device_count`, `m300_server_get_devices` | `(server) -> u32`, `(server, *mut M300Device, max) -> u32` | No | Any (SDK-locked) | Returns a snapshot; a handle can go stale immediately |
| Connection state | `SYST:DEV:CONN?` | `m300_device_is_connected` | `(device) -> i32` (1 / 0) | No | Any | Never fails |
| Release a handle | After enumeration, and in `Drop` | `m300_device_release_handle` | `(device) -> ()` | No | Any | **Required**, else the SDK leaks the boxed handle |
| Query identity | `*IDN?` serial + firmware | `m300_get_hardware_info` | `(device, *mut M300HardwareInfo, *mut u16, u32) -> M300Result` | Yes (round trip) | Any | `serial[10]` is ASCII and **may not be NUL-terminated**; `app_version[3]` is the firmware triple |
| Read sample rate | `DeviceCapabilities.sample_rate_hz` | `m300_get_sample_rate` | `(device, *mut u8, *mut u16, u32) -> M300Result` | Yes | Any | Returns the **enum index**, not hertz — see §6.1 |
| Read data type | `DeviceCapabilities.unit` | `m300_get_data_type` | `(device, *mut u8, *mut u16, u32) -> M300Result` | Yes | Any | `0` velocity, `1` displacement, `2` acceleration |
| Set sample rate | Apply the project's rate | `m300_set_sample_rate` | `(device, rate: u8, *mut u16, u32) -> M300Result` | Yes | Any | **Exported but not declared in any header** — see the note below |
| Set data type | Apply the project's unit | `m300_set_data_type` | `(device, kind: u8, *mut u16, u32) -> M300Result` | Yes | Any | Same caveat |
| Set low-pass filter | Mandatory partner of the sample rate | `m300_set_low_pass_filter` | `(device, lpf: u8, *mut u16, u32) -> M300Result` | Yes | Any | The vendor insists the LPF band **match** the sample rate (§6.1) |
| Set range | Apply the project's range | `m300_set_velocity_range` / `m300_set_displacement_range` / `m300_set_acceleration_range` | `(device, range: u8, *mut u16, u32) -> M300Result` | Yes | Any | Enum index; v1.2.0 added ±7 m/s and ±0.5 m |
| Register sample callback | The sample path itself | `m300_device_set_data_callback` | `(device, CDataCallback, *mut c_void)` | No | Any | `NULL` deregisters |
| Start streaming | `INIT` / `REC:STAR` | `m300_start_acquisition` | `(device, *mut u16, u32) -> M300Result` | Yes | Any | Samples then arrive on the callback until stopped |
| Stop streaming | `ABOR`, watchdog, `Drop` | `m300_stop_acquisition` | `(device, *mut u16, u32) -> M300Result` | Yes | Any, including while a stream is in flight | Per-device lock inside the SDK |
| Health (optional) | Log line, diagnostics | `m300_get_running_status`, `m300_get_board_temp`, `m300_get_signal_strength` | `(device, out, *mut u16, u32) -> M300Result` | Yes | Any | Status: `0` idle, `1` acquiring, `2` upgrading, `3` error |
| Last-error text | — | **Does not exist** | — | — | — | There is no `m300_get_last_error`. The numeric `M300Result` plus the device `result_code` is all the detail there is, so both go into the log verbatim |

Deliberately **not** bound: the whole UDP configuration family (`m300_nc_*`), firmware upgrade
(`m300_start_upgrade`, `m300_transfer_firmware*`, `m300_firmware_encrypt_crc`), pulse output, PID
and laser/TEC debug parameters, batch parameter read/write, and analog-out configuration. QuickVib
neither configures nor reflashes the instrument.

### 3.1 Two exported symbols that no header declares

`m300_set_sample_rate` and `m300_set_data_type` are **exported by both DLLs, used by every vendor
sample, and documented in the API reference and the LabVIEW guide — but declared in neither
`m300.h` nor `m300_macros.h`.** (The vendor generates `m300.h` with `cbindgen`, which does not
expand Rust macros; `m300_macros.h` is the hand-written patch for that gap and simply misses these
two.) They are exactly the two setters QuickVib needs most.

Because we resolve symbols at runtime, this costs us nothing structural: the two prototypes are
declared by hand in `ffi.rs` from the vendor's own C sample and C#/LabVIEW bindings, which agree on

```c
M300Result m300_set_sample_rate(M300Device device, uint8_t rate, uint16_t *result_code, uint32_t timeout_ms);
M300Result m300_set_data_type  (M300Device device, uint8_t type, uint16_t *result_code, uint32_t timeout_ms);
```

and they carry a `SAFETY:` comment pointing here. One dissent worth knowing about: the vendor's
Python `ctypes` binding types both arguments as `c_int` rather than `uint8_t`. Under cdecl, on both
x64 and x86, an 8-bit and a 32-bit integer argument occupy the same slot, so both spellings happen
to work — but `uint8_t` is what the C#, LabVIEW and macro-family declarations say, so `uint8_t` is
what we use. **Bench check:** confirm a set-then-get round trip returns the value we sent.

## 4. `bindgen` — generated truth, reviewed and committed

* `build.rs` runs `bindgen` over the vendor headers **only** when the non-default `bindgen` feature
  is enabled, because it needs `libclang` and the headers, neither of which exists in CI.
* Header location comes from `QUICKVIB_M300_HEADER` (full path to `m300.h`) or `QUICKVIB_M300_SDK`
  (the SDK's `include/` directory).
* Both vendor headers are parsed: `m300.h` and, when present beside it, `m300_macros.h`.
* Generated output is committed as a reviewed snapshot at `src/ffi_generated.rs` — 103 `extern "C"`
  prototypes, the `#[repr(C)]` structs, the callback types and the error enums — so the normal build
  needs neither `libclang` nor the vendor header.
* SDK drift is caught by a bench-side `cargo build -p quickvib-m300 --features bindgen` that
  regenerates and diffs against the snapshot; a non-empty diff fails the build.
* Committing generated *declarations* is not committing a fake DLL (D21, §22): there is no
  implementation and no binary behind them.

**The two vendor headers cannot be included in one translation unit as they ship.**
`m300_macros.h` repeats three prototypes (`m300_write_params`, `m300_read_params`,
`m300_get_device_status`) with `struct M300ParamEntry *`, while `m300.h` declares them with the
typedef of an *anonymous* struct — different types, so clang stops with "conflicting types". Our
`build.rs` therefore feeds `bindgen` a generated wrapper that `#define`s those three names aside
before including the second header and blocklists the renamed duplicates. The real declarations
still come from `m300.h`. This is a vendor-side defect worth reporting; nothing in the DLL is
wrong, only the headers.

Regeneration on a Windows machine that has the SDK:

```powershell
set QUICKVIB_M300_SDK=C:\Program Files\M300\include
cargo build -p quickvib-m300 --features bindgen
:: then review and commit the refreshed crates/quickvib-m300/src/ffi_generated.rs
```

The committed snapshot was generated on Linux (clang 18, `bindgen` 0.70.1) from the vendor headers,
which is sound because every type in it is width-explicit (`u8`/`u16`/`u32`/`i32`/`f32`, fixed-size
arrays, `*mut c_void`). **Bench check:** regenerate once on Windows and confirm the diff is empty.

## 5. Error mapping — native code to SCPI code

Two independent codes come back from every command and both are checked. The **fallback rule** is
fixed, so an unrecognized code can never be swallowed: any non-zero native code with no row of its
own maps to `-240,"Hardware error"` and is logged with its numeric value.

| Native code | Native meaning | SCPI code | SCPI message |
| --- | --- | --- | --- |
| `0` | `M300_SUCCESS` | — | — |
| `-1` | `M300_ERR_INVALID_ARG` — null pointer or illegal value | `-224` | `Illegal parameter value` |
| `-2` | `M300_ERR_BUFFER_SMALL` — our out-buffer was too small | `-240` | `Hardware error` (a QuickVib bug; logged as such) |
| `-3` | `M300_ERR_BAD_HEADER` — reply header was not `"SCZN"` | `-240` | `Hardware error` |
| `-4` | `M300_ERR_CRC_MISMATCH` — CRC32 failed | `-240` | `Hardware error` |
| `-5` | `M300_ERR_BAD_PACKET` — malformed or short packet | `-240` | `Hardware error` |
| `-6` | `M300_ERR_TIMEOUT` — no reply within `timeout_ms` | `-365` | `Time out error` |
| `-7` | `M300_ERR_NETWORK` — socket/bind/send failed | `-240` | `Hardware error` |
| `-8` | `M300_ERR_DISCONNECTED` — device not connected or link dropped | `-241` | `Hardware missing` |
| *(any other non-zero)* | — | `-240` | `Hardware error` |

Device-side verdict, the `uint16_t result_code` out-parameter:

| Value | Meaning | SCPI code |
| --- | --- | --- |
| `0x0000` | `M300_OK` — the device executed the command | — |
| `0x0001` | `M300_FAIL` — the device refused it | `-240`, logged with the command name and the value |

**Trap, if the `m300_nc_*` family is ever bound:** `M300NcResult` uses the same numbers with
`-6` and `-7` **swapped** relative to `M300Result` (`-6` is *network*, `-7` is *timeout*). The
vendor did this deliberately, to stay byte-compatible with the legacy C SDK, and says so in
`m300.h`. Mapping net-conf codes through the table above would report a timeout as a network error
and vice versa.

Failures that happen **before** any native call is made:

| Condition | SCPI code | Detail logged |
| --- | --- | --- |
| DLL not found on any probed path | `-241` | every path probed, with the OS error for each |
| DLL loaded but a required symbol is missing | `-241` | the missing symbol name and the resolved DLL path |
| Backend selected as `m300` on a non-Windows host, or in a build without the `m300` feature | startup error, exit code `2` | refused at `backend_factory`, never a silent fallback to mock |

## 6. Socket ownership — who reads the samples (Q-B)

**Answer: the SDK owns the transport, and samples arrive on a callback.** The previous working
assumption was the opposite, and it was wrong.

The wire-level picture in the brief still holds — the PC is the TCP server, the vibrometer is the
client that dials in — but the listening socket belongs to the SDK, not to us:

| Question | Answer | Source |
| --- | --- | --- |
| Does the SDK open a socket itself? | **Yes.** `m300_server_create(port)` / `m300_server_create_ex(bind_addr, port)` binds and listens; `m300_server_start` runs the accept loop on an SDK thread | API reference §5; every sample |
| Does the SDK need to be told our listen port? | Yes — it *is* the listener. The port is an argument to `m300_server_create*`, and the vendor's own default is `9123`, the same number QuickVib already uses | Samples, usage guide §1 |
| Does streaming deliver samples via callback? | **Yes.** `m300_device_set_data_callback` registers `CDataCallback`, which fires on the device's SDK thread for each uploaded block | API reference §7.1 |
| Sample layout | `data_len` bytes = `point_count` × 4, contiguous IEEE-754 **little-endian `f32`**, single channel, no per-packet header at this level | API reference §7.1, usage guide §4 |
| Units | Already physical, and already exactly QuickVib's units: velocity **μm/s**, displacement **μm**, acceleration **m/s²** | API reference appendix A; matches `SampleUnit::{VelocityUmPerSec, DisplacementUm, AccelerationMPerSec2}` |
| Multiple devices | One `M300Server` accepts many vibrometers on one port, each with its own handle and callbacks | API reference §5 |

What this changes, and what it does not:

* **`M300Backend` becomes a callback-driven backend.** `DeviceBackend::stream` is a pull API on the
  reader thread, so the shim pushes each batch into a bounded queue and `stream` drains it. Nothing
  about the trait, the engine, or any platform-neutral crate changes — that isolation is the entire
  reason the trait exists.
* **With `--backend m300`, QuickVib must not bind `--device-port` itself.** The SDK binds it. The
  two would otherwise fight over the same port, and the loser reports `-7 ERR_NETWORK` at open.
  `--device-port` and `--bind` become the arguments to `m300_server_create_ex`.
* **`quickvib-device::framer` is not on this path.** The SDK has already framed and converted the
  samples; the framer keeps serving the `tcp` backend, which talks to a raw M300 (or `m300-sim`)
  with no SDK in the picture.
* The `tcp` backend remains a faithful rehearsal of the *wire*, not of the SDK.

### 6.1 Sample rate, filter and range are enum indices

`m300_set_sample_rate` takes an **index**, not hertz, and reading it back gives the index too, so
`DeviceCapabilities.sample_rate_hz` needs the table below in both directions.

| Index | Rate | Index | Rate | Index | Rate |
| --- | --- | --- | --- | --- | --- |
| `0x00` | 2 kHz | `0x05` | 100 kHz | `0x0A` | 2 MHz |
| `0x01` | 5 kHz | `0x06` | 200 kHz | `0x0B` | 4 MHz |
| `0x02` | 10 kHz | `0x07` | 400 kHz | `0x0C` | 5 MHz |
| `0x03` | 20 kHz | `0x08` | 800 kHz | `0x0D` | 10 MHz |
| `0x04` | 50 kHz | `0x09` | 1 MHz | `0x0E` | 20 MHz |

A project sample rate that is not in this table cannot be set on real hardware. Per D13 a mismatch
is **logged, not enforced**, but for the M300 backend the nearest legal index has to be chosen
deliberately and the substitution logged.

**The vendor repeats one rule in every document: the low-pass filter must be set together with the
sample rate, at a matching band** (10 kHz sampling with the 10 kHz filter, and so on), or "data may
be abnormal or the device may refuse". Filter indices: `0` 100 Hz, `1` 500 Hz, `2` 1 kHz, `3` 2 kHz,
`4` 5 kHz, `5` 10 kHz, `6` 20 kHz, `7` 40 kHz, `8` 80 kHz, `9` 100 kHz, `10` 160 kHz, `11` 320 kHz,
`12` 500 kHz, `13` 1 MHz, `14` 3 MHz.

Data type: `0` velocity, `1` displacement, `2` acceleration. Velocity range spans `0x00` ±2.45 μm/s
to `0x10` ±7 m/s; displacement `0x00` ±0.245 μm to `0x12` ±0.5 m; acceleration `0x00` ±1.225 m/s² to
`0x0B` ±612500 m/s². The last velocity and displacement entries are the two v1.2.0 additions, so
they are also the first thing that will be missing if a bench turns out to have v1.1.0 installed.

## 7. Callback ABI

Q-B resolved to "the SDK pushes samples", so this section is live. The vendor's types:

```c
typedef void (*M300DataCallback)(M300Device device, uint32_t data_type, uint32_t point_count,
                                 const uint8_t *data, uint32_t data_len, void *user_data);
typedef void (*M300ConnectCallback)(M300Server server, M300Device device, void *user_data);
typedef void (*M300DisconnectCallback)(M300Server server, M300Device device, void *user_data);
```

All three return `void`: **there is no way to report an error back to the SDK from a callback.**
Rules, fixed regardless of signature:

* The Rust callback is an `extern "C"` shim whose entire body is wrapped in
  `std::panic::catch_unwind` — unwinding across an FFI boundary is undefined behavior. Since the
  ABI has no error channel, a caught panic is recorded in shared state and surfaced on the next
  `stream` poll as a `DeviceError`, and the run is aborted from our side.
* Context travels through the SDK's `*mut c_void` user-data pointer, which is a pointer to a pinned
  struct owned by `M300Backend` and guaranteed to outlive the registration.
* The shim does no allocation, no locking beyond one uncontended `Mutex`, and no I/O; it copies the
  batch (it must — `data` dies when the callback returns) and returns, so a slow consumer can never
  stall the SDK's reader thread. An overrun drops the batch and counts it rather than blocking.
* Deregistration (`m300_device_set_data_callback(device, NULL, NULL)`) happens before the owning
  struct is dropped, and the pairing is asserted in `Drop`.
* `point_count` and `data_len` are cross-checked (`data_len == point_count * 4`) before the buffer
  is read, and a mismatch aborts the run rather than trusting either number.
* `data_type` is compared against the unit the project asked for; a mid-run change means the device
  was reconfigured behind our back and is treated as an error, not silently relabelled.

Thread rules the vendor states explicitly (API reference appendix D):

* The **connect** callback runs on the accept thread. Long work there stalls accepts, so the shim
  only records the handle and wakes the backend.
* **Data, disconnect and upgrade** callbacks run on that device's callback thread, and the docs
  confirm synchronous commands may be called from inside them.

## 8. Threading rules

| Question | Answer | Status |
| --- | --- | --- |
| Is the SDK thread-safe? | **Yes, and documented as such**: per-device locks, replies matched by `command_id`, `m300_server_get_devices` and `m300_server_device_count` internally locked, callback registration locked | Confirmed (appendix D) |
| Concurrent commands to the same device | Safe | Confirmed |
| Must open/close be on the same thread? | No such requirement is stated | Confirmed by omission — **bench check** |
| May `stop` be called from another thread while a stream is in flight? | Yes: `m300_stop_acquisition` is an ordinary per-device command | Confirmed |

This is better than the pessimistic assumption the plan carried. Consequences:

* `DeviceBackend::stop(&self)` can call `m300_stop_acquisition` directly from the SCPI thread
  instead of going through an `AtomicBool` and a socket shutdown.
* `M300Backend` can be `Send`; whether it is also `Sync` is decided by our own interior state, not
  by the SDK.
* The pessimism that remains is deliberate: callbacks arrive on SDK threads we do not own, so the
  shared state between the shim and `stream` is the one place that needs real synchronization.

## 9. Manual Windows smoke checklist

The only verification CI cannot do (§11.8, §17.3). Run once against real hardware before v1 sign-off,
and record the date, SDK version, and QuickVib commit alongside the results.

| # | Step | Expected |
| --- | --- | --- |
| 0 | On the bench, `cargo build -p quickvib-m300 --features bindgen` with `QUICKVIB_M300_SDK` set | Build succeeds with "snapshot is up to date"; an empty diff confirms the Linux-generated snapshot (§4) |
| 1 | Start `quickvib.exe --project Test.proj --backend m300 --headless` with the SDK installed | Starts, banner names both ports, no error; the log records the resolved `m300_sdk.dll` path and `m300_sdk_version()` = `1.2.0` |
| 2 | Power on the M300 and let it dial in to `9123` | Connect callback fires; connection logged with peer address and port |
| 3 | `SYST:DEV:CONN?` | `1` |
| 4 | `*IDN?` | Four fields; serial and firmware match the physical device (`M300HardwareInfo.serial`, `.app_version`) |
| 5 | Capabilities read back | `m300_get_sample_rate` / `m300_get_data_type` decode to the project's rate and unit; a mismatch is logged, not enforced (D13) |
| 6 | Set rate and filter together, then `CONF:REC:DUR 5.0`, `INIT`, `REC:WAIT?` | Both setters return `0` with `result_code == 0`; `#REC:DONE` pushed; `REC:WAIT?` returns `1` |
| 7 | `TRAC:POIN?` | `duration × sample rate`, within one sample; batch sizes and `point_count × 4 == data_len` hold throughout |
| 8 | `CALC:MEAS:ALL?` | Peak / RMS / p-p physically plausible for the excitation applied, in **μm/s** |
| 9 | `MMEM:STOR:TRAC "run001.csv"` | File written; opens in Excel; row count matches step 7 |
| 10 | Unplug the device mid-capture | Disconnect callback fires; `REC:STAT?` → `ABORTED`; `SYST:ERR?` → `-240,"Hardware error"` (or `-241` if a command reports `-8`); process still alive |
| 11 | Rename `m300_sdk.dll` and rerun step 1 | `-241,"Hardware missing"` at `INIT`, a log line naming every probed path, and a clean exit — no crash |
| 12 | Rerun with the default (mock) backend on the same host | Full record → measure → export cycle passes with the SDK absent |
| 13 | Deliberately mismatch filter and sample rate once | Confirms the vendor's warning and tells us whether the device refuses (`result_code = 1`) or silently degrades — decides whether QuickVib must enforce the pairing |
| 14 | Start QuickVib twice against the same port | The second instance fails at `m300_server_create*` with `-7`; confirms the port really belongs to the SDK (§6) |

## 10. Current implementation status

| Item | State |
| --- | --- |
| `Cargo.toml` | Written — `libloading` as a `cfg(windows)` dependency, `bindgen` as an optional build-dependency behind the non-default `bindgen` feature |
| `build.rs` | Written — no-op in a normal build; with `--features bindgen` it parses both vendor headers through a generated wrapper, regenerates, and fails on drift from the committed snapshot (§4) |
| `src/resolver.rs` | Written — the probe order of §1 as pure path arithmetic, plus `ResolveError`, which names every path tried and maps to `-241`. Platform-neutral and unit-tested on Linux |
| `src/ffi_generated.rs` | **Written** — reviewed `bindgen` snapshot of the real v1.2.0 headers: 103 prototypes, the `#[repr(C)]` structs, callback types and error enums. Declarations only; nothing calls them, nothing links them |
| `src/maps.rs`, `src/error.rs`, `src/batch.rs`, `src/shim.rs` | **Written** — the platform-neutral half: the enum ladders of §6.1, the two-level result mapping of §5, what a data callback may believe about its buffer (§7), and the `extern "C"` shims plus the bounded queue that turns the SDK's push into `DeviceBackend::stream`'s pull. All compiled and unit-tested on Linux |
| `src/ffi.rs` | **Written** — the `libloading` symbol table: the probe order of §1, every entry point of §3 including the two §3.1 exports no header declares, and the version read-back. Windows only |
| `M300Backend` (`DeviceBackend` impl) | **Written** — loads the DLL, brings the SDK's server up on `bind_host:device_port`, registers the link and data callbacks, applies the project's rate/filter/range, and drains the queue. Windows only |
| Real SDK calls | **Yes**, all of them through `src/ffi.rs`, each `unsafe` block carrying a `SAFETY:` comment naming the row of §2 it depends on |
| Application wiring | **Written** — `backend_factory` constructs the backend and reports that it owns the device port; the app skips its own `DeviceServer` so the two do not fight over the port (§6) |
| Fake DLL | **None, and none will be added** (D21). The vendor's real DLL is not committed either |
| Linux impact | None. The crate is outside `default-members`, so a plain `cargo build`/`test`/`clippy` never compiles it. Under `--workspace` it does compile — the Windows-only modules reduce to nothing — and every platform-neutral test runs on Linux, by design |

**The code is complete; the verification is not.** Nothing above has touched real hardware, and
without a fake DLL nothing in CI can. The bench run in §9 is the whole of what is left, and until it
is recorded — date, SDK version, QuickVib commit — `--backend m300` should be treated as untried.
The rows most likely to be wrong are the ones CI cannot reach at all: the two undeclared symbols
resolving and round-tripping (§3.1), the snapshot regenerating identically on Windows (§4), the
filter/rate pairing rule (§6.1), and open/close thread affinity (§8).
