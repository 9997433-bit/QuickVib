# QuickVib Round-1 审计报告 — 架构 / 完成度 / SOTA 差距

- **审计代理**: R1-fable-A (cloud), MODEL_SLUG: claude-fable-5-thinking-xhigh
- **审计对象**: 分支 `cursor/review-optimize-f6c8`(基于 `cursor/build-quickvib-3de8` / PR #1),commit `5895498`
- **审计性质**: 只读审计,未改动任何产品 crate
- **基线验证**: `cargo test --workspace --locked` 全绿(495 项测试通过,0 失败);`cargo clippy --workspace --all-targets --locked` 本地复跑无警告

> **并发提交说明**:本报告审计的是 commit `5895498`。在报告提交时,共享分支上已出现兄弟代理的并发提交:`064d172`(*fix: drop Complete after adopt_project invalidates capture* —— 正是本报告 §3 确认的缺陷 #1 的修复)与 `1a6db07`(*fix: reject duplicate ports and stop mock false-positive faults*)。§3 的缺陷分析描述的是修复前的基线状态,仍可作为该修复的评审依据;§5 的 P1 项据此可视为"已有候选修复待评审"而非"待实现"。

## 0. 结论速览(TL;DR)

1. **PLAN.md §25 的自述与代码事实相符**:Phase 0–5 全部落地,mock 路径端到端可用;Phase 8(GUI)虽在 §25 中未提及,但已按 §19 的 "Landed" 描述实现并有充分测试。Phase 6(M300 原生后端)按 D21/§21.1 有意推迟,推迟方式干净(占位 crate、无假 DLL、`--backend m300` 显式报错)。Phase 7 是唯一 **PARTIAL** 的阶段:双语 README 和 release profile 已就绪,但 `docs/SCPI.md`、UTS 示例转录文档、`cargo deny` 门禁、tagged release 均缺。
2. **R1–R23 中,除 2 项按计划排除(R7/R21)、1 项按计划推迟(R3)、1 项传输层就绪但 M300 侧随 Phase 6 推迟(R4)外,其余 19 项全部 IMPLEMENTED**,均有代码与测试证据(见 §1 表)。
3. **已知缺陷确认为真,且波及面比报告的更广**:`Engine::adopt_project` 清空 `result`/`outcome` 但不复位 `state`。不止 GUI Apply — SCPI `MMEM:LOAD:STAT`(经 `load_project`)同样触发;受影响的可观测面包括 `REC:STAT?`(误报 `COMPLETE`)、`REC:WAIT?`(误报 `1`)、GUI 状态徽标(显示"完成"而测量卡显示"尚无完成的采集"),同时 `FETC?`/`CALC:MEAS:*?`/`MMEM:STOR:TRAC` 一致地返回 `-230`。详见 §3。
4. **最大的 SOTA 差距是 IEEE 488.2 通用命令完备性**:全仓只实现了 `*IDN? *RST *CLS *OPC *OPC?` 五条(这与 PLAN §8 的承诺一致,所以 R15 按计划算达标),但 488.2 强制的 `*ESE/*ESE?/*ESR?/*SRE/*SRE?/*STB?/*TST?/*WAI` 缺失,OPC 位有内部记账却无从线上读取。详见 §4。
5. Round-2 必修项:缺陷 #1 状态归位、`docs/SCPI.md`、488.2 强制命令最小集。排序见 §5。

---

## 1. 需求追踪:PLAN §3 R1–R23 逐条对照

判定口径:**IMPLEMENTED** = 代码 + 测试可证;**PARTIAL** = 部分落地;**INTENTIONALLY DEFERRED** = PLAN 明文推迟且推迟方式被遵守;**EXCLUDED(遵守)** = PLAN 明文排除且代码确实没做。

| # | 需求 | 判定 | 证据 |
| --- | --- | --- | --- |
| R1 | Windows 宿主可执行文件,`cmd` 启动 | **IMPLEMENTED** | `crates/quickvib/src/main.rs`(入口、退出码)、`crates/quickvib/src/cli.rs`(手写参数解析);CI `x86_64-pc-windows-gnu` 交叉构建产出 `quickvib.exe`(`.github/workflows/ci.yml`);PLAN §25 自述交叉构建绿 |
| R2 | UTS 经 SCPI-over-TCP 驱动(Keysight 风格) | **IMPLEMENTED** | `crates/quickvib/src/scpi_server.rs`(TCP 会话服务器)+ `quickvib-scpi` 全链路(`lexer.rs`/`tree.rs`/`session.rs`/`format.rs`)+ `quickvib-engine/src/dispatch.rs`(穷举 `match`);回环集成测试 `crates/quickvib/tests/{protocol,happy_path,sessions,errors,recording,binary}.rs` |
| R3 | 经 `bindgen`/`libloading` 封装 M300 SDK v1.2.0 (C ABI) | **INTENTIONALLY DEFERRED**(Phase 6,受 §21.1 门禁) | `crates/quickvib-m300/src/lib.rs` 为占位:无 `extern "C"`、无 `unsafe`、无 SDK 调用;`Cargo.toml` 保留 Windows-only + `bindgen` feature 接线;`--backend m300` 在 `crates/quickvib/src/backend_factory.rs` 中是启动错误而非静默回退,符合 D21 |
| R4 | 宿主为 TCP 服务器;M300 **入站**连接 `9123` | **PARTIAL(传输层 IMPLEMENTED,M300 侧随 Phase 6 推迟)** | 入站设备服务器与端口默认值已全部落地:`crates/quickvib/src/device_server.rs`、`quickvib-device/src/listener.rs`(含对端白名单)、schema 默认端口 9123;TCP 后端全链路已测(`tests/tcp_backend.rs`、`tests/device_link.rs`)。真实 M300 接入这条链路的最后一步属于 Phase 6 |
| R5 | 数据流为 little-endian `f32` | **IMPLEMENTED** | `quickvib-device/src/framer.rs`:任意 `Read` 源上的 LE `f32` 分帧,含块边界穷举测试(1 字节一切、跨块半个样本等) |
| R6 | 单位:速度 μm/s、位移 μm、加速度 m/s² | **IMPLEMENTED** | `quickvib-core/src/unit.rs`(`SampleUnit` 三变体,ASCII 导出拼写);UI 侧 `quickvib-ui/src/i18n.rs` `unit_symbol`(μm/s、μm、m/s²)与 `unit_label`(速度/位移/加速度) |
| R7 | DUT 激励 / pass-fail / 重试归 UTS,不归 QuickVib | **EXCLUDED(遵守)** | 全仓无 pass/fail 判定、无 DUT 激励逻辑;`quickvib-ui` Advanced 页明文说明"QuickVib 不会凭空造出仪器无法执行的设置" |
| R8 | 加载项目(JSON) | **IMPLEMENTED** | `quickvib-project/src/{schema,store,validate}.rs`(v1 schema、加载、逐字段校验);SCPI `MMEM:LOAD:STAT`(`tree.rs:166`);GUI Open(`controller.rs`) |
| R9 | 自动加载上次项目 | **IMPLEMENTED** | `quickvib-project/src/last_project.rs`(状态目录记录)+ `crates/quickvib/src/app.rs` `resolve_project`(优先级:`--project` > 记录 > 空启动);SCPI `MMEM:LOAD:AUTO`/`AUTO?` 开关 |
| R10 | 录制 N 秒 | **IMPLEMENTED** | `quickvib-engine/src/recording.rs`(`RunPlan`/`RunResult`/`RunOutcome`)+ `engine.rs` 运行管线:`Condvar` watchdog(时长 × 系数 + 1 s 上界)、capture 字节数上限守卫;`CONF:REC:DUR`/`INIT`/`REC:STAR` |
| R11 | 完成通知(`#REC:DONE` + `REC:WAIT?`) | **IMPLEMENTED** | `#REC:DONE` 扇出:`scpi_server.rs`(每会话写半 `Mutex` 保证不与响应交错)+ `format.rs`;`REC:WAIT?` → `engine.wait_for_run()`(`engine.rs:523-534`,watchdog 有界);`tests/sessions.rs` 双会话验证 |
| R12 | 导出 CSV / TXT | **IMPLEMENTED** | `quickvib-measure/src/export/mod.rs`(CSV 含前导信息与表头、TXT 纯数值);SCPI `MMEM:STOR:TRAC`、`FORM`/`FORM:DATA`;GUI 导出按钮 |
| R13 | 计算 peak / RMS / peak-to-peak | **IMPLEMENTED** | `quickvib-measure/src/stats.rs`;`CALC:MEAS:{PEAK,RMS,PP,ALL}?`;引擎侧解析正弦 oracle 测试(`engine.rs:1304-1312`) |
| R14 | CLI `--project --scpi-port --device-port --headless` | **IMPLEMENTED** | `crates/quickvib/src/cli.rs`:四项全支持,另有 `--backend`、`--version` 等;零依赖手写解析,带单元测试 |
| R15 | 完整 SCPI 表面(IEEE 488.2 + 仪器命令) | **IMPLEMENTED(按 PLAN §8 的表)** | `quickvib-scpi/src/tree.rs` 覆盖 §8 全部命令:`SYST:ERR?`/`:NEXT?`/`VERS?`/`DEV:CONN?`、`MMEM:LOAD:STAT`/`STOR:STAT`/`LOAD:AUTO`/`STOR:TRAC`、`CONF:REC:DUR`、`FORM(:DATA)`、`INIT(:IMM)`/`REC:STAR`/`ABOR`、`REC:STAT?`/`WAIT?`、`FETC?`/`TRAC:DATA?`/`TRAC:POIN?`、`CALC:MEAS:*?` 及 `*IDN? *RST *CLS *OPC *OPC?`(`tree.rs:401-405`)。**注意**:§8 本身只承诺这 5 条通用命令,488.2 完备性缺口计入 SOTA 差距(§4.1),不算违背计划 |
| R16 | 默认后端为 mock | **IMPLEMENTED** | schema `device.backend` 默认 `mock`(D4);`backend_factory.rs` 按项目/CLI 选择;mock 为种子化 PRNG 确定性信号,支持故障注入 |
| R17 | 薄 `DeviceBackend` trait 隔离 mock 与 M300 FFI | **IMPLEMENTED(mock + tcp 两实现)** | `quickvib-device/src/backend.rs`(trait + 支撑类型);`MockBackend` 与 `StreamBackend`(`stream.rs`)两实现;M300 第三实现随 Phase 6 |
| R18 | **Rust**(edition 2021, stable);测试可在 Linux 跑 | **IMPLEMENTED** | 根 `Cargo.toml` workspace(edition 2021、`rust-version`);本次审计在 Linux 上复跑 495 项测试全绿;除 `quickvib-m300` 外全部 `#![forbid(unsafe_code)]` |
| R19 | 样例 `Test.proj` | **IMPLEMENTED** | `samples/Test.proj` 存在,与 §14 的评审版一致 |
| R20 | 中英双语 README | **IMPLEMENTED** | `README.md` 双语(英文 + 简体中文),含快速上手、SCPI 表、telnet `*IDN?` 冒烟指引 |
| R21 | 无 pass/fail、无 DUT 激励、**无假 DLL** | **EXCLUDED(遵守)** | `quickvib-m300` 无任何 stub DLL 或假实现;`--backend m300` 显式报错(D21);全仓 rg 无 pass/fail 逻辑 |
| R22 | 可复现产出 Windows `.exe`(交叉构建 和/或 原生 CI) | **IMPLEMENTED** | CI mingw 交叉构建 job(必选)+ `win-msvc` 编译检查;`Cargo.lock` 已提交(D26);release profile 已调优(根 `Cargo.toml:73-77`:`lto="thin"`, `codegen-units=1`, `strip=true`, `panic="unwind"`)。tagged release 流水线是 Phase 7 余项,不影响本条判定("and/or"已满足) |
| R23 | 零或最少运行时依赖,均有论证 | **IMPLEMENTED** | 运行时直接依赖仅 `serde`/`serde_json`;`eframe` optional 且默认关(D18);`libloading` 只进 `quickvib-m300`;`tempfile` 仅 dev;CI `deps` job 断言直接依赖清单与 §6.3 一致 |

**统计**:IMPLEMENTED 19,PARTIAL 1(R4,推迟部分有计划背书),INTENTIONALLY DEFERRED 1(R3),EXCLUDED 遵守 2(R7/R21)。**没有无计划背书的 MISSING 项。**

---

## 2. Phase 0–8 评分

| Phase | 内容 | 判定 | 证据要点 |
| --- | --- | --- | --- |
| 0 | workspace 脚手架 | **IMPLEMENTED** | 根 `Cargo.toml`(members / default-members 排除 `quickvib-m300` / workspace lints)、`rust-toolchain.toml`、`clippy.toml`(D20 disallowed-methods)、`.cargo/config.toml`(mingw 链接器)、`Cargo.lock` 已提交、CI 从 Phase 0 即含交叉构建 job |
| 1 | 领域核心 | **IMPLEMENTED** | `quickvib-core`(`unit.rs`/`error.rs`(SCPI-99 错误目录)/`clock.rs`/`cancel.rs`)、`quickvib-project`(schema/store/validate/last_project)、`quickvib-measure`(stats + CSV/TXT 导出)、`samples/Test.proj`;几乎全是纯函数 + 单元测试 |
| 2 | 后端与流 | **IMPLEMENTED** | `quickvib-device`:trait、种子化 PRNG mock + 故障注入、`framer.rs` 块边界穷举、`listener.rs` 回环测试、`sink.rs` 有界 `SampleChannel`(丢最旧 + `dropped`/`received` 计数)、全程 `Clock` 注入 |
| 3 | 仪器引擎 | **IMPLEMENTED** | `quickvib-engine`:状态机(`state.rs` 穷举 `(state, event)` 表测试)、32 深错误队列 + `-350` 溢出(`errors.rs`)、OPC 记账(`opc.rs`)、录制管线 + `Condvar` watchdog + capture 上限、`wait_for_run` 原语、`#REC:DONE` 扇出、panic 遏制(D27) |
| 4 | SCPI 层 | **IMPLEMENTED** | `quickvib-scpi`:lexer、命令树(长短形、`;` 复合消息、`\n`/`\r\n`)、覆盖 §8 全部命令的 `Command` 枚举、流式响应格式化(`FETC?` 不整串拼接)、会话状态;`dispatch.rs` 穷举 `match` |
| 5 | App / CLI / TCP | **IMPLEMENTED** | `quickvib` bin:`cli.rs`、`scpi_server.rs`(会话上限 8,`scpi_server.rs:21`)、`device_server.rs`、`backend_factory.rs`、auto-load-last、`--headless` 结构化日志(key=value)、退出码;§17.2 的回环集成测试全部在 `crates/quickvib/tests/` 落地。PLAN §25 "mock 路径端到端可用"的自述与实测一致 |
| 6 | M300 原生后端 | **INTENTIONALLY DEFERRED** | 按 §21.1 的 SDK ABI 问题门禁;crate 边界、`bindgen` feature 钩子、库解析器思路就位,但无 FFI 代码;`docs/M300-NATIVE.md` 为占位合同文档。推迟方式符合 D21(无假 DLL、显式启动错误) |
| 7 | 文档与打磨 | **PARTIAL** | 已有:双语 README(R20)、release profile 调优(`Cargo.toml:73-77`)。缺:**`docs/SCPI.md` 不存在**(docs/ 下只有 PLAN.md 与 M300-NATIVE.md)、独立的 UTS 示例转录文档(README 有简易冒烟指引但非 §23 承诺的完整注释转录)、`cargo deny` 门禁(仓库无 `deny.toml`)、tagged release 产 MSVC exe 的流水线 |
| 8 | GUI | **IMPLEMENTED(含两项自述遗留)** | `quickvib-ui` 按 §19 "Landed" 描述分两半:视图模型(`i18n`/`form`/`controller`/`ports`/`status`,无窗口依赖、headless 可测)+ egui 视图(`window`/`theme`/`font`,off-by-default `gui` feature)。D28(中文默认 + 穷举双语标签表 + `ui-language.txt`)、D29(实际监听端口与项目端口分开显示,`ports.rs`)、D30(内嵌 Noto Sans SC 子集 + cmap 覆盖测试)均有测试证据。自述遗留(PLAN 已声明,非缺陷):Advanced 页只有说明文案(等 schema/后端支持激光/TEC/PID/触发)、后端就地重开(改采样率仍需重启) |

---

## 3. 已知缺陷核查:`Engine::adopt_project` 状态不归位 — **确认(CONFIRMED)**

### 3.1 根因

```327:337:crates/quickvib-engine/src/engine.rs
    pub fn adopt_project(&self, project: Project, path: Option<PathBuf>) {
        let mut guard = self.lock();
        guard.duration_seconds = project.recording.duration_seconds;
        guard.format = project.export.format;
        guard.result = None;
        guard.outcome = None;
        guard.project = Some(project);
        guard.project_path = path;
        drop(guard);
        self.changed.notify_all();
    }
```

`result`/`outcome` 被清空,但 `guard.state` 原样保留。若采纳项目前状态是 `Complete`,引擎进入一个状态机永远不该出现的复合态:**`state == Complete` 且 `result == None`**。`state.rs` 的 `transition` 表本身是自洽且被穷举测试的 —— 问题在于 `adopt_project` 绕过了状态机,直接改内部字段而不发事件。

### 3.2 触发路径(两条,不止 GUI)

1. **GUI Apply**:`quickvib-ui/src/controller.rs:247-265` — `apply()` 校验通过后调 `engine.adopt_project(...)`。运行中会被 `-221` 拒绝,但 `Complete` 态不会。
2. **SCPI `MMEM:LOAD:STAT`**:`engine.rs:304-322` — `load_project` 同样只在 `is_running()` 时拒绝,`Complete` 态照样走进 `adopt_project`。**因此纯 UTS(无 GUI)路径同样能触发**,这一点比原始缺陷描述的范围更广。

### 3.3 可观测后果(复现序列:`INIT` → 等完成 → `MMEM:LOAD:STAT "Test.proj"` 或 GUI Apply)

| 查询 | 实际返回 | 应当返回 |
| --- | --- | --- |
| `REC:STAT?` | `COMPLETE` | `IDLE`(数据已被丢弃) |
| `REC:WAIT?` | `1`(`engine.rs:533`:`guard.state == State::Complete`) | 不应示意"有一次完成的运行可取数" |
| `FETC?` / `TRAC:DATA?` | `-230 Data corrupt or stale`(`engine.rs:542-548` 要求 `(Some(result), Complete)` 同时成立) | 与状态一致 —— `-230` 本身是对的,错的是状态还说 COMPLETE |
| `CALC:MEAS:*?` / `MMEM:STOR:TRAC` | 同样 `-230`(`engine.rs:554-560`、`568-572`) | 同上 |
| GUI | 状态徽标"完成",测量卡"尚无完成的采集" | 两处应一致 |

即:**协议表面自相矛盾** —— 状态查询宣称有完成的采集,取数查询说没有。对按 "`REC:WAIT?` 返回 1 就 `FETC?`" 编写的 UTS 脚本,这是一次必然的 `-230` 意外。

### 3.4 修复方向(Round-2,此处只给结论不动代码)

两个调用方(`load_project`、GUI `apply`)都已排除运行中状态,所以在 `adopt_project` 内把 `guard.state` 归位 `Idle` 是安全且语义正确的(*RST 的既有语义就是回 `Idle`,采纳新项目丢弃旧数据与之同构)。同时应审视 `any_run_started` 是否一并复位(否则 `REC:WAIT?` 在新项目下的语义仍暧昧),并补三条回归测试:SCPI 载入后 `REC:STAT?` == `IDLE`;GUI Apply 后同;`REC:WAIT?` 在采纳后不再返回 `1`。风险低:状态机穷举表测试不受影响(`adopt_project` 不走 `transition`),会话/通知路径无涉。

---

## 4. SOTA 差距分析(对标 Keysight 风格 SCPI 振动测量宿主)

### 4.1 IEEE 488.2 完备性 — **最大差距**

已实现的通用命令只有 5 条(`tree.rs:401-405`):`*IDN?`、`*RST`、`*CLS`、`*OPC`、`*OPC?`。IEEE 488.2 §10 规定的强制通用命令中,**`*ESE`/`*ESE?`/`*ESR?`/`*SRE`/`*SRE?`/`*STB?`/`*TST?`/`*WAI` 全部缺失**,也没有 `STATus:OPERation/QUEStionable` 子系统与状态字节模型。连带两个具体的不自洽:

- `opc.rs` 对 OPC 位有完整记账(`bit_set`,含 `*CLS` 清位、新 run 清位),**但没有 `*ESR?` 可读它** —— 对 UTS 而言 `*OPC` 的置位效果不可观测,只有阻塞式 `*OPC?` 可用。
- PLAN §8 对 `*CLS` 的描述是"清错误队列**与状态/事件寄存器**",而寄存器并不存在,文档与实现的措辞已经先行漂移。

对真实 Keysight 仪器,UTS/VISA 层常用 `*STB?` + SRQ 轮询或 `*ESR?` 判完成;缺这套意味着 UTS 只能用本产品自有的 `REC:WAIT?`/`#REC:DONE`。**在 UTS 接入合同(§21.1 Q1)确认前,这是集成风险而不只是洁癖。**

### 4.2 错误队列 — 基本达标,小缺口

32 深 FIFO、空读 `0,"No error"`、溢出压 `-350`、`*CLS` 清空 —— 符合 SCPI-99 且有测试。全局共享队列是 D8 的明确决策,与真实仪器一致(多客户端互相"偷"错误在 Keysight 上同样发生),可接受。缺口:无 `SYST:ERR:COUN?`、无 `SYST:ERR:ALL?`(SCPI-99 常见便利查询,UTS 清队列时好用)。

### 4.3 OPC 语义 — 实现优于多数自制宿主,受限于 4.1

`*OPC?` 按 D8 是 per-session 的,阻塞时不持引擎锁(PLAN §9 的承诺在 `dispatch.rs`/`engine.rs` 得到落实),且被 watchdog 上界兜底不会永久挂起 —— 这三点都是自制 SCPI 宿主常见的坑,这里都躲开了。差距即 4.1:`*OPC` 事件路径(置 ESR 位 → SRQ)不可观测。

### 4.4 会话隔离 — 良好

会话上限 8(`scpi_server.rs:21`,可配),超限拒绝并留日志;每会话独立线程 + 读半 `BufReader`、写半 `Mutex<TcpStream>`,`#REC:DONE` 不会与响应交错;会话 panic 被遏制不拖垮进程(D27);UTF-8 校验与行协议健壮性有测试(`tests/protocol.rs`)。差距:无会话空闲超时 —— 8 个半死连接可占满槽位把真 UTS 挡在门外(现场 TCP 半开连接是真实发生的事);无 per-session 标识进日志(多客户端排障时靠 peer 地址区分,尚可)。

### 4.5 背压与数据面 — 设计正确,传输格式单一

`SampleChannel` 有界、满则丢**最旧**并计数(`sink.rs:171-177`)—— 这是活仪器持续吐流场景的正确选择;capture 字节上限在项目校验与引擎两侧都有守卫;`FETC?` 流式格式化,不在内存里拼整串。差距:数据只有 ASCII 逗号分隔一种线格式,无 488.2 definite-length block(`#4xxxx` + 二进制)选项 —— 100 kHz × 数十秒的 capture 用 ASCII 传输量增约 2–3 倍,Keysight 仪器一律提供 `FORM REAL,32`;另外 `dropped` 计数没有暴露成 SCPI 查询,链路降级对 UTS 不可见。

### 4.6 可观测性 — headless 合格,缺运维深度

`--headless` 有结构化 key=value 日志(启动、绑定端口、会话进出、载入/导出、拒绝原因),GUI 显示会话数与错误队列深度。差距:无日志级别开关、无日志落盘选项(现场只能靠重定向)、无运行时自检查询(`*TST?` 缺失,连"永远回 0"的占位都没有)、设备链路统计(收到/丢弃样本数、重连次数)未上任何查询表面。

---

## 5. Round-2 实施优先级(建议排序)

### 必修(阻塞 UTS 集成或正确性)

| # | 项 | 理由 | 规模 |
| --- | --- | --- | --- |
| P1 | **修复 `adopt_project` 状态归位**(§3.4)+ 3 条回归测试 | 协议表面自相矛盾,UTS 按 `REC:WAIT?`→`FETC?` 编排必踩;两个调用方都已排除运行态,修复面小、风险低 | 单文件小改 + 测试 |
| P2 | **补 `docs/SCPI.md` + 完整 UTS 注释转录** | Phase 7 明文承诺;UTS 集成方(§21.1 Q1)没有命令参考无法写脚本;纯文档零风险 | 文档 |
| P3 | **488.2 强制通用命令最小集**:`*ESR?`/`*ESE(?)`/`*SRE(?)`/`*STB?`/`*WAI`/`*TST?` + 最小 ESR/STB 位模型,让 `opc.rs` 已有的 bit 可读 | §4.1;`Opc::bit()` 记账已在,主要是 `tree.rs`/`dispatch.rs` 增条目和一个小寄存器结构;同时修正 `*CLS` 文档措辞 | SCPI 层中等 |

### 次优(改善质量,不阻塞)

| # | 项 | 理由 |
| --- | --- | --- |
| P4 | `SYST:ERR:COUN?`(及可选 `:ALL?`) | UTS 清队列的惯用查询,实现成本极低 |
| P5 | 会话空闲超时(读侧 `set_read_timeout` + 空闲计时) | §4.4 的半开连接占槽风险 |
| P6 | `FORM REAL,32` / definite-length block 的 `FETC?` 二进制传输 | 大 capture 传输效率;`format.rs` 已流式,加一条编码路径 |
| P7 | `cargo deny` 门禁 + tagged release 流水线(Phase 7 余项) | 补全 Phase 7;发布可复现性 |
| P8 | 设备链路统计上查询表面(如 `SYST:DEV:STAT?` 暴露 dropped/received)+ 日志级别/落盘开关 | §4.5/§4.6 可观测性 |
| P9 | GUI 自述遗留:后端就地重开(采样率变更免重启) | PLAN §19 Phase 8 自列的 "still to come";涉及 backend 生命周期,规模中等 |

### 保持推迟(有明确门禁,Round-2 不做)

- **Phase 6 M300 FFI**:仍被 §21.1 的 SDK ABI 问题门禁,且 D21 禁止假 DLL。在拿到 SDK v1.2.0 头文件/回执前动工只会制造需要返工的猜测代码。
- **Advanced 页真实控件**(激光/TEC/PID/触发):等 v2 schema 与 M300 后端,与 Phase 6 同批。

---

## 6. 审计方法与限制

- 逐文件通读了全部 9 个 crate 的源码与测试(`quickvib-core/-project/-measure/-device/-engine/-scpi/-ui/-sim/-m300` 及 `quickvib` bin),对照 PLAN §3 表、§19 里程碑、§25 状态自述与决策 D1–D30。
- 在 Linux 上复跑 `cargo test --workspace --locked`(495 通过 / 0 失败)与 `cargo clippy --workspace --all-targets --locked`(无警告)。未复跑 Windows 交叉构建(以 CI 必选 job 为证据)。
- 缺陷 #1 的确认基于代码逻辑推演(`engine.rs:327-337` 与 `542-548`/`523-534` 的联立),未编写新测试固化 —— 按任务约束,本轮不改动产品 crate,固化测试留给 Round-2 P1。
- GUI 的 egui 视图半(`window.rs`/`theme.rs`/`font.rs`)以代码审读 + 既有 headless 测试为证据,未做真机截图验证(环境无显示器;视图模型半的测试覆盖已充分)。
