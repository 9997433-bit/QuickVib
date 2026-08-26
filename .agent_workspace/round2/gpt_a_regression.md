# Round 2 — R2-gpt-A 全量回归报告

- 模型：`gpt-5.6-sol-xhigh-fast`
- 日期：2026-08-25
- 最终验证分支：`cursor/review-optimize-f6c8`
- 最终验证提交：`0a84fb410919521b60bfdc6e6cb15e76fabcafa5`
- 结论：**PASS**；相对 Round 1 无新增失败、无 clippy warning、依赖策略通过。

## 执行方式

先在 detached 隔离 worktree `/tmp/quickvib-r2-gpt-a` 对任务开始时的 HEAD
`1da87401d8c524611c7845b94ae0168ec032b034` 运行完整检查；待 sibling 修复合入后，再按要求在
`/workspace` 对最终 HEAD `0a84fb4` 运行一次。两轮均设置 `CARGO_TERM_COLOR=never`。

可复现脚本：

```text
.agent_workspace/round2/probes/regression.sh
```

最终日志：

```text
.agent_workspace/round2/probes/regression-workspace/
```

隔离旧 HEAD 日志：

```text
.agent_workspace/round2/probes/regression-isolated/
```

## 最终结果

| 命令 | 结果 | wall |
| --- | --- | ---: |
| `cargo test --workspace` | **PASS** | 8.668 s |
| `cargo test --workspace --features gui` | **PASS** | 33.129 s |
| `cargo clippy --workspace --all-targets -- -D warnings` | **PASS** | 1.420 s |
| `cargo clippy --workspace --all-targets --features gui -- -D warnings` | **PASS** | 1.582 s |
| `ci/check-deps.sh` | **PASS**（`dependency policy OK`） | 0.286 s |

两次 test 命令各通过：

- 517 个非 doc 测试；
- 11 个 doc-test；
- Cargo 汇总共 528 个测试，0 failed / 0 ignored。

Round 1 简报采用“495 tests”口径，不计 `quickvib-testkit` 自身的 3 个基础设施测试。按同一口径，
Round 2 为 **514 = 495 + 19**，新增 19 个测试且全部通过。若直接累计 Cargo 输出，则隔离旧 HEAD
为 498 个非 doc 测试、最终 HEAD 为 517 个，差值仍为 19。

## 相对 Round 1

- 新失败：**0**
- 新 clippy warning：**0**
- 新依赖策略失败：**0**
- 新增非 doc 测试：**19，全部通过**
- 隔离旧 HEAD 的五项检查也全部通过，说明最终结果不是由共享 target 或并发源码中间态造成。

## B1 / B4 / B5 回归

### B1 — spawn 失败与 sequence 防线

以下测试在普通与 GUI workspace 两轮中均通过：

- `a_reader_that_cannot_be_spawned_settles_the_run`
- `a_watchdog_that_cannot_be_spawned_ends_the_run_it_cannot_supervise`
- `late_completion_from_reset_run_cannot_finalize_newer_sequence`

前两项直接注入 reader/watchdog spawn 失败；reader 失败后状态落到 `Aborted`，不再卡在 `Armed`。
第三项覆盖旧 sequence 的延迟完成不能污染较新 run。

### B4 — paced mock 可中断

以下测试均通过：

- `cancellation_interrupts_a_paced_batch`
- `stop_interrupts_a_paced_batch`
- `paced_stream_cancel_wakes_before_full_batch_deadline`
- `pacing_still_takes_real_time_on_the_system_clock`
- 既有 `pacing_uses_the_injected_clock`

结果同时覆盖 cancel、`stop()`、及时返回、无取消后批次发布，以及真实时钟/注入时钟的节流语义。

### B5 — 项目加载/采纳与活动 run 的 TOCTOU

以下测试均通过：

- `adopting_a_project_mid_run_is_refused_under_the_lock`
- `adopting_a_project_mid_run_leaves_the_run_in_flight`
- `adopt_project_rejects_active_run_without_clobbering_plan`
- `load_project_rejects_active_run_without_clobbering_plan`

活动 run 中 `adopt_project` / `load_project` 均返回 `SettingsConflict`（SCPI `-221`），且
duration、format、project name/path 不被覆盖。

## 工作区约束

本代理未修改任何 product crate，未 commit、未 push、未创建 PR；仅新增本报告、回归脚本及日志。
