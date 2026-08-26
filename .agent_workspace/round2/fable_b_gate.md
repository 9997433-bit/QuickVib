# Round 2 修复守门报告 — R2-fable-B

- **模型:** `claude-fable-5-thinking-xhigh`
- **日期:** 2026-08-25
- **审查快照:** HEAD `0a84fb4`(已推送 origin),工作树干净(仅 `.agent_workspace/round2/probes/` 未跟踪)。
- **验证结果:** `cargo test --workspace` **528 通过 / 0 失败**(基线 495,+33);`cargo clippy --workspace --all-targets -- -D warnings` 干净;`ci/check-deps.sh` OK。
- **⚠️ 唯一阻塞项:** `cargo fmt --all --check` **失败** —— `crates/quickvib/src/scpi_server.rs:281`(opus-B 提交 `561d1ff` 引入的 B8 修复)`engine.push_error(...)` 应合并为一行。CI 强制 fmt(`.github/workflows/ci.yml:26`),推最终产物前必须跑一次 `cargo fmt`。本人按守门职责不改 crate,请修复 agent 执行。

---

## 0. 审查过程说明

守门期间修复以两阶段落地,本报告对**两个已落地提交的全部 diff** 逐项判定:

| 提交 | 作者 | 内容 |
| --- | --- | --- |
| `561d1ff` | opus-B(含 gpt-B 测试) | B4、B6、B8、B10、B11、`docs/SCPI.md`、4 个回归测试文件 |
| `0a84fb4` | opus-A | B1、B3、B5、B9、`cancel_run` 合并重构、GUI/CLI 调用方适配 |

审查中曾目击一次**中间态冲突**(详见 §2),最终提交前已自行收敛,终态无冲突。

---

## 1. B1 / B3 / B4 / B5 / B6:最小正确修复判定

每项按「落地方案 vs 最小方案 vs 过度设计红线」以及三条不可破坏不变量(TestClock 虚拟时间、D9 abort 丢数据、in-flight run 保护)逐一核验。

### B1 — reader spawn 失败卡死 Armed:**ACCEPT**

- **落地**(engine.rs):`spawn_reader`/`spawn_watchdog` 在构造闭包前取 `let sequence = plan.sequence;`,失败分支 `finish_run(sequence, LinkLost, …)`;新增 `spawn_detached` 统一入口 + `#[cfg(test)] thread_local FAIL_SPAWN` 注入座;watchdog spawn 失败改为 `cancel_run(Some(sequence), LinkLost)` 兜底(无人监督的 run 立即失败,而非放任设备 stall 时永久悬挂)。
- **判定:** 核心修复与审计建议逐字一致。`FAIL_SPAWN` 注入座是 cfg(test) 门控的约 20 行,换来两个失败即红的测试(`a_reader_that_cannot_be_spawned_settles_the_run`、`a_watchdog_that_cannot_be_spawned_ends_the_run_it_cannot_supervise`)——**不算过度设计**;若引入 spawner trait 抽象贯穿产品代码才会被拒,这里没有。watchdog 失败以 `-240` 上报,与 reader panic 用 LinkLost 的既有 D27 口径一致,可接受。
- gpt-B 的 `stale_run_completion.rs` 额外锁定了 B1 依赖的 sequence 防线(旧 reader 的迟到 Completed 不能改写新 sequence 的终态、不能发第二条通知)——通过。

### B3 — 活动 run 中 `stop()` 不可达:**ACCEPT**

- **落地**:`DeviceBackend::stop_handle() -> Option<Arc<dyn Fn() + Send + Sync>>`(默认 `None`,backend.rs 文档解释「轮询 cancel 的后端返回 None 即正确」);`run_capture` 在 `stream` 前 `cancel.on_cancel(move || stop())`。
- **判定:** 这是审计给出的两个方案里更轻的那个。关键正确性依据:① 每次 `INIT` 换新 `CancelToken`,钩子不会跨 run 累积;② `CancelToken::on_cancel` 的「注册时已取消则立刻执行」语义封死了 INIT 后立即 ABOR 的窗口;③ `abort`/`fire_watchdog` 保留 `stop_backend()`,覆盖 reader 尚未 take backend 的前置窗口。测试 `abort_reaches_a_backend_that_only_wakes_through_its_stop_handle` 用只认 stop 不认 cancel 的 `BlockingBackend` 做到失败即红(修复前 state 停在 Armed、断言必红)。默认 `None` 优于强制实现——Mock/Stream 本就轮询 cancel,不需要句柄。
- **一处遗留(不阻塞,Round 3 一行注释):** `finish_run` 收尾时的 `cancel.cancel()`(为唤醒 watchdog)会在**正常完成后**也触发 stop 钩子。当前两个后端 `stream()` 入口复位 `stopped`,无害;但 Phase 6 的 M300 后端若用 socket shutdown 实现句柄,会在每次成功 run 后切断持久链路。建议在 `stop_handle` 的 trait 文档补一句「钩子在每次 run 结束(含正常完成)都会执行,必须幂等且不得永久失效传输」。

### B4 — paced mock 不可中断 sleep:**ACCEPT**

- **落地**(mock.rs):`pace_batch` 将整批 sleep 切成 ≤10ms(`PACE_SLICE`)片,每片前查 `cancel`/`stopped`,**继续走 `clock.sleep`** 而非 `cancel.wait_timeout`。
- **判定:** 这是唯一同时满足三个约束的路线,必须点名表扬:审计原文的「直接换 `cancel.wait_timeout(batch)`」方案其实会**打破 TestClock**——既有测试 `pacing_uses_the_injected_clock` 在 TestClock 下开 pacing 并断言虚拟时间恰好推进 5s,且「TestClock 下 paced 流瞬时完成」是 `with_pacing` 的文档化契约;`wait_timeout` 是真实 Condvar 等待,既不推进虚拟时间又会真等。落地方案用注入时钟切片:TestClock 下每片 `sleep` 即时推进、片长求和精确等于批时长(Duration 减法无损),该测试原样通过;SystemClock 下取消延迟上界 10ms。**已核验 TestClock 不变量无回归。**
- 回归:`cancellation_interrupts_a_paced_batch` / `stop_interrupts_a_paced_batch`(修复前会睡满 10s 且报 Completed,三重断言必红)、`pacing_still_takes_real_time_on_the_system_clock`(防切片把 pacing 切没),外加 gpt-B 的 `pace_cancel.rs`(750ms 批、250ms 内要求返回)。时间裕量取得保守,CI 抖动风险低。
- 未动「sleep 在 `on_batch` 之前」的次要项(首批前 REC:STAT? 停 ARMED)——审计本就列为可选,不修正确。

### B5 — `load_project` TOCTOU:**ACCEPT**

- **落地**:`adopt_project` 改为 `Result<(), ScpiError>`,`-221` 检查移入**同一锁持有期**;`load_project` 保留锁外预检为快速失败、锁内复检为权威(注释写明);全部调用方适配——dispatch/lib 文档/三个测试文件 `.unwrap()`,CLI 启动 `let _ =`(附「启动时不可能有 run」注释),GUI `apply()` 映射为 `RunInFlight` FieldError 且**仅在 Ok 时提交 `self.base`**。
- **判定:** 与审计建议一致且更彻底。特别指出:GUI `apply` 只在引擎确认后才更新本地 `base`,顺带消除了我在中间态曾记录的「GUI 本地状态与引擎失同步」竞态——最终形态无此残留。**in-flight 保护不变量已核验**:既有测试改为断言 `Err(SettingsConflict)` 且 run 继续在飞(`adopting_a_project_mid_run_leaves_the_run_in_flight`),新增 `adopting_a_project_mid_run_is_refused_under_the_lock` 直接模拟 MMEM:LOAD:STAT 的窗口;gpt-B 的 `project_mutation_while_running.rs` 验证 duration/format/name/path 四项零覆盖。
- `Invalidate` 处 `if let Ok` 改为 `unwrap_or(State::Idle)` 并注释「running 态已在上方拒绝」——语义等价,不掩盖新错误(transition 对 settled 态必有边)。
- **D9 不变量核验:** `finish_run` 的非 Complete 清 `result` 路径未被触碰,`an_aborted_run_publishes_no_data` 原样通过。

### B6 — ErrorQueue 第二次溢出无 `-350`:**ACCEPT**

- **落地**(errors.rs):删除 sticky `overflowed` 字段,满队时改查队尾 `back().map(|e| e.error) != Some(QueueOverflow)` 决定是否换标;`pop`/`clear` 随之简化(净 -10 行)。
- **判定:** 与审计给出的最小修复逐字一致,是本轮最干净的一笔。既有测试 `overflow_replaces_the_last_entry`(连续溢出只占一格)原样通过;新增「溢出→pop 一条→填回→再溢出」用例断言队尾重新是 `-350` 且 drain 可见两枚标记(修复前只有一枚,失败即红);gpt-B 的集成版 `error_queue_reoverflow.rs` 同口径。SCPI-99「最新可读条目为 -350」不变量恢复。

---

## 2. 冲突与 match 完整性核查

1. **opus-A / opus-B 文件冲突:无。** 终态分界干净:opus-B 独占 `mock.rs`/`sink.rs`/`errors.rs`/`lexer.rs`/`scpi_server.rs`,opus-A 独占 `engine.rs`/`dispatch.rs`/`recording.rs`/`controller.rs`/`app.rs`。唯一跨界是 opus-A 为 B3 在 `quickvib-device/src/backend.rs` 加 trait 方法——B3 天然横跨两 crate,opus-B 未触碰该文件,无碰撞。
2. **中间态冲突(已自愈,记录在案):** 守门期间曾目击 opus-A 的第一版 API 为 `try_adopt_project(Result) + adopt_project(静默忽略)`,而 gpt-B 的 `project_mutation_while_running.rs`(经 `IntoAdoptResult` 兼容垫片)期望 `adopt_project` 本身返回 `Result`——该测试在第一版下修复后仍红(gpt_b_tests.md 自报 FAIL 也证实)。opus-A 最终提交收敛为单一 `Result` 返回的 `adopt_project`,冲突消解;`rg try_adopt_project` 已零残留。垫片的 `impl IntoAdoptResult for ()` 现为死代码但 clippy 未报,可留 Round 3 顺手清理(非必须)。
3. **Event/Invalidate match 完整性:** `state.rs` 本轮零改动;未新增 Event/State 变体;穷举表测试 `the_full_state_event_table_is_covered`(rejected == 10)原样通过。`transition` 的 match 无不完整分支。
4. **一次全 workspace 编译 + 528 测试 + clippy -D warnings 通过**,即为最终一致性证明。

---

## 3. 安全项处置建议(任务第 3 项)

| 项 | 建议 | 理由 |
| --- | --- | --- |
| SCPI 口默认绑定 `0.0.0.0`、无 `--bind` | **Round 3 做**(低成本) | `AppBuilder::bind_host` setter 已存在(当前仅测试可达),只差 CLI/`server.bindHost` 配置面 + 帮助文本 + 一个测试,约 30 行;默认值保持 `0.0.0.0` 不破坏传统仪器语义,收紧是运维可选项。与设备口有 `allowedPeers` 而 SCPI 口全裸的不对称,这是性价比最高的补偿。 |
| export/MMEM 路径遍历(`resolve_path` 无 canonicalize) | **won't-fix(默认行为)+ 文档注记** | 台式仪器 `MMEM` 语义本就接受任意绝对路径,UTS 部署依赖这一点;做包含性校验会破坏 `MMEM:STOR:STAT "/abs/path"` 的合法用例。真实风险面 = 「SCPI 口暴露到不可信网络」,其正解是上一行的 `--bind`(或防火墙),而非给文件语义加沙箱。建议在 `docs/SCPI.md` 或 README 加 3 行部署注记:「SCPI 口无鉴权,可写进程权限内任意路径;不可信网段请绑定 127.0.0.1 或配防火墙」。若 Round 3 仍有余力,可加**可选** `export.restrictToDirectory` 开关,默认关。 |
| SCPI 口 allow-list(对齐设备口 `allowedPeers`) | Round 3 可选,优先级低于 `--bind` | 字符串比较的 peer filter 已有现成实现可复用;但 `--bind 127.0.0.1` 已覆盖主要场景。 |

---

## 4. 逐 diff 判定汇总

| 落点 | 所属提交 | 判定 |
| --- | --- | --- |
| B1 spawn 失败收尾 + FAIL_SPAWN 注入座 + 2 测试(engine.rs) | `0a84fb4` | **ACCEPT** |
| B3 `stop_handle` trait 默认 None(backend.rs)+ on_cancel 钩子 + BlockingBackend 测试 | `0a84fb4` | **ACCEPT**(附 Round 3 一行文档建议,见 §1-B3) |
| B5 `adopt_project -> Result` 锁内复检 + 全调用方适配(engine/dispatch/lib/controller/app + 3 测试文件) | `0a84fb4` | **ACCEPT** |
| B9 `RunResult.duration_seconds`(recording.rs + engine.rs 导出元数据) | `0a84fb4` | **ACCEPT** |
| `cancel_run` 合并 abort/watchdog 路径重构 | `0a84fb4` | **ACCEPT**(行为等价,去重合理) |
| B4 `pace_batch` 切片(mock.rs)+ 3 单测 | `561d1ff` | **ACCEPT**(TestClock 不变量保全,见 §1-B4) |
| B6 ErrorQueue 队尾换标(errors.rs)+ 2 单测 | `561d1ff` | **ACCEPT** |
| B8 `ReadOutcome::Unterminated`(scpi_server.rs)+ 2 单测 | `561d1ff` | **ACCEPT,但须 `cargo fmt`**(281 行,CI 阻塞) |
| B10 lexer `after_quote` + 引号必须整参(lexer.rs)+ 2 测试 | `561d1ff` | **ACCEPT** |
| B11 `received()` 文档修正(sink.rs) | `561d1ff` | **ACCEPT** |
| `docs/SCPI.md`(命令表 + 错误表 + Load/Apply 丢弃采集归 IDLE) | `561d1ff` | **ACCEPT**(Phase 7 最小文档达标) |
| gpt-B 4 个回归测试文件(stale_run_completion / project_mutation_while_running / error_queue_reoverflow / pace_cancel) | `561d1ff`/`0a84fb4` | **ACCEPT**(全部失败即红设计,终态全绿) |

**REJECT:无。** 无一处越过「过度设计」红线:没有为 B1 引入 spawner trait、没有为 B4 改 Clock trait、没有为 B3 强制全后端实现句柄、没有做整套 IEEE 488.2 寄存器组。

---

## 5. 遗留清单(交 Round 3 / 编排器)

1. **[阻塞 CI] `cargo fmt`** — scpi_server.rs:281 一行,推送前必修。
2. B7 通知写无超时(`write_line` 无 `set_write_timeout`)— 本轮未动,Round 3 小修(每 session socket 设写超时即可,勿上专职写线程)。
3. B12(`set_nonblocking` 失败误标 peerNotAllowed)、B13(`Opc::bit_set` 不可观测注释)、B14(`Command::is_query` 死代码)— 信息级,Round 3 顺手。
4. `docs/PLAN.md` §5.3 状态图仍缺 `Invalidate` 边(SCPI.md 已有行为文档,PLAN 规范图未同步)。
5. `stop_handle` trait 文档补幂等/正常完成后也触发的契约句(§1-B3)。
6. `IntoAdoptResult` 垫片清理(可选)。
7. 性能项(FETC? 序列化、64KiB BufWriter 常驻、CSV time_s 精度、GUI 每帧拷贝)本轮有意未动,维持简报口径。
8. 安全:`--bind` 进 Round 3;export 沙箱 won't-fix + 文档注记(§3)。

---

*守门完毕。未修改任何 crate;未 commit/push 产品代码;本文件为唯一写入。*
