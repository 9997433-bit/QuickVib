# Round 2 — R2-opus-B 修复报告

- **模型:** `claude-opus-5-thinking-high-fast`
- **分支:** `cursor/review-optimize-f6c8`（未提交、未推送）
- **范围:** B4 / B6 / B8 / B10 / B11 + `docs/SCPI.md`。未触碰 `crates/quickvib-engine/src/engine.rs`（R2-opus-A 独占）。

---

## B4（P1）paced mock 的整批 sleep 不可中断

**文件:** `crates/quickvib-device/src/mock.rs`

原实现一次 `clock.sleep(batch_len / sample_rate_hz)`，最长 4096 样本一批：1 kHz 时 4.1 s、
10 Hz 时可达 409 s，睡眠期间 `cancel` 与 `stopped` 都无法唤醒，`ABOR`/watchdog/GUI「停止」体感失灵。

**改法:** 新增 `MockBackend::pace_batch(batch, cancel)`，以批时长为 deadline，按 `PACE_SLICE = 10ms`
切片等待，每片前检查 `cancel.is_cancelled()` 与 `stopped`；被打断即 `return Ok(StreamOutcome::Cancelled)`。

关键取舍：等待仍走**注入时钟** `clock.sleep`，而不是 `CancelToken::wait_timeout`。

- `TestClock` 下 `sleep` 只推进虚拟时间，切片求和与原来完全相等（deadline 用 `Duration` 精确算术，
  无累积误差），既有测试 `pacing_uses_the_injected_clock` 仍断言恰好 5 s；若改用 `wait_timeout`，
  虚拟时间不再推进，deadline 永远到不了 → 死循环。
- `SystemClock` 下最坏 10 ms 才注意到取消，与既有 `MockFault::Stall` 分支的 10 ms 轮询同量级。
- 生产 `pace_mock=true` 仍然按实时节流（下面第三个测试锁定这一点）。

保持不变：sleep 仍在 `on_batch` 之前（B4 备注里提到的"首批前 `REC:STAT?` 停留 ARMED"是批次粒度的
固有现象，调整顺序会改变现有节流语义，本轮不动）。

**测试**（`mock.rs` 单元测试）：

- `cancellation_interrupts_a_paced_batch` — 10 S/s × 100 样本 = 单批 10 s；20 ms 后另一线程
  `cancel()`，断言 outcome 为 `Cancelled`、未投递任何样本、真实耗时 < 2 s。修复前：睡满 10 s
  后返回 `Completed`（双重红）。
- `stop_interrupts_a_paced_batch` — 同上，改由 `DeviceBackend::stop()` 触发。
- `pacing_still_takes_real_time_on_the_system_clock` — 1 kS/s × 200 样本，断言真实耗时 ≥ 150 ms，
  防止"切片"被误改成不再节流。
- 另有探针代理预置的 `crates/quickvib-device/tests/pace_cancel.rs`
  （`paced_stream_cancel_wakes_before_full_batch_deadline`），本修复后转绿。

---

## B6（P2）ErrorQueue 第二次溢出不再标记 `-350`

**文件:** `crates/quickvib-engine/src/errors.rs`（仅此文件）

删除 sticky 的 `overflowed` 字段，改为按队尾判定：

```rust
if self.entries.back().map(|e| e.error) != Some(ScpiError::QueueOverflow) {
    self.entries.pop_back();
    self.entries.push_back(ScpiErrorEntry::new(ScpiError::QueueOverflow));
}
```

`pop`/`clear` 随之简化（不再需要复位标志）。语义：连续溢出仍只占一个 `-350` 槽；一旦 `SYST:ERR?`
腾出空间后再次溢出，队尾会重新变成 `-350`，恢复 SCPI-99 的"最新可读条目必须是 -350"不变量。

**测试:**

- `a_second_overflow_after_a_partial_drain_is_marked_again`（单元）— 灌满 → 溢出 → pop 1 →
  push 2 → 排空后队尾是 `-350` 且总共可见 2 个 `-350`。修复前红。
- `consecutive_overflows_share_one_marker`（单元）— DEPTH+10 次连续 push 只产生 1 个 `-350`，
  锁定"不要每条丢失都吃一个槽"。
- 探针代理预置的 `crates/quickvib-engine/tests/error_queue_reoverflow.rs` 本修复后转绿。

---

## B8 EOF 截断行被误报为"行过长"

**文件:** `crates/quickvib/src/scpi_server.rs`

`read_line_capped` 原来把"无 `\n` 结尾"一律当 `TooLong`，于是对端 half-close 前的最后一段
（如 `*IDN?` 无换行）会压入 detail 为 `input line exceeded 65536 bytes` 的 `-100`，排障时误导。

**改法:** 新增 `ReadOutcome::Unterminated`。判定顺序改为：无终止符时，`len > MAX_LINE_BYTES`
才是真超长（并继续 `discard_to_terminator`），否则是 EOF 截断；有终止符但 `len > MAX` 仍是超长
（终止符已被消费，无需丢弃）。会话循环对 `Unterminated` 压 `-100` +
detail `line not terminated before EOF`，且**不执行**该片段（IEEE 488.2 要求终止符），随后返回
（对端已关闭）。

**测试:** `a_line_cut_short_by_eof_is_not_reported_as_over_long`、
`an_over_long_line_cut_short_by_eof_is_still_over_long`（后者守住"超长且无终止符"仍走 TooLong）。

---

## B10 lexer 闭引号后的尾随字符

**文件:** `crates/quickvib-scpi/src/lexer.rs`

`lex_arguments` 新增 `after_quote` 状态：闭引号之后只允许空白与 `,`，其余字符报
`-100 trailing characters after a quoted string`（此前 `"a"x` 被静默拼成 `Quoted("ax")`，
`"a" "b"` 变成 `"a b"`）。对称地，引号前若已有非空白字符（`x"a"`）报
`a quoted string must be the whole argument`。

顺带修掉一处小疵：逗号后的前导空白不再被并进引号内容（`A, "b"` 此前得到 `Quoted(" b")`）。

**测试:** `characters_after_a_closing_quote_are_rejected`（4 种畸形写法均为 `-100`）、
`whitespace_and_commas_may_follow_a_closing_quote`（`"a.csv" , "b.csv"  ` 仍正常解析为两个参数）。

---

## B11 `SampleChannel::received()` 文档与实现不符

**文件:** `crates/quickvib-device/src/sink.rs`

注释由 "dropped ones excluded" 改为"按到达计数、包含随后被挤掉的样本"，并说明
`received() - dropped()` 才是真正留下的数量。实现与既有测试
（`reset_discards_what_arrived_while_idle` 依赖当前语义）不变。

---

## 文档 `docs/SCPI.md`（Phase 7 缺口）

新建，面向操作员的精简版：命令表（含短/长形、响应样例、错误码）、错误码表、`#REC:DONE` /
`#REC:ABORT` 通知，以及**"加载项目会丢弃采集"**一节 —— `MMEM:LOAD:STAT` / `MMEM:LOAD:AUTO` /
GUI Apply 采纳新项目后 `REC:STAT?` 返回 `IDLE`（不是 `COMPLETE`），`FETC?`/`TRAC:*`/`CALC:*`/
`MMEM:STOR:TRAC` 一律 `-230`；run 进行中加载被 `-221` 拒绝且不改动任何状态。
明确写明 PLAN §8–§9 仍是规范来源。

---

## 验证

| 命令 | 结果 |
| --- | --- |
| `cargo test -p quickvib-device` | 70 lib + 1 探针（`pace_cancel`）+ 1 doc，全绿 |
| `cargo test -p quickvib-scpi` | 50 lib + 1 doc，全绿 |
| `cargo test -p quickvib-engine --lib errors::` | 8/8 绿（新增 2 个）；`tests/error_queue_reoverflow.rs` 绿 |
| `cargo test -p quickvib` | 55 lib（含新增 2 个 EOF 用例）+ 全部集成套件，全绿 |
| `cargo clippy -p quickvib-device -p quickvib-scpi -p quickvib --all-targets -- -D warnings` | 干净 |

唯一剩余红灯：`crates/quickvib-engine/tests/project_mutation_while_running.rs::adopt_project_rejects_active_run_without_clobbering_plan`
（B5 探针）。本轮与 R2-opus-A 共用同一工作树，`engine.rs`/`backend.rs`（B1/B3/B5/B9）由对方并发
修改中（期间出现过瞬时编译失败），该用例不属本代理范围，也不受本报告任何改动影响。

未 commit、未 push、未开 PR。
