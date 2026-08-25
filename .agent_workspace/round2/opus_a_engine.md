# R2-opus-A — `quickvib-engine` 修复（B1 / B5 / B3 / B9）

- **模型:** `claude-opus-5-thinking-high-fast`（无降级）
- **分支:** `cursor/review-optimize-f6c8`（未提交、未推送，按指令留在工作区）
- **范围:** B1（P0）、B5（P0）、B3（P1）、B9（P2）
- **未触碰:** `mock.rs`、`errors.rs`、scpi lexer（B4 / B6 / B10 属 opus-B）

---

## 1. B1（P0）reader spawn 失败时 run 永久卡在 Armed

**根因**：`start_recording` 已把 `guard.sequence` 递增到 ≥1，而 `spawn_reader` 的失败分支传
`finish_run(0, …)`；`finish_run` 首行 `guard.sequence != sequence` 立即 return，兜底完全失效。
状态停在 `Armed` → `INIT` 永远 `-221`、`*OPC?` 永远 in progress，直到 `*RST`。

**修复**（`engine.rs`）：

1. 在闭包 move 走 `plan` 之前取 `let sequence = plan.sequence;`，失败分支传真实序号。
2. **watchdog spawn 失败**同样收尾：没有看门狗的 run 一旦设备 stall 就再没有任何东西能结束它，
   与 B1 是同一类缺陷。失败时调用新的 `cancel_run(Some(sequence), RunOutcome::LinkLost)`：
   - reader 已起来 → 收到 cancel，由 reader 自己走正常收尾路径（无并发写状态机的风险）；
   - reader 也没起来 → reader 的失败分支已经把状态落定，此处按序号/状态检查为 no-op。
3. 顺带把 `fire_watchdog` / `abort` 里重复的「检查 → 记录 abort_reason → stop_backend → cancel」
   三段收敛为 `cancel_run(sequence: Option<u64>, reason)`；`None` 表示「当前在跑的那个 run」。

**测试注入手段**：新增 `spawn_detached(name, body)` 作为唯一 spawn 入口，配一个 `#[cfg(test)]`
的 **thread-local** 种子 `FAIL_SPAWN`（值为「下一次哪个线程名 spawn 必须失败」）。thread-local
而非全局，因此并行测试互不干扰；真去耗尽线程上限会拖垮整个测试进程，不可取。

**新增测试**（`engine.rs`）：

- `a_reader_that_cannot_be_spawned_settles_the_run`：状态落到 `Aborted`、`-240` 入队、
  `REC:WAIT?` 立即返回 false，且**随后一次完整 run 仍能成功**（证明仪器没被卡死）。
- `a_watchdog_that_cannot_be_spawned_ends_the_run_it_cannot_supervise`：stall 后端 + 看门狗
  spawn 失败 → run 被结束而不是无限挂起。

---

## 2. B5（P0）`load_project` 与 `INIT` 的 TOCTOU

**根因**：`load_project` 先 `is_running()` 检查、**放锁**、做磁盘 I/O、再 `adopt_project` 无条件
写入。窗口内另一 session 可以完成 `INIT`：`duration_seconds` / `format` / `project`（进而
`measurement.remove_dc`）在 run 进行中被换掉。R1 的 `Invalidate` 只保住了 `state`。

**修复**：把检查搬进 `adopt_project`，与写入共用同一次持锁：

```rust
pub fn adopt_project(&self, project: Project, path: Option<PathBuf>) -> Result<(), ScpiError> {
    let mut guard = self.lock();
    if guard.state.is_running() {
        return Err(ScpiError::SettingsConflict);
    }
    ...
    // `Invalidate` 对每个 settled 态都有边，运行态已在上面被拒。
    guard.state = transition(guard.state, Event::Invalidate).unwrap_or(State::Idle);
    ...
}
```

`load_project` 保留锁外预检**仅作快速失败**（并注释说明它不是权威检查），随后
`self.adopt_project(project, Some(path))?`。R1 的 Invalidate 语义原样保留。

**API 变更（跨 crate，注意）**：`Engine::adopt_project` 由 `()` 改为 `Result<(), ScpiError>`。
这与并发 agent 新增的红测 `tests/project_mutation_while_running.rs` 的期望一致（该文件用
`IntoAdoptResult` 兼容 trait 写成，断言要求 `Err(SettingsConflict)`）。调用方相应更新：

| 文件 | 改动 |
| --- | --- |
| `quickvib/src/app.rs:242` | 启动期唯一调用，`let _ =` + 一行注释说明此时不可能有 run |
| `quickvib-ui/src/controller.rs` `apply()` | `map_err(\|_\| run_in_flight())?`；把原先内联的 `RunInFlight` 错误提成 `fn run_in_flight()` 复用 |
| `quickvib-ui/src/controller.rs`（2 处测试） | `.unwrap()` |
| `quickvib-engine` 内测试 / `dispatch.rs` 测试 / `tests/adopt_project_semantics.rs` / `lib.rs` doctest | `.unwrap()` |

> **越界说明**：任务约定「不编辑 UI」。但 `RUSTFLAGS: -D warnings` 是 CI 全局设置，未使用的
> `Result` 会直接让构建失败，所以 UI 的 3 处调用点必须同步。改动是外科式的：`apply()` 内 4 行 +
> 提取一个 8 行 helper + 2 处测试 `.unwrap()`，全部用 StrReplace 局部替换，未触碰
> `snapshot()` / `is_dirty()` 等性能改造区域。`cargo clippy -p quickvib-ui -p quickvib
> --features gui --all-targets -- -D warnings` 通过。

**新增测试**：

- `adopting_a_project_mid_run_is_refused_under_the_lock`：显式复现「调用方看到 settled 态 →
  另一 session `INIT` → 采纳才落地」的窗口，断言 `-221` 且 duration / format / 项目名三者均未变。
- `adopting_a_project_mid_run_leaves_the_run_in_flight`（R1 旧测试）加强为断言返回 `Err` +
  duration / format 未被覆盖。

---

## 3. B3（P1）活动 run 中 `DeviceBackend::stop()` 不可达

**约束**：`stream(&mut self)` 在 reader 线程上独占后端，安全 Rust 里没有任何办法在此期间从另一
线程拿到 `&self` 调 `stop()`。因此必须**在 run 开始前**取出一个脱离对象生命周期的句柄。

**修复**：

1. `quickvib-device/src/backend.rs`：给 trait 加一个**带默认实现**的方法（纯增量，未改
   `mock.rs` / `stream.rs`，两者继续返回默认 `None`，行为零变化）：

   ```rust
   fn stop_handle(&self) -> Option<Arc<dyn Fn() + Send + Sync>> { None }
   ```

2. `engine.rs` `run_capture`：进入 `stream` 之前把句柄挂到本次 run 的 `CancelToken` 上——
   `cancel.on_cancel(move || stop())`。`ABOR`、看门狗、以及新的 watchdog-spawn 失败路径都经过
   `cancel_run` → `cancel.cancel()`，因此三条路径统一都会触发句柄。token 每次 `INIT` 重建，
   hook 不会跨 run 泄漏。

**为什么默认 `None` 是对的**：现有两个后端在批间轮询 cancel（5–10 ms），不需要句柄；需要句柄的是
将来阻塞在 socket / FFI 读上的 M300 后端——这正是 `CancelToken::on_cancel` 文档里写的「用
`TcpStream::shutdown` 解除阻塞读」用途，此前无人使用。**未实现 M300 FFI。**

**新增测试** `abort_reaches_a_backend_that_only_wakes_through_its_stop_handle`：engine 测试内
定义 `BlockingBackend`，它**故意完全不看 cancel token**，只在自己的 condvar 上 park，唯一唤醒途径
是 `stop_handle` 返回的闭包（另有 5 s 兜底超时，让回归表现为断言失败而不是挂死测试进程）。断言
`ABOR` 后状态为 `Aborted` 且**错误队列为空**——若机制失效，reader 会一直 park 到看门狗在约 1 s 后
开火，结果会变成 `-365`，测试因此可区分「谁把流放出来的」。

**未做（留给 quickvib-device 属主）**：`StreamBackend` 已经有 `stopped: Arc<AtomicBool>`，
覆写 `stop_handle` 只需 4 行，就能让 tcp 路径也走真实句柄。因避免与 opus-B 在同 crate 抢文件而
未动；纯增益、无风险，可随时补。

---

## 4. B9（P2）导出元数据的 `durationSeconds`

`RunResult` 新增 `duration_seconds: f64`，在 `finish_run` 构造结果时从引擎状态取（此刻仍在
run 内，所有 setter 与 `adopt_project` 都答 `-221`，故必然是本次 run 的值——已加注释说明该不变量）；
`export_capture` 改读 `result.duration_seconds`。

**新增测试** `export_metadata_reports_the_runs_duration_not_a_later_override`：跑完 0.05 s 的 run
→ `CONF:REC:DUR 2`（`Complete` 态合法）→ 导出，断言 CSV 前言仍是
`# durationSeconds=0.05` / `# samples=50`。

---

## 5. 红/绿验证

先按原缺陷形态临时回退全部 4 处修复（错序号、无 watchdog 兜底、无 stop hook、无锁内检查、
导出读当前 override），跑 `cargo test -p quickvib-engine --lib`：

```
6 failed:
  a_reader_that_cannot_be_spawned_settles_the_run
  a_watchdog_that_cannot_be_spawned_ends_the_run_it_cannot_supervise
  adopting_a_project_mid_run_is_refused_under_the_lock
  adopting_a_project_mid_run_leaves_the_run_in_flight
  abort_reaches_a_backend_that_only_wakes_through_its_stop_handle
  export_metadata_reports_the_runs_duration_not_a_later_override
```

恢复后全绿。**每条新测试都是失败即红的回归。**

## 6. 验证命令

| 命令 | 结果 |
| --- | --- |
| `cargo test -p quickvib-engine` | 77 + 3 + 1 + 2 + 1 + 1 doctest 全通过（含并发 agent 的 `project_mutation_while_running.rs` 2 条与 `error_queue_reoverflow.rs`） |
| `cargo test --workspace` | 全通过（含 UI、bin、集成） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 干净 |
| `cargo clippy -p quickvib-ui -p quickvib --features gui --all-targets -- -D warnings` | 干净 |
| `cargo fmt -p quickvib-engine -- --check` + `rustfmt --check`（controller.rs / app.rs / backend.rs） | 干净 |
| engine lib 测试连跑 5 次 | 77 passed × 5，无抖动 |

## 7. 变更文件

```
crates/quickvib-device/src/backend.rs        +16   trait 增量方法 stop_handle（默认 None）
crates/quickvib-engine/src/engine.rs        +412/-81  B1/B3/B5/B9 + 6 条测试 + BlockingBackend
crates/quickvib-engine/src/recording.rs       +3   RunResult::duration_seconds
crates/quickvib-engine/src/dispatch.rs      +10/-3  测试调用点适配
crates/quickvib-engine/src/lib.rs            +3/-1  doctest 适配
crates/quickvib-engine/tests/adopt_project_semantics.rs  +8/-4  调用点适配
crates/quickvib-ui/src/controller.rs        +27/-9  apply() 处理 -221 + run_in_flight() helper
crates/quickvib/src/app.rs                   +3/-1  启动期调用点
```

## 8. 给 Round 3 / 汇总者的备注

1. `Engine::adopt_project` 现在是 fallible，README / `docs/PLAN.md` 若有示例需同步（本轮未改文档，
   避免与文档 agent 抢文件）。
2. `StreamBackend::stop_handle` 覆写待补（见 §3 末）。
3. `cancel_run` 现在是 `ABOR` / 看门狗 / watchdog-spawn 失败三条路径的单一入口，将来若要加
   「关闭进程时终止 run」，接到这里即可。
4. B1 的注入种子 `FAIL_SPAWN` 只在 `#[cfg(test)]` 编译，release 二进制里 `spawn_detached`
   与原先的 `Builder::new().name().spawn()` 完全等价。
