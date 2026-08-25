# QuickVib Round 1 源码审计 — R1-fable-B

- **审计对象**：`/workspace`，分支 `cursor/review-optimize-f6c8`
- **审计基线**：初始产品代码基线为 `5895498`（产品 crate 与 `a1a7f86` 相同）。审计进行期间有并发 Round 1 agent 持续提交修复（`064d172`、`1a6db07`），本报告所有 file:line 均以**当前 HEAD `1a6db07`** 复核为准，并单独标注"已在基线后修复"的项。
- **方法**：全量通读 11 个 crate 的产品源码（约 1.9 万行），对照 `docs/PLAN.md` §5.4 / §7 / §8 / §9 / §10。未修改任何 crate，未运行 `cargo`。

---

## 0. 总评

代码质量整体很高：无 `unsafe`（全 workspace `#![forbid(unsafe_code)]`，`quickvib-m300` 仅为脚手架、尚无 FFI），锁纪律严格（持锁不做 I/O、`Condvar::wait_timeout_while` 阻塞查询全部有界）、poison 统一用 `unwrap_or_else(PoisonError::into_inner)` 恢复、panic 收容（reader 线程与 SCPI session 均 `catch_unwind`）落实到位，SCPI 面与 PLAN §8 高度一致且测试覆盖细。发现 1 个高危状态机卡死 bug（B1）、1 个已被并发 agent 修复的高危不变量破坏（B2，即待验证项，**确认成立**）、以及若干中低危问题，详见 §5。

---

## 1. 并发

### 1.1 Mutex / Condvar 纪律 —— 合格

- 引擎单锁模型（`Engine::state: Mutex<EngineState>` + `changed: Condvar`）与 PLAN §5.4 一致。`REC:WAIT?` / `*OPC?` 通过 `wait_timeout_while` 等待（`crates/quickvib-engine/src/engine.rs:512-543`），等待期间释放锁，且上界为 `watchdog + 1s`，不会无限挂起。
- `notify_all` 一律在 `drop(guard)` 之后调用（如 `engine.rs:344-345`、`sink.rs:163-164`），无"持锁通知"反模式。
- `finish_run` 在锁外做 `broadcast`，用 `notifying` 标志（而非引擎锁）保证 `#REC:DONE` 先于阻塞查询返回（`engine.rs:904-910`）——设计正确；但同步逐 sink 写 socket 无发送超时，见 B7。

### 1.2 recording 线程 panic 收容 —— 合格（含一个失效路径）

- reader 线程 `catch_unwind(AssertUnwindSafe(...))`，panic 后按 `RunOutcome::LinkLost`（-240）收尾（`engine.rs:685-696`），符合 D27 / PLAN §7.10。
- SCPI session 同样收容，panic 后向队列压 `-100`、只关本 session（`crates/quickvib/src/scpi_server.rs:234-248`），且 `FAULT_INJECTION_PANIC` 默认关闭。
- **例外**：reader 线程 *spawn 失败*的兜底路径本身是坏的（B1，见 §5）。

### 1.3 TCP accept 循环 —— 合格

- 两个监听器均为非阻塞 accept + `cancel.wait_timeout(5ms)` 轮询（`scpi_server.rs:118-131`、`quickvib-device/src/listener.rs:173-187`），取消响应及时且不空转（每循环 park 在 Condvar 上）。
- SCPI 会话上限用 `AtomicUsize::fetch_update` 在 spawn 前原子预留（`scpi_server.rs:143-158`），两次并发 accept 无法同时挤过上限；spawn 失败正确回滚计数。行为符合 PLAN §5.4"accept 后拒绝并关闭"。
- 设备口同一时刻只接一条链路，第二条按 `AlreadyConnected` 立即关闭（`listener.rs:201-205`）。`is_connected` 检查与 `state.set(Some)` 都在单线程 accept 循环内，无自竞争。

### 1.4 SampleChannel drop-oldest —— 正确

- `on_samples` 先 append 再 `drain(..overflow)` 丢最旧，`dropped` 计数单调（`quickvib-device/src/sink.rs:167-182`）；`take` 用 `wait_timeout_while(|s| s.samples.is_empty() && s.connected)` 正确区分 `Idle` / `Disconnected`，且断链后先排空残余再报 `Disconnected`（`sink.rs:137-155`）——与模块文档声明的两条设计动机完全吻合。
- 小疵：`received()` 文档写 "dropped ones excluded"，实际计数包含随后被挤掉的样本（B11）。

### 1.5 Poison 处理 —— 合格

全部锁点（engine、sinks、backend 槽、CancelToken、SampleChannel、ConnectionState、logger、session writer）统一 `unwrap_or_else(PoisonError::into_inner)`，配合 `engine.rs:141-143` 的注释论证（状态为纯数据、状态机每次转移重校验），策略自洽。`CancelToken`（`quickvib-core/src/cancel.rs`）的 hook 语义（已取消时注册即回调、hook 恰好执行一次）实现正确且有测试。

### 1.6 值得注意的结构性风险

- **`stop_backend()` 在 run 进行期间必然是 no-op**（B3）：`run_capture` 通过 `take_backend()` 把 backend 从 `Engine::backend` 槽里取走（`engine.rs:241-248`、`720`），因此 `ABOR`/watchdog 调 `stop_backend()` 时槽内是 `None`（`engine.rs:814-819`），`DeviceBackend::stop()` 在活动 run 中**从未被调用**——这与 PLAN §10 "stop(&self) 必须能在 stream 持有 &mut self 时从另一线程调用"的设计意图相悖。当前两个后端（`StreamBackend`、`MockBackend`）都以 5–10ms 轮询 `cancel`，所以被掩盖；一旦 Phase 6 的 M300 FFI/阻塞 socket 后端依赖 `stop()` 解除阻塞读，`ABOR` 将失效。
- **paced mock 的不可中断 sleep**（B4）：一次 `clock.sleep(batch_len/rate)` 最长可睡数百秒，期间 cancel 无法唤醒。

---

## 2. SCPI 正确性 vs PLAN §8

### 2.1 lexer（`quickvib-scpi/src/lexer.rs`）—— 基本合格

- 头部大写化、`:` 分段、`*` common、尾部 `?`、引号参数（含双写引号转义、单引号）、逗号分参均正确；非 ASCII、空节点、非法字符 → `-100`，符合 §9。
- 缺陷：**闭引号后的尾随字符被静默并入参数**——`MMEM:LOAD:STAT "a"x` 得到 `Quoted("ax")`，`"a" "b"` 得到 `"a b"`（`lexer.rs:122-147` 状态机在 `quote=None` 后继续 `current.push(c)`）。标准 SCPI 应报 `-100`（B10，低危：正常客户端不会触发）。

### 2.2 header-path carry —— 正确实现

`parse_line`（`quickvib-scpi/src/session.rs:56-97`）实现了完整的 §7.2 语义：相对头拼接前一消息头去掉末节点的路径、前导 `:` 复位、`*` common 不扰动路径、解析失败中止整行（IEEE 488.2 行为）。测试 `header_path_carries_across_a_compound_message` 等覆盖到位。**此前怀疑 tree.rs 未消费 `leading_colon` 是误判**——carry 在 session 层完成后才进 `resolve`。

### 2.3 IEEE 488.2 —— 与 PLAN 一致，两处备注

- `*IDN?`（4 字段、identity 可配置、serial 回退设备序列号）、`*RST`（终止 run、丢弃 capture、恢复项目 duration/format、清错误队列、回 Idle）、`*CLS`、`*OPC`/`*OPC?`（有界阻塞）均按 PLAN §8.1 实现。注意 `*RST` 清错误队列是 PLAN 的明确规定（与 IEEE 488.2 原文不同），实现忠实于 PLAN。
- 备注 1：`Opc::bit()`（标准事件寄存器 OPC 位）**没有任何 SCPI 命令可读**（无 `*ESR?`，PLAN 也未列），`bit_set` 是死状态（B13）。
- 备注 2：会话读到"无终止符即 EOF 的最后一行"时归为 `TooLong`，压入的 `-100` 详情写 "exceeded 65536 bytes"，文案误导（B8，`scpi_server.rs:333-341`）。

### 2.4 错误队列溢出 -350 —— 基本符合，边角有偏差

`ErrorQueue`（`quickvib-engine/src/errors.rs:27-39`）深度 32、满时弹掉队尾换 `-350`，符合 §8.1。**边角偏差（B6）**：`overflowed` 标志只在队列**完全排空**时复位（`errors.rs:45-53`）。序列"灌满→溢出（队尾=-350）→pop 1 条→push 新错误 E（重新灌满）→再 push F"时，F 被静默丢弃而队尾 E **不会**被替换为 `-350`——第二次溢出事件无标记。UTS 仍能在队列中部看到一个旧 `-350`，但"最新条目应为 -350"的 SCPI-99 不变量被破坏。

### 2.5 abort D9 丢弃 capture —— 正确

`finish_run` 中非 `Complete` 终态一律 `guard.result = None`（`engine.rs:884-889`）；`capture()`/`measurements()` 要求 `(Some(result), State::Complete)` 同时成立（`engine.rs:551-569`）。因此 `ABOR` / watchdog / 断链后 `FETC?`、`TRAC:POIN?`、`CALC:*` 全部 `-230`，操作员 abort 不产生错误码、watchdog 产生 `-365`、断链产生 `-240`（`fire_watchdog`/`abort` 写 `abort_reason`，`finish_run` 依其改写 outcome，`engine.rs:827-834`）——与 §8.4/§9/D9 完全一致，测试 `an_aborted_run_publishes_no_data` 锁定该行为。

### 2.6 其余命令面

`SYST:ERR?`（`0,"No error"`、`SYST:ERR:NEXT?` 别名）、`MMEM:*`（-256/-257/-221/-224 映射经 `ProjectError::scpi_error`）、`CONF:REC:DUR`（`{:.3}`、范围 (0,3600]、buffer cap `-222`、运行中 `-221`）、`FORM`（`-224`）、`INIT`/`REC:STAR`/`INIT:IMM`、`REC:STAT?` 五态字串、`FETC?`/`TRAC:DATA?`/`TRAC:POIN?`、`CALC:MEAS:*`（`{:.decimals}`）、`SYST:DEV:CONN?`、`SYST:VERS?`（1999.0）、`#REC:DONE`/`#REC:ABORT` 均与 §8 表格逐项对得上；短形/长形匹配拒绝部分补全（`tree.rs:307-310`），符合 D6。

---

## 3. API / crate 边界

依赖方向与 PLAN §6.2 相符：`core` 零依赖；`scpi` 不依赖 `engine`（`engine/src/lib.rs` 文档明确单向）；`device` 不知道 SCPI；`measure`/`project` 平台无关；bin crate 是唯一组合根。以下为观察项（非 bug）：

1. **engine 公开面偏大**：`pub use errors::ErrorQueue; pub use opc::Opc; pub use state::transition` （`engine/src/lib.rs:53-58`）把内部机件全部导出。`ErrorQueue`/`Opc` 只被 engine 自身使用，`transition` 只被测试消费。建议收敛为 `pub(crate)` 或标注 `#[doc(hidden)]`，防止外部依赖内部不变量。
2. **`Engine::capture()` 返回 `Arc<Vec<f32>>`**（`engine.rs:551`）——暴露了内部存储类型。换 `Arc<[f32]>` 更中性且省一次间接；当前形态为性能考虑可接受。
3. **bin 深入 device 子模块**：`use quickvib_device::framer::Framed; use quickvib_device::listener::Refusal;`（`quickvib/src/device_server.rs:22-23`），而其它类型走 crate 根导出。建议把 `Framed`/`Refusal` 提到根导出，统一边界。
4. **`Command::is_query()`**（`quickvib-scpi/src/command.rs:86-106`）产品代码零调用（响应有无由 `Response::is_some` 决定），属死 API（B14）。
5. `NotificationSink` 定义在 engine 而 `NOTIFY_*` 常量在 scpi——按 PLAN 分工可接受，跨 crate 常量单点定义正确。
6. UI 分层干净：`controller/form/i18n/ports` 无窗口依赖，`window` 由 `gui` feature 门控，所有变更经 `UiController` 进入同一个 `Arc<Engine>`——与 SCPI 完全共享状态，无第二份真相。

---

## 4. 安全

### 4.1 unsafe —— 无意外

每个 crate（除 `quickvib-m300`）均 `#![forbid(unsafe_code)]`；`quickvib-m300` 允许 unsafe（D22）但当前**零 unsafe、零 extern**，只有纯路径探测逻辑（`resolver.rs`）。framer 热循环用 `chunks_exact(4)+f32::from_le_bytes`，无越界风险。mock 自带 Xoshiro256++ 纯安全实现。

### 4.2 export 路径遍历 —— 存在，需明确风险定位

- `resolve_path`（`quickvib-measure/src/export/mod.rs:104-110`）对相对路径仅做 `base.join(path)`，**不做 canonicalize / 包含性校验**：`MMEM:STOR:TRAC "../../../../etc/cron.d/x"` 可逃出 `export.directory`；绝对路径原样放行；`write_capture` 还会 `create_dir_all` 任意父目录（`mod.rs:139-145`）。`MMEM:STOR:STAT`、`MMEM:LOAD:STAT` 同理接受任意路径。
- 放大因素：**SCPI 口无任何鉴权、无 allow-list，且二进制固定绑定 `0.0.0.0`**——`AppBuilder::new` 默认 `bind_host: "0.0.0.0"`（`quickvib/src/app.rs:128`），CLI 没有 `--bind` 类选项（已核对 `cli.rs`），`bind_host()` setter 仅测试可达。即局域网内任意主机可以以进程权限写任意可写路径（内容受限于 CSV/TXT/项目 JSON 格式）。
- 评估：传统台式仪器的 `MMEM` 语义本就允许任意路径，UTS 内网部署下通常可接受；但与设备口有 `allowedPeers` 而 SCPI 口没有的**不对称**值得修正。建议：(a) 增加 `--bind`/`server.bindHost`；(b) 给 SCPI 口对等的 allow-list；(c) 至少提供可选的"export 限制在 `export.directory` 内"（`canonicalize` 后前缀校验）开关。

### 4.3 设备口 peer filter —— 已实现且位置正确

`is_peer_allowed` 在交给 framer **之前**执行，拒绝即 `shutdown`（`listener.rs:196-200`）；空列表放行所有（文档化行为）；条目匹配 IP 或完整 `ip:port` 字符串。注意匹配是字符串等值：IPv6 表示差异或 `192.168.001.010` 之类写法不会命中——建议 parse 成 `IpAddr` 再比较。另有一处拒绝理由误标（B12）。

---

## 5. 具体 bug（file:line + 修复建议）

行号以当前 HEAD `1a6db07` 为准。

### B1（高）reader 线程 spawn 失败时 run 永久卡在 Armed

`crates/quickvib-engine/src/engine.rs:697-700`：

```697:700:crates/quickvib-engine/src/engine.rs
        if spawned.is_err() {
            self.log(Level::Error, "rec", "could not spawn the reader thread");
            self.finish_run(0, RunOutcome::LinkLost, Vec::new(), Duration::ZERO);
        }
```

`start_recording` 已把 `guard.sequence` 递增到 ≥1，而这里传 `sequence=0`，`finish_run` 首行 `guard.sequence != sequence` 命中即 return（`engine.rs:822-826`）——兜底完全失效。后果：状态停在 `Armed`，watchdog 只会 `cancel()` 而不会调 `finish_run`（它依赖 reader 收尾），于是 `INIT` 永远 `-221`、`opc` 永远 in progress，直到 `*RST`。
**修复**：spawn 前取 `let sequence = plan.sequence;`，失败分支传该值（`spawn_watchdog` 失败分支最好也补一个同样的收尾，防止两个都 spawn 失败时无人清场）。

### B2（高，已修复）——待验证项判定：**确认成立（于原基线）**

原基线 `5895498` 的 `adopt_project` 只清 `result`/`outcome`，**不改 `state`**：run 完成后加载/Apply 新项目 → `REC:STAT?` 仍答 `COMPLETE`、`REC:WAIT?` 答 `1`，而 `FETC?`/`TRAC:POIN?`/`CALC:*` 全部 `-230`——状态与数据可见性矛盾，且 `state.rs` 里并不存在处理该场景的事件。并发 agent 已在 `064d172` 修复：新增 `Event::Invalidate`（settled 态 → `Idle`，run 中拒绝），`adopt_project` 现于 `engine.rs:339-341` 调用它，并配了 engine/dispatch/集成三层测试。**当前 HEAD 无此问题**；修复方案与我独立得出的建议一致，无需返工。

### B3（中）`DeviceBackend::stop()` 在活动 run 中不可达

`engine.rs:719-728`（`take_backend` 取走 backend）与 `engine.rs:814-819`（`stop_backend` 只对槽内 `Some` 调 `stop`）组合，使 `ABOR`/watchdog 期间 `stop()` 永远不会执行，违背 PLAN §10 对 `stop(&self)` 的设计意图。当前靠后端轮询 `cancel`（5–10ms）掩盖，对未来阻塞式 M300 后端是定时炸弹。
**修复**：在 `take_backend` 时同时留存一个 stop 句柄——例如给 trait 加 `fn stop_handle(&self) -> Arc<dyn Fn() + Send + Sync>`，或在 `run_capture` 进入 `stream` 前 `cancel.on_cancel(move || { let _ = handle.stop(); })`（`CancelToken::on_cancel` 正是为"用 socket shutdown 解除阻塞读"设计的，`cancel.rs:78-95`，目前无人使用该能力）。

### B4（中）paced mock 的 `ABOR`/watchdog 响应可延迟数秒至数百秒

`crates/quickvib-device/src/mock.rs:264-268`：

```264:268:crates/quickvib-device/src/mock.rs
            if self.pace && self.sample_rate_hz > 0.0 {
                self.clock.sleep(Duration::from_secs_f64(
                    batch_len as f64 / self.sample_rate_hz,
                ));
            }
```

批大小最多 4096：1 kHz 时单次 `thread::sleep` 4.096 s；10 Hz 时可达 409 s。睡眠期间 `cancel`/`stopped` 均无法唤醒（仅在批间检查，`mock.rs:249-251`）。生产二进制 `pace_mock=true`（`app.rs:135`），GUI「停止」按钮与 `ABOR` 在低采样率 mock 下体感失灵；watchdog 触发后状态同样要等睡醒才收尾。另外 sleep 在 `on_batch` **之前**，首批样本前 `REC:STAT?` 长时间停留 `ARMED`。
**修复**：把整段 sleep 换成 `if cancel.wait_timeout(batch_duration) { return Ok(StreamOutcome::Cancelled); }`（真实时钟下语义等价且可中断；TestClock 路径不受影响，因为测试从不开 pacing），或切成 ≤10ms 片轮询。

### B5（中）`load_project` 的 `-221` 检查与生效之间存在 TOCTOU

`engine.rs:304-312`：`self.lock().state.is_running()` 检查后立刻释放锁，随后是**磁盘 I/O**（`ProjectStore::load`）再 `adopt_project`。窗口内另一 session 可完成 `INIT`：新项目会在 run 进行中被装入，`duration_seconds`/`format` 被覆盖（`adopt_project` 无条件写入），`finish_run` 读到的 `measurement.remove_dc` 来自新项目——run 的配置一致性被破坏（`Invalidate` 修复只保住了 `state`，其余字段仍被覆盖）。
**修复**：把校验挪进 `adopt_project` 的锁内并让其返回 `Result`——`if guard.state.is_running() { return Err(ScpiError::SettingsConflict); }`；`load_project` 保留外层预检仅作快速失败。

### B6（低）ErrorQueue 第二次溢出不再标记 `-350`

`crates/quickvib-engine/src/errors.rs:27-39` + `42-55`：`overflowed` 只在队列彻底排空时复位；"溢出→pop 一条→push 至满→再 push"时新错误被静默丢弃且队尾不换 `-350`（详见 §2.4）。
**修复**：删除 `overflowed` 字段，push 满时改为 `if self.entries.back().map(|e| e.error) != Some(ScpiError::QueueOverflow) { self.entries.pop_back(); self.entries.push_back(ScpiErrorEntry::new(ScpiError::QueueOverflow)); }`。

### B7（低）广播通知无发送超时，可永久占死 reader 线程

`engine.rs:908`（`broadcast` 在 reader 线程同步执行）→ `scpi_server.rs:197-202`（`write_line` 直接 `write_all` 到 `TcpStream`，无 `set_write_timeout`）。对端 TCP 窗口收满且不读时，`write_all` 无限阻塞：该 run 的 reader 线程泄漏、其后注册的 sink 收不到本次通知（顺序遍历，`engine.rs:180-191`）。状态机本身不卡（`notifying` 有 watchdog+1s 兜底，且状态转移先于 broadcast 完成）。
**修复**：session socket `set_write_timeout(Some(...))`，或通知走每 session 的有界队列 + 专职写线程。

### B8（低）无终止符的最后一行被误报为"行过长"

`scpi_server.rs:333-341`：`read > 0 && !ends_with(b"\n")` 时（对端 half-close 前最后一段无 `\n`）返回 `TooLong`，随后压入 detail 为 "input line exceeded 65536 bytes" 的 `-100`。按 -100 处理可辩护（IEEE 488.2 要求终止符），但 detail 文案错误。
**修复**：区分 `line.len() > MAX_LINE_BYTES`（真超长）与"EOF 截断"，后者 detail 改为 "line not terminated before EOF"。

### B9（低）`MMEM:STOR:TRAC` 元数据的 `durationSeconds` 取当前 override 而非该次 run 的值

`engine.rs:590`：`duration_seconds: guard.duration_seconds`。run 完成后（Complete 态允许）执行 `CONF:REC:DUR 2` 再导出，CSV preamble 写 `durationSeconds=2` 而数据是原时长的样本。
**修复**：在 `RunResult` 里记录该 run 的 `duration_seconds`（`RunPlan` 已有），导出时从 result 读。

### B10（低）lexer 闭引号后的尾随字符被并入参数

`quickvib-scpi/src/lexer.rs:122-147`，见 §2.1。**修复**：加 `after_quote: bool`，闭引号后遇到非 `,`/空白字符即 `ParseError::command("trailing characters after quoted string")`。

### B11（信息）`SampleChannel::received()` 文档与实现不符

`quickvib-device/src/sink.rs:93-97` 说 "dropped ones excluded"，实现 `sink.rs:179-181` 无条件累加整批。改注释即可（测试 `reset_discards_what_arrived_while_idle` 实际依赖当前语义）。

### B12（信息）`set_nonblocking(false)` 失败的拒绝理由误标

`listener.rs:206-210` 回报 `Refusal::PeerNotAllowed`，日志会写 `reason=peerNotAllowed`，误导排障。可加 `Refusal::SocketError` 变体。

### B13（信息）`Opc::bit_set` 不可观测

`quickvib-engine/src/opc.rs`：无 `*ESR?`，OPC 位无读取途径。要么实现 `*ESR?`，要么注释说明留待后续。

### B14（信息）`Command::is_query()` 死代码

`quickvib-scpi/src/command.rs:86-106` 仅测试使用。删除或 `#[doc(hidden)]`。

---

## 6. 性能热点

按影响排序（目标场景：500 000 样本 / 100 kS/s / 4 Hz GUI 刷新）：

1. **`FETC?` ASCII 序列化**（`quickvib-scpi/src/format.rs:82-97`）：每样本 2 次 `write_all`（逗号+数值）+ 1 次 `fmt::Display`（f32 最短往返格式化）。500k 样本 ≈ 100 万次 BufWriter 调用 + 50 万次 Grisu 格式化，估计数十 ms。改进：把逗号并入 scratch（`scratch.push(',')` 后 `write!`）减半调用数；进一步可积累 ~64 KiB 块再整体 `write_all`。当前 `Response::Samples(Arc<Vec<f32>>)` 已避免物化多 MB 字符串、写路径在引擎锁外——设计正确。
2. **每条响应行新建 64 KiB `BufWriter`**（`quickvib/src/scpi_server.rs:188-195`）：`write_responses` 每次分配并丢弃 64 KiB 缓冲。对高频短查询（UTS 轮询 `REC:STAT?`）是纯浪费。改进：缓冲区常驻 `Session`（`Mutex<(TcpStream, Vec<u8>)>`），或对非 `Samples` 响应直接小写。
3. **CSV 导出 `time_s` 全精度格式化**（`quickvib-measure/src/export/csv.rs:40-44`）：`index/rate` 的 f64 最短往返表示常见 17 位（如 `0.30000000000000004` 类），既拖慢格式化又使文件膨胀 ~2×。改进：`time_s` 用固定小数位（由 rate 推导所需精度）或累加 `dt` 输出定长。捕获写盘路径（temp + fsync + rename 原子替换、64 KiB BufWriter）本身设计良好。
4. **capture 分配**：`Vec::with_capacity(expected)` 一次到位（`engine.rs:732`）、批间 `extend_from_slice` 无锁无分配、`finish_run` 直接 move 进 `Arc`——已是最优形态。唯一小点：`INIT` 在持引擎锁时 `guard.result = None` 可能触发上一个最多 512 MiB capture 的释放（若无其他 `Arc` 引用），锁内大段 `free`；可先 `let _old = guard.result.take();` 挪到锁外 drop。
5. **GUI 帧工作**（`quickvib-ui/src/window.rs:1030-1031`、`controller.rs:363-385`）：每帧 `snapshot()` 约 10 次引擎锁往返 + **整个 `Project` 深拷贝**（含字符串/数组）+ `identity()` 再拷贝；`window.rs:283` 每帧 `is_dirty()` 重建 `ProjectForm`（数十个 `String` 分配）再比较。空闲 4 Hz（`request_repaint_after(250ms)`，`window.rs:1067`）无压力，交互期 60+ Hz 也仅是浪费而非卡顿；低成本改进：`snapshot` 合并为一次锁内采集、`is_dirty` 缓存基线 form 或改脏标记。
6. **accept 轮询**：两个循环各 5ms 唤醒（合计 ~400 次/s），CPU 占用可忽略；若追求零唤醒可改阻塞 accept + 自连接唤醒，不值当。
7. **`SampleChannel` 逐元素 `extend`/`drain`**（`sink.rs:172-175`）：`VecDeque<f32>` 逐元素拷贝，100 kS/s 下 ≈ 每秒 20 万次元素操作，微不足道；如未来上 MS/s 可换双缓冲 `Vec` 交换。

---

## 7. 附：与 PLAN 的其余一致性抽查（无问题项）

- 线程清单与 §5.4 六类一一对应（SCPI accept / session / engine / device accept / reader / watchdog），全部 `std::thread`，无 async 运行时（D19）。
- `expected_samples` checked 算术 + `maxCaptureBytes` 守卫（`schema.rs:392-409`），`watchdog = duration×multiplier+1s` clamp 到 [1ms, 86400s]（`schema.rs:413-416`），与 §7.9/§9 一致。
- `measure::compute` f64 单/双 pass、NaN 传播为文档化行为（D15），`format_fixed` locale 无关（D16）。
- 时钟注入（D20）贯彻彻底：`clippy.toml` disallowed-methods 封锁 `Instant::now`/`SystemTime::now`，唯一豁免点在 `core::clock`。
- 导出原子写（temp sibling + `sync_all` + `rename`）符合 §7.8 "UTS 轮询永不见半个文件"。
- `m300-sim` / testkit 未纳入本轮深审（非产品路径）。

---

*报告完。仅写入本文件；未改动任何 crate；未提交、未推送。*
