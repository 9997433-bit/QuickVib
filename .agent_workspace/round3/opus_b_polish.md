# Round 3 — opus-B 收尾打磨报告

- **代理:** R3-opus-B（local，`/workspace`）
- **日期:** 2026-08-25
- **分工:** 不碰 `scpi_server.rs` / `app.rs` / `cli.rs`（opus-A 所有）
- **提交:** 未 commit / 未 push / 未开 PR（按指令）

## 1. 变更清单

| # | 项 | 文件 | 性质 |
| --- | --- | --- | --- |
| 1 | `DeviceBackend::stop_handle` 契约文档 | `crates/quickvib-device/src/backend.rs` | 文档 |
| 1b | `finish_run` 处交叉引用该契约 | `crates/quickvib-engine/src/engine.rs` | 注释 |
| 2 | PLAN §5.3 补 `Invalidate` 边；§8.1 `*CLS`/`*OPC` 措辞对齐；§21.2 新增第 16 问 | `docs/PLAN.md` | 文档 |
| 3 | B12 `Refusal::SocketError` | `crates/quickvib-device/src/listener.rs`、`crates/quickvib/src/device_server.rs` | 行为（日志理由）+ 测试 |
| 4 | B13 `Opc::bit_set` 不可观测注释 | `crates/quickvib-engine/src/opc.rs` | 注释 |
| 5 | B14 删除死代码 `Command::is_query` | `crates/quickvib-scpi/src/command.rs` | 删除 |
| 6 | 删除 `IntoAdoptResult` 死垫片 | `crates/quickvib-engine/tests/project_mutation_while_running.rs` | 删除 |
| 7 | SCPI.md `*CLS` / `*OPC` 一句话对齐 | `docs/SCPI.md` | 文档 |

合计 `+104 / -83`，9 个文件。

## 2. 逐项说明

### 1. `stop_handle` 契约（ROUND2_BRIEF 边界风险 #1，最高优先级）

在 trait 方法上加了 `# Contract` 小节，写死三条：

1. **每次运行结束都会触发，不只是中止。** 正常完成时 `finish_run` 自己 `cancel.cancel()` 去放行看门狗线程，钩子随之执行；**到达这里并不表示采集失败**。
2. **必须幂等、空闲时调用安全。** `ABOR` 会同时走 `DeviceBackend::stop` 和 token 取消两条路；钩子也可能在 `stream` 尚未 park 时、或已返回后触发。
3. **不得永久失效传输。** 只允许打断*当前*这次读；同一已打开的传输上的下一次 `stream` 必须无需重新 `open` 就能工作。「置一个 `stream` 入口清除的标志位」满足该约束（mock/tcp 后端即如此），**持久链路上的 `TcpStream::shutdown` 不满足**——那种后端要么在 `stream` 内透明重连，要么此处返回 `None` 改为轮询 token。

外加一句线程约束：由任意线程（SCPI 会话、看门狗、reader 自身）调用，不得阻塞、不得需要 `&mut` 状态。这正是 M300 后端将来最容易踩的坑（brief 点名的场景）。

`engine.rs` 里 `finish_run` 的 `cancel.cancel()` 旁补了一行注释指向该契约，避免读引擎代码的人以为钩子只在 abort 路径跑。

### 2. PLAN.md

- **§5.3 状态机图** 补了三条 `Invalidate` 边（`Complete → Idle`、`Aborted → Idle`、`Idle → Idle`，标注为 "project adopted"），并在图下加一段说明：该边是带外的采集作废（`MMEM:LOAD:STAT` / `MMEM:LOAD:AUTO` / GUI *Apply*），终态运行状态随采集一起丢弃，因此 `REC:STAT?` 不可能在 `FETC?` 返回 `-230` 时答 `COMPLETE`；从 `Armed`/`Recording` 出发被拒，即运行中加载报 `-221`；在途检查与状态写入在同一次持锁内完成。与 `state.rs::transition` 的实现（`(Idle | Complete | Aborted, Invalidate) => Idle`，`(Armed | Recording, Invalidate) => Err`）逐条对应。
- **§8.1 `*CLS`** 原文 "Clear the error queue and status/event registers" 与实现不符——实现是 `errors.clear()` + `opc.clear()`，并没有寄存器。改为：清空错误队列 + 清除已武装的 `*OPC` 请求与 OPC 位；并明确状态字节/标准事件寄存器**未实现**（无 `*STB?`/`*ESR?`），所谓"清寄存器"就只剩这两件事。
- **§8.1 `*OPC`** 同步说明该位是引擎内部记账，UTS 通过 `*OPC?` / `REC:WAIT?` / `#REC:DONE` 观察完成。
- **§21.2 新增第 16 问**（488.2 状态寄存器组是否为 UTS 合同所需），并让 `*CLS` 行引用它。原 brief 写的"§21.1 Q1"在文中并不存在（§21.1 只有 Q-A/Q-B/Q-C，且都是 Phase 6 SDK 问题），故落在非阻塞问题清单里更准确。

### 3. B12 `Refusal::SocketError`

`listener.rs::dispatch` 中 `set_nonblocking(false)` 失败原先回报 `Refusal::PeerNotAllowed`，日志会写 `reason=peerNotAllowed`，把套接字故障误导成安全策略拒绝。新增 `Refusal::SocketError` 变体，`device_server.rs` 日志映射为 `reason=socketError`。

`Refusal` **不加** `#[non_exhaustive]`：跨 crate 的 `match` 保持穷尽，将来再加变体时是编译错误而不是被 `_` 吞掉。

顺带把 `listener.rs` 的单元测试从"只数拒绝次数"升级为"记录拒绝理由序列"（`Refusals(Mutex<Vec<Refusal>>)` + `wait_for_refusal` 辅助函数），`peer_allow_list_filters_connections` 与 `second_connection_is_refused_while_one_is_live` 现在断言具体变体，理由再被写错就会红。

另注：他人已在树里放了 `crates/quickvib-device/tests/refusal_reasons.rs`（断言三个变体互不相等），本改动使其编译并通过。

### 4. B13 `Opc::bit_set`

字段上写明**不是 SCPI 可见的**：标准事件寄存器未实现，没有 `*ESR?`（也没有 `*STB?`）可读回；UTS 用 `*OPC?` / `REC:WAIT?` / `#REC:DONE` 观察完成。保留该位的理由也写了：`*OPC` 仍须被接受，且寄存器模型是纯增量改动（指向 PLAN §21.2 第 16 问）。`bit()` 访问器同样只有测试在用，加了一行指回字段注释。

### 5. B14 `Command::is_query`

产品代码零调用（是否有响应由 `Response::is_some` 决定），唯一调用者是它自己的单元测试——自证循环。整个 `impl Command` 块与 `queries_are_classified_correctly` 一并删除。`quickvib-scpi` 是 `publish = false` 的工作区内部 crate，无外部 API 承诺。

### 6. `IntoAdoptResult` 死垫片

`adopt_project` 已固定返回 `Result<(), ScpiError>`，为兼容旧 `()` 签名而写的 trait + 两个 impl 已无意义（`impl for ()` 分支永远不会被选中，反而会让签名回退变成静默通过而非编译错误）。删除 trait 与两处 `.into_adopt_result()`，断言语义不变。

### 7. SCPI.md

- `*CLS` 行：`Clear the error queue only.` → 清错误队列 + 已武装的 `*OPC` 请求，明确不动采集/状态/已加载项目。
- `*OPC` 行：补一句无 `*ESR?`/`*STB?`，等待请用 `*OPC?` / `REC:WAIT?` / `#REC:DONE`。
- Apply 语义不需要再加句子：现有「Loading a project discards the capture」小节已覆盖 `MMEM:LOAD:*` 与 GUI *Apply* 丢弃采集、`REC:STAT?` 答 `IDLE`、数据类查询答 `-230`、运行中加载 `-221` 且不改变任何东西。这次只把 PLAN §5.3 的状态机图补齐到与该节一致。

## 3. 验证

工作区是多代理共享的（opus-A 的 `--bind`/`bindHost`/写超时改动同时在树里），所以按 crate 分别验证：

| 命令 | 结果 |
| --- | --- |
| `cargo test -p quickvib-device -p quickvib-engine -p quickvib-scpi` | **全绿**（device 70、engine 77+3+1+2+1、scpi 49，含 doc-test） |
| `cargo test -p quickvib` | **全绿**（63 + 各集成测试，共 12 个测试二进制） |
| `cargo clippy -p quickvib-device -p quickvib-engine -p quickvib-scpi --all-targets -- -D warnings` | **干净** |
| `cargo doc -p quickvib-device --no-deps` | **干净**（新 rustdoc 的 intra-doc link 均可解析） |
| `cargo clippy -p quickvib --all-targets -- -D warnings` | **干净** |
| `cargo fmt --all -- --check` | **干净**（全工作区，含他人在途文件） |

### 期间观察到的两条他人在途红（均已自行消失，本代理未触碰）

1. `crates/quickvib-ui/src/form.rs:383` 缺 `bind_host` 字段（opus-A 的 `server.bindHost` 半途状态）——数分钟后复查已修。
2. `crates/quickvib/src/scpi_server.rs:462` `Instant::now` 触发 D20 的 `disallowed_methods`（opus-A 的写超时代码）——复查已修。

两者都在 opus-A 的文件里，仅作记录，供 gpt 终测时留意时序。

## 4. 未做 / 留给他人

- B7 SCPI 写超时、`--bind` / `server.bindHost`：opus-A。
- 全量 `cargo test --workspace` + `--features gui` 回归、`ci/check-deps.sh`：gpt（本代理只按 crate 验证，避免与在途改动纠缠）。
- 未实现 `*ESR?`/`*STB?`：按 brief「不做」，改为在 PLAN §21.2 立为第 16 问 + 代码注释说明。
- `Refusal` 的 spawn 失败路径（`dispatch` 中线程创建失败）目前不走 `on_refused`，只是把连接状态复位；属既有行为，未纳入本轮范围。
