# QuickVib 软件使用说明 · Usage Manual

**中文 | [English](#quickvib-usage-manual-english)**

本文是 QuickVib 的**操作手册**：面向在产线／试验间里真正要把它跑起来的人——编写 UTS 脚本的测试工程
师，和坐在工位前用窗口配置工程的操作员。它按「要做什么」组织，一步一步给出可以照抄的命令。

如果你要找的是**逐条命令的权威定义**，请看：

| 文档 | 内容 |
| --- | --- |
| [`../README.md`](../README.md) | 项目总览与完整参考 |
| [`SCPI.md`](SCPI.md) | SCPI 命令与错误码速查表 |
| [`SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md) | 设备侧 SCZN 传输协议 |
| [`M300-NATIVE.md`](M300-NATIVE.md) | 厂商 SDK 的原生 ABI 契约 |
| [`PLAN.md`](PLAN.md) | 规范性设计文档 |

> 本文中的所有命令行、响应报文与退出码，都是在本仓库 `1.0.0` 版本上**实际执行后记录**的，而不是照着
> 设计文档抄写的。涉及真实 M300 硬件的部分（`--backend m300`）目前仍**未经上机验证**，文中会逐处标出。

## 目录

1. [QuickVib 是什么](#1-quickvib-是什么)
2. [安装与编译](#2-安装与编译)
3. [三种运行模式](#3-三种运行模式)
4. [启动方式](#4-启动方式)
5. [操作员界面使用指南](#5-操作员界面使用指南)
6. [UTS 的 SCPI 作业流程](#6-uts-的-scpi-作业流程)
7. [工程文件](#7-工程文件)
8. [两个模拟器](#8-两个模拟器)
9. [常见故障排查](#9-常见故障排查)
10. [功能边界](#10-功能边界)

---

## 1. QuickVib 是什么

QuickVib 把一台 **M300 激光多普勒测振仪**包装成一台**类 Keysight 的 SCPI 仪器**。它是单个自包含的可
执行程序，对外提供两条互不相干的 TCP 链路：

```
   UTS  ──SCPI 文本命令──▶  :5025  ┌──────────────────────────┐
                                   │        quickvib          │
                                   │  SCPI 解析 → 仪器引擎     │
                                   │  → 采集流水线            │──▶  *.csv / *.txt
                                   │  → 测量（峰值/RMS/峰峰值）│
   M300 ──采样数据流──────▶  :9123  └──────────────────────────┘
```

**两条链路上 QuickVib 都是服务端，从不主动外连。** 这是最容易搞错的一点：不是 QuickVib 去连测振仪，
而是测振仪主动拨入 QuickVib 监听的设备端口。

### 1.1 两类使用者

同一个可执行文件服务于两类完全不同的人，请先确认自己属于哪一类：

| | **UTS（自动化）** | **操作员（手动）** |
| --- | --- | --- |
| 怎么启动 | 由 UTS 作为子进程拉起，带 `--headless` | 双击或在命令行直接运行，不带 `--headless` |
| 界面 | 没有窗口，stdout 上是单行结构化日志 | 简体中文桌面窗口 |
| 怎么操作 | 通过 5025 端口发送 SCPI 文本命令 | 点按钮：打开 / 应用 / 开始录制 / 导出数据 |
| 编译要求 | `cargo build --release` | `cargo build --release --features gui` |
| 典型场景 | 产线节拍测试、回归测试 | 首次配置工程、现场排查、演示 |
| 详见 | [§6](#6-uts-的-scpi-作业流程) | [§5](#5-操作员界面使用指南) |

两者共用同一份工程文件和同一个仪器引擎。操作员在窗口里点「应用」改掉的录制时长，紧接着从 5025 端口
发来的 `CONF:REC:DUR?` 就能读到——它们操作的是同一个运行中的引擎实例。

### 1.2 QuickVib 负责什么

只有三件事：**采集、测量、导出**。

一次典型作业是：加载工程 → `INIT` 开始采集 → 收满 `时长 × 采样率` 个采样点 → 状态变为 `COMPLETE` →
读取峰值 / 有效值 / 峰峰值 → 可选地把整段波形写成 CSV 或 TXT 文件。

合格判定、DUT 振动激励、失败重试都**不在** QuickVib 之内——见 [§10](#10-功能边界)。

---

## 2. 安装与编译

QuickVib 不提供安装包，从源码编译，产出的是可直接拷贝的独立可执行文件。

### 2.1 前置条件

| 用途 | 需要什么 |
| --- | --- |
| 编译（任何模式） | Rust stable 工具链，edition 2021。具体版本固定在 `rust-toolchain.toml`（当前为 **1.83**），`rustup` 会自动按它切换 |
| 运行（mock / tcp 模式） | 什么都不需要。可执行文件静态链接，不依赖运行库 |
| 运行桌面窗口 | 以 `--features gui` 编译，且有可用的桌面会话。Linux 上需要 X11 或 Wayland 加 `libxkbcommon`（桌面发行版自带）。**无需安装中文字体**，程序已内嵌一份 |
| 运行真实 M300 | Windows x64 + `--features m300` + 厂商 `m300_sdk.dll` + VC++ 运行库，见 [§2.5](#25-真实-m300-的额外前置条件) |
| 在 Linux 上交叉编译 Windows 程序 | `gcc-mingw-w64-x86-64` 与 `x86_64-pc-windows-gnu` 目标 |

### 2.2 编译

```bash
cargo build --release
```

产出三个可执行文件，都在 `target/release/` 下：

| 文件 | 作用 |
| --- | --- |
| `quickvib` | 主程序：SCPI 仪器 |
| `m300-sim` | 模拟器，配合 `--backend tcp` 使用（[§8.1](#81-m300-sim配合---backend-tcp)） |
| `m300-device-sim` | 模拟器，配合 `--backend m300` 使用（[§8.2](#82-m300-device-sim配合---backend-m300)） |

验证编译结果：

```bash
./target/release/quickvib --version
# quickvib 1.0.0
```

`Cargo.lock` 已入库，所以交付构建建议加上 `--locked`（例如 `cargo build --release --locked`）：它要求
完全按锁文件里的版本编译，任何会改动锁文件的情况都直接报错，从而保证两次构建拿到的依赖一模一样。

### 2.3 两个可选 feature

窗口和 M300 原生后端都是**编译期开关**，默认关闭。交付给 UTS 的二进制文件因此不带任何图形依赖，也不
带任何 Windows 专有代码。

| 命令 | 得到什么 |
| --- | --- |
| `cargo build --release` | 只有命令行 / SCPI 的仪器。**UTS 用这个** |
| `cargo build --release --features gui` | 上述功能 + 简体中文桌面窗口。**操作员用这个** |
| `cargo build --release --features m300` | 上述功能 + 真实 M300 原生后端（仅 Windows） |
| `cargo build --release --features gui,m300` | 全部功能，产线工位的完全体（仅 Windows） |

未启用 `--features gui` 编译出的程序**没有窗口可开**：不带 `--headless` 运行它，只会打印启动横幅并
在控制台照常提供服务，而不是报错退出。启用了该 feature、但运行在没有显示环境的机器上时同理——程序会
打印一行说明并回退到控制台。

`--features m300` 在非 Windows 主机上可以编译（该 crate 大部分代码与平台无关），但**选择**这个后端会
被拒绝，见 [§3](#3-三种运行模式)。

### 2.4 生成 Windows 可执行文件

**路线 A —— 在 Windows 上原生 MSVC 编译（交付用的就是这个）：**

```bat
rustup target add x86_64-pc-windows-msvc
set RUSTFLAGS=-C target-feature=+crt-static
cargo build --release --target x86_64-pc-windows-msvc --locked
:: 产物 -> target\x86_64-pc-windows-msvc\release\quickvib.exe
```

`+crt-static` 把 MSVC 的 C 运行库静态链接进去，因此**测试机不需要装 Visual C++ 运行库**——前提是不使
用 `--features m300`，厂商 DLL 有它自己的运行库依赖（见下节）。

**路线 B —— 在 Linux 上用 mingw-w64 交叉编译：**

```bash
sudo apt-get install -y gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu --locked
# 产物 -> target/x86_64-pc-windows-gnu/release/quickvib.exe
```

之所以行得通，是因为整个依赖图都是纯 Rust，没有任何 C 构建脚本。产物在 Wine 下也能跑通 mock 路径。
这是给开发和 CI 用的便利目标；**正式交付的是路线 A 的 MSVC 构建**。

### 2.5 真实 M300 的额外前置条件

只有走 `--backend m300` 时才需要这一节。三个条件必须同时满足：

1. **Windows x64 主机**
2. **用 `--features m300` 编译的构建**
3. **厂商 `m300_sdk.dll`**（SDK v1.2.0 包 `M300SDK_v1.2.0.zip` 中的 `x64\bin\m300_sdk.dll`）

SDK **不随本仓库分发**，请向仪器供货方索取。仓库中不存在任何假的或桩实现的 DLL，将来也不会加入。

**DLL 的查找顺序**（进程启动时不解析，第一次真正用到时才加载）：

| 顺序 | 位置 |
| --- | --- |
| 1 | 工程文件中的 `device.sdkPath` 指定的目录 |
| 2 | 环境变量 `QUICKVIB_M300_SDK` 指定的目录 |
| 3 | `quickvib.exe` 所在目录 |
| 4 | 系统默认 DLL 搜索路径 |

最省事的做法是**把 `m300_sdk.dll` 和 `quickvib.exe` 放在同一个目录里**（第 3 条）。

**VC++ 运行库。** 厂商的 DLL 是用 MSVC 编译的，导入 `VCRUNTIME140.dll` 与 UCRT，因此目标机必须安装
**Visual C++ 2015–2022 可再发行组件包（x64）**。QuickVib 自身的 `+crt-static` 静态链接管不到厂商
DLL——这是两件事。若缺少运行库，DLL 会加载失败，表现为 `-241,"Hardware missing"`，日志里会逐条列出
尝试过的每个路径和对应的操作系统错误。

> **上机状态。** M300 原生后端代码已经写完，但**尚未在真机上验证**。上机检查清单见
> [`M300-NATIVE.md`](M300-NATIVE.md) §9。在那次记录完成之前，请把 `--backend m300` 当作未经验证的
> 路径看待。

---

## 3. 三种运行模式

这是使用 QuickVib 时**第一个要做的决定**：数据从哪里来。三种后端对「设备」的定义完全不同，因此各自
需要不同的替身——或者根本不需要。

| 模式 | 启动参数 | 需要的设备端替身 | 线上格式 | 什么时候用 |
| --- | --- | --- | --- | --- |
| **模拟** | `--backend mock` | 无 | 采样不出进程 | 开发、CI、脚本调试、演示 |
| **TCP 直连** | `--backend tcp` | `m300-sim`，或真实 M300 | 裸小端 `f32`，无任何分帧 | 没有 SDK 时演练真实入站链路 |
| **M300 SDK** | `--backend m300` | `m300-device-sim`（SCZN），或真实 M300 | SCZN 分帧协议 | 产线工位、真机测试 |

选择方式：命令行 `--backend <mock|tcp|m300>`，或工程文件里的 `"device": { "backend": "…" }`。命令行
优先。

### 3.1 模拟模式（`--backend mock`）

**默认模式，也是唯一不需要任何外部东西的模式。** 它在进程内按工程配置的正弦分量合成确定性信号，可叠
加带种子的高斯噪声。因为合成正弦的峰值 / RMS / 峰峰值有解析解，它同时充当测量算法的「标准答案」。

`SYST:DEV:CONN?` 在这个模式下**恒为 `1`**——模拟后端不需要链路。

适合：UTS 脚本的初次联调、CI、给不带硬件的机器做演示。

### 3.2 TCP 直连模式（`--backend tcp`）

QuickVib 自己 bind `--device-port`，记录**任何连入者**推送来的小端 `f32` 数据流。真实 M300 推送的正
是这个格式，因此这个模式跑通就意味着 QuickVib 的监听器、分帧器、采集流水线全都走过一遍真实字节。

没有真机时用 `m300-sim` 作为替身（[§8.1](#81-m300-sim配合---backend-tcp)）。

适合：想验证 socket 路径、但手上没有厂商 SDK 或没有 Windows 机器。

### 3.3 M300 SDK 模式（`--backend m300`）

走厂商原生 SDK。这条路径上有一个**必须知道的差别：设备端口不归 QuickVib 所有**。

| | `mock` / `tcp` | `m300` |
| --- | --- | --- |
| 谁 bind `--device-port` | QuickVib | SDK，在 `m300_server_create_ex` 内部 |
| `--bind` 的作用 | 两个监听器共用的网卡 | SCPI 监听网卡，同时作为 bind 地址传给 SDK |
| 采样路径 | 监听器 → 分帧器 → 引擎 | SDK 回调 → 有界队列 → 引擎 |
| 启动横幅 | `device on port 9123` | `device on port 9123 (bound by the SDK)` |
| `SYST:DEV:CONN?` 的依据 | QuickVib 接受的入站链路 | SDK 自己的 `m300_device_is_connected` |
| `device.allowedPeers` | 生效 | **不生效**，请改用防火墙规则 |
| 退出摘要里的链路计数 | 有效 | 恒为零，应以 `TRAC:POIN?` 为准 |
| 窗口里的「实际监听端口·设备」 | 显示端口号 | 显示一个短横线 `—` |

QuickVib **刻意不去** bind 设备端口：否则两个监听器争抢同一地址，失败的一方报「地址已被占用」，从
SDK 侧看就是 open 时返回 `-7 ERR_NETWORK`。

在非 Windows 主机上、或在未启用 `m300` feature 的构建上选择这个后端，会**直接启动失败**（退出码
`2`），绝不静默回退到模拟后端：

```console
$ ./target/release/quickvib --headless --backend m300 --project samples/Test.proj
2026-08-26T08:02:58.600Z INFO  proj    loaded path=samples/Test.proj name=Test
quickvib: backend 'm300' is unavailable: the M300 backend requires a Windows host
$ echo $?
2
```

### 3.4 接上真实的 M300（现场清单）

在工位上第一次接真机时，按顺序过一遍：

1. **装 SDK 与运行库。** 把 `m300_sdk.dll` 放到 `quickvib.exe` 旁边，或者用环境变量指出它在哪；并确认
   已安装 VC++ 2015–2022 运行库（[§2.5](#25-真实-m300-的额外前置条件)）。

   ```bat
   set QUICKVIB_M300_SDK=C:\M300SDK_v1.2.0\x64\bin
   ```

2. **确定本机对测振仪可达的 IP。** 测振仪是主动拨入的一方，因此它必须能连到这台 PC。同机测试用
   `--bind 127.0.0.1`；测振仪在网络另一头时用 `--bind 0.0.0.0`（或指定某块网卡的地址）。
3. **放行防火墙。** 入站 TCP **5025**（UTS 连入）和 **9123**（测振仪连入）都要放行。
4. **把测振仪配置为连接这台 PC 的 `9123` 端口。**
5. **启动 QuickVib：**

   ```bat
   target\release\quickvib.exe --headless --backend m300 --bind 0.0.0.0 ^
     --scpi-port 5025 --device-port 9123 --project samples\Test.proj
   ```

6. **确认链路。** 用 `SYST:DEV:CONN?` 查，返回 `1` 才说明 SDK 已经接受了设备。返回 `0` 就按
   [§9.2](#92-设备链路) 逐项排查。

注意这条路径上 `device.allowedPeers` **不生效**（那是 QuickVib 自己监听器的能力，SDK 没有对应机制），
需要限制来源就用防火墙规则。

---

## 4. 启动方式

### 4.1 操作员：打开桌面窗口

编译时带上 `gui` feature，运行时**不要**带 `--headless`：

```bash
cargo build --release --features gui
./target/release/quickvib --project samples/Test.proj
```

```bat
:: Windows
cargo build --release --features gui
target\release\quickvib.exe --project samples\Test.proj
```

窗口默认以**简体中文**、**深色**主题打开。SCPI 与设备服务移到后台线程继续工作——也就是说窗口开着的
时候，UTS 依然可以连 5025 端口。关闭窗口即关闭两个服务。

操作细节见 [§5](#5-操作员界面使用指南)。

### 4.2 UTS：无窗口运行

这就是 UTS 使用的启动命令。四个参数中有三个本来就是默认值，但仍全部显式写出——在测试工位上，一条明
确的命令行可以少一个意外来源：

```bat
:: Windows，UTS 拉起子进程
quickvib.exe --headless --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

```bash
# Linux / macOS
./target/release/quickvib --headless --scpi-port 5025 --device-port 9123 --project samples/Test.proj
```

启动成功后 stdout 上是这样三行（这是实际输出）：

```
2026-08-26T08:01:58.965Z INFO  proj    loaded path=samples/Test.proj name=Test
2026-08-26T08:01:58.965Z INFO  scpi    listening port=15025
2026-08-26T08:01:58.965Z INFO  device  listening port=19123
```

**UTS 应当等到 `scpi listening` 这一行出现之后再去连接**，或者直接对端口做带重试的连接。

进程会一直在前台运行直到被终止，因此 UTS 可以把它作为子进程管理、结束时杀掉即可。带内的正常关闭流
程是先 `ABOR`、再关闭 SCPI 连接。

### 4.3 命令行参数速查

以下内容与 `quickvib --help` 的输出一致。

| 参数 | 取值 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--project <path>` | 文件路径 | *(自动加载上次工程)* | 启动时加载该工程。失败即致命错误（退出码 `4`） |
| `--scpi-port <n>` | 1–65535 | *(工程配置，否则 `5025`)* | **UTS 连入**的端口。覆盖 `server.scpiPort` |
| `--device-port <n>` | 1–65535 | *(工程配置，否则 `9123`)* | **M300 连入**的端口。覆盖 `device.port`。`--backend m300` 时交由 SDK bind |
| `--bind <host>` | IP 字面量或主机名 | *(工程配置，否则 `0.0.0.0`)* | **两个**监听器绑定的网卡。`127.0.0.1` 把两条链路都限制在本机，UTS 与测振仪同机时应当这样配 |
| `--headless` | 开关 | 关 | 不打开窗口、无交互式控制台，仅输出结构化日志 |
| `--backend <mock\|tcp\|m300>` | 枚举 | *(工程配置，否则 `mock`)* | 覆盖工程中的后端选择，见 [§3](#3-三种运行模式) |
| `--no-auto-load` | 开关 | 关 | 禁用「自动加载上次工程」，保证运行环境干净 |
| `--log-level <level>` | 枚举 | `info` | `trace` \| `debug` \| `info` \| `warn` \| `error` |
| `--version` | 开关 | — | 打印版本号并以 `0` 退出 |
| `--help` | 开关 | — | 打印用法并以 `0` 退出 |

`--flag=value` 与 `--flag value` 两种写法都支持；`--` 终止参数解析；本程序**不接受任何位置参数**
（`quickvib Test.proj` 会被拒绝，必须写成 `quickvib --project Test.proj`）。

**关于 `--no-auto-load`。** QuickVib 会把最近一次加载或保存的工程路径记在状态目录里（Windows 为
`%LOCALAPPDATA%\QuickVib\`，Unix 为 `$XDG_STATE_HOME/quickvib/`），下次不带 `--project` 启动时自动
加载。这对操作员很方便，对 UTS 则是隐患——**UTS 启动时请始终显式指定 `--project`**，需要绝对干净时
再加上 `--no-auto-load`。

### 4.4 退出码

| 码 | 含义 | 常见原因 |
| --- | --- | --- |
| `0` | 正常退出 | — |
| `2` | 参数错误 | 未知参数、缺少取值、端口越界，**以及在不支持的主机上选择 `--backend m300`** |
| `3` | 端口绑定失败 | 端口被占用；`--bind` 的主机名解析不了或不是本机地址 |
| `4` | 工程加载失败 | 仅当显式指定了 `--project`：文件不存在或 schema 校验不过 |
| `5` | 后端打开失败 | 后端本身初始化失败（例如 SDK 拒绝创建服务） |

三个实际例子：

```console
$ ./target/release/quickvib --frobnicate
quickvib: unknown option '--frobnicate'
（随后打印完整用法到 stderr）
$ echo $?
2

$ ./target/release/quickvib --headless --scpi-port 15025 …   # 该端口已被占用
quickvib: could not bind the SCPI listener on 127.0.0.1:15025: Address already in use (os error 98)
$ echo $?
3

$ ./target/release/quickvib --headless --project /nope/missing.proj
quickvib: could not load /nope/missing.proj: project file not found: /nope/missing.proj
$ echo $?
4
```

---

## 5. 操作员界面使用指南

本节针对以 `--features gui` 编译、且不带 `--headless` 启动的桌面窗口。窗口里的每一处文字都是简体中
文；下面用到的按钮名就是屏幕上的原文。

### 5.1 界面分区

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ QuickVib  Test  [已连接] [录制状态 完成] [SCPI 15025 · 设备 19123] [浅色|深色] [中文|EN] │
├──────────────────────────────────────────────────────────────────────────────┤
│ 文件▼ 打开 保存 另存为 应用 还原                                    [配置|高级] │
├────────────────────────────────────────────┬─────────────────────────────────┤
│ 项目信息 · 设备与采样 · 滤波器 · 量程       │ 仪表面板                        │
│ 录制 · 通信端口 · 数据导出                  │  录制状态 / 连接状态            │
│                                            │  [开始录制] [停止]              │
│ （可滚动的配置卡片列）                      │  测量结果 峰值 / 有效值 / 峰峰值 │
│                                            │  样本数 · 时长 · 导出           │
│                                            │  链路信息                       │
├────────────────────────────────────────────┴─────────────────────────────────┤
│ ● 完成 · 已连接   录制已开始          实际监听端口: SCPI 15025 · 设备 19123   │
└──────────────────────────────────────────────────────────────────────────────┘
```

* **标题栏** —— 产品名、当前工程名及是否有未保存的修改，然后是三个状态标签（设备链路、录制状态、
  实际监听端口），最右侧是主题与语言开关。
* **左栏** —— 可滚动，按卡片分组的全部工程配置。这里编辑的就是 `.proj` 文件的内容。
* **右栏** —— 宽度固定，就是「仪表面板」：状态、`*IDN?`、会话数、开始 / 停止按钮，以及最近一次完成
  采集的测量结果。
* **状态栏** —— 重复显示当前状态、最近一次操作的结果，并**始终**标出本进程实际监听的端口。

左栏的七张卡片：

| 卡片 | 字段 |
| --- | --- |
| **项目信息** | 名称、说明 |
| **设备与采样** | 采样率（输入时同步标注 kHz）、数据类型（速度 μm/s / 位移 μm / 加速度 m/s²）、数据来源（模拟 / TCP设备 / M300） |
| **滤波器** | 低通与高通截止频率，提示为「采样率与低通须同档」。未勾选**固定截止**时，低通随采样率跟随奈奎斯特频率 |
| **量程** | 速度量程 μm/s、位移量程 μm、加速度量程 m/s²；与当前数据类型对应的那一项以粗体显示 |
| **录制** | 录制时长（秒） |
| **通信端口** | 上半部分只读的实际监听端口，下半部分可编辑的项目端口——见 [§5.3](#53-实际监听端口-vs-项目端口) |
| **数据导出** | CSV 或 TXT、导出目录、是否写 CSV 注释头、是否去直流 |

窗口没有显示的字段——看门狗倍数、采集缓冲上限、`*IDN?` 标识块、会话上限、`server.bindHost`、允许的
对端地址、mock 信号定义——在加载、编辑、保存的整个过程中**原样保留**，因此用窗口打开一个工程再存回
去，不会悄悄丢掉任何字段。

**高级**标签页目前是占位。激光功率、TEC 设定点、PID 增益与外部触发都是真实的设备控制项，但在版本 1
的工程 schema 中没有对应字段，QuickVib 不会凭空造出仪器无法执行的设置。

### 5.2 打开 / 保存 / 应用 / 还原

这四个按钮的区别是操作员最需要弄清的一件事：

| 按钮 | 做什么 | 什么时候用 |
| --- | --- | --- |
| **打开** | 从磁盘读入一个 `.proj` 文件 | 切换测试项目 |
| **应用** | 校验表单并把它交给**正在运行的引擎** | **每次改完设置都要点它，否则改动不生效** |
| **保存** / **另存为** | 写回 `.proj` 文件（写入前会先执行一次「应用」） | 想让改动下次启动仍然有效 |
| **还原** | 丢弃表单上的修改，重新读取引擎里的工程 | 改乱了想退回去 |

**「应用」是真正关键的按钮。** 它逐字段解析并校验，成功后把编辑好的工程交给正在运行的引擎——也就是
各个 SCPI 会话共享的同一个引擎实例，因此紧接着从 5025 端口发来的 `CONF:REC:DUR?` 返回的就是刚刚输入
的时长。校验失败则**什么都不写入**，并按当前界面语言逐条列出被拒绝的字段与原因。

**有五项设置需要重启才能生效**，因为进程已经绑定了套接字、打开了后端：

1. SCPI 项目端口
2. 设备项目端口
3. 数据来源（后端）
4. 采样率
5. 数据类型

「应用」会把这五项**保存下来**，并明确提示哪些需要重启。这不是失败，只是生效时机不同。

> **注意：加载工程会丢弃已采集的数据。** 「打开」和「应用」都会替换引擎里的工程，而一段采集数据是属
> 于它所在的那个工程的。因此换工程之后录制状态回到**空闲**，测量结果和导出都会报「无数据」。**请先
> 导出、再换工程。** 录制进行中尝试换工程会被拒绝，且不改变任何东西。

### 5.3 实际监听端口 vs 项目端口

工程文件里记着 `server.scpiPort` 和 `device.port`，但命令行的 `--scpi-port` / `--device-port` 会覆盖
它们。因此**正在运行的 QuickVib 所监听的端口，经常不是文件里写的那两个**。窗口从不把两者混为一谈：

| | **实际监听端口** | **项目端口（需重启）** |
| --- | --- | --- |
| 是什么 | 本进程真正 bind 的端口 | `.proj` 文件里存的值 |
| 能否编辑 | 只读 | 可编辑 |
| 显示在哪 | 标题栏、通信端口卡片上半部分、状态栏 | 通信端口卡片下半部分 |
| 何时生效 | 已经生效 | 下次启动 |

**UTS 要连的是「实际监听端口」。** 当两者不一致时——例如用 `quickvib --scpi-port 15025` 启动一个写着
`5025` 的工程，这是最常见的情况——卡片会直接给出提示，而不是让表单看起来像是权威值。

走 `--backend m300` 时，「实际监听端口」里的**设备端口显示为一个短横线**。这是正确的：那一栏列的是
QuickVib 自己 bind 的套接字，而这个后端上设备端口归 SDK 所有（[§3.3](#33-m300-sdk-模式--backend-m300)）。
此时以「项目端口」字段和启动横幅上的数字为准。

### 5.4 录制与导出

1. 确认标题栏的**连接状态**为**已连接**。模拟后端恒为已连接；`TCP设备` 与 `M300` 需要设备真的拨入。
2. 在**录制**卡片里设定录制时长，点**应用**。
3. 点**开始录制**（等同于 SCPI 的 `INIT`）。状态依次经过**武装 → 录制中**。
4. 收满 `时长 × 采样率` 个采样点后状态变为**完成**，右栏的**峰值 / 有效值 / 峰峰值**同时刷新。
5. 需要波形文件时点**导出数据**（等同于 `MMEM:STOR:TRAC`），路径基于**数据导出**卡片里的目录解析。

**停止**按钮等同于 `ABOR`：中止当前采集，状态变为**已中止**。**已中止的采集不可取回**——测量和导出
都会报「无数据」，这是刻意设计。

### 5.5 主题与语言

两个开关都在标题栏最右侧，都是点一下下一帧就生效，不需要重启，也不重新加载任何东西。

| 开关 | 位置 | 默认 | 记在哪 |
| --- | --- | --- | --- |
| **浅色 / 深色** | 语言开关左侧 | 深色 | 状态目录下的 `ui-theme.txt` |
| **中文 / EN** | 最右侧 | 中文 | 状态目录下的 `ui-language.txt` |

状态目录：Windows 为 `%LOCALAPPDATA%\QuickVib\`，Unix 为 `$XDG_STATE_HOME/quickvib/`。删掉对应文件
即回到默认（深色、中文）。

深色是关灯开激光的试验间里的默认外观；浅色配色不是简单反相，每种强调色都加深到与所在卡片对比度不低
于 4.5:1，因此在明亮的白色台面上同样清晰。两种主题之间只有颜色不同，布局、间距与字体完全一致。

**关于字体：** 显示中文需要 CJK 字体，程序已内嵌一份 Noto Sans SC（SIL 开放字体许可证 1.1），构建时
不下载、运行时不读取系统字体。因此在一台刚装好系统的 Windows 测试工位上，界面渲染结果和开发机完全一
致，**不需要额外安装任何字体**。

---

## 6. UTS 的 SCPI 作业流程

### 6.1 链路参数

| 项 | 值 |
| --- | --- |
| 传输 | 裸 TCP socket（**不是** VISA / HiSLIP / VXI-11 / USBTMC） |
| 端口 | `--scpi-port`，默认 `5025` |
| 编码 | ASCII |
| 命令结束符 | `\n`（`\r\n` 也接受） |
| 响应 | 单行，以 `\n` 结束 |
| 大小写 | 头部不区分大小写 |
| 复合命令 | 同一行内用 `;` 分隔，如 `*CLS;*IDN?` |
| 并发会话 | 默认上限 8（`server.maxSessions`） |

**只有查询（以 `?` 结尾）才有响应。** 命令没有响应，等它的响应会一直挂着——这是 SCPI 仪器的标准行为，
也是编写 UTS 脚本时最容易踩的坑。

### 6.2 一次完整测量的步骤

| 步 | 命令 | 目的 | 必需？ |
| --- | --- | --- | --- |
| 1 | `*IDN?` | 确认连到的确实是 QuickVib | 建议 |
| 2 | `*CLS` | 清空错误队列，从干净状态开始 | 建议 |
| 3 | `MMEM:LOAD:STAT "<path>"` | 加载工程 | 若启动时已用 `--project` 指定则可省 |
| 4 | `SYST:ERR?` | 确认第 3 步成功 | 建议 |
| 5 | `SYST:DEV:CONN?` | 确认设备链路已建立 | **必需**（除 mock 外） |
| 6 | `CONF:REC:DUR <秒>` | 覆盖本次的录制时长 | 可选 |
| 7 | `FORM CSV` 或 `FORM TXT` | 选择导出格式 | 仅在要导出时 |
| 8 | `INIT` | **开始采集**（不阻塞，无响应） | **必需** |
| 9 | `REC:WAIT?` | 阻塞直到采集结束 | **必需**（或轮询 `REC:STAT?`） |
| 10 | `CALC:MEAS:ALL?` | 读取峰值、有效值、峰峰值 | 取标量时 |
| 11 | `MMEM:STOR:TRAC "<path>"` | 把整段波形写成文件 | 取波形时 |
| 12 | `SYST:ERR?` | 确认整个过程无错 | 建议 |

### 6.3 完整会话记录

下面是在本仓库上**实际跑出来的**一次会话（mock 后端，录制时长改为 1.0 s）。`>` 是发给 QuickVib 的，
`<` 是收到的。

```
> *IDN?
< QuickVib,M300-SCPI,SN-0001,1.0.0

> *CLS                                    ← 从干净的错误队列开始
> MMEM:LOAD:STAT "samples/Test.proj"      ← 采样率、单位、时长、标识
> SYST:ERR?
< 0,"No error"                            ← 加载成功

> SYST:DEV:CONN?
< 1                                       ← mock 恒为 1；真机必须已拨入

> CONF:REC:DUR 1.0                        ← 覆盖本次录制时长
> CONF:REC:DUR?
< 1.000                                   ← 回读永远是三位小数
> FORM CSV
> FORM?
< CSV

> INIT                                    ← 不阻塞，无响应，采集已经开始
> REC:WAIT?                               ← 阻塞直到采集结束
< #REC:DONE                               ← 主动推送的完成通知
< 1                                       ← REC:WAIT? 返回 1 = 已完成

> REC:STAT?
< COMPLETE

> TRAC:POIN?
< 100000                                  ← 1.0 s × 100 kS/s

> CALC:MEAS:ALL?
< 278.2689,179.0509,555.6500              ← 峰值, 有效值, 峰峰值（单位同工程配置）

> MMEM:STOR:TRAC "/tmp/qvout/run001.csv"
> SYST:ERR?
< 0,"No error"

> BOGUS:CMD?                              ← 未知命令：没有任何响应
> SYST:ERR?
< -113,"Undefined header"                 ← 只能靠 SYST:ERR? 发现
```

同一次运行在 QuickVib 侧的日志：

```
2026-08-26T08:02:01.034Z INFO  scpi    session opened peer=127.0.0.1:52206
2026-08-26T08:02:01.080Z INFO  proj    loaded path=samples/Test.proj name=Test
2026-08-26T08:02:01.169Z INFO  rec     started duration=1.000 expectedSamples=100000
2026-08-26T08:02:02.175Z INFO  rec     complete samples=100000 elapsed=1.006 peak=278.2689 rms=179.0509 pp=555.6500
2026-08-26T08:02:02.201Z INFO  export  wrote path=/tmp/qvout/run001.csv samples=100000 format=CSV
2026-08-26T08:02:02.217Z WARN  scpi    error -113,"Undefined header" (undefined header 'BOGUS:CMD?')
2026-08-26T08:02:02.265Z INFO  scpi    session closed peer=127.0.0.1:52206
```

用命令行快速验证一条链路是否通：

```bash
printf '*IDN?\n' | nc 127.0.0.1 5025
# QuickVib,M300-SCPI,SN-0001,1.0.0
```

### 6.4 等待采集完成的三种方式

| 方式 | 命令 | 特点 |
| --- | --- | --- |
| **阻塞等待** | `REC:WAIT?` | 最简单。完成返回 `1`，中止或超时返回 `0` |
| **轮询** | `REC:STAT?` | 不阻塞，可安全地反复调用。返回 `IDLE`\|`ARMED`\|`RECORDING`\|`COMPLETE`\|`ABORTED` |
| **等通知** | `#REC:DONE` / `#REC:ABORT` | 主动推送给所有已连接会话，无需轮询 |

`*OPC?` 与 `REC:WAIT?` 类似，也是阻塞到当前采集结束。

三者都由服务端的看门狗兜底：看门狗 = `时长 × recording.timeoutMultiplier + 1 s`，`REC:WAIT?` 的条件
变量再多等 1 s，因此**任何一种等待都不会超过 `时长 × timeoutMultiplier + 2 s`**。UTS 侧的 socket
超时按这个上界来设即可。

### 6.5 编写 UTS 脚本时的注意事项

* **`INIT` 是命令不是查询**，没有响应。等它的响应会一直挂着。
* **未知命令返回空**，只把 `-113` 压进错误队列。这是标准仪器行为——用 `SYST:ERR?` 去发现拼写错误。
* **以 `#` 开头的行是主动推送的通知，不是查询响应。** 等在 `REC:WAIT?` 上的客户端会先收到
  `#REC:DONE` 再收到 `1`（中止时是先 `#REC:ABORT` 再 `0`）。读响应时必须能识别并跳过 `#` 开头的行，
  否则会把通知当成查询结果。
* **`FETC?` 的响应是一整行**。5 s × 100 kS/s 的采集大约 6 MB，全在一行里。先调 `TRAC:POIN?` 拿到点
  数再决定读缓冲区大小；只要标量的话直接用 `CALC:MEAS:ALL?`；要波形建议用 `MMEM:STOR:TRAC` 落盘后读
  文件，比在 socket 上拉 6 MB 稳妥。
* **一次采集只从 `INIT` 之后开始。** 仪器空闲期间到达的数据全部丢弃。
* **已中止的采集不可取回。** `FETC?`、`CALC:MEAS:*?`、`MMEM:STOR:TRAC` 都会返回 `-230`。
* **加载工程会丢弃已采集的数据**，见 [§5.2](#52-打开--保存--应用--还原) 的提示框。**先导出，再换工
  程。**
* **错误队列深 32。** 溢出时最新一条被替换成 `-350`，因此一串错误之后读到的最后一条永远在说「有错误
  丢失了」。排查问题时请**反复读 `SYST:ERR?` 直到返回 `0,"No error"`**。
* **导出是原子的。** 先写目标目录下的临时文件再重命名到位，因此轮询该文件的 UTS 绝不会读到写了一半
  的内容。

### 6.6 常用命令速查

完整表格见 [`SCPI.md`](SCPI.md)。

| 命令 | 类型 | 响应示例 | 说明 |
| --- | --- | --- | --- |
| `*IDN?` | 查询 | `QuickVib,M300-SCPI,SN-0001,1.0.0` | 厂商、型号、序列号、固件版本 |
| `*RST` | 命令 | — | 中止采集、丢弃数据、恢复工程默认值、清空错误队列 |
| `*CLS` | 命令 | — | 只清空错误队列和待决的 `*OPC` |
| `*OPC?` | 查询 | `1` | 阻塞到当前采集结束 |
| `SYST:ERR?` | 查询 | `0,"No error"` | 取出最早的一条错误 |
| `SYST:VERS?` | 查询 | `1999.0` | SCPI 标准版本 |
| `SYST:DEV:CONN?` | 查询 | `1` / `0` | 设备链路是否已建立 |
| `MMEM:LOAD:STAT "<path>"` | 命令 | — | 加载工程 |
| `MMEM:STOR:STAT "<path>"` | 命令 | — | 保存当前工程（含运行时覆盖值） |
| `MMEM:STOR:TRAC "<path>"` | 命令 | — | 按当前 `FORM` 导出最近一次完成的采集 |
| `CONF:REC:DUR <秒>` | 命令 | — | 录制时长，范围 `(0, 3600]` |
| `CONF:REC:DUR?` | 查询 | `5.000` | 三位小数 |
| `FORM CSV\|TXT` | 命令 | — | 导出格式 |
| `INIT` | 命令 | — | 开始采集（别名 `REC:STAR`、`INIT:IMM`） |
| `ABOR` | 命令 | — | 中止采集；未在采集时是空操作 |
| `REC:STAT?` | 查询 | `COMPLETE` | 当前状态，可安全轮询 |
| `REC:WAIT?` | 查询 | `1` / `0` | 阻塞到采集结束 |
| `FETC?` | 查询 | `v1,v2,…,vN` | 整段波形（别名 `TRAC:DATA?`） |
| `TRAC:POIN?` | 查询 | `500000` | 采样点数 |
| `CALC:MEAS:PEAK?` | 查询 | `12.3400` | `max(abs(x))` |
| `CALC:MEAS:RMS?` | 查询 | `4.5600` | 均方根 |
| `CALC:MEAS:PP?` | 查询 | `24.6800` | `max − min` |
| `CALC:MEAS:ALL?` | 查询 | `12.3400,4.5600,24.6800` | 峰值, 有效值, 峰峰值 |

状态机：

```
空闲 IDLE ──INIT──▶ 武装 ARMED ──首个采样──▶ 录制中 RECORDING ──收满──▶ 完成 COMPLETE
              │                      │
              └──── ABOR / 超时 ─────┴────────▶ 已中止 ABORTED
*RST 从任何状态回到 IDLE；进入 COMPLETE 之后 ABOR 是空操作。
```

### 6.7 错误码

| 码 | 消息 | 典型原因与处理 |
| --- | --- | --- |
| `0` | `No error` | 队列为空 |
| `-100` | `Command error` | 报文格式错误、行太长、未结束、非 UTF-8 输入 |
| `-113` | `Undefined header` | 命令拼错或不支持该写法 |
| `-221` | `Settings conflict` | 未加载工程、已在采集中、或采集途中改配置 |
| `-222` | `Data out of range` | 时长超出 `(0, 3600]`，或 `时长 × 采样率 × 4 字节` 超过 `recording.maxCaptureBytes` |
| `-224` | `Illegal parameter value` | `FORM` 取值非法，或工程文件无效 |
| `-230` | `Data corrupt or stale` | 没有已完成的采集就去取数据。中止过的采集刻意不可取回 |
| `-240` | `Hardware error` | 采集途中设备链路断开。状态变为 `ABORTED`，重新 `INIT` 即可 |
| `-241` | `Hardware missing` | `INIT` 时没有设备。见 [§9](#9-常见故障排查) |
| `-256` | `File name not found` | 工程文件不存在 |
| `-257` | `File name error` | 路径非法或不可写 |
| `-350` | `Queue overflow` | 未读错误超过 32 条，最新一条被它替换 |
| `-365` | `Time out error` | 看门狗超时：采样在收满之前就停了 |

### 6.8 导出格式

`FORM CSV|TXT` 选格式，`MMEM:STOR:TRAC "<path>"` 写文件。相对路径基于 `export.directory` 解析，缺失
目录自动创建，已存在的文件会被覆盖。

**CSV** —— 可选注释头 + 表头行 + 每采样一行，CRLF 换行。下面是上文那次 1 s 采集实际写出的文件开头：

```csv
# project=Test
# timestamp=2026-08-26T08:02:02.175Z
# sampleRateHz=100000
# unit=um/s
# samples=100000
# durationSeconds=1
index,time_s,value
0,0,40.759865
1,0.00001,44.48746
2,0.00002,46.231735
```

`# unit=` 写的是单位标签本身（`um/s`、`um`、`m/s^2`），不是 schema 里的枚举名。注释头由
`export.includeHeader` 控制。

**TXT** —— 每行一个数值，没有别的，给只想要数字的脚本用：

```text
40.759865
44.48746
46.231735
```

数值使用 Rust 的最短往返浮点格式，**与区域设置无关**：小数点永远是 `.`，绝不会变成逗号。

### 6.9 测量定义

| 指标 | 定义 | 查询 |
| --- | --- | --- |
| 峰值 Peak | `max(abs(xᵢ))` | `CALC:MEAS:PEAK?` |
| 有效值 RMS | `sqrt( (1/N) · Σ xᵢ² )` | `CALC:MEAS:RMS?` |
| 峰峰值 P-P | `max(xᵢ) − min(xᵢ)` | `CALC:MEAS:PP?` |

一遍扫描算完，尽管采样是 `f32`，累加用 `f64`，因此几十万点的采集不会损失精度。
`measurement.removeDc` 为 `true` 时会在算峰值和有效值之前先减去均值；峰峰值按定义不受去直流影响。

单位跟随工程配置的通道单位——速度 **μm/s**、位移 **μm**、加速度 **m/s²**。**QuickVib 不做任何单位换
算**：设备配成什么单位，它就按什么单位上报并如实标注。响应的小数位数由
`measurement.responseDecimals` 决定，默认 4 位。

---

## 7. 工程文件

工程是一个 JSON 文件（惯用扩展名 `.proj`），描述一次测试要用的全部设置。未知属性会被忽略，以便向前
兼容。

### 7.1 完整示例

这是仓库里的 `samples/Test.proj`，一字不差：

```json
{
  "schemaVersion": 1,
  "name": "Test",
  "description": "Sample QuickVib project: 5 s velocity capture, CSV export.",
  "device": {
    "backend": "mock",
    "port": 9123,
    "sampleRateHz": 100000,
    "unit": "velocity_um_s",
    "allowedPeers": [],
    "connectTimeoutSeconds": 30,
    "lpfHz": 50000,
    "highPassHz": 0,
    "velocityRange": 1000,
    "displacementRange": 1000,
    "accelerationRange": 100
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
  "identity": {
    "manufacturer": "QuickVib",
    "model": "M300-SCPI",
    "serialNumber": "SN-0001",
    "firmwareVersion": "1.0.0"
  },
  "server": {
    "maxSessions": 8,
    "scpiPort": 5025
  },
  "mock": {
    "signal": {
      "components": [
        { "frequencyHz": 120.0, "amplitude": 250.0, "phaseDeg": 0.0 },
        { "frequencyHz": 360.0, "amplitude": 40.0, "phaseDeg": 90.0 }
      ],
      "noiseStdDev": 2.5,
      "seed": 12345
    }
  }
}
```

### 7.2 字段说明

| 字段 | 默认值 | 说明 |
| --- | --- | --- |
| `schemaVersion` | *(必填)* | 目前只有 `1` |
| `name` / `description` | *(必填 / 可选)* | 工程名会写进 CSV 注释头 |
| `device.backend` | `"mock"` | `"mock"` \| `"tcp"` \| `"m300"`，见 [§3](#3-三种运行模式) |
| `device.port` | `9123` | 测振仪连入的端口；`--device-port` 优先 |
| `device.sampleRateHz` | *(必填)* | 例如 `100000` |
| `device.unit` | *(必填)* | `"velocity_um_s"` \| `"displacement_um"` \| `"acceleration_m_s2"` |
| `device.allowedPeers` | `[]` | 空表示接受任何入站对端。`--backend m300` 上**不生效** |
| `device.connectTimeoutSeconds` | `30` | 等待设备拨入的超时 |
| `device.sdkPath` | `null` | 查找 M300 SDK 的目录，仅 M300 后端使用 |
| `device.lpfHz` | `null` | 低通截止；`null` 表示跟随奈奎斯特频率（`sampleRateHz / 2`） |
| `device.highPassHz` | `0` | 高通截止；`0` 表示关闭。**必须小于低通截止** |
| `device.velocityRange` | `1000` | 速度量程，μm/s |
| `device.displacementRange` | `1000` | 位移量程，μm |
| `device.accelerationRange` | `100` | 加速度量程，m/s² |
| `recording.durationSeconds` | *(必填)* | 范围 `(0, 3600]` |
| `recording.timeoutMultiplier` | `2.0` | 看门狗 = 时长 × 本值 + 1 s |
| `recording.maxCaptureBytes` | `536870912` | 超过此上限的采集会被 `INIT` 以 `-222` 拒绝 |
| `measurement.removeDc` | `false` | 算峰值和有效值前先减去均值 |
| `measurement.responseDecimals` | `4` | `CALC:*` 响应的小数位数，`0..=9` |
| `export.format` | `"CSV"` | `"CSV"` 或 `"TXT"` |
| `export.directory` | `"."` | 相对导出路径的基准目录 |
| `export.includeHeader` | `true` | 是否写 CSV 的 `#` 注释头 |
| `identity.*` | 见示例 | `*IDN?` 的四个字段，可用来满足既有 UTS 的标识校验 |
| `server.maxSessions` | `8` | 并发 SCPI 会话上限 |
| `server.scpiPort` | `5025` | SCPI 监听端口；`--scpi-port` 优先。**不能与 `device.port` 相同** |
| `server.bindHost` | `"0.0.0.0"` | 两个监听器绑定的网卡；`--bind` 优先。主机名在 bind 时才解析，解析失败是退出码 `3` |
| `mock.signal.components[]` | 一个 100 Hz 分量 | 正弦分量：`frequencyHz`、`amplitude`、`phaseDeg` |
| `mock.signal.noiseStdDev` | — | 叠加的高斯噪声标准差 |
| `mock.signal.seed` | — | 随机数种子，固定它即可复现同一段波形 |

### 7.3 滤波器与量程

**低通 `lpfHz`。** 留 `null` 时自动跟随奈奎斯特频率（采样率的一半），这几乎总是想要的行为。窗口里对
应「固定截止」复选框：不勾就是跟随，勾上就锁定为自己填的值。厂商设备上采样率与低通是**成对的档位**，
不能任意组合——窗口里的提示「采样率与低通须同档」说的就是这件事。

**高通 `highPassHz`。** `0` 表示关闭。填了就必须**小于**低通截止，否则工程校验不通过。想去掉直流分
量通常有两个办法：设一个很低的高通，或者把 `measurement.removeDc` 设为 `true`（后者是在算测量值时减
均值，不改变导出的波形）。

**三个量程。** `velocityRange` / `displacementRange` / `accelerationRange` 三个字段**同时存在**，但
只有与 `device.unit` 对应的那一个真正起作用；窗口里会把生效的那一项加粗。三个都留着是为了切换数据类
型时不丢配置。

### 7.4 自动加载上次工程

最近一次加载或保存的工程路径会被记住（Windows 为 `%LOCALAPPDATA%\QuickVib\`，Unix 为
`$XDG_STATE_HOME/quickvib/`），启动时若未指定 `--project` 就自动加载它。

* 操作员：方便，接着上次干。
* UTS：**请始终显式写 `--project`**；要绝对干净再加 `--no-auto-load`。

对应的 SCPI 命令是 `MMEM:LOAD:AUTO`（加载）和 `MMEM:LOAD:AUTO?`（查询会加载哪个路径）。

---

## 8. 两个模拟器

没有真机也可以把整条链路跑通。**但两个模拟器不可互换**——把其中一个接到另一个的后端上，链路能连上，
然后什么有用的事都不会发生。

| 后端 | 该用哪个模拟器 | 线上格式 | 前置条件 |
| --- | --- | --- | --- |
| `--backend mock` | *(不需要)* | 采样不出进程 | 无 |
| `--backend tcp` | **`m300-sim`** | 裸小端 `f32`，无分帧 | 无 |
| `--backend m300` | **`m300-device-sim`** | SCZN 分帧协议 | Windows、`--features m300`、厂商 DLL |

两者都**不是假 DLL**：它们都只是普通的 TCP 客户端。在 `m300` 路径上，负责解析协议的是厂商真实的 SDK。

### 启动顺序：先 QuickVib，后模拟器

**这一点没有例外。** QuickVib（或 SDK）是服务端，模拟器是客户端，服务端必须先处于监听状态。顺序反了
的表现是 `m300-sim` 立刻以退出码 `3` 结束（`m300-device-sim` 则会按 `--retry` 的间隔一直重试，直到
连上）。

### 8.1 `m300-sim`：配合 `--backend tcp`

模拟后端很方便，但它绕过了传输层：采样直接交给引擎，socket 路径上的代码一行都没跑到。
`--backend tcp` 补上这一段——它记录从 `--device-port` 连入的小端 `f32` 数据流，也就是真实 M300 推送的
内容；`m300-sim` 就是那个主动连入并推送正弦信号的替身。

```bash
# 终端 1 —— 仪器（先启动）
./target/release/quickvib --headless --backend tcp --device-port 9123 --project samples/Test.proj

# 终端 2 —— 「M300」
./target/release/m300-sim --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

```bat
:: Windows
quickvib.exe --headless --backend tcp --device-port 9123 --project samples\Test.proj
m300-sim.exe --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

之后照常通过 SCPI 驱动——返回的数值就是对真正穿过 socket 的字节所做的测量。下面是**实际跑出来**的一
段，包括设备连上之前的行为：

```
--- 设备还没连进来 ---
> SYST:DEV:CONN?
< 0
> INIT
> SYST:ERR?
< -241,"Hardware missing"                 ← 没有设备就 INIT，必然是这个

--- 启动 m300-sim 之后 ---
> SYST:DEV:CONN?
< 1
> CONF:REC:DUR 1.0
> INIT
> REC:WAIT?
< #REC:DONE
< 1
> TRAC:POIN?
< 100000
> CALC:MEAS:ALL?
< 250.0000,176.7767,500.0000              ← 幅度 250 的纯正弦：峰值 250，有效值 250/√2
```

**参数：**

| 参数 | 取值 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--host <host>` | 主机名或 IP | `127.0.0.1` | QuickVib 所在地址 |
| `--port <n>` | 1–65535 | `9123` | QuickVib 的 `--device-port` |
| `--rate <hz>` | > 0 | `100000` | 每秒采样点数，按真实时间节流 |
| `--amplitude <a>` | 有限数 | `250` | 峰值幅度，单位由工程配置决定 |
| `--frequency <hz>` | ≥ 0 | `120` | 正弦频率；`0` 表示发送恒定的平直信号 |
| `--duration <s>` | > 0 | *(直到断开)* | 达到该时长后停止 |
| `--version` / `--help` | 开关 | — | 打印并以 `0` 退出 |

默认值与 `samples/Test.proj` 里的第一个 mock 分量一致，因此除端口外无需任何参数即可配对运行。退出
码：`0` 对端关闭或 `--duration` 到时，`2` 参数错误，`3` 连不上。

**几点需要知道的：**

* **同一时刻只接受一条设备链路**，第二条会被立即拒绝。
* **采集从 `INIT` 之后开始**，空闲期间到达的数据全部丢弃。
* **采样率要对齐。** `--rate` 应等于工程的 `device.sampleRateHz`。模拟器更慢会让采集时间成比例变长
  并可能触发看门狗；更快则只是有一部分采样永远不会被记录。
* 采集途中断开链路会以 `-240,"Hardware error"` 中止。

### 8.2 `m300-device-sim`：配合 `--backend m300`

`--backend m300` 是 `m300-sim` 唯一够不到的一条路径。这条路径上 socket 归 SDK 所有，而它期待收到的不
是裸采样，而是 **SCZN**：一套分帧的请求 / 应答协议——上位机发 `0x00` 开始采集、下位机应答，随后不断
上传 `0x04` 采样块，直到被要求停止。`m300-device-sim` 就是一台会讲这套协议的设备。协议本身见
[`SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md)。

它需要一台装有厂商 DLL 的 Windows 主机，因为 socket 另一端就是真实的 SDK：

```bat
:: 终端 1 —— 仪器，设备端口由 SDK bind（先启动）
quickvib.exe --headless --backend m300 --device-port 9123 --project samples\Test.proj

:: 终端 2 —— 「M300」
m300-device-sim.exe --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

模拟器本身与平台无关，在 Linux 上也能编译和运行；只是没有 SDK 时，除了测试套件里自带的上位机替身之
外，它没有可以对话的对象。

**参数：**

| 参数 | 取值 | 默认值 | 说明 |
| --- | --- | --- | --- |
| `--host <host>` | 主机名或 IP | `127.0.0.1` | SDK 监听器所在地址 |
| `--port <n>` | 1–65535 | `9123` | SDK bind 的端口 |
| `--rate <hz>` | > 0 | `100000` | 每秒采样点数，按真实时间节流 |
| `--amplitude <a>` | 有限数 | `250` | 峰值幅度，单位由数据类型决定 |
| `--frequency <hz>` | ≥ 0 | `120` | 正弦频率；`0` 表示发送恒定的平直信号 |
| `--data-type <kind>` | `velocity` \| `displacement` \| `acceleration` \| `iq` | `velocity` | 设备声称上传的物理量 |
| `--sn <serial>` | ≤ 10 个 ASCII 字符 | `SIM-000001` | 硬件信息中的序列号，`*IDN?` 会报告它 |
| `--block <n>` | 1–262144 | `4096` | 每个 `0x04` 上传包的采样点数 |
| `--crc <mode>` | `standard` \| `castagnoli` \| `zero` | `standard` | 发送哪种校验值——见下 |
| `--prefix-endian <e>` | `big` \| `little` | `big` | 上传负载中两个前缀字的字节序 |
| `--retry <s>` | > 0 | `1` | 两次拨号之间的间隔秒数 |
| `--once` | 开关 | *(无限重连)* | 链路断开即退出 |
| `--duration <s>` | > 0 | *(直到被中断)* | 整轮运行达到该时长后停止 |
| `--version` / `--help` | 开关 | — | 打印并以 `0` 退出 |

退出码：`0` 至少建立过一次会话，`2` 参数错误，`3` 整轮运行结束时一次都没连上。

**几点需要知道的：**

* **连上之后是安静的，采样要等 `0x00`。** 与连上就开始推送的 `m300-sim` 不同，这个模拟器在上位机下
  达开始命令之前一直空闲，收到 `0x02` 立即停止。这正是协议的规定，也正因如此，QuickVib 那侧的 `INIT`
  在线上才真的对应一件事。**因此「连上了但一直没数据」在 `INIT` 之前是正常现象。**
* **采样率和滤波器是档位号，不是赫兹。** `--rate 100000` 会变成第 `0x05` 档，外加与之匹配的 100 kHz
  低通档位——厂商坚持这两者必须成对设置。若请求的采样率不在档位表上，上报的是最接近的一档，而节流仍
  按请求值执行；这正好可以刻意复现「设备与工程不一致」的场景。
* **`--crc` 的存在是因为厂商规范自相矛盾。** 规范既给出了标准 CRC-32 的 C 表，又给出了一段只可能由
  CRC-32C 算出结果的 Python 示例。设备默认发送前者，收包时三种（含规范明确允许的全零）一律接受。
  **如果 SDK 报 `-4 ERR_CRC_MISMATCH`，就依次试 `--crc castagnoli` 和 `--crc zero`。** 详见
  [`SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md) §7。
* **固件升级是「拒绝」，不是「无视」。** `0xF8`–`0xFF` 会按文档规定的应答格式返回失败，这样上位机自
  己的错误处理会真的跑一遍，链路也保持可用。
* **它会自己重新拨号**（真实设备也是如此），并且回来时处于**空闲**状态——一次采集不会在启动它的那条
  链路之外幸存。

---

## 9. 常见故障排查

### 9.1 启动阶段

| 现象 | 原因与处理 |
| --- | --- |
| 退出码 `3`，`Address already in use` | 端口被占用。多半是另一个 QuickVib 实例，或者 `5025` / `9123` 上有别的东西。用 `--scpi-port` / `--device-port` 换个空闲端口。Windows 上用 `netstat -ano \| findstr :5025` 查占用者，Linux 上用 `ss -ltnp \| grep 5025` |
| 退出码 `3`，但端口没被占用 | `--bind` 或 `server.bindHost` 给的主机名解析不了，或者不是本机的地址。错误信息里会写出实际尝试的地址 |
| 退出码 `2`，`unknown option` | 参数拼错，或者写成了位置参数。工程路径必须写作 `--project <path>` |
| 退出码 `2`，`backend 'm300' is unavailable` | 在非 Windows 主机上、或在未启用 `m300` feature 的构建上选了 M300 后端。这是刻意为之，不会静默回退到 mock |
| 退出码 `4` | 显式指定的 `--project` 文件不存在或 schema 校验不过。日志里带 JSON 的行号列号 |
| 退出码 `5` | 后端自身初始化失败。M300 路径上最常见的是 `-7 ERR_NETWORK`——见下条 |
| `--backend m300` 启动时报 `-7 ERR_NETWORK` | 设备端口已被别人占着，SDK bind 不上。常见元凶是第二个 QuickVib 实例，或者上一次 `--backend tcp` 的残留进程。注意这条路径上 QuickVib 自己**不**bind 该端口 |
| 没有窗口弹出 | 三种可能：编译时没加 `--features gui`；启动时带了 `--headless`；机器上没有显示环境。后两种会打印一行说明并回退到控制台。**两个服务在任何一种情况下都照常工作** |

### 9.2 设备链路

| 现象 | 原因与处理 |
| --- | --- |
| `SYST:DEV:CONN?` 返回 `0` | 设备没有拨进来。逐项检查：设备是否上电、是否与本机同网段、是否配置为连接本机的 `9123`、防火墙是否放行入站连接、`device.allowedPeers`（若非空）是否包含它的地址。`--backend mock` 下这里恒为 `1`；`--backend m300` 下这里是 SDK 自己的判断，与对端白名单无关 |
| `INIT` 返回 `-241,"Hardware missing"` | 两种可能：**(a)** 根本没有设备连入——先查 `SYST:DEV:CONN?`；**(b)** 仅 M300 路径：SDK DLL 加载失败。后者的日志会**逐条列出尝试过的每个路径和对应的操作系统错误**，照着 [§2.5](#25-真实-m300-的额外前置条件) 核对 DLL 位置和 VC++ 运行库 |
| 模拟器连上了，但一直没有采样 | 多半是给这个后端用错了模拟器。`--backend tcp` 要配 `m300-sim`（裸 `f32`），`--backend m300` 要配 `m300-device-sim`（SCZN）。两者都能**连上**对方的后端，然后什么都不发生 |
| `m300-device-sim` 连上了但很安静 | 在 `INIT` 之前这是**正常的**——它要等上位机的 `0x00`。若 `INIT` 已经发出仍无数据，查日志里有没有 `-4 ERR_CRC_MISMATCH`，有的话试 `--crc castagnoli` 或 `--crc zero` |
| `m300-sim` 以退出码 `3` 退出 | 目标端口上没有人监听。**先启动 QuickVib**，并确认它的 `--device-port` 就是模拟器 `--port` 的值 |
| 采集途中 `-240,"Hardware error"` | 设备链路在 `ARMED` 或 `RECORDING` 期间断开了。状态变为 `ABORTED`，重新 `INIT` 即可 |

### 9.3 采集与数据

| 现象 | 原因与处理 |
| --- | --- |
| `-365,"Time out error"` | 看门狗超时：采样在收满预期点数之前就停了。`--backend tcp` 上最常见的原因是模拟器 `--rate` 低于工程的 `device.sampleRateHz`。把两者对齐，或调大 `recording.timeoutMultiplier` |
| `-230,"Data corrupt or stale"` | 没有已完成的采集就去取数据。检查三件事：是否真的 `INIT` 过；采集是不是被中止了（**中止的采集刻意不可取回**）；有没有在取数据之前加载了新工程（**加载会丢弃采集**） |
| `-222,"Data out of range"` | 时长超出 `(0, 3600]`，或 `时长 × 采样率 × 4 字节` 超过了 `recording.maxCaptureBytes`（默认 512 MiB） |
| `-221,"Settings conflict"` | 未加载工程、已经在采集中、或者试图在采集途中改配置。先 `ABOR` 或等采集结束 |
| 命令好像被忽略了 | 未知命令**按设计不返回任何东西**。读 `SYST:ERR?`——`-113,"Undefined header"` 就是拼写错误或不支持的写法 |
| `FETC?` 在 UTS 侧被截断或超时 | 响应是很长的一整行（5 s × 100 kS/s 约 6 MB）。先 `TRAC:POIN?` 拿点数再设读缓冲区，或者干脆用 `MMEM:STOR:TRAC` 落盘后读文件 |
| 测量值明显不对 | 确认工程的 `device.unit` 与设备实际输出的物理量一致——**QuickVib 不做单位换算**。另外检查 `measurement.removeDc` 是否符合预期 |
| 换了工程之后数据没了 | 这是设计行为：一段采集属于它所在的工程，`MMEM:LOAD:STAT`、`MMEM:LOAD:AUTO` 和窗口里的**应用**都会丢弃它。**先导出、再换工程** |

### 9.4 界面

| 现象 | 原因与处理 |
| --- | --- |
| 改了设置但仪器没反应 | 只有**应用**会把表单推给运行中的引擎。而端口、数据来源、采样率、数据类型这四类改动即使应用了也**需要重启**，应用时会明确提示 |
| 窗口显示的端口 UTS 连不上 | 看状态栏的**实际监听端口**，不要看可编辑的**项目端口**：命令行参数会覆盖工程文件，窗口正是为此把两者分开标注（[§5.3](#53-实际监听端口-vs-项目端口)） |
| 窗口是中文，想要英文 | 标题栏右上角的 **中文 / EN** 开关。选择记在状态目录的 `ui-language.txt` 里 |
| 深色界面在明亮台面上看不清 | **浅色 / 深色** 开关就在语言开关左边。选择记在 `ui-theme.txt` 里 |
| `--backend m300` 时设备端口显示成短横线 | **这是正确的。**「实际监听端口」列的是 QuickVib 自己 bind 的套接字，这个后端上设备端口归 SDK。以「项目端口」字段和启动横幅上的数字为准 |

### 9.5 报告问题时请附上

1. `--headless` 的完整日志输出（需要时加 `--log-level debug`）。
2. **反复执行 `SYST:ERR?` 直到返回 `0,"No error"`** 的全部结果——错误队列是 FIFO，只读一条会漏掉先
   发生的错误。
3. 用到的工程文件。
4. 精确的启动命令行，以及 `quickvib --version` 的输出。

---

## 10. 功能边界

QuickVib **不做**下面这些事，这是刻意的设计决定，不是尚未实现：

* **不做合格判定。** 没有上下限、没有判定结论、没有公差带。QuickVib 只返回数字，**判定由 UTS 负责**。
* **不驱动 DUT 振动。** 不控制振动台或激励器。模拟后端合成的信号是测试用的夹具，**不是**对被测件的
  激励。
* **不负责重试。** 采集失败或被中止，QuickVib 如实报告；是否重跑由 UTS 决定。
* **不提供假的原生 DLL。** 在 Linux 上可测试是靠模拟后端和 `m300-sim` 这个普通 socket 客户端实现
  的，而不是靠伪造厂商库。

版本 1 同样不包含：实时波形绘图、FFT / 频谱分析、多通道同步采集、VISA / HiSLIP / VXI-11 传输
（**只支持裸 socket**）、USBTMC，以及 `INIT` 立即触发之外的任何触发方式。

桌面窗口是同一套工程 schema 与同一个仪器引擎之上的前端——它**不提供任何 SCPI 接口之外的额外能力**，
并且在 `--headless` 下永远不会打开。

---
---

# QuickVib Usage Manual (English)

**[中文](#quickvib-软件使用说明--usage-manual) | English**

This is QuickVib's **operating manual**: written for the people who actually have to run it on a
line or in a test cell — the test engineer writing the UTS script, and the operator sitting at the
bench configuring a project in the window. It is organised by task, and every command is one you
can copy.

If you want the **authoritative per-command definitions** instead, read:

| Document | Contents |
| --- | --- |
| [`../README.md`](../README.md) | Project overview and full reference |
| [`SCPI.md`](SCPI.md) | SCPI command and error-code tables |
| [`SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md) | The device-side SCZN transfer protocol |
| [`M300-NATIVE.md`](M300-NATIVE.md) | The vendor SDK's native ABI contract |
| [`PLAN.md`](PLAN.md) | The normative design document |

> Every command line, response and exit code below was **executed and recorded** against `1.0.0` in
> this repository, not transcribed from a design document. The parts that need real M300 hardware
> (`--backend m300`) are **not yet bench-verified**, and are marked as such where they appear.

## Contents

1. [What QuickVib is](#1-what-quickvib-is)
2. [Installation and build](#2-installation-and-build)
3. [Three operation modes](#3-three-operation-modes)
4. [Launch paths](#4-launch-paths)
5. [Operator guide to the window](#5-operator-guide-to-the-window)
6. [The UTS SCPI workflow](#6-the-uts-scpi-workflow)
7. [The project file](#7-the-project-file)
8. [The two simulators](#8-the-two-simulators)
9. [Troubleshooting](#9-troubleshooting)
10. [Out of scope](#10-out-of-scope)

---

## 1. What QuickVib is

QuickVib wraps an **M300 laser Doppler vibrometer** so that it looks like a **Keysight-style SCPI
instrument**. It is a single self-contained executable exposing two unrelated TCP links:

```
   UTS  ──SCPI text commands──▶  :5025  ┌──────────────────────────┐
                                        │        quickvib          │
                                        │  SCPI parser → engine    │
                                        │  → recording pipeline    │──▶  *.csv / *.txt
                                        │  → measurements          │
   M300 ──sample stream───────▶  :9123  └──────────────────────────┘
```

**QuickVib is the server on both links and never dials out.** This is the easiest thing to get
backwards: QuickVib does not connect to the vibrometer — the vibrometer dials in to the device port
QuickVib is listening on.

### 1.1 Two kinds of user

One executable serves two very different people. Work out which one you are first:

| | **UTS (automation)** | **Operator (manual)** |
| --- | --- | --- |
| How it starts | Launched as a child process by the UTS, with `--headless` | Double-clicked or run from a prompt, without `--headless` |
| Interface | No window; single-line structured log records on stdout | A Simplified-Chinese desktop window |
| How you drive it | SCPI text commands on port 5025 | Buttons: 打开 / 应用 / 开始录制 / 导出数据 |
| Build needed | `cargo build --release` | `cargo build --release --features gui` |
| Typical use | Line takt testing, regression runs | First-time project setup, on-site diagnosis, demos |
| See | [§6](#6-the-uts-scpi-workflow) | [§5](#5-operator-guide-to-the-window) |

Both share one project file and one instrument engine. A record duration the operator changes with
**应用 / Apply** is visible to a `CONF:REC:DUR?` arriving on port 5025 a moment later — they are
driving the same running engine instance.

### 1.2 What QuickVib is responsible for

Three things only: **record, measure, export**.

A typical job is: load a project → `INIT` starts a capture → `duration × sample rate` samples are
collected → the state reaches `COMPLETE` → read peak / RMS / peak-to-peak → optionally write the
whole waveform to a CSV or TXT file.

Pass/fail verdicts, DUT excitation and retry policy are **not** part of QuickVib — see
[§10](#10-out-of-scope).

---

## 2. Installation and build

There is no installer. You build from source and get standalone executables you can copy.

### 2.1 Prerequisites

| To… | You need |
| --- | --- |
| Build (any mode) | A stable Rust toolchain, edition 2021. The exact version is pinned in `rust-toolchain.toml` (currently **1.83**); `rustup` switches to it automatically |
| Run (mock / tcp modes) | Nothing. The executable is statically linked and needs no runtime |
| Run the desktop window | A build with `--features gui` and a desktop session. On Linux that means X11 or Wayland plus `libxkbcommon`, which every desktop install already has. **No Chinese system font is needed** — one is embedded |
| Run a real M300 | Windows x64 + `--features m300` + the vendor `m300_sdk.dll` + the VC++ redistributable — see [§2.5](#25-extra-prerequisites-for-a-real-m300) |
| Cross-compile a Windows exe from Linux | `gcc-mingw-w64-x86-64` and the `x86_64-pc-windows-gnu` target |

### 2.2 Building

```bash
cargo build --release
```

Three executables land in `target/release/`:

| File | Purpose |
| --- | --- |
| `quickvib` | The main program: the SCPI instrument |
| `m300-sim` | Simulator for `--backend tcp` ([§8.1](#81-m300-sim--for---backend-tcp)) |
| `m300-device-sim` | Simulator for `--backend m300` ([§8.2](#82-m300-device-sim--for---backend-m300)) |

Check the result:

```bash
./target/release/quickvib --version
# quickvib 1.0.0
```

`Cargo.lock` is committed, so add `--locked` for release builds (`cargo build --release --locked`):
it requires the lock file to be used exactly as committed and fails rather than updating it, which
is what makes two builds resolve to identical dependencies.

### 2.3 The two optional features

Both the window and the native M300 backend are **build-time switches**, off by default. The binary
shipped to the UTS therefore carries no graphics dependency and no Windows-only code at all.

| Command | What you get |
| --- | --- |
| `cargo build --release` | The command-line / SCPI instrument. **This is the UTS build** |
| `cargo build --release --features gui` | The above plus the Simplified-Chinese desktop window. **This is the operator build** |
| `cargo build --release --features m300` | The above plus the real M300 native backend (Windows only) |
| `cargo build --release --features gui,m300` | Everything — the full bench-station build (Windows only) |

A binary built *without* `--features gui` **has no window to open**: running it without
`--headless` prints the banner and serves from the console as usual rather than failing. A binary
built *with* it on a machine that has no display behaves the same way and says so.

`--features m300` compiles on non-Windows hosts (most of that crate is platform-neutral), but
*selecting* the backend there is refused — see [§3](#3-three-operation-modes).

### 2.4 Producing a Windows executable

**Route A — native MSVC build on Windows (this is the shipped artifact):**

```bat
rustup target add x86_64-pc-windows-msvc
set RUSTFLAGS=-C target-feature=+crt-static
cargo build --release --target x86_64-pc-windows-msvc --locked
:: -> target\x86_64-pc-windows-msvc\release\quickvib.exe
```

`+crt-static` links the MSVC C runtime statically, so the test host needs **no Visual C++
redistributable** — provided you are not using `--features m300`, because the vendor DLL brings its
own runtime requirement (next section).

**Route B — cross-compile from Linux with mingw-w64:**

```bash
sudo apt-get install -y gcc-mingw-w64-x86-64
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu --locked
# -> target/x86_64-pc-windows-gnu/release/quickvib.exe
```

This works because the dependency graph is pure Rust with no C build scripts. The resulting exe
runs under Wine for the mock path. It is a developer and CI convenience target; **route A is what
ships**.

### 2.5 Extra prerequisites for a real M300

This section only matters for `--backend m300`. All three conditions must hold:

1. **A Windows x64 host**
2. **A build with `--features m300`**
3. **The vendor `m300_sdk.dll`** (`x64\bin\m300_sdk.dll` from the `M300SDK_v1.2.0.zip` package)

The SDK is **not redistributed here** — request it from the instrument supplier. There is no fake
or stub DLL in this repository and none will be added.

**Where the DLL is looked for** (nothing is resolved at process start; the load happens on first
real use):

| Order | Location |
| --- | --- |
| 1 | The directory named by `device.sdkPath` in the project file |
| 2 | The directory named by the `QUICKVIB_M300_SDK` environment variable |
| 3 | The directory containing `quickvib.exe` |
| 4 | The process's default DLL search path |

The simplest arrangement is to **put `m300_sdk.dll` next to `quickvib.exe`** (rule 3).

**The VC++ runtime.** The vendor's DLL is an MSVC build and imports `VCRUNTIME140.dll` plus the
UCRT, so the host must have the **Visual C++ 2015–2022 redistributable (x64)** installed.
QuickVib's own `+crt-static` does not cover the vendor DLL — these are two separate things. Without
the runtime the DLL fails to load, which surfaces as `-241,"Hardware missing"` with a log line
naming every path probed and the OS error for each.

> **Bench status.** The M300 native backend is written but **not yet verified on real hardware**.
> The checklist is [`M300-NATIVE.md`](M300-NATIVE.md) §9. Until that run is recorded, treat
> `--backend m300` as untried.

---

## 3. Three operation modes

This is the **first decision** you make when using QuickVib: where the samples come from. Each
backend has a different notion of "device", so each needs a different stand-in — or none.

| Mode | Flag | Device-side stand-in | Wire | When to use it |
| --- | --- | --- | --- | --- |
| **Mock** | `--backend mock` | none | samples never leave the process | Development, CI, script bring-up, demos |
| **TCP wire** | `--backend tcp` | `m300-sim`, or a real M300 | bare little-endian `f32`, no framing | Rehearsing the real inbound link without the SDK |
| **M300 SDK** | `--backend m300` | `m300-device-sim` (SCZN), or a real M300 | SCZN frames | The production bench, real hardware |

Select with `--backend <mock|tcp|m300>` on the command line, or `"device": { "backend": "…" }` in
the project. The command line wins.

### 3.1 Mock mode (`--backend mock`)

**The default, and the only mode that needs nothing external.** It synthesizes a deterministic
signal in process from the project's sine components plus optional seeded Gaussian noise. Because
the peak / RMS / p-p of a synthesized sine are analytically known, it also serves as the oracle for
the measurement tests.

`SYST:DEV:CONN?` is **always `1`** here — the mock needs no link.

Good for: first bring-up of a UTS script, CI, demonstrating on a machine with no hardware.

### 3.2 TCP wire mode (`--backend tcp`)

QuickVib binds `--device-port` itself and records the little-endian `f32` stream pushed by
**whatever dials in**. That is exactly the format a real M300 pushes, so getting this mode working
means QuickVib's listener, framer and recording pipeline have all seen real bytes.

Without real hardware, `m300-sim` is the stand-in ([§8.1](#81-m300-sim--for---backend-tcp)).

Good for: exercising the socket path when you have no vendor SDK and no Windows machine.

### 3.3 M300 SDK mode (`--backend m300`)

This goes through the vendor's native SDK, and there is one difference you **must** know about:
**the device port does not belong to QuickVib.**

| | `mock` / `tcp` | `m300` |
| --- | --- | --- |
| Who binds `--device-port` | QuickVib | the SDK, inside `m300_server_create_ex` |
| What `--bind` does | the interface both listeners use | the SCPI interface, and the SDK's bind address |
| Sample path | listener → framer → engine | SDK callback → bounded queue → engine |
| Startup banner | `device on port 9123` | `device on port 9123 (bound by the SDK)` |
| What `SYST:DEV:CONN?` reports | the inbound link QuickVib accepted | the SDK's own `m300_device_is_connected` |
| `device.allowedPeers` | enforced | **not enforced** — use a firewall rule instead |
| Link counters in the shutdown summary | meaningful | always zero; trust `TRAC:POIN?` instead |
| Device port in the window's live-port row | the port number | a dash `—` |

QuickVib deliberately does **not** bind the device port here: both listeners would fight for the
same address, and whichever lost would report "address already in use" — which from the SDK
surfaces as `-7 ERR_NETWORK` at open.

Selecting this backend on a non-Windows host, or in a build without the `m300` feature, is a
**startup failure** (exit code `2`) and never a silent fallback to mock:

```console
$ ./target/release/quickvib --headless --backend m300 --project samples/Test.proj
2026-08-26T08:02:58.600Z INFO  proj    loaded path=samples/Test.proj name=Test
quickvib: backend 'm300' is unavailable: the M300 backend requires a Windows host
$ echo $?
2
```

### 3.4 Connecting a real M300 (bench checklist)

Working through this in order the first time you wire up real hardware:

1. **Install the SDK and the runtime.** Put `m300_sdk.dll` next to `quickvib.exe`, or point at it
   with the environment variable, and confirm the VC++ 2015–2022 redistributable is installed
   ([§2.5](#25-extra-prerequisites-for-a-real-m300)).

   ```bat
   set QUICKVIB_M300_SDK=C:\M300SDK_v1.2.0\x64\bin
   ```

2. **Work out which of this PC's addresses the vibrometer can reach.** The vibrometer is the side
   that dials in, so it has to be able to reach this machine. Use `--bind 127.0.0.1` when both are
   on the same box, and `--bind 0.0.0.0` (or a specific interface address) when the vibrometer is
   across the network.
3. **Open the firewall** for inbound TCP **5025** (the UTS) and **9123** (the vibrometer).
4. **Configure the vibrometer to dial this PC on port `9123`.**
5. **Start QuickVib:**

   ```bat
   target\release\quickvib.exe --headless --backend m300 --bind 0.0.0.0 ^
     --scpi-port 5025 --device-port 9123 --project samples\Test.proj
   ```

6. **Confirm the link** with `SYST:DEV:CONN?`. Only a `1` means the SDK has accepted the device; on
   a `0`, work through [§9.2](#92-the-device-link).

Note that `device.allowedPeers` is **not enforced** on this path — it is a capability of QuickVib's
own listener and the SDK has no equivalent — so restrict the link with a firewall rule instead.

---

## 4. Launch paths

### 4.1 Operator: open the desktop window

Build with the `gui` feature and run **without** `--headless`:

```bash
cargo build --release --features gui
./target/release/quickvib --project samples/Test.proj
```

```bat
:: Windows
cargo build --release --features gui
target\release\quickvib.exe --project samples\Test.proj
```

The window opens in **Simplified Chinese** on the **dark** theme. The SCPI and device servers move
to background threads and keep working — so while the window is open, a UTS can still connect to
port 5025. Closing the window shuts both servers down.

Operating detail is in [§5](#5-operator-guide-to-the-window).

### 4.2 UTS: headless

This is the invocation the UTS uses. Three of the four arguments are already the defaults, but all
are written out explicitly — in a test cell an explicit command line is one less thing to be
surprised by:

```bat
:: Windows, launched as a child process by the UTS
quickvib.exe --headless --scpi-port 5025 --device-port 9123 --project samples\Test.proj
```

```bash
# Linux / macOS
./target/release/quickvib --headless --scpi-port 5025 --device-port 9123 --project samples/Test.proj
```

A successful start puts these three lines on stdout (actual output):

```
2026-08-26T08:01:58.965Z INFO  proj    loaded path=samples/Test.proj name=Test
2026-08-26T08:01:58.965Z INFO  scpi    listening port=15025
2026-08-26T08:01:58.965Z INFO  device  listening port=19123
```

**The UTS should wait for the `scpi listening` line before connecting**, or retry the connection.

The process stays in the foreground until it is terminated, so the UTS can manage it as a child
process and kill it at the end. The normal in-band shutdown is `ABOR` followed by closing the SCPI
socket.

### 4.3 Command-line reference

This matches the output of `quickvib --help`.

| Argument | Value | Default | Behaviour |
| --- | --- | --- | --- |
| `--project <path>` | file path | *(auto-load-last)* | Load this project at startup. Failure is fatal (exit `4`) |
| `--scpi-port <n>` | 1–65535 | *(project, else `5025`)* | The port **the UTS connects in on**. Overrides `server.scpiPort` |
| `--device-port <n>` | 1–65535 | *(project, else `9123`)* | The port **the M300 connects in on**. Overrides `device.port`. Handed to the SDK under `--backend m300` |
| `--bind <host>` | IP literal or host name | *(project, else `0.0.0.0`)* | The interface **both** listeners bind. `127.0.0.1` keeps both links on this machine, which is right when the UTS and the vibrometer share a PC |
| `--headless` | flag | off | No window and no interactive console; structured log lines only |
| `--backend <mock\|tcp\|m300>` | enum | *(project, else `mock`)* | Override the project's backend — see [§3](#3-three-operation-modes) |
| `--no-auto-load` | flag | off | Suppress auto-load-last, for a clean run |
| `--log-level <level>` | enum | `info` | `trace` \| `debug` \| `info` \| `warn` \| `error` |
| `--version` | flag | — | Print the version and exit `0` |
| `--help` | flag | — | Print usage and exit `0` |

`--flag=value` and `--flag value` are both accepted, `--` terminates flag parsing, and the program
takes **no positional arguments at all** (`quickvib Test.proj` is rejected; write
`quickvib --project Test.proj`).

**About `--no-auto-load`.** QuickVib remembers the most recently loaded or saved project in a state
directory (`%LOCALAPPDATA%\QuickVib\` on Windows, `$XDG_STATE_HOME/quickvib/` on Unix) and
auto-loads it when started without `--project`. That is convenient for an operator and a hazard for
a UTS — **always name `--project` explicitly in UTS runs**, and add `--no-auto-load` when you want
a guaranteed-clean start.

### 4.4 Exit codes

| Code | Meaning | Usual cause |
| --- | --- | --- |
| `0` | Clean shutdown | — |
| `2` | Bad arguments | Unknown flag, missing value, out-of-range port, **and selecting `--backend m300` on a host that cannot provide it** |
| `3` | Port bind failure | Port already in use; a `--bind` host that does not resolve or is not an address on this machine |
| `4` | Project load failure | Only when `--project` was given: file missing or schema validation failed |
| `5` | Backend open failure | The backend itself would not initialise (for example the SDK refusing to create its server) |

Three real examples:

```console
$ ./target/release/quickvib --frobnicate
quickvib: unknown option '--frobnicate'
(full usage follows on stderr)
$ echo $?
2

$ ./target/release/quickvib --headless --scpi-port 15025 …   # that port is taken
quickvib: could not bind the SCPI listener on 127.0.0.1:15025: Address already in use (os error 98)
$ echo $?
3

$ ./target/release/quickvib --headless --project /nope/missing.proj
quickvib: could not load /nope/missing.proj: project file not found: /nope/missing.proj
$ echo $?
4
```

---

## 5. Operator guide to the window

This section is about the desktop window: a build with `--features gui`, started without
`--headless`. Every operator-visible string is Simplified Chinese, so the button names below are
given as they appear on screen, with the English translation the **中文 / EN** switch produces.

### 5.1 Layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ QuickVib  Test  [已连接] [录制状态 完成] [SCPI 15025 · 设备 19123] [浅色|深色] [中文|EN] │
├──────────────────────────────────────────────────────────────────────────────┤
│ 文件▼ 打开 保存 另存为 应用 还原                                    [配置|高级] │
├────────────────────────────────────────────┬─────────────────────────────────┤
│ 项目信息 · 设备与采样 · 滤波器 · 量程       │ 仪表面板                        │
│ 录制 · 通信端口 · 数据导出                  │  录制状态 / 连接状态            │
│                                            │  [开始录制] [停止]              │
│ (scrolling column of cards)                │  测量结果 峰值 / 有效值 / 峰峰值 │
│                                            │  样本数 · 时长 · 导出           │
│                                            │  链路信息                       │
├────────────────────────────────────────────┴─────────────────────────────────┤
│ ● 完成 · 已连接   录制已开始          实际监听端口: SCPI 15025 · 设备 19123   │
└──────────────────────────────────────────────────────────────────────────────┘
```

* **Title bar** — product name, the open project and whether it has unsaved edits, then three
  status chips (device link, record state, live ports), with the theme and language switches at
  the far right.
* **Left column** — scrolls; the whole project configuration, grouped into cards. What you edit
  here *is* the `.proj` file.
* **Right column** — fixed width; the instrument panel: state, `*IDN?`, session count, the
  start/stop buttons and the measurements of the last completed capture.
* **Status bar** — repeats the state, shows the outcome of the last action, and **always** names
  the ports this process is actually listening on.

The seven cards:

| Card | Fields |
| --- | --- |
| **项目信息 · Project** | Name, description |
| **设备与采样 · Device and sampling** | Sample rate (annotated in kHz as you type), data type (速度 μm/s / 位移 μm / 加速度 m/s²), backend (模拟 / TCP设备 / M300) |
| **滤波器 · Filters** | Low-pass and high-pass cutoff, with the hint 采样率与低通须同档. Unpinned, the low-pass follows Nyquist as the sample rate changes; tick 固定截止 to hold your own value |
| **量程 · Measuring ranges** | 速度量程 μm/s, 位移量程 μm, 加速度量程 m/s²; the one matching the selected data type is bold |
| **录制 · Recording** | Record duration in seconds |
| **通信端口 · I/O ports** | Read-only live ports on top, editable project ports below — see [§5.3](#53-live-ports-vs-project-ports) |
| **数据导出 · Export** | CSV or TXT, directory, CSV preamble, DC removal |

Everything the window does not show — the watchdog multiplier, the capture-size cap, the `*IDN?`
identity block, the session cap, `server.bindHost`, the allowed-peer list, the mock signal
definition — is **carried through untouched** on load, edit and save, so opening a project in the
window and saving it back never silently drops a field.

The **高级 / Advanced** tab is a placeholder. Laser power, TEC set point, PID gains and external
triggering are real device controls with no representation in the version-1 project schema, and
QuickVib does not invent settings the instrument cannot honour.

### 5.2 Open / Save / Apply / Revert

The difference between these four buttons is the single most important thing for an operator to
get right:

| Button | What it does | When to use it |
| --- | --- | --- |
| **打开 / Open** | Reads a `.proj` file from disk | Switching to a different test project |
| **应用 / Apply** | Validates the form and hands it to the **running engine** | **After every change — otherwise nothing takes effect** |
| **保存 / Save**, **另存为 / Save as** | Writes the `.proj` file (applying first) | To make a change survive a restart |
| **还原 / Revert** | Discards form edits and re-reads the engine's project | To back out of a mess |

**应用 / Apply is the button that matters.** It parses and validates every field, and on success
hands the edited project to the *running* engine — the very same instance the SCPI sessions
dispatch onto, so a `CONF:REC:DUR?` arriving on port 5025 a moment later answers with the duration
just typed. On failure **nothing is pushed**, and each rejected field is listed with its reason in
the window's language.

**Five settings need a restart**, because the process has already bound its sockets and opened its
backend:

1. The SCPI project port
2. The device project port
3. The backend
4. The sample rate
5. The data type

Apply **saves** these five and says explicitly which need a restart. That is not a failure, just a
different moment of taking effect.

> **Careful: loading a project discards the capture.** Both **打开 / Open** and **应用 / Apply**
> replace the engine's project, and a capture belongs to the project it was taken under. After a
> switch the record state returns to **空闲 / IDLE** and measurements and export answer "no data".
> **Export first, then switch.** A load attempted during a run is refused and changes nothing.

### 5.3 Live ports vs project ports

The project file stores `server.scpiPort` and `device.port`, but `--scpi-port` and `--device-port`
override them. So **the ports a running QuickVib is listening on are frequently not the ones in the
file.** The window never conflates the two:

| | **实际监听端口 / Live listening ports** | **项目端口（需重启）/ Project ports (restart required)** |
| --- | --- | --- |
| What it is | What this process actually bound | What is stored in the `.proj` file |
| Editable | Read-only | Editable |
| Shown where | Title bar, top of the 通信端口 card, status bar | Bottom of the 通信端口 card |
| In effect | Already | Next start |

**The UTS connects to the live ports.** When the two disagree — the ordinary case for
`quickvib --scpi-port 15025` against a project that says `5025` — the card says so explicitly
rather than letting the form look authoritative.

Under `--backend m300` the **device port shows a dash** in the live-port row. That is correct: that
row lists the sockets QuickVib bound, and on this backend the device port belongs to the SDK
([§3.3](#33-m300-sdk-mode---backend-m300)). Use the 项目端口 field and the startup banner instead.

### 5.4 Recording and exporting

1. Check that the title bar's connection chip reads **已连接 / Connected**. The mock backend is
   always connected; `TCP设备` and `M300` need the device to have actually dialled in.
2. Set the record duration in the **录制 / Recording** card and press **应用 / Apply**.
3. Press **开始录制 / Start** (the SCPI `INIT`). The state passes through **武装 / ARMED** and
   **录制中 / RECORDING**.
4. Once `duration × sample rate` samples have arrived the state becomes **完成 / COMPLETE** and the
   **峰值 / 有效值 / 峰峰值** readings on the right refresh.
5. For a waveform file, press **导出数据 / Export** (the SCPI `MMEM:STOR:TRAC`); the path resolves
   against the directory in the **数据导出 / Export** card.

**停止 / Stop** is `ABOR`: it aborts the run and the state becomes **已中止 / ABORTED**. **An
aborted capture is deliberately not fetchable** — measurement and export both answer "no data".

### 5.5 Theme and language

Both switches are at the far right of the title bar, both repaint on the next frame with no restart
and nothing to reload.

| Switch | Position | Default | Remembered in |
| --- | --- | --- | --- |
| **浅色 / 深色 · Light / Dark** | Left of the language switch | Dark | `ui-theme.txt` in the state directory |
| **中文 / EN** | Far right | Chinese | `ui-language.txt` in the state directory |

State directory: `%LOCALAPPDATA%\QuickVib\` on Windows, `$XDG_STATE_HOME/quickvib/` on Unix.
Deleting either file restores the default (dark, Chinese).

Dark is the right default for a test cell with the lights down and the laser running. The light
palette is not the dark one inverted: every accent is darkened until it clears a 4.5:1 contrast
ratio against the card it sits on, so it stays readable on a bright white bench. Only colours
change between the two — the layout, spacing and font are identical.

**About the font:** Chinese text needs a CJK font, so the crate embeds Noto Sans SC (SIL Open Font
License 1.1). Nothing is downloaded at build time and no system font is consulted, so a
freshly-imaged Windows test station renders exactly like the development machine. **No font
installation is required.**

---

## 6. The UTS SCPI workflow

### 6.1 Link parameters

| Item | Value |
| --- | --- |
| Transport | Raw TCP socket (**not** VISA / HiSLIP / VXI-11 / USBTMC) |
| Port | `--scpi-port`, default `5025` |
| Encoding | ASCII |
| Terminator | `\n` (`\r\n` tolerated) |
| Response | A single `\n`-terminated line |
| Case | Headers are case-insensitive |
| Compound messages | `;`-separated within one line, e.g. `*CLS;*IDN?` |
| Concurrent sessions | 8 by default (`server.maxSessions`) |

**Only queries — the ones ending in `?` — produce a response.** Commands produce nothing, and
waiting for a response to one will hang. This is standard instrument behaviour and the most common
trap when writing a UTS script.

### 6.2 One measurement, step by step

| Step | Command | Purpose | Required? |
| --- | --- | --- | --- |
| 1 | `*IDN?` | Confirm you reached QuickVib | Recommended |
| 2 | `*CLS` | Start from a clean error queue | Recommended |
| 3 | `MMEM:LOAD:STAT "<path>"` | Load the project | Skip if `--project` already did it |
| 4 | `SYST:ERR?` | Confirm step 3 succeeded | Recommended |
| 5 | `SYST:DEV:CONN?` | Confirm the device link is up | **Required** (except on mock) |
| 6 | `CONF:REC:DUR <seconds>` | Override the duration for this run | Optional |
| 7 | `FORM CSV` or `FORM TXT` | Choose the export format | Only if exporting |
| 8 | `INIT` | **Start the capture** (non-blocking, no response) | **Required** |
| 9 | `REC:WAIT?` | Block until the capture finishes | **Required** (or poll `REC:STAT?`) |
| 10 | `CALC:MEAS:ALL?` | Read peak, RMS, peak-to-peak | For the scalars |
| 11 | `MMEM:STOR:TRAC "<path>"` | Write the whole waveform to a file | For the waveform |
| 12 | `SYST:ERR?` | Confirm nothing went wrong | Recommended |

### 6.3 A complete session

This is a session **actually executed** against this repository (mock backend, duration set to
1.0 s). `>` is sent to QuickVib, `<` is received.

```
> *IDN?
< QuickVib,M300-SCPI,SN-0001,1.0.0

> *CLS                                    ← start from a clean error queue
> MMEM:LOAD:STAT "samples/Test.proj"      ← sample rate, unit, duration, identity
> SYST:ERR?
< 0,"No error"                            ← the load succeeded

> SYST:DEV:CONN?
< 1                                       ← always 1 on mock; real hardware must have dialled in

> CONF:REC:DUR 1.0                        ← override the duration for this run
> CONF:REC:DUR?
< 1.000                                   ← read-back is always three decimals
> FORM CSV
> FORM?
< CSV

> INIT                                    ← non-blocking, no response, the run has started
> REC:WAIT?                               ← blocks until the run finishes
< #REC:DONE                               ← unsolicited completion notification
< 1                                       ← REC:WAIT? unblocks with 1 = completed

> REC:STAT?
< COMPLETE

> TRAC:POIN?
< 100000                                  ← 1.0 s × 100 kS/s

> CALC:MEAS:ALL?
< 278.2689,179.0509,555.6500              ← peak, RMS, p-p, in the project's units

> MMEM:STOR:TRAC "/tmp/qvout/run001.csv"
> SYST:ERR?
< 0,"No error"

> BOGUS:CMD?                              ← unknown command: no response at all
> SYST:ERR?
< -113,"Undefined header"                 ← only SYST:ERR? reveals it
```

QuickVib's own log for the same run:

```
2026-08-26T08:02:01.034Z INFO  scpi    session opened peer=127.0.0.1:52206
2026-08-26T08:02:01.080Z INFO  proj    loaded path=samples/Test.proj name=Test
2026-08-26T08:02:01.169Z INFO  rec     started duration=1.000 expectedSamples=100000
2026-08-26T08:02:02.175Z INFO  rec     complete samples=100000 elapsed=1.006 peak=278.2689 rms=179.0509 pp=555.6500
2026-08-26T08:02:02.201Z INFO  export  wrote path=/tmp/qvout/run001.csv samples=100000 format=CSV
2026-08-26T08:02:02.217Z WARN  scpi    error -113,"Undefined header" (undefined header 'BOGUS:CMD?')
2026-08-26T08:02:02.265Z INFO  scpi    session closed peer=127.0.0.1:52206
```

A one-liner to check that a link is alive:

```bash
printf '*IDN?\n' | nc 127.0.0.1 5025
# QuickVib,M300-SCPI,SN-0001,1.0.0
```

### 6.4 Three ways to wait for completion

| Approach | Command | Character |
| --- | --- | --- |
| **Block** | `REC:WAIT?` | Simplest. `1` on completion, `0` on abort or timeout |
| **Poll** | `REC:STAT?` | Non-blocking, safe to call repeatedly. Returns `IDLE`\|`ARMED`\|`RECORDING`\|`COMPLETE`\|`ABORTED` |
| **Be notified** | `#REC:DONE` / `#REC:ABORT` | Pushed unsolicited to every open session, so no polling is needed |

`*OPC?` behaves like `REC:WAIT?` and also blocks until the active run finishes.

All three are bounded server-side by the watchdog (`duration × recording.timeoutMultiplier + 1 s`),
plus one extra second on the condvar wait, so **no wait can hang past
`duration × timeoutMultiplier + 2 s`**. Size the UTS socket timeout against that bound.

### 6.5 Notes for the UTS author

* **`INIT` is a command, not a query** — it produces no response. Waiting for one will hang.
* **Unknown commands return nothing** and only push `-113` onto the error queue. That is standard
  instrument behaviour; use `SYST:ERR?` to discover typos.
* **Lines beginning with `#` are unsolicited notifications, not query responses.** A client waiting
  on `REC:WAIT?` receives `#REC:DONE` before the response `1` (or `#REC:ABORT` before `0`). The
  response reader must recognise and skip `#` lines, or it will mistake a notification for a result.
* **`FETC?` is one very long line.** A 5 s × 100 kS/s capture is roughly 6 MB on a single line.
  Call `TRAC:POIN?` first and size the read buffer, use `CALC:MEAS:ALL?` if only the scalars are
  needed, or prefer `MMEM:STOR:TRAC` and read the file — safer than pulling 6 MB off a socket.
* **A capture starts at `INIT`.** Whatever arrived while the instrument was idle is discarded.
* **An aborted capture is not fetchable.** `FETC?`, `CALC:MEAS:*?` and `MMEM:STOR:TRAC` all answer
  `-230`.
* **Loading a project discards the capture** — see the note in [§5.2](#52-open--save--apply--revert).
  **Export before you load.**
* **The error queue is 32 deep.** On overflow the newest entry is replaced by `-350`, so the last
  error read after a burst always says that errors were lost. When diagnosing, **drain `SYST:ERR?`
  until it returns `0,"No error"`**.
* **Export is atomic.** Writes go to a temporary file in the target directory and are renamed into
  place, so a UTS polling for the file never sees a half-written one.

### 6.6 Command quick reference

The full table is in [`SCPI.md`](SCPI.md).

| Command | Type | Example response | Notes |
| --- | --- | --- | --- |
| `*IDN?` | Query | `QuickVib,M300-SCPI,SN-0001,1.0.0` | Manufacturer, model, serial, firmware |
| `*RST` | Command | — | Abort, discard the capture, restore project values, clear the error queue |
| `*CLS` | Command | — | Clear only the error queue and any armed `*OPC` |
| `*OPC?` | Query | `1` | Blocks until the active run finishes |
| `SYST:ERR?` | Query | `0,"No error"` | Pop the oldest error |
| `SYST:VERS?` | Query | `1999.0` | SCPI standard version |
| `SYST:DEV:CONN?` | Query | `1` / `0` | Whether a device link is up |
| `MMEM:LOAD:STAT "<path>"` | Command | — | Load a project |
| `MMEM:STOR:STAT "<path>"` | Command | — | Save the current project, runtime overrides included |
| `MMEM:STOR:TRAC "<path>"` | Command | — | Export the last completed capture in the active `FORM` |
| `CONF:REC:DUR <seconds>` | Command | — | Record duration, range `(0, 3600]` |
| `CONF:REC:DUR?` | Query | `5.000` | Three decimals |
| `FORM CSV\|TXT` | Command | — | Export format |
| `INIT` | Command | — | Start a run (aliases `REC:STAR`, `INIT:IMM`) |
| `ABOR` | Command | — | Abort the run; a no-op when nothing is running |
| `REC:STAT?` | Query | `COMPLETE` | Current state, safe to poll |
| `REC:WAIT?` | Query | `1` / `0` | Blocks until the run ends |
| `FETC?` | Query | `v1,v2,…,vN` | The whole capture (alias `TRAC:DATA?`) |
| `TRAC:POIN?` | Query | `500000` | Sample count |
| `CALC:MEAS:PEAK?` | Query | `12.3400` | `max(abs(x))` |
| `CALC:MEAS:RMS?` | Query | `4.5600` | Root mean square |
| `CALC:MEAS:PP?` | Query | `24.6800` | `max − min` |
| `CALC:MEAS:ALL?` | Query | `12.3400,4.5600,24.6800` | Peak, RMS, peak-to-peak |

The state machine:

```
IDLE ──INIT──▶ ARMED ──first sample──▶ RECORDING ──expected count──▶ COMPLETE
                 │                          │
                 └──── ABOR / timeout ──────┴────────▶ ABORTED
*RST returns to IDLE from any state. ABOR is a no-op once COMPLETE.
```

### 6.7 Error codes

| Code | Message | Typical cause and fix |
| --- | --- | --- |
| `0` | `No error` | Queue empty |
| `-100` | `Command error` | Malformed message, over-long or unterminated line, non-UTF-8 input |
| `-113` | `Undefined header` | Unknown command — a typo or an unsupported spelling |
| `-221` | `Settings conflict` | No project loaded, already recording, or a configuration change during a run |
| `-222` | `Data out of range` | Duration outside `(0, 3600]`, or `duration × rate × 4 bytes` over `recording.maxCaptureBytes` |
| `-224` | `Illegal parameter value` | Bad `FORM` value, or an invalid project file |
| `-230` | `Data corrupt or stale` | A data query with no completed capture. Aborted runs are deliberately not fetchable |
| `-240` | `Hardware error` | The device link dropped mid-run. The state becomes `ABORTED`; re-arm with `INIT` |
| `-241` | `Hardware missing` | No device at `INIT` — see [§9](#9-troubleshooting) |
| `-256` | `File name not found` | Project file missing |
| `-257` | `File name error` | Path invalid or not writable |
| `-350` | `Queue overflow` | More than 32 unread errors; replaces the newest entry |
| `-365` | `Time out error` | The watchdog fired: samples stopped before the expected count |

### 6.8 Export formats

`FORM CSV|TXT` selects the format and `MMEM:STOR:TRAC "<path>"` writes the file. Relative paths
resolve against `export.directory`, missing directories are created, and existing files are
overwritten.

**CSV** — optional comment preamble, a header row, then one row per sample, CRLF line endings.
This is the real start of the file written by the 1 s capture above:

```csv
# project=Test
# timestamp=2026-08-26T08:02:02.175Z
# sampleRateHz=100000
# unit=um/s
# samples=100000
# durationSeconds=1
index,time_s,value
0,0,40.759865
1,0.00001,44.48746
2,0.00002,46.231735
```

`# unit=` carries the unit label itself (`um/s`, `um`, `m/s^2`), not the schema's enum name. The
preamble is controlled by `export.includeHeader`.

**TXT** — one value per line and nothing else, for scripts that just want numbers:

```text
40.759865
44.48746
46.231735
```

Values use Rust's shortest round-tripping float formatting, which is **locale-independent by
construction**: a decimal point is always a `.`, never a comma.

### 6.9 Measurement definitions

| Metric | Definition | Query |
| --- | --- | --- |
| Peak | `max(abs(xᵢ))` | `CALC:MEAS:PEAK?` |
| RMS | `sqrt( (1/N) · Σ xᵢ² )` | `CALC:MEAS:RMS?` |
| Peak-to-peak | `max(xᵢ) − min(xᵢ)` | `CALC:MEAS:PP?` |

Computed in one pass, accumulated in `f64` even though samples are `f32`, so a several-hundred-
thousand-sample capture loses no precision. `measurement.removeDc` optionally subtracts the mean
before peak and RMS; peak-to-peak is unaffected by DC removal by definition.

Units follow the project's channel unit — velocity **μm/s**, displacement **μm**, acceleration
**m/s²**. **QuickVib performs no unit conversion**: it reports samples in the unit the device is
configured for and labels them accordingly. Decimal places come from
`measurement.responseDecimals`, 4 by default.

---

## 7. The project file

A project is a JSON file (conventionally `.proj`) describing everything one test needs. Unknown
properties are ignored for forward compatibility.

### 7.1 The complete sample

This is `samples/Test.proj` verbatim:

```json
{
  "schemaVersion": 1,
  "name": "Test",
  "description": "Sample QuickVib project: 5 s velocity capture, CSV export.",
  "device": {
    "backend": "mock",
    "port": 9123,
    "sampleRateHz": 100000,
    "unit": "velocity_um_s",
    "allowedPeers": [],
    "connectTimeoutSeconds": 30,
    "lpfHz": 50000,
    "highPassHz": 0,
    "velocityRange": 1000,
    "displacementRange": 1000,
    "accelerationRange": 100
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
  "identity": {
    "manufacturer": "QuickVib",
    "model": "M300-SCPI",
    "serialNumber": "SN-0001",
    "firmwareVersion": "1.0.0"
  },
  "server": {
    "maxSessions": 8,
    "scpiPort": 5025
  },
  "mock": {
    "signal": {
      "components": [
        { "frequencyHz": 120.0, "amplitude": 250.0, "phaseDeg": 0.0 },
        { "frequencyHz": 360.0, "amplitude": 40.0, "phaseDeg": 90.0 }
      ],
      "noiseStdDev": 2.5,
      "seed": 12345
    }
  }
}
```

### 7.2 Fields

| Field | Default | Notes |
| --- | --- | --- |
| `schemaVersion` | *(required)* | Only `1` exists |
| `name` / `description` | *(required / optional)* | The name goes into the CSV preamble |
| `device.backend` | `"mock"` | `"mock"` \| `"tcp"` \| `"m300"` — see [§3](#3-three-operation-modes) |
| `device.port` | `9123` | Inbound port the vibrometer dials; `--device-port` overrides |
| `device.sampleRateHz` | *(required)* | e.g. `100000` |
| `device.unit` | *(required)* | `"velocity_um_s"` \| `"displacement_um"` \| `"acceleration_m_s2"` |
| `device.allowedPeers` | `[]` | Empty accepts any inbound peer. **Not enforced** under `--backend m300` |
| `device.connectTimeoutSeconds` | `30` | How long to wait for the device to dial in |
| `device.sdkPath` | `null` | Directory probed for the M300 SDK; M300 backend only |
| `device.lpfHz` | `null` | Low-pass cutoff; `null` tracks Nyquist (`sampleRateHz / 2`) |
| `device.highPassHz` | `0` | High-pass cutoff; `0` disables. **Must be below the low-pass cutoff** |
| `device.velocityRange` | `1000` | Velocity range, µm/s |
| `device.displacementRange` | `1000` | Displacement range, µm |
| `device.accelerationRange` | `100` | Acceleration range, m/s² |
| `recording.durationSeconds` | *(required)* | Range `(0, 3600]` |
| `recording.timeoutMultiplier` | `2.0` | Watchdog = duration × this + 1 s |
| `recording.maxCaptureBytes` | `536870912` | `INIT` rejects larger runs with `-222` |
| `measurement.removeDc` | `false` | Subtract the mean before peak and RMS |
| `measurement.responseDecimals` | `4` | Decimals in `CALC:*` responses, `0..=9` |
| `export.format` | `"CSV"` | `"CSV"` or `"TXT"` |
| `export.directory` | `"."` | Base for relative export paths |
| `export.includeHeader` | `true` | Whether to write the CSV `#` preamble |
| `identity.*` | see sample | The four `*IDN?` fields, so an existing UTS ID check can be satisfied |
| `server.maxSessions` | `8` | Concurrent SCPI session cap |
| `server.scpiPort` | `5025` | SCPI listen port; `--scpi-port` overrides. **Must differ from `device.port`** |
| `server.bindHost` | `"0.0.0.0"` | Interface both listeners bind; `--bind` overrides. A host name is resolved at bind time, so a name that does not resolve is exit `3` |
| `mock.signal.components[]` | one 100 Hz component | Sine components: `frequencyHz`, `amplitude`, `phaseDeg` |
| `mock.signal.noiseStdDev` | — | Standard deviation of the added Gaussian noise |
| `mock.signal.seed` | — | PRNG seed; fix it to reproduce the same waveform |

### 7.3 Filters and ranges

**Low-pass `lpfHz`.** Leave it `null` and it follows the Nyquist frequency (half the sample rate),
which is almost always what you want. In the window this is the 固定截止 checkbox: unticked means
follow, ticked means hold the value you typed. On the vendor's hardware the sample rate and the
low-pass are **paired rungs** and cannot be combined freely — that is what the window's hint
采样率与低通须同档 is telling you.

**High-pass `highPassHz`.** `0` disables it. If you set it, it must be **below** the low-pass
cutoff or the project fails validation. To remove a DC component there are usually two options: a
very low high-pass, or `measurement.removeDc: true` — the latter subtracts the mean when computing
measurements and does not change the exported waveform.

**The three ranges.** `velocityRange`, `displacementRange` and `accelerationRange` all exist at
once, but only the one matching `device.unit` actually applies; the window shows the active one in
bold. All three are kept so that switching data type does not lose the configuration.

### 7.4 Auto-load-last

The most recently loaded or saved project path is remembered (`%LOCALAPPDATA%\QuickVib\` on
Windows, `$XDG_STATE_HOME/quickvib/` on Unix) and auto-loaded at startup when `--project` is not
given.

* Operator: convenient — pick up where you left off.
* UTS: **always name `--project` explicitly**, and add `--no-auto-load` when you need a guaranteed
  clean start.

The SCPI equivalents are `MMEM:LOAD:AUTO` (load it) and `MMEM:LOAD:AUTO?` (ask which path that
would be).

---

## 8. The two simulators

You can exercise the whole chain without real hardware. **But the two simulators are not
interchangeable** — point one at the other's backend and you get a link that connects and then does
nothing useful.

| Backend | Which simulator | Wire | Needs |
| --- | --- | --- | --- |
| `--backend mock` | *(none needed)* | samples never leave the process | nothing |
| `--backend tcp` | **`m300-sim`** | bare little-endian `f32`, no framing | nothing |
| `--backend m300` | **`m300-device-sim`** | SCZN frames | Windows, `--features m300`, the vendor DLL |

Neither is a fake DLL: both are ordinary TCP clients. On the `m300` path the thing parsing the
protocol is the vendor's real SDK.

### Start order: QuickVib first, then the simulator

**No exceptions.** QuickVib (or the SDK) is the server, the simulator is the client, and the server
must already be listening. Get it backwards and `m300-sim` exits immediately with code `3`, while
`m300-device-sim` keeps retrying at its `--retry` interval until it succeeds.

### 8.1 `m300-sim` — for `--backend tcp`

The mock backend is convenient but it short-circuits the transport: samples go straight to the
engine and nothing on the socket path is exercised. `--backend tcp` closes that gap by recording
the little-endian `f32` stream arriving on `--device-port`, which is exactly what a real M300
pushes; `m300-sim` is the stand-in that dials in and streams a sine.

```bash
# terminal 1 — the instrument (start this first)
./target/release/quickvib --headless --backend tcp --device-port 9123 --project samples/Test.proj

# terminal 2 — the "M300"
./target/release/m300-sim --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

```bat
:: Windows
quickvib.exe --headless --backend tcp --device-port 9123 --project samples\Test.proj
m300-sim.exe --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

Then drive it over SCPI as usual — the numbers that come back are measurements of bytes that
crossed a socket. Here is an **actual run**, including the behaviour before the device dials in:

```
--- before the device dials in ---
> SYST:DEV:CONN?
< 0
> INIT
> SYST:ERR?
< -241,"Hardware missing"                 ← INIT with no device always gives this

--- after starting m300-sim ---
> SYST:DEV:CONN?
< 1
> CONF:REC:DUR 1.0
> INIT
> REC:WAIT?
< #REC:DONE
< 1
> TRAC:POIN?
< 100000
> CALC:MEAS:ALL?
< 250.0000,176.7767,500.0000              ← a pure sine of amplitude 250: peak 250, RMS 250/√2
```

**Arguments:**

| Argument | Value | Default | Behaviour |
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

**Things worth knowing:**

* **Only one device link is honoured at a time**; a second is refused immediately.
* **A capture starts at `INIT`**; whatever arrived while the instrument was idle is discarded.
* **Rates should match.** `--rate` should equal the project's `device.sampleRateHz`. A slower
  simulator makes captures take proportionally longer and can trip the watchdog; a faster one just
  means some samples are never recorded.
* Unplugging mid-run aborts with `-240,"Hardware error"`.

### 8.2 `m300-device-sim` — for `--backend m300`

`--backend m300` is the one path `m300-sim` cannot reach. There the SDK owns the socket, and what
it expects to hear is not bare samples but **SCZN**: a framed request/response protocol in which
the host sends `0x00` to start a capture and the device answers, then uploads `0x04` sample blocks
until it is told to stop. `m300-device-sim` is a device that speaks it; the protocol itself is
[`SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md).

It needs a Windows host with the vendor DLL, because the thing on the other end of the socket is
the real SDK:

```bat
:: terminal 1 — the instrument, with the SDK binding the device port (start this first)
quickvib.exe --headless --backend m300 --device-port 9123 --project samples\Test.proj

:: terminal 2 — the "M300"
m300-device-sim.exe --host 127.0.0.1 --port 9123 --rate 100000 --amplitude 250 --frequency 120
```

The simulator itself is platform-neutral and builds and runs on Linux too; without the SDK there is
simply nothing for it to talk to except the test suite's own host stand-in.

**Arguments:**

| Argument | Value | Default | Behaviour |
| --- | --- | --- | --- |
| `--host <host>` | host or IP | `127.0.0.1` | Where the SDK's listener is |
| `--port <n>` | 1–65535 | `9123` | The port it bound |
| `--rate <hz>` | > 0 | `100000` | Samples per second, paced against real time |
| `--amplitude <a>` | finite | `250` | Peak amplitude, in the unit the data type implies |
| `--frequency <hz>` | ≥ 0 | `120` | Tone frequency; `0` sends a flat line |
| `--data-type <kind>` | `velocity` \| `displacement` \| `acceleration` \| `iq` | `velocity` | What the device reports uploading |
| `--sn <serial>` | ≤ 10 ASCII | `SIM-000001` | Serial in the hardware-information blob, which `*IDN?` reports |
| `--block <n>` | 1–262144 | `4096` | Samples per `0x04` upload |
| `--crc <mode>` | `standard` \| `castagnoli` \| `zero` | `standard` | Which checksum to send — see below |
| `--prefix-endian <e>` | `big` \| `little` | `big` | Byte order of the upload payload's two prefix words |
| `--retry <s>` | > 0 | `1` | Seconds between dial attempts |
| `--once` | flag | *(redial forever)* | Exit when the link drops |
| `--duration <s>` | > 0 | *(until interrupted)* | Stop the whole run after this long |
| `--version` / `--help` | flag | — | Print and exit `0` |

Exit codes: `0` the run ended after at least one session, `2` bad arguments, `3` it ended without
ever connecting.

**Things worth knowing:**

* **It is quiet after connecting; samples wait for `0x00`.** Unlike `m300-sim`, which streams the
  moment it connects, this one is idle until the host starts it and stops dead when the host sends
  `0x02`. That is what the protocol says, and it is what makes `INIT` on the QuickVib side mean
  something on the wire. **So "connected but no data" before `INIT` is normal.**
* **Rate and filter are enum indices, not hertz.** `--rate 100000` becomes rung `0x05` plus the
  matching 100 kHz low-pass band, since the vendor insists the two be set together. A rate off the
  ladder is reported as the nearest rung while pacing stays at what was asked for — a deliberate
  way to reproduce a device/project mismatch.
* **`--crc` exists because the vendor specification contradicts itself.** It prints a C table for
  standard CRC-32 and a Python example that can only have produced CRC-32C. The device sends the
  former by default and accepts all three variants on the way in. **If the SDK answers
  `-4 ERR_CRC_MISMATCH`, try `--crc castagnoli` and then `--crc zero`.**
  [`SCZN-PROTOCOL.md`](SCZN-PROTOCOL.md) §7 has the detail.
* **Firmware upgrade is refused, not ignored.** `0xF8`–`0xFF` get a failure reply in the documented
  shape, so the host's error handling runs and the link stays usable.
* **It redials on its own**, as a real device does, and comes back **idle** — an acquisition does
  not survive the link that started it.

---

## 9. Troubleshooting

### 9.1 Startup

| Symptom | Cause and fix |
| --- | --- |
| Exit `3`, `Address already in use` | A port is taken — usually another QuickVib instance, or something else on `5025` / `9123`. Pick a free port with `--scpi-port` / `--device-port`. Find the holder with `netstat -ano \| findstr :5025` on Windows or `ss -ltnp \| grep 5025` on Linux |
| Exit `3` with no port conflict | The `--bind` or `server.bindHost` host name does not resolve, or is not an address on this machine. The message names the address that was attempted |
| Exit `2`, `unknown option` | A misspelled flag, or a value passed positionally. The project path must be written `--project <path>` |
| Exit `2`, `backend 'm300' is unavailable` | The M300 backend was selected on a non-Windows host or in a build without the `m300` feature. This is deliberate — there is no silent fallback to mock |
| Exit `4` | The explicitly named `--project` file is missing or fails schema validation. The log carries the JSON line and column |
| Exit `5` | The backend itself would not initialise. On the M300 path the usual case is `-7 ERR_NETWORK` — see the next row |
| `-7 ERR_NETWORK` at startup with `--backend m300` | Something else already holds the device port, so the SDK could not bind it. The usual culprits are a second QuickVib instance and a leftover `--backend tcp` run. Note that QuickVib itself does **not** bind that port on this path |
| No window appears | Three possibilities: the binary was built without `--features gui`; `--headless` was passed; the host has no display. The last two print a line saying so and fall back to the console. **Both servers run either way** |

### 9.2 The device link

| Symptom | Cause and fix |
| --- | --- |
| `SYST:DEV:CONN?` returns `0` | The device has not dialled in. Check each of: the device is powered; it is on the same network; it is configured to connect to this host on `9123`; no firewall blocks the inbound connection; `device.allowedPeers` (if non-empty) includes its address. Under `--backend mock` this is always `1`. Under `--backend m300` it is the SDK's own judgement and the peer allow-list is not involved |
| `INIT` returns `-241,"Hardware missing"` | Either **(a)** no device is connected — check `SYST:DEV:CONN?` first; or **(b)** on the M300 path only, the SDK DLL could not be loaded. The log for (b) **lists every path probed with the OS error for each**; check the DLL location and the VC++ runtime against [§2.5](#25-extra-prerequisites-for-a-real-m300) |
| A simulator connects but no samples arrive | Most likely the wrong simulator for the backend. `--backend tcp` needs `m300-sim` (bare `f32`); `--backend m300` needs `m300-device-sim` (SCZN). Either will **connect** to either, and then nothing useful happens |
| `m300-device-sim` connects but stays silent | **Correct** until `INIT` — it waits for the host's `0x00`. If `INIT` has run and blocks still are not arriving, check the log for `-4 ERR_CRC_MISMATCH` and try `--crc castagnoli` or `--crc zero` |
| `m300-sim` exits `3` | Nothing is listening on `--port`. **Start QuickVib first**, and check that its `--device-port` is the port the simulator is dialing |
| `-240,"Hardware error"` mid-run | The device link dropped during `ARMED` or `RECORDING`. The run moves to `ABORTED`; re-arm with `INIT` |

### 9.3 Capture and data

| Symptom | Cause and fix |
| --- | --- |
| `-365,"Time out error"` | The watchdog fired: samples stopped before the expected count. On `--backend tcp` the usual cause is the simulator's `--rate` being below the project's `device.sampleRateHz`. Match the two, or raise `recording.timeoutMultiplier` |
| `-230,"Data corrupt or stale"` | A data query with no completed capture. Check three things: that `INIT` really ran; that the run was not aborted (**aborted captures are deliberately not fetchable**); and that no project was loaded before the fetch (**loading discards the capture**) |
| `-222,"Data out of range"` | Duration outside `(0, 3600]`, or `duration × sampleRateHz × 4 bytes` over `recording.maxCaptureBytes` (512 MiB by default) |
| `-221,"Settings conflict"` | No project loaded, already recording, or a configuration change attempted during a run. `ABOR` first or wait for completion |
| A command seems ignored | Unknown commands return nothing **by design**. Read `SYST:ERR?` — `-113,"Undefined header"` means a typo or an unsupported spelling |
| `FETC?` truncates or times out on the UTS | The response is one very long line (≈6 MB for 5 s × 100 kS/s). Call `TRAC:POIN?` first and size the read buffer, or export with `MMEM:STOR:TRAC` and read the file |
| The measurements look wrong | Check that the project's `device.unit` matches what the device actually outputs — **QuickVib performs no unit conversion**. Also check whether `measurement.removeDc` is what you intended |
| The data vanished after switching projects | By design: a capture belongs to its project, and `MMEM:LOAD:STAT`, `MMEM:LOAD:AUTO` and the window's **应用 / Apply** all discard it. **Export first, then switch** |

### 9.4 The window

| Symptom | Cause and fix |
| --- | --- |
| An edit did not reach the instrument | Only **应用 / Apply** pushes the form into the running engine. And the ports, backend, sample rate and data type need a restart even after Apply, which says so explicitly |
| The window shows a port the UTS cannot connect to | Read **实际监听端口 / Live listening ports** in the status bar, not the editable project-port fields: the command line overrides the project file, and the window labels the two separately for exactly this reason ([§5.3](#53-live-ports-vs-project-ports)) |
| The window is in Chinese and you want English | The **中文 / EN** switch is at the top right of the title bar. The choice is remembered in `ui-language.txt` in the state directory |
| The dark window is unreadable on a bright bench | The **浅色 / 深色 · Light / Dark** switch is immediately left of the language switch. The choice is remembered in `ui-theme.txt` |
| The device port shows a dash under `--backend m300` | **Correct.** The live-port row lists the sockets QuickVib bound, and on this backend the device port is the SDK's. Use the project-port field and the startup banner |

### 9.5 When reporting a problem, include

1. The full `--headless` log output (add `--log-level debug` if needed).
2. The result of **draining `SYST:ERR?` until it returns `0,"No error"`** — the queue is a FIFO, so
   reading one entry misses everything that happened earlier.
3. The project file in use.
4. The exact command line, and the output of `quickvib --version`.

---

## 10. Out of scope

QuickVib deliberately **does not** do the following. These are design decisions, not missing
features:

* **No pass/fail evaluation.** No limits, no verdicts, no tolerance bands. QuickVib returns
  numbers; **the UTS judges them**.
* **No DUT vibration control.** No shaker or exciter drive. The mock's synthesized signal is a test
  fixture, **not** DUT excitation.
* **No retry logic.** A failed or aborted capture is reported faithfully; re-running it is the
  UTS's decision.
* **No fake native DLL.** Linux testability comes from the mock backend and from `m300-sim`, a
  plain socket client — not from faking the vendor library.

Also out of scope for v1: real-time plotting, FFT / spectral analysis, multi-channel capture,
VISA / HiSLIP / VXI-11 transports (**raw socket only**), USBTMC, and triggering beyond immediate
`INIT`.

The desktop window is a front end over the same project schema and the same engine — it **adds no
behaviour the SCPI surface does not already have**, and it is never opened under `--headless`.
