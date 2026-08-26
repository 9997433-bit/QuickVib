# QuickVib Software Usage Manual

**English | [简体中文](#quickvib-软件使用手册简体中文)**

This manual is for test-cell operators and UTS integrators. It focuses on running QuickVib and
completing a measurement. For protocol details and design rationale, use the links at the end.

## 1. Overview

QuickVib exposes an M300 laser Doppler vibrometer as a raw-socket SCPI instrument. It records one
channel for a fixed duration, calculates peak/RMS/peak-to-peak, and exports CSV or TXT. The UTS
remains responsible for DUT stimulus, pass/fail decisions, and retries.

There are two independent inbound TCP links:

| Link | QuickVib role | Default port | Connecting peer |
| --- | --- | --- | --- |
| SCPI | Server | `5025` | UTS |
| Device | Server through QuickVib or the vendor SDK | `9123` | M300 or a simulator |

QuickVib never dials either peer. The UTS connects to `5025`; the device connects to `9123`.

Run modes:

| Mode | Invocation | Use |
| --- | --- | --- |
| Headless | Add `--headless` | UTS/automation; no window, structured logs |
| GUI | Omit `--headless` from a build made with `--features gui` | Operator setup and manual runs |

## 2. Build

The pinned stable Rust toolchain is defined by `rust-toolchain.toml`.

### Linux

```bash
# Headless QuickVib plus both simulators
cargo build --release --locked

# Include the desktop GUI
cargo build --release --locked --features gui

# Verify the workspace
cargo test --workspace --locked
```

Outputs are:

```text
target/release/quickvib
target/release/m300-sim
target/release/m300-device-sim
```

To cross-compile a Windows mock/TCP build from Linux:

```bash
sudo apt-get install -y gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --locked --target x86_64-pc-windows-gnu
```

### Windows (`cmd.exe` / `.bat`)

```bat
:: Headless QuickVib plus both simulators
cargo build --release --locked

:: Include the desktop GUI
cargo build --release --locked --features gui
```

For the native M300 backend, use Windows x64 and enable the feature:

```bat
cargo build --release --locked --features m300
:: Use --features gui,m300 if the same binary also needs the desktop window.
```

The native backend also needs the vendor's `m300_sdk.dll` at runtime and its VC++ 2015–2022
redistributable. The SDK is not included in this repository. The native backend is implemented but
has not yet completed the real-instrument bench checklist.

## 3. Run scenarios

Commands below assume the repository root is the current directory.

### Scenario A: UTS rehearsal with no hardware (`mock`)

The mock backend is the default and is always connected.

```bat
target\release\quickvib.exe --headless --backend mock --bind 127.0.0.1 ^
  --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

```bash
./target/release/quickvib --headless --backend mock --bind 127.0.0.1 \
  --scpi-port 5025 --device-port 9123 --project samples/Test.proj
```

Point the UTS raw TCP client at `127.0.0.1:5025`. A quick Linux smoke query is:

```bash
printf '*IDN?\n' | nc 127.0.0.1 5025
```

### Scenario B: Exercise the inbound sample socket (`tcp`)

Start QuickVib first:

```bat
:: Terminal 1
target\release\quickvib.exe --headless --backend tcp --bind 127.0.0.1 ^
  --device-port 9123 --project samples\Test.proj

:: Terminal 2
target\release\m300-sim.exe --host 127.0.0.1 --port 9123 ^
  --rate 100000 --amplitude 250 --frequency 120
```

```bash
# Terminal 1
./target/release/quickvib --headless --backend tcp --bind 127.0.0.1 \
  --device-port 9123 --project samples/Test.proj

# Terminal 2
./target/release/m300-sim --host 127.0.0.1 --port 9123 \
  --rate 100000 --amplitude 250 --frequency 120
```

Then run the same SCPI workflow on port `5025`. Keep simulator `--rate` equal to
`device.sampleRateHz`.

### Scenario C: Native SDK with a real M300 (`m300`)

Windows only:

```bat
set QUICKVIB_M300_SDK=C:\M300SDK_v1.2.0\x64\bin
target\release\quickvib.exe --headless --backend m300 --bind 0.0.0.0 ^
  --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

Configure the M300 to dial the host's reachable IP on port `9123`. Allow inbound TCP `5025` and
`9123` in the host firewall. With this backend the vendor SDK, not QuickVib's device server, binds
port `9123`.

### Scenario D: Native SDK protocol rehearsal (`m300` + SCZN simulator)

This still requires Windows, an `m300` build, and the real vendor DLL:

```bat
:: Terminal 1: the SDK owns port 9123
target\release\quickvib.exe --headless --backend m300 --bind 127.0.0.1 ^
  --device-port 9123 --project samples\Test.proj

:: Terminal 2: simulated SCZN device
target\release\m300-device-sim.exe --host 127.0.0.1 --port 9123 ^
  --rate 100000 --amplitude 250 --frequency 120
```

### Process behavior and exit codes

QuickVib stays in the foreground. A UTS normally launches it as a child process, waits until the
SCPI port is accepting connections, and terminates it after sending `ABOR` and closing the socket.

| Exit | Meaning | First action |
| --- | --- | --- |
| `0` | Clean exit, `--help`, or `--version` | None |
| `2` | Bad arguments, or `m300` unavailable on this host/build | Check stderr and `--help` |
| `3` | SCPI/device listener bind failure | Check host address and ports |
| `4` | Explicit `--project` load/validation failure | Check path and JSON error |
| `5` | Selected backend failed to open | Check backend-specific log details |

## 4. GUI operator workflow

Build with `--features gui`, then omit `--headless`:

```bat
target\release\quickvib.exe --project samples\Test.proj
```

```bash
./target/release/quickvib --project samples/Test.proj
```

The GUI opens in Simplified Chinese with a dark theme. Use the title-bar **中文 / EN** and
**浅色 / 深色 · Light / Dark** switches as needed.

Recommended operator sequence:

1. Use **打开 / Open** to load a `.proj` file.
2. Check the data source, sample rate, unit, filters, range, duration, and export directory.
3. Read **实际监听端口 / Live listening ports** for sockets QuickVib itself bound. With `m300`, the
   SDK owns the device socket, so use the project device port and startup log when that live field
   is a dash. Editable project ports are next-start values and may also differ because CLI options
   take precedence.
4. Select **应用 / Apply**. If validation fails, correct every listed field and apply again.
5. Select **开始录制 / Start recording**. Use **停止 / Stop** to abort.
6. Wait for **完成 / Complete**, review peak/RMS/peak-to-peak, then export the capture.
7. Save the project if the configuration should be reused.

Ports, backend, sample rate, and data type are saved by Apply but require a process restart. Applying
or loading a project invalidates the previous capture, so export it first. The Advanced tab is
currently a placeholder; it does not control laser power, TEC, PID, or external triggering.

A build without `--features gui`, a run with `--headless`, or a host without a usable display will
serve from the console instead of opening a window.

## 5. UTS workflow

### Startup

Use explicit arguments so the test cell does not depend on the last-project record:

```bat
start "" /b quickvib.exe --headless --no-auto-load --backend mock ^
  --bind 127.0.0.1 --scpi-port 5025 --device-port 9123 ^
  --project C:\QuickVib\samples\Test.proj
```

```bash
./quickvib --headless --no-auto-load --backend mock \
  --bind 127.0.0.1 --scpi-port 5025 --device-port 9123 \
  --project /opt/quickvib/samples/Test.proj
```

Use `0.0.0.0` or a specific network interface instead of `127.0.0.1` when either peer is on another
machine.

### Typical session

Open one persistent raw TCP socket to port `5025`. Send ASCII commands terminated by `\n`; `\r\n`
is also accepted. Only queries produce response lines.

```text
> *CLS
> *IDN?
< QuickVib,M300-SCPI,SN-0001,1.0.0
> SYST:DEV:CONN?
< 1
> CONF:REC:DUR 5.0
> FORM CSV
> INIT
> REC:WAIT?
< #REC:DONE
< 1
> REC:STAT?
< COMPLETE
> TRAC:POIN?
< 500000
> CALC:MEAS:ALL?
< 252.4913,178.5402,504.9826
> MMEM:STOR:TRAC "run001.csv"
> SYST:ERR?
< 0,"No error"
```

Integration rules:

- `INIT` is non-blocking and returns no line. Do not wait for a command response.
- Lines beginning with `#` are unsolicited notifications, not query responses. A client waiting on
  `REC:WAIT?` may receive `#REC:DONE` before the response `1`, or `#REC:ABORT` before `0`.
- Use `REC:WAIT?` or poll `REC:STAT?`; both are bounded by the recording watchdog.
- Drain `SYST:ERR?` until `0,"No error"` after a failed operation. Unknown commands normally return
  no line and queue `-113,"Undefined header"`.
- Call `TRAC:POIN?` before `FETC?`. `FETC?` is one potentially multi-megabyte ASCII line; use
  `CALC:MEAS:ALL?` when only scalars are needed.
- Fetch or export before loading/applying another project, because project adoption discards the
  stored capture.
- QuickVib reports measurements; the UTS applies limits, decides pass/fail, and owns retry policy.

## 6. Backends

The CLI `--backend` overrides `device.backend` in the project.

| Backend | Device data source | Port `9123` owner | Requirements | Intended use |
| --- | --- | --- | --- | --- |
| `mock` | Deterministic in-process signal | QuickVib binds it, but capture uses the mock | None | UTS and operator rehearsal |
| `tcp` | Bare little-endian `f32` stream | QuickVib | A device or `m300-sim` dials in | Exercise listener/framer and end-to-end recording |
| `m300` | Vendor SDK callback | Vendor SDK | Windows x64, `--features m300`, `m300_sdk.dll` | Real instrument or SDK/SCZN rehearsal |

The native SDK is searched in this order: project `device.sdkPath`, `QUICKVIB_M300_SDK`, the
QuickVib executable directory, then the normal DLL search path. `device.allowedPeers` is enforced by
QuickVib for `mock`/`tcp`, but not for `m300`; use a firewall rule for the native path.

Never allow a requested hardware backend to fall back silently to `mock`. QuickVib deliberately
rejects an unavailable `m300` selection.

## 7. Simulators

The simulators are TCP clients and are not interchangeable:

| Simulator | Pair with | Wire format | Starts sending |
| --- | --- | --- | --- |
| `m300-sim` | `--backend tcp` | Bare LE `f32` samples | Immediately after connecting |
| `m300-device-sim` | `--backend m300` | Framed SCZN protocol | After the SDK sends start (`0x00`) |

`m300-sim` needs no SDK and is the normal cross-platform transport test. It exits `0` when its run
ends, `2` for bad arguments, and `3` if it cannot connect.

`m300-device-sim` simulates the device side of the vendor protocol. The simulator itself is
cross-platform, but a real QuickVib `m300` peer requires Windows and the vendor DLL. It redials by
default; use `--once` to exit after a dropped session. It exits `0` after at least one session, `2`
for bad arguments, and `3` if it ends without ever connecting.

If the native SDK reports a CRC mismatch during rehearsal, try:

```bat
m300-device-sim.exe --host 127.0.0.1 --port 9123 --crc castagnoli
m300-device-sim.exe --host 127.0.0.1 --port 9123 --crc zero
```

The CRC variants exist because the vendor specification contains conflicting examples.

## 8. Project file basics

A project is versioned JSON stored in a `.proj` file. Start from `samples/Test.proj`.

```json
{
  "schemaVersion": 1,
  "name": "Test",
  "device": {
    "backend": "mock",
    "port": 9123,
    "sampleRateHz": 100000,
    "unit": "velocity_um_s"
  },
  "recording": {
    "durationSeconds": 5.0,
    "timeoutMultiplier": 2.0
  },
  "measurement": {
    "removeDc": false,
    "responseDecimals": 4
  },
  "export": {
    "format": "CSV",
    "directory": "./out",
    "includeHeader": true
  },
  "server": {
    "scpiPort": 5025
  }
}
```

Operator/integrator essentials:

- `device.backend`: `mock`, `tcp`, or `m300`.
- `device.unit`: `velocity_um_s`, `displacement_um`, or `acceleration_m_s2`.
- `durationSeconds × sampleRateHz × 4` must fit `recording.maxCaptureBytes` (512 MiB by default).
- `server.scpiPort` and `device.port` must differ.
- CLI `--backend`, `--scpi-port`, `--device-port`, and `--bind` override project values.
- Relative export paths are based on `export.directory`; missing directories are created.
- Unknown JSON properties are ignored for forward compatibility; invalid known values reject the
  project.
- Without `--project` or `--no-auto-load`, QuickVib may load the most recently used project.

## 9. Troubleshooting quick table

| Symptom | Check / action |
| --- | --- |
| Exit `2` | Fix CLI syntax; `m300` also requires Windows and an `m300`-feature build |
| Exit `3` | Another process may own `5025`/`9123`; verify `--bind`, then choose free ports |
| Exit `4` | Verify the explicit project path and the JSON line/column in the log |
| Exit `5` | Read the backend-open log; for `m300`, check DLL path, SDK version, runtime, and port ownership |
| No GUI | Rebuild with `--features gui`, remove `--headless`, and use a desktop session |
| GUI edit has no effect | Select Apply; restart for ports, backend, sample rate, or data type |
| UTS cannot connect | Use the GUI's live listening port or startup log, not only the project value |
| `SYST:DEV:CONN?` is `0` | For `tcp`/`m300`, start or power the device and check IP, port `9123`, firewall, and peer rules |
| `INIT` queues `-241` | No device is connected; check device power, address, firewall, and the inbound link |
| Simulator connects but no data arrives | Pair `m300-sim` with `tcp`, and `m300-device-sim` with `m300` |
| `tcp` capture times out | Match simulator `--rate` to `device.sampleRateHz`; inspect the link and watchdog |
| Native open reports `-7 ERR_NETWORK` | Port `9123` is already occupied; stop the other QuickVib/TCP listener |
| `FETC?` is truncated | Increase the UTS line/read buffer after `TRAC:POIN?`, or export to a file |
| A command appears ignored | Query `SYST:ERR?`; commands and unknown headers do not return response lines |
| Measurement query returns `-230` | Complete a capture first; aborted or invalidated captures are not readable |

When escalating a problem, include the complete `--headless` log, the launch command, backend,
project file, and all `SYST:ERR?` entries through `0,"No error"`.

## 10. Related documentation

- [`docs/SCPI.md`](SCPI.md) — complete command, response, notification, and SCPI error reference
- [`docs/M300-NATIVE.md`](M300-NATIVE.md) — SDK deployment, ABI, native backend, and bench checklist
- [`docs/SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md) — protocol used by `m300-device-sim`
- [`docs/PLAN.md`](PLAN.md) — architecture, decisions, scope, and implementation plan

---

# QuickVib 软件使用手册（简体中文）

**[English](#quickvib-software-usage-manual) | 简体中文**

本手册面向测试工位操作员与 UTS 集成人员，重点说明如何启动 QuickVib 并完成一次测量。协议细节与设计依据见
文末链接。

## 1. 概述

QuickVib 将 M300 激光多普勒测振仪封装成一台通过裸 TCP socket 通信的 SCPI 仪器。它负责单通道定时采集、
计算峰值 / RMS / 峰峰值，并导出 CSV 或 TXT。DUT 激励、合格判定和重试策略仍由 UTS 负责。

系统有两条互相独立的入站 TCP 链路：

| 链路 | QuickVib 角色 | 默认端口 | 主动连接方 |
| --- | --- | --- | --- |
| SCPI | 服务端 | `5025` | UTS |
| 设备 | 由 QuickVib 或厂商 SDK 提供服务端 | `9123` | M300 或模拟器 |

QuickVib 不主动连接任何一端。UTS 连入 `5025`；设备连入 `9123`。

| 模式 | 启动方式 | 用途 |
| --- | --- | --- |
| 无头模式 | 加 `--headless` | UTS / 自动化；无窗口，只输出结构化日志 |
| GUI | 使用 `--features gui` 构建，并且不传 `--headless` | 操作员配置与手动采集 |

## 2. 编译

项目使用 `rust-toolchain.toml` 中固定的稳定版 Rust 工具链。

### Linux

```bash
# 无头 QuickVib 与两个模拟器
cargo build --release --locked

# 加入桌面 GUI
cargo build --release --locked --features gui

# 验证整个 workspace
cargo test --workspace --locked
```

输出文件：

```text
target/release/quickvib
target/release/m300-sim
target/release/m300-device-sim
```

在 Linux 上交叉编译 Windows 的 mock/TCP 版本：

```bash
sudo apt-get install -y gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --locked --target x86_64-pc-windows-gnu
```

### Windows（`cmd.exe` / `.bat`）

```bat
:: 无头 QuickVib 与两个模拟器
cargo build --release --locked

:: 加入桌面 GUI
cargo build --release --locked --features gui
```

原生 M300 后端必须在 Windows x64 上启用对应 feature：

```bat
cargo build --release --locked --features m300
:: 同一程序还需要桌面窗口时，使用 --features gui,m300
```

原生后端运行时还需要厂商 `m300_sdk.dll` 及其 VC++ 2015–2022 运行库。本仓库不附带 SDK。原生后端代码已经
实现，但尚未完成真实仪器上机检查清单。

## 3. 运行场景

以下命令均假定当前目录为仓库根目录。

### 场景 A：无硬件演练 UTS（`mock`）

mock 是默认后端，并始终报告设备已连接。

```bat
target\release\quickvib.exe --headless --backend mock --bind 127.0.0.1 ^
  --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

```bash
./target/release/quickvib --headless --backend mock --bind 127.0.0.1 \
  --scpi-port 5025 --device-port 9123 --project samples/Test.proj
```

让 UTS 的裸 TCP 客户端连接 `127.0.0.1:5025`。Linux 下可快速检查：

```bash
printf '*IDN?\n' | nc 127.0.0.1 5025
```

### 场景 B：演练设备入站数据 socket（`tcp`）

必须先启动 QuickVib：

```bat
:: 终端 1
target\release\quickvib.exe --headless --backend tcp --bind 127.0.0.1 ^
  --device-port 9123 --project samples\Test.proj

:: 终端 2
target\release\m300-sim.exe --host 127.0.0.1 --port 9123 ^
  --rate 100000 --amplitude 250 --frequency 120
```

```bash
# 终端 1
./target/release/quickvib --headless --backend tcp --bind 127.0.0.1 \
  --device-port 9123 --project samples/Test.proj

# 终端 2
./target/release/m300-sim --host 127.0.0.1 --port 9123 \
  --rate 100000 --amplitude 250 --frequency 120
```

随后仍通过 `5025` 执行同一套 SCPI 流程。模拟器的 `--rate` 应与 `device.sampleRateHz` 一致。

### 场景 C：使用真实 M300 的原生 SDK（`m300`）

仅限 Windows：

```bat
set QUICKVIB_M300_SDK=C:\M300SDK_v1.2.0\x64\bin
target\release\quickvib.exe --headless --backend m300 --bind 0.0.0.0 ^
  --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

把 M300 配置为主动连接主机可达 IP 的 `9123` 端口，并在防火墙中放行入站 TCP `5025` 与 `9123`。该后端下
由厂商 SDK 而不是 QuickVib 的设备服务绑定 `9123`。

### 场景 D：原生 SDK 协议演练（`m300` + SCZN 模拟器）

这条路径仍需要 Windows、启用了 `m300` 的构建以及真实厂商 DLL：

```bat
:: 终端 1：SDK 持有 9123
target\release\quickvib.exe --headless --backend m300 --bind 127.0.0.1 ^
  --device-port 9123 --project samples\Test.proj

:: 终端 2：模拟 SCZN 设备
target\release\m300-device-sim.exe --host 127.0.0.1 --port 9123 ^
  --rate 100000 --amplitude 250 --frequency 120
```

### 进程行为与退出码

QuickVib 一直在前台运行。UTS 通常把它作为子进程启动，等待 SCPI 端口可连接；结束时先发送 `ABOR`、关闭
socket，再终止进程。

| 退出码 | 含义 | 首要处理 |
| --- | --- | --- |
| `0` | 正常退出，或执行 `--help` / `--version` | 无 |
| `2` | 参数错误，或当前主机/构建不支持 `m300` | 检查 stderr 与 `--help` |
| `3` | SCPI / 设备监听绑定失败 | 检查主机地址与端口 |
| `4` | 显式指定的 `--project` 加载或校验失败 | 检查路径与 JSON 错误 |
| `5` | 所选后端打开失败 | 查看对应后端的详细日志 |

## 4. GUI 操作流程

使用 `--features gui` 构建，然后不传 `--headless`：

```bat
target\release\quickvib.exe --project samples\Test.proj
```

```bash
./target/release/quickvib --project samples/Test.proj
```

GUI 默认以简体中文和深色主题打开。可使用标题栏中的 **中文 / EN** 与
**浅色 / 深色 · Light / Dark** 开关。

建议操作顺序：

1. 用**打开 / Open**加载 `.proj` 文件。
2. 检查数据来源、采样率、单位、滤波器、量程、录制时长与导出目录。
3. 查看**实际监听端口 / Live listening ports**，其中列出 QuickVib 自己绑定的 socket。`m300` 下设备
   socket 属于 SDK；若实际设备端口显示短横线，应以项目设备端口与启动日志为准。可编辑的项目端口是下次
   启动值；命令行参数优先，因此两者也可能不同。
4. 点击**应用 / Apply**。若校验失败，修正列出的所有字段后重新应用。
5. 点击**开始录制 / Start recording**；需要中止时点击**停止 / Stop**。
6. 等待状态变为**完成 / Complete**，检查峰值 / RMS / 峰峰值，然后导出。
7. 若配置需要复用，保存工程文件。

点击应用会保存端口、后端、采样率与数据类型，但这些字段需要重启进程才生效。应用或加载工程会使上一次采集
失效，因此应先导出。高级标签页目前只是占位，不控制激光功率、TEC、PID 或外部触发。

如果构建时未启用 `gui`、启动时传了 `--headless`，或主机没有可用显示环境，程序会从控制台提供服务而不
打开窗口。

## 5. UTS 工作流

### 启动

显式给出参数，避免测试工位依赖“上次工程”记录：

```bat
start "" /b quickvib.exe --headless --no-auto-load --backend mock ^
  --bind 127.0.0.1 --scpi-port 5025 --device-port 9123 ^
  --project C:\QuickVib\samples\Test.proj
```

```bash
./quickvib --headless --no-auto-load --backend mock \
  --bind 127.0.0.1 --scpi-port 5025 --device-port 9123 \
  --project /opt/quickvib/samples/Test.proj
```

若任一对端位于另一台机器，请把 `127.0.0.1` 改为 `0.0.0.0` 或指定可用网卡地址。

### 典型会话

与 `5025` 建立一个持久的裸 TCP socket。命令使用 ASCII，以 `\n` 结束，也接受 `\r\n`。只有查询命令才返回
响应行。

```text
> *CLS
> *IDN?
< QuickVib,M300-SCPI,SN-0001,1.0.0
> SYST:DEV:CONN?
< 1
> CONF:REC:DUR 5.0
> FORM CSV
> INIT
> REC:WAIT?
< #REC:DONE
< 1
> REC:STAT?
< COMPLETE
> TRAC:POIN?
< 500000
> CALC:MEAS:ALL?
< 252.4913,178.5402,504.9826
> MMEM:STOR:TRAC "run001.csv"
> SYST:ERR?
< 0,"No error"
```

集成规则：

- `INIT` 是无响应的非阻塞命令，不要等待命令响应。
- 以 `#` 开头的是主动通知，不是查询响应。等待 `REC:WAIT?` 时，客户端可能先收到 `#REC:DONE` 再收到
  `1`，或先收到 `#REC:ABORT` 再收到 `0`。
- 使用 `REC:WAIT?`，或轮询 `REC:STAT?`；两者都受录制看门狗约束。
- 操作失败后反复读取 `SYST:ERR?`，直到得到 `0,"No error"`。未知命令通常不返回任何行，而是把
  `-113,"Undefined header"` 放入错误队列。
- `FETC?` 前先查询 `TRAC:POIN?`。`FETC?` 是一条可能长达数 MB 的 ASCII 行；只需要标量时使用
  `CALC:MEAS:ALL?`。
- 加载或应用其他工程前先读取或导出数据，因为采用新工程会丢弃已保存的采集。
- QuickVib 只报告测量结果；限值、合格判定与重试由 UTS 负责。

## 6. 后端

命令行 `--backend` 优先于工程中的 `device.backend`。

| 后端 | 设备数据来源 | `9123` 持有者 | 前置条件 | 主要用途 |
| --- | --- | --- | --- | --- |
| `mock` | 进程内确定性信号 | QuickVib 绑定，但采集使用 mock | 无 | UTS 与操作员演练 |
| `tcp` | 裸小端 `f32` 数据流 | QuickVib | 设备或 `m300-sim` 主动连入 | 演练监听器、framer 与端到端采集 |
| `m300` | 厂商 SDK 回调 | 厂商 SDK | Windows x64、`--features m300`、`m300_sdk.dll` | 真实仪器或 SDK/SCZN 演练 |

原生 SDK 按以下顺序查找：工程中的 `device.sdkPath`、`QUICKVIB_M300_SDK`、QuickVib 可执行文件所在目录、
系统默认 DLL 搜索路径。`device.allowedPeers` 由 QuickVib 在 `mock` / `tcp` 路径上执行，但不适用于
`m300`；原生路径应使用防火墙规则限制来源。

请求硬件后端时绝不能静默回退到 `mock`。QuickVib 会主动拒绝当前环境不支持的 `m300` 选择。

## 7. 模拟器

两个模拟器都是 TCP 客户端，不能互换：

| 模拟器 | 配套后端 | 线上格式 | 开始发送的时机 |
| --- | --- | --- | --- |
| `m300-sim` | `--backend tcp` | 裸小端 `f32` 采样 | 建立连接后立即发送 |
| `m300-device-sim` | `--backend m300` | 分帧 SCZN 协议 | SDK 下发启动命令（`0x00`）之后 |

`m300-sim` 不需要 SDK，是常用的跨平台传输链路测试工具。退出码：`0` 表示运行结束，`2` 表示参数错误，
`3` 表示无法连接。

`m300-device-sim` 模拟厂商协议的设备端。模拟器自身可跨平台运行，但若对端是真正的 QuickVib `m300`
后端，则仍需要 Windows 与厂商 DLL。默认会自动重连；使用 `--once` 可在会话断开后退出。退出码：`0` 表示
至少建立过一次会话，`2` 表示参数错误，`3` 表示结束前从未连接成功。

若原生 SDK 在演练中报告 CRC 不匹配，可尝试：

```bat
m300-device-sim.exe --host 127.0.0.1 --port 9123 --crc castagnoli
m300-device-sim.exe --host 127.0.0.1 --port 9123 --crc zero
```

之所以提供多种 CRC，是因为厂商规范中的示例互相矛盾。

## 8. 工程文件基础

工程是带版本号的 JSON `.proj` 文件。建议从 `samples/Test.proj` 开始修改。

```json
{
  "schemaVersion": 1,
  "name": "Test",
  "device": {
    "backend": "mock",
    "port": 9123,
    "sampleRateHz": 100000,
    "unit": "velocity_um_s"
  },
  "recording": {
    "durationSeconds": 5.0,
    "timeoutMultiplier": 2.0
  },
  "measurement": {
    "removeDc": false,
    "responseDecimals": 4
  },
  "export": {
    "format": "CSV",
    "directory": "./out",
    "includeHeader": true
  },
  "server": {
    "scpiPort": 5025
  }
}
```

操作员与集成人员需要掌握：

- `device.backend`：`mock`、`tcp` 或 `m300`。
- `device.unit`：`velocity_um_s`、`displacement_um` 或 `acceleration_m_s2`。
- `durationSeconds × sampleRateHz × 4` 必须小于 `recording.maxCaptureBytes`（默认 512 MiB）。
- `server.scpiPort` 与 `device.port` 不能相同。
- 命令行 `--backend`、`--scpi-port`、`--device-port` 与 `--bind` 覆盖工程值。
- 相对导出路径以 `export.directory` 为基准；不存在的目录会自动创建。
- 为向前兼容，未知 JSON 字段会被忽略；已知字段取值非法则拒绝整个工程。
- 未指定 `--project` 或 `--no-auto-load` 时，QuickVib 可能自动加载上次使用的工程。

## 9. 快速故障排查

| 现象 | 检查 / 处理 |
| --- | --- |
| 退出码 `2` | 修正命令行；`m300` 还要求 Windows 与启用了 `m300` 的构建 |
| 退出码 `3` | 可能已有进程占用 `5025` / `9123`；检查 `--bind` 并改用空闲端口 |
| 退出码 `4` | 检查显式工程路径，以及日志中的 JSON 行号/列号 |
| 退出码 `5` | 查看后端打开日志；`m300` 下检查 DLL 路径、SDK 版本、运行库和端口占用 |
| 没有 GUI | 使用 `--features gui` 重新编译，移除 `--headless`，并确认有桌面环境 |
| GUI 修改未生效 | 点击应用；端口、后端、采样率、数据类型需要重启 |
| UTS 无法连接 | 以 GUI 的实际监听端口或启动日志为准，不要只看工程值 |
| `SYST:DEV:CONN?` 为 `0` | `tcp` / `m300` 下启动设备并检查 IP、`9123`、防火墙和来源限制 |
| `INIT` 产生 `-241` | 没有设备连接；检查设备电源、地址、防火墙与入站链路 |
| 模拟器已连接但无数据 | `m300-sim` 必须配 `tcp`；`m300-device-sim` 必须配 `m300` |
| `tcp` 采集超时 | 让模拟器 `--rate` 与 `device.sampleRateHz` 一致，并检查链路与看门狗 |
| 原生打开报告 `-7 ERR_NETWORK` | `9123` 已被占用；停止另一个 QuickVib 或 TCP 监听器 |
| `FETC?` 被截断 | 先读 `TRAC:POIN?` 再增大 UTS 行/读取缓冲，或改用文件导出 |
| 命令似乎被忽略 | 查询 `SYST:ERR?`；普通命令和未知 header 都不返回响应行 |
| 测量查询返回 `-230` | 先完成采集；中止或被工程切换失效的采集不可读取 |

上报问题时请附上完整 `--headless` 日志、启动命令、后端、工程文件，以及直到
`0,"No error"` 为止的全部 `SYST:ERR?` 结果。

## 10. 相关文档

- [`docs/SCPI.md`](SCPI.md) — 完整命令、响应、通知与 SCPI 错误参考
- [`docs/M300-NATIVE.md`](M300-NATIVE.md) — SDK 部署、ABI、原生后端与上机检查清单
- [`docs/SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md) — `m300-device-sim` 使用的协议
- [`docs/PLAN.md`](PLAN.md) — 架构、决策、范围与实现计划
