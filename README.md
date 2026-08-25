# QuickVib

**English | [中文](#quickvib-中文)**

QuickVib makes an **M300 laser Doppler vibrometer** look and behave like a **Keysight-style SCPI
instrument** to an existing UTS (unit test system). It is a single self-contained Windows console
executable: the UTS launches it from a command prompt, opens a TCP socket, and sends ordinary SCPI
text commands (`*IDN?`, `INIT`, `FETC?`, …) exactly as it would to a bench instrument. QuickVib
records a fixed-duration vibration capture, returns the samples and the derived scalars
(peak, RMS, peak-to-peak), and exports CSV or TXT.

Scope is deliberately narrow: QuickVib **records, measures, and exports**. It does not decide
pass/fail, does not drive DUT vibration, and does not own retry policy — those stay in the UTS.

The default device backend is a pure-Rust **mock**, so the whole product builds, runs, and is fully
tested on Linux as well as Windows, with no hardware and no vendor SDK. For rehearsing the *real*
inbound device link without an M300, `--backend tcp` records from whatever dials into the device
port, and the bundled **`m300-sim`** executable is a stand-in that dials in and streams — see
[§9.1](#91-m300-sim--simulating-the-inbound-device-link).

> **Status.** This is an in-progress build of the plan in [`docs/PLAN.md`](docs/PLAN.md). The
> supported paths are the mock backend and the socket-fed `tcp` backend. The M300 native backend
> (`crates/quickvib-m300`) is **scaffolding only** — no SDK calls are implemented, and by design no
> fake DLL exists in this repository. See [`docs/M300-NATIVE.md`](docs/M300-NATIVE.md).

---

## Table of contents

1. [Requirements](#1-requirements)
2. [Quick start](#2-quick-start)
3. [How the UTS launches QuickVib](#3-how-the-uts-launches-quickvib)
4. [Command-line reference](#4-command-line-reference)
5. [How it works](#5-how-it-works)
6. [SCPI command reference](#6-scpi-command-reference)
7. [Example UTS session](#7-example-uts-session)
8. [Project file format](#8-project-file-format)
9. [Mock backend vs real device](#9-mock-backend-vs-real-device)
10. [Export formats](#10-export-formats)
11. [Measurements](#11-measurements)
12. [Building and testing](#12-building-and-testing)
13. [Troubleshooting](#13-troubleshooting)
14. [Scope — what QuickVib does not do](#14-scope--what-quickvib-does-not-do)
15. [Contributing and license](#15-contributing-and-license)

---

## 1. Requirements

| To… | You need |
| --- | --- |
| Build | A stable Rust toolchain, edition 2021 (the pinned version is in `rust-toolchain.toml`) |
| Run (mock backend) | Nothing. The executable is statically linked — no runtime, no redistributable. Any OS: Windows, Linux, macOS |
| Run (real M300) | Windows x64, an M300 vibrometer, and the M300 SDK v1.2.0 installed on the host. The SDK is **not** bundled with QuickVib |
| Cross-compile a Windows `.exe` from Linux | `gcc-mingw-w64-x86-64` and the `x86_64-pc-windows-gnu` Rust target |

Runtime dependencies are deliberately minimal: `serde` + `serde_json` for the project file, and
`libloading` on Windows only for the M300 crate. No async runtime, no CLI framework, no logging
crate.

## 2. Quick start

```bash
cargo build --release
```

Run it against the sample project with the mock backend:

```bash
# Linux / macOS
./target/release/quickvib --project samples/Test.proj --headless
```

```bat
:: Windows
target\release\quickvib.exe --project samples\Test.proj --headless
```

Then talk to it from any raw-socket client — `telnet`, PuTTY in raw mode, `nc`, or your UTS:

```bash
printf '*IDN?\n' | nc 127.0.0.1 5025
# QuickVib,M300-SCPI,SN-0001,1.0.0
```

To rehearse the real device link instead of the mock, start QuickVib with `--backend tcp` and dial
in with the bundled simulator — see [§9.1](#91-m300-sim--simulating-the-inbound-device-link).

## 3. How the UTS launches QuickVib

This is the invocation the UTS uses. All four arguments are shown explicitly even though three of
them are already the defaults, because an explicit command line is one less thing to be surprised
by in a test cell:

```bat
quickvib.exe --headless --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

| Part | Meaning |
| --- | --- |
| `--headless` | No interactive console UI — structured single-line log records on stdout only. This is the mode for UTS-launched runs |
| `--scpi-port 5025` | Port QuickVib **listens on for the UTS**. `5025` is the conventional SCPI raw-socket port |
| `--device-port 9123` | Port QuickVib **listens on for the M300**. The vibrometer is the one that dials in |
| `--project samples\Test.proj` | Project file loaded at startup: sample rate, unit, record duration, export format, `*IDN?` identity |

QuickVib is a **TCP server on both links** and never dials out. It stays in the foreground until the
process is terminated, so the UTS can start it as a child process and shut it down by killing it;
the normal in-band shutdown is `ABOR` followed by closing the SCPI socket.

Exit codes: `0` clean shutdown, `2` bad arguments, `3` port bind failure, `4` project load failure
(only when `--project` was given explicitly), `5` backend open failure.

## 4. Command-line reference

| Argument | Value | Default | Behavior |
| --- | --- | --- | --- |
| `--project <path>` | file path | *(auto-load-last)* | Load this project at startup. Failure is fatal (exit `4`) |
| `--scpi-port <n>` | 1–65535 | `5025` | SCPI server port (the UTS connects in) |
| `--device-port <n>` | 1–65535 | `9123` | Device server port (the M300 connects in). Overrides `device.port` |
| `--headless` | flag | off | Structured log lines only; no interactive console UI |
| `--backend <mock\|tcp\|m300>` | enum | *(project, else `mock`)* | Override the project's backend. `mock` synthesizes samples in process; `tcp` records the little-endian `f32` stream arriving on `--device-port` (a real M300, or `m300-sim`); `m300` uses the native SDK and, on a non-Windows host or in a build without the `m300` feature, is a startup error — never a silent fallback |
| `--no-auto-load` | flag | off | Suppress auto-load-last, for a clean UTS run |
| `--log-level <level>` | enum | `info` | `trace\|debug\|info\|warn\|error` |
| `--version` | flag | — | Print the version and exit `0` |
| `--help` | flag | — | Print usage and exit `0` |

`--flag=value` and `--flag value` are both accepted; `--` terminates flag parsing. Unknown flags,
missing values, and out-of-range ports exit `2` with usage on stderr.

## 5. How it works

Two independent TCP roles exist, and confusing them is the classic first-day mistake:

| Role | Direction | Default port | Peer |
| --- | --- | --- | --- |
| **SCPI server** | QuickVib listens, **UTS connects in** | `5025` | UTS test executive |
| **Device server** | QuickVib listens, **M300 connects in** | `9123` | M300 vibrometer |

```
   UTS  ──SCPI over TCP──▶  :5025  ┌──────────────────────────┐
                                   │        quickvib.exe      │
                                   │  SCPI parser → engine    │
                                   │  → recording pipeline    │──▶  *.csv / *.txt
                                   │  → measurements          │
   M300 ──LE f32 stream───▶  :9123  └──────────────────────────┘
```

Sample data arrives as a raw **little-endian `f32`** stream, framed on arbitrary chunk boundaries
into `f32` values. There is exactly one wire format and no handshake, header or length prefix: four
bytes per sample, back to back, for as long as the link is up. A run is bounded by **sample count**,
not wall-clock, with a wall-clock watchdog as the backstop, so captures are deterministic and
tolerate device jitter.

The device port is bound and framed on every backend. With `--backend mock` the framed samples are
counted and logged but the mock supplies the capture; with `--backend tcp` they *are* the capture.

The flow for one measurement is: load project → `INIT` → samples stream in until
`duration × sample rate` are collected → the state machine reaches `COMPLETE`, `#REC:DONE` is pushed
to connected sessions, and `REC:WAIT?` unblocks → `CALC:MEAS:*?` returns the scalars → optionally
`MMEM:STOR:TRAC` writes the capture to disk.

Instrument states, reported by `REC:STAT?`:

```
IDLE ──INIT──▶ ARMED ──first sample──▶ RECORDING ──expected count──▶ COMPLETE
                 │                          │                             │
                 └──── ABOR / timeout ──────┴────────▶ ABORTED ◀──────────┘
```

## 6. SCPI command reference

Commands are ASCII, terminated by `\n` (`\r\n` tolerated), case-insensitive, with short and long
forms both accepted. Semicolon-chained compound messages (`*CLS;*IDN?`) work. Responses are a single
`\n`-terminated line.

**IEEE 488.2 mandated**

| Command | Response | Behavior |
| --- | --- | --- |
| `*IDN?` | `QuickVib,M300-SCPI,<serial>,<fw>` | Manufacturer, model, serial, firmware. All four configurable from the project's `identity` block |
| `*RST` | — | Abort any run, discard data, reset to the project's values, clear the error queue |
| `*CLS` | — | Clear the error queue and status registers |
| `*OPC` / `*OPC?` | — / `1` | Operation-complete bit; the query blocks until the pending run finishes |
| `SYST:ERR?` | `<code>,"<message>"` | Pop the oldest error. `0,"No error"` when empty |

**Project / mass memory**

| Command | Behavior |
| --- | --- |
| `MMEM:LOAD:STAT "<path>"` | Load a JSON project file |
| `MMEM:STOR:STAT "<path>"` | Save the current project, including runtime overrides |
| `MMEM:LOAD:AUTO` / `MMEM:LOAD:AUTO?` | Load / report the most recently used project |
| `MMEM:STOR:TRAC "<path>"` | Export the last completed capture in the active format |

**Configuration, recording, data**

| Command | Response | Behavior |
| --- | --- | --- |
| `CONF:REC:DUR <s>` / `CONF:REC:DUR?` | — / `5.000` | Record duration in seconds, range `(0, 3600]` |
| `FORM CSV\|TXT` / `FORM?` | — / `CSV` | Export format |
| `INIT` (alias `REC:STAR`) | — | Start a recording. Non-blocking; discards the previous capture |
| `ABOR` | — | Abort the active run. Not an error when idle |
| `REC:STAT?` | `IDLE\|ARMED\|RECORDING\|COMPLETE\|ABORTED` | Current state; safe to poll |
| `REC:WAIT?` | `1` \| `0` | Block until the run completes (`1`) or aborts/times out (`0`) |
| `FETC?` (alias `TRAC:DATA?`) | `v1,v2,…,vN` | All samples of the last completed capture, comma-separated |
| `TRAC:POIN?` | `500000` | Sample count, so the UTS can size its read buffer before `FETC?` |
| `CALC:MEAS:PEAK?` | `12.3400` | `max(abs(x))` |
| `CALC:MEAS:RMS?` | `4.5600` | Root mean square |
| `CALC:MEAS:PP?` | `24.6800` | `max − min` |
| `CALC:MEAS:ALL?` | `12.3400,4.5600,24.6800` | Peak, RMS, p-p in that fixed order — one round trip |
| `SYST:DEV:CONN?` | `1` \| `0` | Whether a device backend session is live |
| `SYST:VERS?` | `1999.0` | SCPI standard version |

**Out-of-band notifications.** Lines beginning with `#` are unsolicited, never responses:
`#REC:DONE` when a run completes, `#REC:ABORT` when one aborts or times out. A per-session write
lock guarantees a notification can never appear in the middle of a response line. Clients that
prefer strict request/response can ignore them and use `REC:WAIT?` instead.

**Error codes** are standard SCPI-99 — none are invented:

| Code | Message | Raised when |
| --- | --- | --- |
| `0` | `No error` | Queue empty |
| `-100` | `Command error` | Malformed message, over-long line, non-UTF-8 input |
| `-113` | `Undefined header` | Unknown command |
| `-221` | `Settings conflict` | No project loaded, already recording, or config changed during a run |
| `-222` | `Data out of range` | Duration outside `(0, 3600]`, or over the capture-size cap |
| `-224` | `Illegal parameter value` | Bad `FORM` value or invalid project schema |
| `-230` | `Data corrupt or stale` | Data queried with no completed capture |
| `-240` | `Hardware error` | Device link dropped during a run; non-zero SDK return |
| `-241` | `Hardware missing` | No device at `INIT`; SDK DLL or symbol not found |
| `-256` / `-257` | `File name not found` / `File name error` | Missing project; unwritable path |
| `-350` | `Queue overflow` | More than 32 unread errors |
| `-365` | `Time out error` | Watchdog fired during a capture |

## 7. Example UTS session

A complete measurement, as the UTS would drive it. `>` is sent to QuickVib, `<` is received.

```
> *IDN?
< QuickVib,M300-SCPI,SN-0001,1.0.0

> *CLS                                  ; start from a clean error queue
> MMEM:LOAD:STAT "samples\Test.proj"    ; sample rate, unit, duration, identity
> SYST:ERR?
< 0,"No error"                          ; the load succeeded

> SYST:DEV:CONN?
< 1                                     ; mock is always connected; M300 must have dialed in

> CONF:REC:DUR 5.0                      ; override the project's duration for this run
> FORM CSV

> INIT                                  ; non-blocking: returns nothing, run starts
> REC:WAIT?                             ; blocks until the run finishes
< #REC:DONE                             ; unsolicited notification, pushed on completion
< 1                                     ; REC:WAIT? unblocks with 1 = completed

> REC:STAT?
< COMPLETE

> TRAC:POIN?
< 500000                                ; 5.0 s × 100 kS/s

> CALC:MEAS:ALL?
< 252.4913,178.5402,504.9826            ; peak, RMS, peak-to-peak, in project units

> MMEM:STOR:TRAC "out\run001.csv"       ; write the capture to disk
> SYST:ERR?
< 0,"No error"
```

Notes for the UTS author:

* `INIT` is a command, not a query — it produces no response. Waiting for one will hang.
* Poll `REC:STAT?` or block on `REC:WAIT?`; both are bounded server-side by the watchdog, so neither
  can hang past `duration × timeoutMultiplier + 1 s`.
* An unknown command pushes `-113` onto the error queue and returns **nothing**, which is standard
  instrument behavior. Read `SYST:ERR?` to discover it.
* `FETC?` on a 5 s × 100 kS/s capture is roughly a 6 MB single line. Call `TRAC:POIN?` first and
  size the read buffer accordingly, or use `CALC:MEAS:ALL?` if only the scalars are needed.

## 8. Project file format

A project is JSON (`*.proj`). Unknown properties are ignored for forward compatibility; the sample
lives at `samples/Test.proj`.

```json
{
  "schemaVersion": 1,
  "name": "Test",
  "device": {
    "backend": "mock",
    "port": 9123,
    "sampleRateHz": 100000,
    "unit": "velocity_um_s",
    "connectTimeoutSeconds": 30
  },
  "recording": { "durationSeconds": 5.0, "timeoutMultiplier": 2.0 },
  "measurement": { "removeDc": false, "responseDecimals": 4 },
  "export": { "format": "CSV", "directory": "./out", "includeHeader": true },
  "identity": {
    "manufacturer": "QuickVib",
    "model": "M300-SCPI",
    "serialNumber": "SN-0001",
    "firmwareVersion": "1.0.0"
  },
  "mock": {
    "signal": {
      "components": [{ "frequencyHz": 120.0, "amplitude": 250.0, "phaseDeg": 0.0 }],
      "noiseStdDev": 2.5,
      "seed": 12345
    }
  }
}
```

| Field | Default | Notes |
| --- | --- | --- |
| `device.backend` | `"mock"` | `"mock"` or `"m300"` |
| `device.port` | `9123` | Inbound port the M300 dials; `--device-port` overrides |
| `device.sampleRateHz` | *(required)* | e.g. `100000` |
| `device.unit` | *(required)* | `"velocity_um_s"`, `"displacement_um"`, `"acceleration_m_s2"` |
| `device.allowedPeers` | `[]` | Empty accepts any inbound peer |
| `device.sdkPath` | `null` | Directory probed for the M300 SDK (M300 backend only) |
| `device.lpfHz` | `null` | Low-pass cutoff; `null` tracks Nyquist (`sampleRateHz / 2`) |
| `device.highPassHz` | `0` | High-pass cutoff; `0` disables. Must be below the low-pass cutoff |
| `device.velocityRange` / `.displacementRange` / `.accelerationRange` | `1000` / `1000` / `100` | Measuring range in µm/s, µm, m/s²; the one matching `device.unit` applies |
| `recording.durationSeconds` | *(required)* | `(0, 3600]` |
| `recording.timeoutMultiplier` | `2.0` | Watchdog = duration × this + 1 s |
| `recording.maxCaptureBytes` | `536870912` | `INIT` rejects larger runs with `-222` |
| `measurement.removeDc` | `false` | Subtract the mean before peak and RMS |
| `measurement.responseDecimals` | `4` | Decimals in `CALC:*` responses, `0..=9` |
| `export.format` / `.directory` / `.includeHeader` | `"CSV"` / `"."` / `true` | Export defaults |
| `identity.*` | see above | The four `*IDN?` fields, so an existing UTS ID check can be satisfied |
| `server.maxSessions` | `8` | Concurrent SCPI session cap |
| `server.scpiPort` | `5025` | SCPI listen port stored in the project; `--scpi-port` overrides |
| `mock.signal.*` | one 100 Hz component | Sine components, Gaussian noise σ, and PRNG seed |

The most recently loaded or saved project is remembered (`%LOCALAPPDATA%\QuickVib\` on Windows,
`$XDG_STATE_HOME/quickvib/` on Unix) and auto-loaded at startup unless `--project` or
`--no-auto-load` is given.

## 9. Mock backend vs real device

**The mock is the default**, and it is what makes the product developable and testable without
hardware. It synthesizes a deterministic signal from the project's sine components plus optional
seeded Gaussian noise. Because the peak/RMS/p-p of a synthesized sine are analytically known, the
mock doubles as the oracle for the measurement tests. It also has fault-injection modes (stall,
link-drop, short stream) for negative testing.

The **M300 backend** is opt-in and requires all three of: a Windows x64 host, a build with
`--features m300`, and an explicit selection via `--backend m300` or `"backend": "m300"` in the
project. Selecting it anywhere else is a startup error rather than a silent fallback to mock.

```bat
cargo build --release --features m300
quickvib.exe --project samples\Test.proj --backend m300 --headless
```

The SDK DLL is located at runtime, in this order: `device.sdkPath` from the project, the
`QUICKVIB_M300_SDK` environment variable, the directory containing `quickvib.exe`, then the default
DLL search path. Nothing is resolved at process start, so a missing SDK yields
`-241,"Hardware missing"` and a log line naming every path probed — not a process that refuses to
launch.

There is **no fake or stub `M300Sdk.dll` in this repository**, and none will be added. The native
ABI is captured as a reviewed contract in [`docs/M300-NATIVE.md`](docs/M300-NATIVE.md), verified on a
Windows bench with the real SDK.

### 9.1 `m300-sim` — simulating the inbound device link

The mock is convenient but it short-circuits the transport: it hands samples straight to the engine,
so nothing on the socket path is exercised. `--backend tcp` closes that gap. It records the
little-endian `f32` stream arriving on `--device-port`, which is exactly what a real M300 pushes, and
`m300-sim` is a stand-in that dials in and streams a sine.

Start the two, in this order — QuickVib must be listening before the simulator dials in:

```bat
:: terminal 1 — the instrument
quickvib.exe --headless --backend tcp --device-port 9123 --project samples\Test.proj

:: terminal 2 — the "M300"
m300-sim.exe --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

```bash
# Linux / macOS
./target/release/quickvib --headless --backend tcp --device-port 9123 --project samples/Test.proj
./target/release/m300-sim --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

Then drive it over SCPI as usual — `INIT`, `REC:WAIT?`, `CALC:MEAS:ALL?` — and the numbers that come
back are measurements of bytes that crossed a socket.

| `m300-sim` argument | Value | Default | Behavior |
| --- | --- | --- | --- |
| `--host <host>` | host or IP | `127.0.0.1` | Where QuickVib is listening |
| `--port <n>` | 1–65535 | `9123` | QuickVib's `--device-port` |
| `--rate <hz>` | > 0 | `100000` | Samples per second, paced against real time |
| `--amplitude <a>` | finite | `250` | Peak amplitude, in whatever unit the project declares |
| `--frequency <hz>` | ≥ 0 | `120` | Tone frequency; `0` sends a flat line |
| `--duration <s>` | > 0 | *(until disconnected)* | Stop after this long |
| `--version` / `--help` | flag | — | Print and exit `0` |

The defaults match the first mock component in `samples/Test.proj`, so the pair works with no
arguments beyond the port. Exit codes: `0` the peer closed or `--duration` elapsed, `2` bad
arguments, `3` could not connect.

Things worth knowing:

* **The simulator is a TCP client.** QuickVib is the server on both links and never dials out, so
  the simulator connects *in*, just as the vibrometer does. Only one device link is honored at a
  time; a second is refused immediately.
* **A run captures from `INIT` onwards.** Whatever arrived while the instrument was idle is
  discarded, so a capture is always the samples that followed the trigger.
* **`SYST:DEV:CONN?` is `0` until something dials in**, and `INIT` before that is
  `-241,"Hardware missing"`. Unplugging mid-run aborts with `-240,"Hardware error"`.
* **Rates should match.** The simulator's `--rate` should equal the project's `device.sampleRateHz`.
  A slower simulator makes captures take proportionally longer and can trip the watchdog; a faster
  one just means some samples are never recorded.
* **Still no fake DLL.** `m300-sim` is a socket client and nothing else — it does not stub, wrap or
  imitate the vendor SDK, and the QuickVib code it exercises is the code a real vibrometer
  exercises.

## 10. Export formats

`FORM CSV|TXT` selects the format; `MMEM:STOR:TRAC "<path>"` writes the last completed capture.
Relative paths resolve against `export.directory`, missing directories are created, and existing
files are overwritten. Writes go to a temporary file in the target directory and are renamed into
place, so a UTS polling for the file never sees a half-written one.

**CSV** — optional comment preamble, then a header row, then one row per sample (CRLF line endings):

```csv
# project=Test
# timestamp=2026-01-31T12:34:56Z
# sampleRateHz=100000
# unit=velocity_um_s
# samples=500000
# durationSeconds=5.000
index,time_s,value
0,0,251.37
1,0.00001,248.9
2,0.00002,244.61
```

**TXT** — one value per line, nothing else, for scripts that just want numbers:

```text
251.37
248.9
244.61
```

Values use Rust's shortest round-tripping float formatting, which is locale-independent by
construction — a decimal point is always a `.`, never a comma.

## 11. Measurements

| Metric | Definition | SCPI query |
| --- | --- | --- |
| Peak | `max(abs(xᵢ))` | `CALC:MEAS:PEAK?` |
| RMS | `sqrt( (1/N) · Σ xᵢ² )` | `CALC:MEAS:RMS?` |
| Peak-to-peak | `max(xᵢ) − min(xᵢ)` | `CALC:MEAS:PP?` |

Computed in one pass over the capture, accumulated in `f64` even though samples are `f32`, so a
multi-hundred-thousand-sample capture does not lose precision. `measurement.removeDc` optionally
subtracts the mean before peak and RMS; peak-to-peak is unaffected by DC removal by definition.

Units follow the project's configured channel unit — velocity **μm/s**, displacement **μm**,
acceleration **m/s²**. QuickVib performs **no unit conversion**: it reports samples in the unit the
device is configured for and labels them accordingly.

## 12. Building and testing

```bash
cargo build --release              # release binaries: quickvib and m300-sim
cargo test                         # full test suite; runs on Linux, no hardware needed
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`cargo test` covers everything except the native M300 path: the Windows-only `quickvib-m300` crate is
excluded from the workspace's `default-members`, so a plain build, test, or clippy run on Linux never
compiles it. That includes the inbound device link end to end —
`crates/quickvib/tests/tcp_backend.rs` records, measures and exports over loopback, and
`crates/quickvib-sim/tests/pair.rs` spawns the built `m300-sim` binary against an in-process
instrument, so the pair documented in §9.1 is what CI actually runs.

**Windows executable, route A — native MSVC build (the shipped artifact):**

```bat
rustup target add x86_64-pc-windows-msvc
set RUSTFLAGS=-C target-feature=+crt-static
cargo build --release --target x86_64-pc-windows-msvc --locked
:: -> target\x86_64-pc-windows-msvc\release\quickvib.exe
```

`+crt-static` links the MSVC C runtime statically, so the test host needs **no Visual C++
redistributable**.

**Windows executable, route B — cross-compile from Linux with mingw-w64:**

```bash
sudo apt-get install -y gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu --locked
# -> target/x86_64-pc-windows-gnu/release/quickvib.exe
```

This works because the dependency graph is pure Rust — nothing has a C build script. The resulting
exe runs under Wine for the mock path. It is a developer and CI convenience target; the MSVC build
is what ships.

**Workspace map:**

| Crate | Responsibility |
| --- | --- |
| `quickvib` | Binary and composition root: CLI, TCP servers, backend selection, logging, exit codes |
| `quickvib-core` | Shared vocabulary: units, SCPI error catalogue, `Clock`, `CancelToken` |
| `quickvib-project` | JSON project schema, load/save/validate, auto-load-last |
| `quickvib-measure` | Peak/RMS/p-p statistics and CSV/TXT export |
| `quickvib-device` | `DeviceBackend` trait, mock and socket-fed backends, LE `f32` framer, inbound listener |
| `quickvib-scpi` | Lexer, command tree, parsed commands, response formatting |
| `quickvib-engine` | State machine, error queue, OPC, recording pipeline, dispatch |
| `quickvib-sim` | The `m300-sim` executable: an inbound M300 stand-in (§9.1) |
| `quickvib-testkit` | Dev-only shared test fixtures; ships nothing |
| `quickvib-m300` | **Windows-only**, out of `default-members`, the only crate containing `unsafe` |

## 13. Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| Exit code `3` at startup | A port is already in use. Another QuickVib instance, or something else on `5025`/`9123`. Pick a free port with `--scpi-port` / `--device-port` |
| Exit code `2` | Bad arguments — unknown flag, missing value, or a port outside 1–65535. Usage is on stderr |
| Exit code `4` | `--project` was given but the file is missing or fails schema validation. The log line carries the JSON line and column |
| `SYST:DEV:CONN?` returns `0` | The M300 has not dialed in. Check that it is powered, on the same network, configured to connect to this host on `9123`, that no firewall blocks the inbound connection, and that `device.allowedPeers` (if set) includes its address. With `--backend mock` this is always `1`, because the mock needs no link |
| `m300-sim` exits `3` | Nothing is listening on `--port`. Start QuickVib first, and check that its `--device-port` is the port the simulator is dialing |
| A `--backend tcp` run trips the watchdog | The simulator's `--rate` is below the project's `device.sampleRateHz`, so the expected sample count never arrives in time. Match the two, or raise `recording.timeoutMultiplier` |
| `-241,"Hardware missing"` at `INIT` | Either no device is connected, or the SDK DLL could not be loaded. The log lists every path probed and the OS error for each. Confirm the DLL location per §9 |
| `-240,"Hardware error"` mid-run | The device link dropped during `ARMED` or `RECORDING`. The run moves to `ABORTED`; re-arm with `INIT` |
| `-365,"Time out error"` | The watchdog fired: samples stopped arriving before the expected count was reached. Check the link, or raise `recording.timeoutMultiplier` |
| `-230,"Data corrupt or stale"` | A data or measurement query with no completed capture. Aborted runs are deliberately not fetchable |
| `-222,"Data out of range"` | Duration outside `(0, 3600]`, or `duration × sampleRateHz × 4 bytes` over `recording.maxCaptureBytes` |
| A command seems ignored | Unknown commands return nothing by design. Read `SYST:ERR?` — `-113,"Undefined header"` means a typo or an unsupported spelling |
| `FETC?` truncates or times out on the UTS | The response is one very long line. Call `TRAC:POIN?` first and size the client read buffer; or export with `MMEM:STOR:TRAC` and read the file instead |

When reporting a problem, include the `--headless` log output and the output of `SYST:ERR?` drained
until it returns `0,"No error"`.

## 14. Scope — what QuickVib does not do

* **No pass/fail evaluation.** No limits, no verdicts, no tolerance bands. QuickVib returns numbers;
  the UTS judges them.
* **No DUT vibration control.** No shaker or exciter drive. The mock's synthesized signal is a test
  fixture, not DUT excitation.
* **No retry logic.** A failed or aborted capture is reported; re-running it is the UTS's decision.
* **No fake native DLL.** Linux testability comes from the mock backend and from `m300-sim`, which
  is a plain socket client — not from faking the vendor library.

Also out of scope for v1: GUI, real-time plotting, FFT/spectral analysis, multi-channel capture,
VISA/HiSLIP/VXI-11 transports (raw socket only), USBTMC, and triggering beyond immediate `INIT`.

## 15. Contributing and license

* **`unsafe` policy.** Every crate is `#![forbid(unsafe_code)]` except `quickvib-m300`, which is
  `#![deny(unsafe_op_in_unsafe_fn)]` and requires a `SAFETY:` comment on every `unsafe` block. A pull
  request that introduces `unsafe` anywhere else will not be accepted.
* **Tests run on Linux.** Any change must keep `cargo test` green on Linux without hardware. Windows
  and device-specific code belongs behind the `DeviceBackend` trait.
* **Lints are errors.** `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` gate CI.
* **No new runtime dependencies** without a justification in `docs/PLAN.md` §6.3.
* **Time comes from the `Clock` trait.** `Instant::now` and `SystemTime::now` are banned outside
  `quickvib-core::clock` so a "5-second" capture runs in milliseconds under test.

License: not yet finalized — see `docs/PLAN.md`.

---
---

# QuickVib (中文)

**[English](#quickvib) | 中文**

QuickVib 通过 **SCPI over TCP** 把 **M300 激光多普勒测振仪**封装成一台**类 Keysight 仪器**，供既有的
UTS（单元测试系统）调用。它是一个自包含的 Windows 控制台可执行程序：UTS 从命令行启动它，建立 TCP
连接，然后像对台式仪器那样发送标准 SCPI 文本命令（`*IDN?`、`INIT`、`FETC?` 等）。QuickVib 完成一次
定长振动采集，返回采样数据与导出的标量结果（峰值、有效值 RMS、峰峰值），并可导出 CSV 或 TXT。

功能边界是刻意收窄的：QuickVib 只负责**采集、测量、导出**。它不做合格判定，不驱动 DUT 振动，也不负责
重试策略——这些仍由 UTS 负责。

默认设备后端是纯 Rust 实现的**模拟（mock）后端**，因此整个产品在 Linux 和 Windows 上都可以编译、运行
并完整测试，无需真实硬件，也无需厂商 SDK。若要在没有 M300 的情况下演练**真实的设备入站链路**，可以用
`--backend tcp`：它会记录任何连入设备端口的数据流；仓库同时提供 **`m300-sim`** 可执行程序作为主动连入
并推送数据的替身——参见 [§9.1](#91-m300-sim模拟设备入站链路)。

> **当前状态。** 本仓库正在按 [`docs/PLAN.md`](docs/PLAN.md) 的计划实现。受支持的路径是模拟后端与
> 套接字驱动的 `tcp` 后端。M300 原生后端（`crates/quickvib-m300`）目前**只有脚手架**——尚未实现任何
> SDK 调用；按设计，仓库中不存在任何假的 DLL。详见 [`docs/M300-NATIVE.md`](docs/M300-NATIVE.md)。

## 目录

1. [运行环境](#1-运行环境)
2. [快速开始](#2-快速开始)
3. [UTS 如何启动 QuickVib](#3-uts-如何启动-quickvib)
4. [命令行参数](#4-命令行参数)
5. [工作原理](#5-工作原理)
6. [SCPI 命令参考](#6-scpi-命令参考)
7. [UTS 会话示例](#7-uts-会话示例)
8. [工程文件格式](#8-工程文件格式)
9. [模拟后端与真实设备](#9-模拟后端与真实设备)
10. [导出格式](#10-导出格式)
11. [测量定义](#11-测量定义)
12. [编译与测试](#12-编译与测试)
13. [常见问题排查](#13-常见问题排查)
14. [功能边界](#14-功能边界)
15. [贡献指南与许可证](#15-贡献指南与许可证)

## 1. 运行环境

| 用途 | 需要 |
| --- | --- |
| 编译 | Rust stable 工具链，edition 2021（具体版本固定在 `rust-toolchain.toml`） |
| 运行（模拟后端） | 无需任何依赖。可执行文件为静态链接，不需要运行库或分发包。任意操作系统均可 |
| 运行（真实 M300） | Windows x64、M300 测振仪，以及主机上已安装的 M300 SDK v1.2.0。SDK **不随本仓库分发** |
| 在 Linux 上交叉编译 Windows `.exe` | `gcc-mingw-w64-x86-64` 与 `x86_64-pc-windows-gnu` 目标 |

运行时依赖被刻意压到最少：工程文件解析使用 `serde` + `serde_json`；`libloading` 仅在 Windows 上供
M300 crate 使用。没有异步运行时，没有命令行框架，也没有日志框架。

## 2. 快速开始

```bash
cargo build --release
```

使用示例工程与模拟后端运行：

```bash
# Linux / macOS
./target/release/quickvib --project samples/Test.proj --headless
```

```bat
:: Windows
target\release\quickvib.exe --project samples\Test.proj --headless
```

然后用任意裸 socket 客户端（`telnet`、raw 模式的 PuTTY、`nc`，或 UTS 本身）与它通信：

```bash
printf '*IDN?\n' | nc 127.0.0.1 5025
# QuickVib,M300-SCPI,SN-0001,1.0.0
```

若想演练真实的设备链路而不是模拟后端，用 `--backend tcp` 启动 QuickVib，再用随仓库提供的模拟器连
入——参见 [§9.1](#91-m300-sim模拟设备入站链路)。

## 3. UTS 如何启动 QuickVib

这就是 UTS 使用的启动命令。四个参数中有三个本来就是默认值，但仍全部显式写出——在测试工位上，一条明确
的命令行可以少一个意外来源：

```bat
quickvib.exe --headless --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

| 参数 | 含义 |
| --- | --- |
| `--headless` | 无交互式控制台界面，仅在 stdout 输出单行结构化日志。UTS 启动时使用此模式 |
| `--scpi-port 5025` | QuickVib **监听 UTS** 的端口。`5025` 是 SCPI 裸 socket 的惯用端口 |
| `--device-port 9123` | QuickVib **监听 M300** 的端口。由测振仪主动连入 |
| `--project samples\Test.proj` | 启动时加载的工程文件：采样率、单位、录制时长、导出格式、`*IDN?` 标识 |

QuickVib 在两条链路上**都是 TCP 服务端**，从不主动外连。进程会一直在前台运行直到被终止，因此 UTS 可
以把它作为子进程启动、再通过结束进程关闭；带内的正常关闭流程是先 `ABOR`、再关闭 SCPI 连接。

退出码：`0` 正常退出，`2` 参数错误，`3` 端口绑定失败，`4` 工程加载失败（仅当显式指定 `--project`
时），`5` 后端打开失败。

## 4. 命令行参数

| 参数 | 取值 | 默认值 | 行为 |
| --- | --- | --- | --- |
| `--project <path>` | 文件路径 | *(自动加载上次工程)* | 启动时加载该工程，失败即致命错误（退出码 `4`） |
| `--scpi-port <n>` | 1–65535 | `5025` | SCPI 服务端口（UTS 连入） |
| `--device-port <n>` | 1–65535 | `9123` | 设备服务端口（M300 连入），覆盖 `device.port` |
| `--headless` | 开关 | 关 | 仅输出结构化日志，无交互界面 |
| `--backend <mock\|tcp\|m300>` | 枚举 | *(工程配置，否则 `mock`)* | 覆盖工程中的后端选择。`mock` 在进程内合成信号；`tcp` 记录从 `--device-port` 连入的小端 `f32` 数据流（真实 M300，或 `m300-sim`）；`m300` 走原生 SDK，在非 Windows 主机、或未启用 `m300` feature 的构建上指定它会直接启动失败，绝不静默回退 |
| `--no-auto-load` | 开关 | 关 | 禁用「自动加载上次工程」，保证运行环境干净 |
| `--log-level <level>` | 枚举 | `info` | `trace\|debug\|info\|warn\|error` |
| `--version` | 开关 | — | 打印版本号并以 `0` 退出 |
| `--help` | 开关 | — | 打印用法并以 `0` 退出 |

`--flag=value` 与 `--flag value` 两种写法都支持；`--` 终止参数解析。未知参数、缺少取值、端口越界都会
以 `2` 退出，并在 stderr 打印用法。

## 5. 工作原理

系统中存在两个相互独立的 TCP 角色，混淆两者是最典型的入门错误：

| 角色 | 方向 | 默认端口 | 对端 |
| --- | --- | --- | --- |
| **SCPI 服务端** | QuickVib 监听，**UTS 连入** | `5025` | UTS 测试执行程序 |
| **设备服务端** | QuickVib 监听，**M300 连入** | `9123` | M300 测振仪 |

```
   UTS  ──SCPI over TCP──▶  :5025  ┌──────────────────────────┐
                                   │        quickvib.exe      │
                                   │  SCPI 解析 → 仪器引擎     │
                                   │  → 录制流水线            │──▶  *.csv / *.txt
                                   │  → 测量计算              │
   M300 ──小端 f32 数据流──▶ :9123  └──────────────────────────┘
```

采样数据以原始的**小端 `f32`** 数据流到达，字节分片可以落在任意边界上，由 framer 还原成 `f32` 采样
值。线上格式只有这一种，没有握手、没有报文头、也没有长度前缀：每个采样点 4 字节，首尾相接，只要链路
还在就一直发。一次采集以**采样点数**（而非墙上时钟）为结束条件，另有墙上时钟看门狗兜底，因此结果是
确定性的，同时容忍设备抖动。

无论使用哪个后端，设备端口都会被绑定并完成 framing。`--backend mock` 下这些还原出来的采样只被计数和
记录日志，采集数据由 mock 提供；`--backend tcp` 下它们**就是**采集数据。

一次测量的完整流程是：加载工程 → `INIT` → 采样持续流入，直到收满 `时长 × 采样率` 个点 → 状态机进入
`COMPLETE`，向所有已连接会话推送 `#REC:DONE`，`REC:WAIT?` 解除阻塞 → `CALC:MEAS:*?` 返回标量结果 →
（可选）`MMEM:STOR:TRAC` 将采集数据写入磁盘。

仪器状态（由 `REC:STAT?` 返回）：

```
IDLE ──INIT──▶ ARMED ──首个采样──▶ RECORDING ──收满点数──▶ COMPLETE
                 │                     │                      │
                 └──── ABOR / 超时 ─────┴───────▶ ABORTED ◀────┘
```

## 6. SCPI 命令参考

命令为 ASCII 文本，以 `\n` 结束（同时容忍 `\r\n`），大小写不敏感，长短两种拼写形式均支持。支持用分号
串联的复合消息（`*CLS;*IDN?`）。响应为单行，以 `\n` 结束。

**IEEE 488.2 强制命令**

| 命令 | 响应 | 行为 |
| --- | --- | --- |
| `*IDN?` | `QuickVib,M300-SCPI,<serial>,<fw>` | 厂商、型号、序列号、固件版本。四个字段均可在工程的 `identity` 中配置 |
| `*RST` | — | 中止当前采集、丢弃数据、恢复到工程配置值、清空错误队列 |
| `*CLS` | — | 清空错误队列与状态寄存器 |
| `*OPC` / `*OPC?` | — / `1` | 操作完成位；查询形式会阻塞到当前采集结束 |
| `SYST:ERR?` | `<code>,"<message>"` | 弹出最早的一条错误；队列为空时返回 `0,"No error"` |

**工程 / 文件存储**

| 命令 | 行为 |
| --- | --- |
| `MMEM:LOAD:STAT "<path>"` | 加载 JSON 工程文件 |
| `MMEM:STOR:STAT "<path>"` | 保存当前工程（含运行时覆盖值） |
| `MMEM:LOAD:AUTO` / `MMEM:LOAD:AUTO?` | 加载 / 查询最近一次使用的工程 |
| `MMEM:STOR:TRAC "<path>"` | 按当前格式导出最近一次完成的采集 |

**配置、录制与数据**

| 命令 | 响应 | 行为 |
| --- | --- | --- |
| `CONF:REC:DUR <s>` / `CONF:REC:DUR?` | — / `5.000` | 录制时长（秒），范围 `(0, 3600]` |
| `FORM CSV\|TXT` / `FORM?` | — / `CSV` | 导出格式 |
| `INIT`（别名 `REC:STAR`） | — | 开始录制。非阻塞，并丢弃上一次采集数据 |
| `ABOR` | — | 中止当前录制；空闲时调用不算错误 |
| `REC:STAT?` | `IDLE\|ARMED\|RECORDING\|COMPLETE\|ABORTED` | 当前状态，可安全轮询 |
| `REC:WAIT?` | `1` \| `0` | 阻塞至录制完成（`1`）或中止/超时（`0`） |
| `FETC?`（别名 `TRAC:DATA?`） | `v1,v2,…,vN` | 最近一次完成采集的全部采样，逗号分隔 |
| `TRAC:POIN?` | `500000` | 采样点数，便于 UTS 在 `FETC?` 前分配读缓冲 |
| `CALC:MEAS:PEAK?` | `12.3400` | `max(abs(x))` |
| `CALC:MEAS:RMS?` | `4.5600` | 均方根 |
| `CALC:MEAS:PP?` | `24.6800` | `max − min` |
| `CALC:MEAS:ALL?` | `12.3400,4.5600,24.6800` | 依次为峰值、RMS、峰峰值，一次往返取回三个值 |
| `SYST:DEV:CONN?` | `1` \| `0` | 设备后端会话是否已建立 |
| `SYST:VERS?` | `1999.0` | SCPI 标准版本 |

**带外通知。** 以 `#` 开头的行都是主动推送，绝不会是某条查询的响应：录制成功完成推送 `#REC:DONE`，
中止或超时推送 `#REC:ABORT`。每个会话有独立的写锁，保证通知不会插入到某行响应的中间。偏好严格
「请求—响应」模式的客户端可以忽略这些通知，改用 `REC:WAIT?`。

**错误码**全部沿用 SCPI-99 标准，不自造新码：

| 代码 | 消息 | 触发场景 |
| --- | --- | --- |
| `0` | `No error` | 队列为空 |
| `-100` | `Command error` | 消息格式错误、行过长、非 UTF-8 输入 |
| `-113` | `Undefined header` | 未知命令 |
| `-221` | `Settings conflict` | 未加载工程、已在录制、或录制期间修改配置 |
| `-222` | `Data out of range` | 时长超出 `(0, 3600]`，或超过采集缓冲上限 |
| `-224` | `Illegal parameter value` | `FORM` 取值非法、工程 schema 校验失败 |
| `-230` | `Data corrupt or stale` | 在没有已完成采集时查询数据 |
| `-240` | `Hardware error` | 录制过程中设备链路断开；SDK 返回非零错误码 |
| `-241` | `Hardware missing` | `INIT` 时无设备连接；找不到 SDK 动态库或符号 |
| `-256` / `-257` | `File name not found` / `File name error` | 工程文件不存在；路径不可写 |
| `-350` | `Queue overflow` | 未读错误超过 32 条 |
| `-365` | `Time out error` | 采集过程中看门狗触发 |

## 7. UTS 会话示例

一次完整测量，按 UTS 的实际驱动顺序。`>` 表示发送给 QuickVib，`<` 表示收到的内容。

```
> *IDN?
< QuickVib,M300-SCPI,SN-0001,1.0.0

> *CLS                                  ; 先清空错误队列
> MMEM:LOAD:STAT "samples\Test.proj"    ; 采样率、单位、时长、标识
> SYST:ERR?
< 0,"No error"                          ; 加载成功

> SYST:DEV:CONN?
< 1                                     ; 模拟后端恒为已连接；M300 需已连入

> CONF:REC:DUR 5.0                      ; 覆盖工程中的时长
> FORM CSV

> INIT                                  ; 非阻塞命令，无响应，采集开始
> REC:WAIT?                             ; 阻塞直到采集结束
< #REC:DONE                             ; 完成时主动推送的通知
< 1                                     ; REC:WAIT? 返回 1，表示完成

> REC:STAT?
< COMPLETE

> TRAC:POIN?
< 500000                                ; 5.0 s × 100 kS/s

> CALC:MEAS:ALL?
< 252.4913,178.5402,504.9826            ; 峰值、RMS、峰峰值，单位同工程配置

> MMEM:STOR:TRAC "out\run001.csv"       ; 导出采集数据
> SYST:ERR?
< 0,"No error"
```

给 UTS 开发者的提示：

* `INIT` 是命令而不是查询，**没有响应**；等待响应会一直挂住。
* 用 `REC:STAT?` 轮询，或用 `REC:WAIT?` 阻塞等待；两者都受服务端看门狗约束，最长不会超过
  `时长 × timeoutMultiplier + 1 秒`。
* 未知命令会把 `-113` 压入错误队列并且**不返回任何内容**——这是标准仪器行为。通过 `SYST:ERR?` 查明。
* 5 秒 × 100 kS/s 的采集，`FETC?` 的响应约为 6 MB 的一整行。建议先用 `TRAC:POIN?` 确定点数再分配读
  缓冲；若只需要标量结果，直接用 `CALC:MEAS:ALL?`。

## 8. 工程文件格式

工程文件是 JSON（`*.proj`）。为保证向前兼容，未知字段会被忽略。示例见 `samples/Test.proj`。

```json
{
  "schemaVersion": 1,
  "name": "Test",
  "device": {
    "backend": "mock",
    "port": 9123,
    "sampleRateHz": 100000,
    "unit": "velocity_um_s",
    "connectTimeoutSeconds": 30
  },
  "recording": { "durationSeconds": 5.0, "timeoutMultiplier": 2.0 },
  "measurement": { "removeDc": false, "responseDecimals": 4 },
  "export": { "format": "CSV", "directory": "./out", "includeHeader": true },
  "identity": {
    "manufacturer": "QuickVib",
    "model": "M300-SCPI",
    "serialNumber": "SN-0001",
    "firmwareVersion": "1.0.0"
  },
  "mock": {
    "signal": {
      "components": [{ "frequencyHz": 120.0, "amplitude": 250.0, "phaseDeg": 0.0 }],
      "noiseStdDev": 2.5,
      "seed": 12345
    }
  }
}
```

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `device.backend` | `"mock"` | `"mock"` 或 `"m300"` |
| `device.port` | `9123` | M300 连入的端口；`--device-port` 优先 |
| `device.sampleRateHz` | *(必填)* | 例如 `100000` |
| `device.unit` | *(必填)* | `"velocity_um_s"`、`"displacement_um"`、`"acceleration_m_s2"` |
| `device.allowedPeers` | `[]` | 为空表示接受任意来源地址 |
| `device.sdkPath` | `null` | M300 SDK 的查找目录（仅 M300 后端） |
| `device.lpfHz` | `null` | 低通截止频率；`null` 表示跟随奈奎斯特频率（`sampleRateHz / 2`） |
| `device.highPassHz` | `0` | 高通截止频率；`0` 表示关闭。必须小于低通截止频率 |
| `device.velocityRange` / `.displacementRange` / `.accelerationRange` | `1000` / `1000` / `100` | 量程，单位分别为 µm/s、µm、m/s²；实际生效的是与 `device.unit` 对应的那一个 |
| `recording.durationSeconds` | *(必填)* | `(0, 3600]` |
| `recording.timeoutMultiplier` | `2.0` | 看门狗时限 = 时长 × 该值 + 1 秒 |
| `recording.maxCaptureBytes` | `536870912` | 超过该上限的采集会在 `INIT` 时以 `-222` 拒绝 |
| `measurement.removeDc` | `false` | 计算峰值与 RMS 前先减去均值 |
| `measurement.responseDecimals` | `4` | `CALC:*` 响应的小数位数，`0..=9` |
| `export.format` / `.directory` / `.includeHeader` | `"CSV"` / `"."` / `true` | 导出默认值 |
| `identity.*` | 见上 | `*IDN?` 的四个字段，可满足既有 UTS 的标识校验 |
| `server.maxSessions` | `8` | 并发 SCPI 会话上限 |
| `server.scpiPort` | `5025` | 工程中记录的 SCPI 监听端口；`--scpi-port` 优先 |
| `mock.signal.*` | 一个 100 Hz 分量 | 正弦分量、高斯噪声 σ、随机数种子 |

最近一次加载或保存的工程路径会被记录（Windows 上为 `%LOCALAPPDATA%\QuickVib\`，Unix 上为
`$XDG_STATE_HOME/quickvib/`），并在启动时自动加载，除非指定了 `--project` 或 `--no-auto-load`。

## 9. 模拟后端与真实设备

**默认使用模拟后端**，这正是无需硬件即可开发与测试的原因。它按工程配置的正弦分量合成确定性信号，并可
叠加带种子的高斯噪声。由于合成正弦的峰值 / RMS / 峰峰值有解析解，模拟后端同时充当测量算法测试的
「标准答案」。它还提供故障注入模式（停顿、链路断开、数据流截断）用于异常路径测试。

**M300 后端**必须显式启用，且同时满足三个条件：Windows x64 主机、使用 `--features m300` 编译、并通过
`--backend m300` 或工程中的 `"backend": "m300"` 显式选择。在其他情况下选择它会直接启动失败，而不会静
默回退到模拟后端。

```bat
cargo build --release --features m300
quickvib.exe --project samples\Test.proj --backend m300 --headless
```

SDK 动态库在运行时按以下顺序查找：工程中的 `device.sdkPath`、环境变量 `QUICKVIB_M300_SDK`、
`quickvib.exe` 所在目录、系统默认 DLL 搜索路径。进程启动时不做任何解析，因此缺少 SDK 只会得到
`-241,"Hardware missing"` 以及一条列出所有已尝试路径的日志——而不是一个根本无法启动的进程。

本仓库中**没有、也不会加入任何假的或桩实现的 `M300Sdk.dll`**。原生 ABI 以契约文档的形式记录在
[`docs/M300-NATIVE.md`](docs/M300-NATIVE.md) 中，并在装有真实 SDK 的 Windows 机器上验证。

### 9.1 `m300-sim`：模拟设备入站链路

模拟后端很方便，但它绕过了传输层：采样直接交给仪器引擎，socket 路径上的代码一行都没跑到。
`--backend tcp` 补上了这一段——它记录从 `--device-port` 连入的小端 `f32` 数据流，也就是真实 M300 推送
的内容；而 `m300-sim` 就是那个主动连入并推送正弦信号的替身。

按下面的顺序启动两个进程——必须先让 QuickVib 处于监听状态，模拟器才连得进来：

```bat
:: 终端 1 —— 仪器
quickvib.exe --headless --backend tcp --device-port 9123 --project samples\Test.proj

:: 终端 2 —— 「M300」
m300-sim.exe --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

```bash
# Linux / macOS
./target/release/quickvib --headless --backend tcp --device-port 9123 --project samples/Test.proj
./target/release/m300-sim --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

之后照常通过 SCPI 驱动它——`INIT`、`REC:WAIT?`、`CALC:MEAS:ALL?`——返回的数值就是对真正穿过 socket 的
字节所做的测量。

| `m300-sim` 参数 | 取值 | 默认值 | 行为 |
| --- | --- | --- | --- |
| `--host <host>` | 主机名或 IP | `127.0.0.1` | QuickVib 所在地址 |
| `--port <n>` | 1–65535 | `9123` | QuickVib 的 `--device-port` |
| `--rate <hz>` | > 0 | `100000` | 每秒采样点数，按真实时间节流 |
| `--amplitude <a>` | 有限数 | `250` | 峰值幅度，单位由工程配置决定 |
| `--frequency <hz>` | ≥ 0 | `120` | 正弦频率；`0` 表示发送恒定的平直信号 |
| `--duration <s>` | > 0 | *(直到断开)* | 达到该时长后停止 |
| `--version` / `--help` | 开关 | — | 打印并以 `0` 退出 |

默认值与 `samples/Test.proj` 中的第一个 mock 分量一致，因此除端口外无需任何参数即可配对运行。退出码：
`0` 对端关闭或 `--duration` 到时，`2` 参数错误，`3` 无法连接。

几点需要知道的：

* **模拟器是 TCP 客户端。** QuickVib 在两条链路上都是服务端、从不主动外连，所以模拟器像测振仪一样
  **连入**。同一时刻只接受一条设备链路，第二条会被立即拒绝。
* **一次采集从 `INIT` 之后开始。** 仪器空闲期间到达的数据会被丢弃，因此采集到的永远是触发之后的采样。
* **在有设备连入之前 `SYST:DEV:CONN?` 返回 `0`**，此时 `INIT` 会得到 `-241,"Hardware missing"`；
  采集途中断开则以 `-240,"Hardware error"` 中止。
* **采样率要对齐。** 模拟器的 `--rate` 应等于工程的 `device.sampleRateHz`。模拟器更慢会让采集时间成
  比例变长并可能触发看门狗；更快则只是有一部分采样永远不会被记录。
* **依然没有假 DLL。** `m300-sim` 只是一个 socket 客户端——它不桩实现、不包装、也不模仿厂商 SDK；它
  驱动到的 QuickVib 代码，与真实测振仪驱动到的完全相同。

## 10. 导出格式

`FORM CSV|TXT` 选择格式，`MMEM:STOR:TRAC "<path>"` 写出最近一次完成的采集。相对路径基于
`export.directory` 解析，缺失目录会自动创建，已存在的文件会被覆盖。写入先落到目标目录下的临时文件，
再重命名到位，因此轮询该文件的 UTS 绝不会读到写了一半的内容。

**CSV** —— 可选的注释头、表头行，然后每个采样一行（使用 CRLF 换行）：

```csv
# project=Test
# timestamp=2026-01-31T12:34:56Z
# sampleRateHz=100000
# unit=velocity_um_s
# samples=500000
# durationSeconds=5.000
index,time_s,value
0,0,251.37
1,0.00001,248.9
2,0.00002,244.61
```

**TXT** —— 每行一个数值，没有任何表头，供只需要数字的脚本使用：

```text
251.37
248.9
244.61
```

数值采用 Rust 的最短往返浮点格式化，天然与区域设置无关——小数点永远是 `.`，不会变成逗号。

## 11. 测量定义

| 指标 | 定义 | SCPI 查询 |
| --- | --- | --- |
| 峰值 Peak | `max(abs(xᵢ))` | `CALC:MEAS:PEAK?` |
| 有效值 RMS | `sqrt( (1/N) · Σ xᵢ² )` | `CALC:MEAS:RMS?` |
| 峰峰值 p-p | `max(xᵢ) − min(xᵢ)` | `CALC:MEAS:PP?` |

以单趟遍历计算；尽管采样是 `f32`，累加过程使用 `f64`，因此几十万点的采集不会损失精度。
`measurement.removeDc` 可选地在计算峰值与 RMS 前减去均值；按定义，峰峰值不受去直流影响。

单位沿用工程中配置的通道单位——速度 **μm/s**、位移 **μm**、加速度 **m/s²**。QuickVib **不做任何单位
换算**：设备配置为什么单位，就以什么单位上报并如实标注。

## 12. 编译与测试

```bash
cargo build --release              # 发布版可执行文件：quickvib 与 m300-sim
cargo test                         # 完整测试套件；可在 Linux 上运行，无需硬件
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`cargo test` 覆盖除原生 M300 路径以外的全部代码：仅限 Windows 的 `quickvib-m300` crate 被排除在
workspace 的 `default-members` 之外，因此在 Linux 上执行普通的 build / test / clippy 时根本不会编译它。
这其中也包含端到端的设备入站链路：`crates/quickvib/tests/tcp_backend.rs` 在环回地址上完成录制、测量与
导出，`crates/quickvib-sim/tests/pair.rs` 则直接启动编译好的 `m300-sim` 可执行文件，让它连入进程内的
仪器——也就是说 §9.1 中记录的这一对进程正是 CI 实际运行的对象。

**生成 Windows 可执行文件，方式 A —— Windows 原生 MSVC 构建（实际交付的产物）：**

```bat
rustup target add x86_64-pc-windows-msvc
set RUSTFLAGS=-C target-feature=+crt-static
cargo build --release --target x86_64-pc-windows-msvc --locked
:: -> target\x86_64-pc-windows-msvc\release\quickvib.exe
```

`+crt-static` 会静态链接 MSVC C 运行库，因此测试主机**无需安装 Visual C++ 运行库分发包**。

**生成 Windows 可执行文件，方式 B —— 在 Linux 上用 mingw-w64 交叉编译：**

```bash
sudo apt-get install -y gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu --locked
# -> target/x86_64-pc-windows-gnu/release/quickvib.exe
```

之所以可行，是因为依赖图是纯 Rust 的——没有任何依赖带 C 构建脚本。生成的 exe 在 Wine 下可以跑通模拟
后端路径。该方式定位为开发与 CI 的便利手段，正式交付仍以 MSVC 构建为准。

**workspace 各 crate 职责：**

| Crate | 职责 |
| --- | --- |
| `quickvib` | 可执行程序与组装根：命令行、TCP 服务、后端选择、日志、退出码 |
| `quickvib-core` | 共享词汇：单位、SCPI 错误码表、`Clock`、`CancelToken` |
| `quickvib-project` | JSON 工程 schema、加载/保存/校验、自动加载上次工程 |
| `quickvib-measure` | 峰值 / RMS / 峰峰值统计与 CSV/TXT 导出 |
| `quickvib-device` | `DeviceBackend` trait、模拟后端与套接字驱动后端、小端 `f32` framer、入站监听 |
| `quickvib-scpi` | 词法分析、命令树、命令解析结果、响应格式化 |
| `quickvib-engine` | 状态机、错误队列、OPC、录制流水线、命令分发 |
| `quickvib-sim` | `m300-sim` 可执行程序：模拟 M300 主动连入的替身（§9.1） |
| `quickvib-testkit` | 仅供测试使用的共享夹具，不参与发布 |
| `quickvib-m300` | **仅 Windows**，不在 `default-members` 中，是唯一包含 `unsafe` 的 crate |

## 13. 常见问题排查

| 现象 | 原因与处理 |
| --- | --- |
| 启动即退出码 `3` | 端口被占用：可能是另一个 QuickVib 实例，或其他程序占用了 `5025`/`9123`。用 `--scpi-port` / `--device-port` 换端口 |
| 退出码 `2` | 参数错误——未知参数、缺少取值，或端口不在 1–65535。用法信息输出在 stderr |
| 退出码 `4` | 指定了 `--project` 但文件不存在或 schema 校验失败。日志中包含出错的 JSON 行号与列号 |
| `SYST:DEV:CONN?` 返回 `0` | M300 尚未连入。检查设备是否上电、是否与主机同网段、是否配置为连接本机 `9123`、防火墙是否拦截入站连接，以及 `device.allowedPeers`（若配置）是否包含其地址。`--backend mock` 下该查询恒为 `1`，因为模拟后端不需要链路 |
| `m300-sim` 退出码 `3` | `--port` 上没有任何进程在监听。请先启动 QuickVib，并确认其 `--device-port` 与模拟器拨入的端口一致 |
| `--backend tcp` 采集触发看门狗 | 模拟器的 `--rate` 低于工程的 `device.sampleRateHz`，预期点数无法及时收满。让两者一致，或调大 `recording.timeoutMultiplier` |
| `INIT` 时返回 `-241,"Hardware missing"` | 要么没有设备连入，要么 SDK 动态库加载失败。日志会列出所有尝试过的路径及各自的系统错误。按第 9 节确认 DLL 位置 |
| 运行中出现 `-240,"Hardware error"` | 在 `ARMED` 或 `RECORDING` 期间设备链路断开。状态转为 `ABORTED`，可用 `INIT` 重新开始 |
| `-365,"Time out error"` | 看门狗触发：采样在收满预期点数前停止到达。检查链路，或调大 `recording.timeoutMultiplier` |
| `-230,"Data corrupt or stale"` | 在没有已完成采集时查询数据或测量值。被中止的采集按设计不可读取 |
| `-222,"Data out of range"` | 时长超出 `(0, 3600]`，或 `时长 × 采样率 × 4 字节` 超过 `recording.maxCaptureBytes` |
| 某条命令似乎被忽略 | 未知命令按设计不返回任何内容。读取 `SYST:ERR?`，`-113,"Undefined header"` 说明命令拼写错误或不受支持 |
| UTS 侧 `FETC?` 被截断或超时 | 响应是极长的一行。先用 `TRAC:POIN?` 确定点数并调大客户端读缓冲；或改用 `MMEM:STOR:TRAC` 导出后读文件 |

反馈问题时，请附上 `--headless` 的日志输出，以及反复执行 `SYST:ERR?` 直到返回 `0,"No error"` 为止的
全部错误信息。

## 14. 功能边界

* **不做合格判定。** 没有限值、没有结论、没有容差带。QuickVib 只给出数值，判定由 UTS 负责。
* **不驱动 DUT 振动。** 不控制振动台或激励源。模拟后端合成的信号是测试夹具，不是对 DUT 的激励。
* **不做重试逻辑。** 采集失败或被中止会如实上报，是否重跑由 UTS 决定。
* **不提供假的原生 DLL。** 在 Linux 上的可测试性来自模拟后端，以及只是一个普通 socket 客户端的
  `m300-sim`——而不是伪造厂商库。

v1 同样不包含：图形界面、实时绘图、FFT / 频谱分析、多通道采集、VISA/HiSLIP/VXI-11 传输（仅支持裸
socket）、USBTMC，以及除立即 `INIT` 之外的触发方式。

## 15. 贡献指南与许可证

* **`unsafe` 策略。** 除 `quickvib-m300` 外，每个 crate 都是 `#![forbid(unsafe_code)]`；该 crate 使用
  `#![deny(unsafe_op_in_unsafe_fn)]`，且每个 `unsafe` 块都必须带 `SAFETY:` 注释。在其他任何位置引入
  `unsafe` 的 PR 都不会被接受。
* **测试必须能在 Linux 上跑。** 任何改动都要保证 `cargo test` 在 Linux 上无硬件通过。Windows 与设备
  相关的代码应放在 `DeviceBackend` trait 之后。
* **告警即错误。** CI 以 `cargo fmt --check` 与 `cargo clippy --all-targets -- -D warnings` 为门禁。
* **不随意新增运行时依赖**，除非在 `docs/PLAN.md` §6.3 中给出理由。
* **时间来自 `Clock` trait。** `Instant::now` 与 `SystemTime::now` 在 `quickvib-core::clock` 之外被禁
  用，从而让「5 秒」的采集在测试中只需几毫秒。

许可证：尚未最终确定，详见 `docs/PLAN.md`。
