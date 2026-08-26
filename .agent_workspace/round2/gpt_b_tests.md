# Round 2 — R2-gpt-B 回归测试报告

- 模型：`gpt-5.6-sol-xhigh-fast`
- 日期：2026-08-25
- 范围：B1、B4、B5、B6；仅新增测试文件，未修改产品源码。

## 新增测试

### B1 — sequence 不匹配的延迟收尾

文件：`crates/quickvib-engine/tests/stale_run_completion.rs`

- `late_completion_from_reset_run_cannot_finalize_newer_sequence`
  - 用受控 backend 让 sequence N 的 reader 跨越 `reset` 延迟返回。
  - sequence N+1 在 backend 暂不可用时先以 LinkLost 收尾。
  - 验证旧 reader 的完成结果不能改变 N+1 的 `Aborted` 状态、不能发布 capture、不能产生第二条终态通知或额外错误。
  - 结果：**PASS**。

限制：`std::thread::Builder::spawn` 和 `Engine::finish_run` 均无公开/测试注入点，无法在独立集成测试里稳定制造 reader spawn 失败。因此本测试锁定 B1 所依赖的 mismatched-sequence 防线，但没有直接覆盖 spawn-fail → 非 Armed 的路径。未在 `engine.rs` 中添加同名/重复单元测试，留给 opus-A 的注入或内部测试。

### B4 — paced mock 可及时取消

文件：`crates/quickvib-device/tests/pace_cancel.rs`

- `paced_stream_cancel_wakes_before_full_batch_deadline`
  - 真实时钟下首批 nominal pacing 为 750 ms。
  - 进入 stream 后取消，要求 250 ms 内返回 `StreamOutcome::Cancelled`，且不得发布该批 samples。
  - 结果：**PASS**（约 50 ms）。
  - 运行时工作区已包含 opus-B 在 `mock.rs` 中的分片 pacing 修复；该产品文件不是本代理修改。

### B5 — 活动 run 中拒绝项目替换且不覆盖配置

文件：`crates/quickvib-engine/tests/project_mutation_while_running.rs`

- `adopt_project_rejects_active_run_without_clobbering_plan`
  - 兼容 pre-fix 的 `()` 返回值与预期 post-fix 的 `Result<(), ScpiError>`，因此旧 API 会产生断言失败而非编译失败。
  - 要求活动 run 中返回 `Err(ScpiError::SettingsConflict)`（SCPI `-221`），并保持 duration、format、project name/path 不变。
  - 结果：**FAIL（预期红）**：当前返回 `Ok(())`；产品实现仍会接纳替换项目。

- `load_project_rejects_active_run_without_clobbering_plan`
  - 使用有效磁盘项目，要求活动 run 中返回 `-221`，并保持上述配置不变。
  - 结果：**PASS**。

备注：磁盘 I/O 与锁内 adopt 之间的 TOCTOU 没有可注入的 load hook；直接 `adopt_project` 测试锁定最终锁内校验，正是消除该窗口所需的不变量。

### B6 — 第二次 overflow 仍暴露 `-350`

文件：`crates/quickvib-engine/tests/error_queue_reoverflow.rs`

- `second_overflow_after_partial_drain_surfaces_a_new_queue_overflow`
  - 执行“填满 → overflow → pop 一条 → 填回 → 再 overflow”。
  - 验证队尾为新的 `QueueOverflow`，并且 drain 时可观察到两次 `-350`。
  - 结果：**PASS**。
  - 运行时工作区已包含 opus-B 在 `errors.rs` 中的 tail-marker 修复；该产品文件不是本代理修改。

## 执行命令与结果

```text
cargo test -p quickvib-device --test pace_cancel
  PASS: 1 passed

cargo test -p quickvib-engine --test error_queue_reoverflow
  PASS: 1 passed

cargo test -p quickvib-engine --test project_mutation_while_running
  FAIL: 1 passed, 1 failed
  failing: adopt_project_rejects_active_run_without_clobbering_plan
  left: Ok(())
  right: Err(SettingsConflict)

cargo test -p quickvib-engine --test stale_run_completion
  PASS: 1 passed
```

`git diff --check` 对四个新增测试文件通过。按任务要求未 commit、未 push、未创建 PR。
