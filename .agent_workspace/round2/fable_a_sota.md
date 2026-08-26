# QuickVib Round 2 SOTA 复审 — R2-fable-A

- **模型:** `claude-fable-5-thinking-xhigh`（无降级）
- **复审对象:** 分支 `cursor/review-optimize-f6c8`，最终 HEAD `729d376`（本地 == origin，零未推送提交）
- **复审性质:** 只读；未改动任何 crate；本文件为唯一写入
- **独立验证（在最终 HEAD 729d376 上本人复跑）:** `cargo fmt --all --check` 通过（守门指出的阻塞项已由 `729d376` 消除）；`cargo test --workspace --locked` **528 通过 / 0 失败**（基线 495 → R1 后 509 → R2 后 528）；`cargo clippy --workspace --all-targets --locked -- -D warnings` 干净；`ci/check-deps.sh` 通过。另在 R2 修复落地前对 `1da8740` 复跑过一次基线（509 全绿），两个时点均无回归。

## 0. 结论速览

1. **R1 三项修复在最终 HEAD 全部完好**，无被 R2 改动破坏的迹象（§1）。
2. **B1/B5/B4/B3/B6 五项全部是真修复，无一项 papered over**（§2）。每项都有"修复前必红"的回归测试锁定，且三条不可破坏不变量（TestClock 虚拟时间、D9 abort 丢数据、in-flight run 保护）经我独立核验均保全。顺带修复的 B8/B9/B10/B11 同样成立。
3. 发现并确认**一个真实的 Phase 6 隐患**（非本轮缺陷）：`finish_run` 收尾的 `cancel.cancel()` 会在**正常完成后**也触发 `stop_handle` 钩子。当前两个后端在 `stream()` 入口复位 `stopped`（`mock.rs:253`、`stream.rs:134`），故无害；但未来 M300 后端若用 socket shutdown 实现句柄，每次成功 run 后传输会被切断。与守门报告 §1-B3 的判断一致——Round 3 必须在 trait 文档写死"钩子幂等、正常完成后也会执行"的契约（§4.1）。
4. **SOTA 剩余差距中唯一可能阻塞 UTS 的是 IEEE 488.2 强制通用命令缺失**（`*ESR?`/`*STB?` 等），且它是**条件性阻塞**——取决于 PLAN §21.1 Q1 的 UTS 接入合同是否要求 488.2 状态轮询。QuickVib 自有的 `REC:WAIT?`/`#REC:DONE` 完成通知路径本轮已由 `docs/SCPI.md` 补全文档，UTS 若按此编排则无阻塞项（§3）。
5. Round 3 建议清单见 §4：5 项小成本必修、1 项按合同定夺、其余归"接受为已知"。

---

## 1. R1 修复复核（任务 #1）—— 全部 HOLD

| R1 修复 | 最终 HEAD 状态 | 证据 |
| --- | --- | --- |
| ① `adopt_project` Invalidate（Complete 态不再与 `FETC?` `-230` 矛盾） | **HOLD，且被 R2 强化** | `engine.rs:354` 保留 `transition(state, Event::Invalidate)`（R2 将 `if let Ok` 改为 `unwrap_or(State::Idle)`——running 态已在同一锁内被拒，语义等价）；`state.rs:164-165` Invalidate 边未动，穷举表测试 `the_full_state_event_table_is_covered`（rejected == 10）原样通过；三层回归（engine 单测 / dispatch / `adopt_project_semantics.rs`）适配 `Result` 后全绿。R2 的 B5 修复把"检查与写入同锁"补齐，等于给 R1 修复加了并发防线 |
| ② `scpiPort == device.port` 工程校验 + GUI 数值判重 | **HOLD** | `validate.rs:139-147`（`-224`，带解释注释）；`form.rs:325-332` 按 `Option<u16>` 数值比较；测试 `the_scpi_port_and_the_device_port_must_differ`、`a_duplicate_port_is_caught_however_it_is_spelled`（"09123"/"+9123"/" 9123"）、`two_unparsable_ports_do_not_look_like_a_duplicate` 均在 528 全绿之列。R2 未触碰 validate.rs / form.rs |
| ③ mock 故障阈值大于请求数不报假 LinkLost | **HOLD** | `mock.rs:152-155` 注释与 `fault_outcome` 仅在阈值实际命中时调用的结构未变；测试 `a_fault_limit_beyond_the_request_never_fires` 原样通过。R2 对 mock.rs 的改动（B4 pace_batch）不涉足 fault 路径 |

---

## 2. R2 修复逐项判定（任务 #2）—— 真修 vs 糊弄

R2 落地为两个代码提交：`561d1ff`（opus-B：B4/B6/B8/B10/B11 + `docs/SCPI.md` + gpt-B 四个回归测试文件）与 `0a84fb4`（opus-A：B1/B5/B3/B9 + `cancel_run` 重构），随后 `729d376` 修 fmt。守门（R2-fable-B）已逐 diff ACCEPT；以下为我的独立复核，重点在"是否只治了症状"。

### B1（P0）reader spawn 失败卡死 Armed — **真修复**

- `spawn_reader`（`engine.rs:695-716`）在闭包 move 前取 `let sequence = plan.sequence;`，失败分支 `finish_run(sequence, LinkLost, …)` —— 根因（序号错配导致 `finish_run` 首行 guard 直接 return）被正面消除，而非绕过 guard。
- **超出最小修复但正当**：`spawn_watchdog` 失败也补了收尾（`cancel_run(Some(sequence), LinkLost)`，`engine.rs:728-735`）——无监督的 run 在设备 stall 时没有任何东西能结束它，与 B1 同类；reader 已起则由 reader 正常收尾（无并发写状态机），reader 也没起则按序号检查为 no-op，两个都失败的组合也闭合。
- **测试是失败即红的**：`spawn_detached` 唯一 spawn 入口 + `#[cfg(test)]` thread-local `FAIL_SPAWN` 注入座（`engine.rs:954-973`，不污染产品路径），`a_reader_that_cannot_be_spawned_settles_the_run` 断言 Aborted + `-240` + **随后一次完整 run 仍成功**（证明仪器未卡死）；`a_watchdog_that_cannot_be_spawned_ends_the_run_it_cannot_supervise` 用 Stall 后端锁定。gpt-B 的 `stale_run_completion.rs` 另从公共 API 锁定 sequence 防线本身（旧 reader 迟到的 Completed 不能改写新 sequence 终态、不能发第二条通知）——这是 B1 修复所依赖的机制的独立保险。

### B5（P0）`load_project` TOCTOU — **真修复**

- 权威检查移入 `adopt_project` 的**同一锁持有期**（`engine.rs:343-347`），返回 `Result<(), ScpiError>`；`load_project` 锁外预检降级为注明"非权威"的快速失败（`engine.rs:305-310`）。这是根治：无论多少调用方、无论锁外做多久 I/O，写入前的复检不可绕过。
- 调用方适配完整且无糊弄痕迹：dispatch/lib 文档/三个测试文件 `.unwrap()`；CLI 启动 `let _ =` 附"启动时无 run"注释（`app.rs:242-243`）；**GUI `apply()` 只在引擎确认 Ok 后才提交本地 `self.base`**（`controller.rs:257-262`）——顺带消除了 GUI 本地状态与引擎失同步的次生竞态。
- 测试：`adopting_a_project_mid_run_is_refused_under_the_lock` 直接模拟 MMEM:LOAD:STAT 的窗口（先观察 settled、再 INIT、再 adopt）；gpt-B 的 `project_mutation_while_running.rs` 断言 duration/format/name/path 四项零覆盖，`load_project` 与 `adopt_project` 双路径各一条。D9 不变量（非 Complete 清 result）未被触碰。

### B4（P1）paced mock 不可中断 sleep — **真修复，且方案优于 R1 审计原建议**

- `pace_batch`（`mock.rs:173-185`）以批时长为 deadline、按 `PACE_SLICE = 10ms` 切片，每片前查 `cancel`/`stopped`；被打断返回 `Cancelled`。
- 关键取舍我专门核验过：等待**继续走注入时钟 `clock.sleep`** 而非 R1 审计随口建议的 `cancel.wait_timeout(batch)`。后者会打破 TestClock 契约（`wait_timeout` 是真实 Condvar 等待，不推进虚拟时间，deadline 永远到不了 → 死循环/真等）。落地方案在 TestClock 下每片即时推进、片长求和精确等于批时长——既有测试 `pacing_uses_the_injected_clock`（虚拟时间恰好 5s）原样通过；SystemClock 下取消延迟上界 10ms，与 Stall 分支既有轮询同量级。
- 防"修过头"也有测试：`pacing_still_takes_real_time_on_the_system_clock`（200 样本 @1kS/s ≥150ms）防止切片把 pacing 切没。`cancellation_interrupts_a_paced_batch`/`stop_interrupts_a_paced_batch`（单批 10s、20ms 后打断、断言 <2s）修复前必红。集成层 `pace_cancel.rs`（750ms 批、250ms 内返回，且失败路径回收 worker 线程不留 detached）同口径。
- **有意不修的部分正确**：sleep 仍在 `on_batch` 之前，低采样率下首批交付前 `REC:STAT?` 停留 ARMED——这是批粒度交付的固有现象，opus-B 报告有明确论证（改顺序会改变节流语义），归"接受为已知"。

### B3（P1）活动 run 中 `stop()` 不可达 — **真修复（含一个必须文档化的契约）**

- 采用 R1 审计两方案中较轻的一个：`DeviceBackend::stop_handle() -> Option<Arc<dyn Fn() + Send + Sync>>` 默认 `None`（`backend.rs:176-178`，文档明确"轮询 cancel 的后端返回 None 即正确"），`run_capture` 在 `stream` 前 `cancel.on_cancel(move || stop())`（`engine.rs:753-755`）。
- 正确性三支柱我逐一核验：① 每次 `INIT` 换新 `CancelToken`（`engine.rs:666`），钩子不跨 run 累积；② `on_cancel` 的"注册时已取消则立刻在调用线程执行"语义（`cancel.rs:83-95`，有测试）封死 INIT 后立即 ABOR 的窗口；③ `cancel_run` 保留 `stop_backend()`（槽内检查）覆盖 reader 尚未 take backend 的前置窗口。
- 测试用只认 stop 句柄、**故意不轮询 token** 的 `BlockingBackend`（`engine.rs:1045-1116`）：`abort_reaches_a_backend_that_only_wakes_through_its_stop_handle` 修复前 reader 会一直 parked、状态停 Armed，必红。这正是"为未来阻塞式 M300 后端拆雷"的验证形态。
- **遗留契约（Round 3 必须写进 trait 文档）**：`finish_run` 收尾的 `cancel.cancel()`（唤醒 watchdog，`engine.rs:936`）会在**正常完成后**也执行 stop 钩子。当前 Mock/Stream 均在 `stream()` 入口复位 `stopped`（`mock.rs:253`、`stream.rs:134`，两处我都查验过），无害；但 M300 后端若把句柄实现为 socket shutdown，每次成功 run 后持久链路会被切断。钩子必须**幂等且不得永久失效传输**——这句话现在只存在于守门报告里，不在代码文档里，是本轮唯一接近"隐患未封口"的点。

### B6（P2）ErrorQueue 第二次溢出无 `-350` — **真修复**

- 删除 sticky `overflowed` 字段，满队时按队尾判定（`errors.rs:36-40`），净 -10 行——与 R1 审计的最小修复建议逐字一致。SCPI-99"有错误丢失时最新可读条目为 `-350`"的不变量恢复。
- 测试：`a_second_overflow_after_a_partial_drain_is_marked_again`（drain 可见两枚 `-350`，修复前只有一枚）、`consecutive_overflows_share_one_marker`（连发只占一格）、既有 `overflow_replaces_the_last_entry` 原样通过；集成版 `error_queue_reoverflow.rs` 同口径。

### 顺带项 B8/B9/B10/B11 — 全部成立

- **B8**：`ReadOutcome::Unterminated`（`scpi_server.rs:281-285`）区分 EOF 截断与真超长，detail 改为 "line not terminated before EOF"；单测双向覆盖（EOF 截断的超长行仍按 TooLong）。fmt 阻塞已由 `729d376` 消除，我复跑 `cargo fmt --check` 通过。
- **B9**：`RunResult.duration_seconds`（`recording.rs:65-67`）在 `finish_run` 锁内取值（附"设置器在 run 中一律 -221，此处必为 armed 值"的论证注释，`engine.rs:872-874`），导出元数据改读 `result.duration_seconds`（`engine.rs:604`）。Complete 后 `CONF:REC:DUR` 再导出不再写错 preamble。
- **B10**：lexer `after_quote` 状态机（`lexer.rs:117-152`），闭引号后只容许空白与逗号，另收紧"引号必须是整个参数"（`x"a"` 也拒）；两个测试分别锁拒绝面与容许面。
- **B11**：`received()` 文档改为"到达即计数，`received() - dropped()` 才是通过量"，与实现一致。

### 判定总结

**五项主攻全部真修，零 papered over。** 修复共性：都消除根因而非加旁路；都有修复前必红的测试；都保住了三条不变量。整轮唯一的"新增债务"是 §2-B3 的 stop_handle 契约缺文档 + `project_mutation_while_running.rs` 里 `impl IntoAdoptResult for ()` 兼容垫片成为死代码（API 收敛后无人返回 `()`，clippy 不报，属化石性质）。

---

## 3. 剩余 SOTA 差距 vs Keysight 风格宿主（任务 #3）

按"阻塞 UTS 集成"与"打磨"分档。R1 审计 §4 的差距清单在 R2 后的存活状态：

### 3.1 条件性阻塞（唯一一档需要合同裁决的）

| 差距 | 现状 | 判定 |
| --- | --- | --- |
| **IEEE 488.2 强制通用命令**：`*ESE(?)`/`*ESR?`/`*SRE(?)`/`*STB?`/`*TST?`/`*WAI` 全缺，无 STATus 子系统 | `tree.rs:401-405` 仍只有 5 条；`Opc::bit_set` 仍不可观测（连简报允诺的"至多加注释"也没做，B13 仍开放） | **条件性阻塞**。若 UTS（PLAN §21.1 Q1）按 Keysight 惯例用 `*STB?` 轮询/`*ESR?` 判完成写脚本，这是集成期第一个撞上的墙；若 UTS 接受产品自有的 `REC:WAIT?`/`#REC:DONE`（本轮 `docs/SCPI.md` 已把该路径完整文档化，含阻塞语义与看门狗上界），则降级为打磨。**在合同回执前不动手是对的**——盲做整套寄存器组正是简报明令排除的过度设计 |

### 3.2 不阻塞，但属于 SOTA 宿主应有（Round 3 值得做的打磨）

1. **B7 通知写无超时**（`engine.rs:944` broadcast → `scpi_server.rs` `write_line` 无 `set_write_timeout`）：对端 TCP 窗口收满不读时 reader 线程泄漏、后续 sink 收不到该次通知。状态机本身不卡（`notifying` 有 watchdog+1s 兜底），所以不算协议级挂死，但**是全仓最后一个"单个恶意/半死客户端能造成持久资源泄漏"的点**。每 session socket 设写超时即可，勿上专职写线程。
2. **SCPI 口 `--bind`**：二进制固定 `0.0.0.0`、无鉴权，与设备口有 `allowedPeers` 不对称。`AppBuilder::bind_host` setter 已存在（仅测试可达），补 CLI/schema 面约 30 行——安全项里性价比最高的一个。export 路径遍历维持 won't-fix（台式仪器 MMEM 语义），配 3 行部署注记。
3. `SYST:ERR:COUN?`（可选 `:ALL?`）：UTS 清队列惯用，成本极低。
4. 会话空闲超时：8 个半开连接可占满槽位把真 UTS 挡在门外；现场真实发生。
5. `FORM REAL,32` definite-length block：100 kS/s × 数十秒的 ASCII 传输膨胀 2–3×；`format.rs` 已流式，加一条编码路径即可。
6. 可观测性：`dropped/received` 计数未上 SCPI 表面；无日志级别/落盘开关。
7. 性能四项（`FETC?` 每样本两次 write、每响应新建 64KiB BufWriter、CSV `time_s` 全精度、GUI 每帧深拷贝）本轮有意未动，维持简报"不阻塞正确性"的口径。

### 3.3 文档与流程（Phase 7 余项）

- `docs/SCPI.md` **本轮已落地**（命令表 + 错误表 + "Load/Apply 丢弃采集归 IDLE"专节，与实现逐条对得上，我抽查了 `-350`"替换最新条目"的表述与 B6 修复后语义一致）。Phase 7 剩：完整 UTS 注释转录、`cargo deny` 门禁、tagged release 流水线。
- `docs/PLAN.md` §5.3 状态图仍缺 `Invalidate` 边——规范图与代码漂移，SCPI.md 有行为文档但 PLAN 是 normative。

---

## 4. Round 3 建议（任务 #4）

### 4.1 必修（小成本、封口性质）

| # | 项 | 理由与规模 |
| --- | --- | --- |
| 1 | **`stop_handle` trait 文档补契约**："钩子在每次 run 结束（含正常完成）都会执行，必须幂等且不得永久失效传输" | §2-B3 的 Phase 6 地雷，一段文档；也可顺手在 `finish_run` 处交叉引用。这是 Round 3 优先级最高的一行字 |
| 2 | **B7 写超时**：session socket `set_write_timeout` | 最后的单客户端资源泄漏点；几行 + 一个慢消费者测试 |
| 3 | **`--bind`/`server.bindHost`** | §3.2-2，约 30 行，安全不对称的最低成本补偿 |
| 4 | **文档漂移**：PLAN §5.3 补 Invalidate 边；§8 `*CLS` 措辞去掉不存在的"状态/事件寄存器" | 纯文档 |
| 5 | **信息级清理打包**：B12（`Refusal::SocketError` 变体）、B13（`Opc::bit_set` 注释或 `*ESR?`）、B14（`is_query` 死代码）、`IntoAdoptResult` 死 impl | 各一两行，一次提交 |

### 4.2 按合同定夺（不要盲做）

- **488.2 最小寄存器集**（`*ESR?`/`*STB?`/`*ESE(?)`/`*SRE(?)`/`*WAI`/`*TST?` + 最小 ESR/STB 位模型）：拿到 §21.1 Q1 回执且 UTS 确需 488.2 轮询 → 升 P0；UTS 接受 `REC:WAIT?`/`#REC:DONE` → 归 4.3。`Opc` 记账已在，真做时主要是 tree/dispatch 增条目 + 一个小寄存器结构。

### 4.3 接受为已知（有意不做，写明理由）

- Phase 6 M300 FFI（§21.1 SDK ABI 门禁 + D21 禁假 DLL）与 Advanced 页真实控件——门禁未解除。
- export 路径遍历（仪器 MMEM 语义；缓解 = `--bind` + 部署注记）。
- mock 首批交付前 `REC:STAT?` 停 ARMED（批粒度固有，opus-B 已论证）。
- `FETC?` 仅 ASCII、性能四项、会话空闲超时、`SYST:ERR:COUN?`、链路统计查询——按需求方反馈排期，无一影响正确性。
- 完整 488.2 状态模型 + SRQ——无硬件合同前不做。

### 4.4 交接注记

- 本地与 origin 已同步于 `729d376`，工作树除 `.agent_workspace/round2/probes/regression-*/**.log` 未跟踪日志外干净。
- 528 项测试构成的回归网现覆盖：spawn 失败双路径、sequence 迟到完成、锁内 adopt 拒绝、paced 取消三角（cancel/stop/仍节流）、错误队列二次溢出、lexer 引号尾随——Round 3 改 engine/device 任何一处，这些先红。

---

## 5. 方法与限制

- 逐行读了 R2 全部产品 diff（`git diff 1da8740..HEAD`，10 个产品文件 + 5 个测试文件 + `docs/SCPI.md`），并对 `engine.rs`（修复后全文）、`mock.rs`（全文）、`errors.rs`（全文）、`backend.rs`（全文）、`state.rs`（全文）、`cancel.rs`（全文）、`stream.rs`（stream/stop 段）做了不依赖 diff 的通读。
- 两个时点独立复跑验证：R2 落地前 `1da8740`（509 全绿 + clippy 干净）与最终 `729d376`（fmt/528 测试/clippy/check-deps 四绿）。GUI feature 组合以 gpt-A 在 `0a84fb4` 的探针（PASS，`probes/regression-workspace/summary.tsv`）为证据——其后仅 fmt 空白改动，不影响判定。
- 对守门报告（fable_b_gate.md）的结论做了抽样对抗核验而非照单全收：B4 的 TestClock 论证、B3 的三支柱、stopped 复位点、`IntoAdoptResult` 死代码、fmt 修复，均独立确认。
- 未做真机 GUI 截图（无显示器）；Windows 交叉构建以 CI 必选 job 为证据，本环境无 mingw。
- 一个流程观察供编排器参考：兄弟 Agent 直接提交**本地**分支，我对 origin 的 20 分钟轮询因此扑空——Round 3 若仍多 Agent 共享工作区，复审方应轮询本地 HEAD 而非 remote。
