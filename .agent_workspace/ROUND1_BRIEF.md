# Round 1 结论简报

- **日期:** 2026-08-25
- **分支:** `cursor/review-optimize-f6c8`（HEAD 含 R1 修复；fable-B 报告随后提交）
- **基线:** `5895498` / PR #1 Phases 0–5 + GUI
- **模型:** 2× `claude-fable-5-thinking-xhigh`、2× `claude-opus-5-thinking-high-fast`、2× `gpt-5.6-sol-xhigh-fast`（无静默降级）

## 已实现功能

- **R1–R23:** 19 项 IMPLEMENTED；R3 Phase 6 FFI 有意推迟；R4 传输层已实现、真实 M300 随 Phase 6；R7/R21 按计划排除。无计划外 MISSING。
- **Phases 0–5、8:** 落地。Phase 6 占位干净（无假 DLL）。Phase 7 PARTIAL（缺 `docs/SCPI.md`、UTS 转录、`cargo deny`、tagged release）。
- **基线质量:** 隔离 worktree `cargo test --workspace` 495 通过；clippy `-D warnings` 干净；`ci/check-deps.sh` OK。Windows mingw 交叉构建本环境跳过（无 mingw）。

## Round 1 已合入修复

1. **`adopt_project` 状态归位**（原 PR #1 已知缺陷，已确认且波及 `MMEM:LOAD:STAT`）：`Event::Invalidate` 使 Complete/Aborted/Idle → Idle；`FETC?`/`REC:STAT?` 一致。回归：engine / dispatch / `adopt_project_semantics.rs`。
2. **`scpiPort == device.port`** 工程校验（`-224`）；GUI 按 `Option<u16>` 数值判重，避免 `09123` 漏判与空串假阳性。
3. **mock 故障阈值大于请求采样数时不再把 Completed 报成 LinkLost。**

## 遗留缺陷（Round 2 攻坚）

按优先级。B 编号来自 fable-B 审计（HEAD `1a6db07` 行号）。

| 优先级 | ID | 问题 | 建议落点 |
| --- | --- | --- | --- |
| P0 | B1 | reader **spawn 失败**时 `finish_run(0, …)` 与 `sequence` 不匹配，run 永久卡在 Armed | `engine.rs` |
| P0 | B5 | `load_project` 在锁外 I/O 后 `adopt_project`，与 `INIT` TOCTOU；Invalidate 只保 state，duration/format 仍可被覆盖 | `engine.rs` `adopt_project` 返回 Result |
| P1 | B4 | paced mock 整批 `sleep` 不可被 `ABOR`/watchdog 唤醒（低采样率可卡数秒～数百秒） | `mock.rs` → `cancel.wait_timeout` |
| P1 | B3 | 活动 run 中 `take_backend` 导致 `stop()` 不可达，掩盖未来阻塞式 M300 | engine stop 句柄 / `CancelToken::on_cancel` |
| P2 | B6 | ErrorQueue 第二次溢出不再标记 `-350` | `errors.rs` |
| P2 | B7–B10 | 通知写无超时；EOF 行误报过长；导出 duration 用当前 override；lexer 闭引号尾随字符 | 分文件小修 |
| 文档 | Phase 7 | 缺 `docs/SCPI.md`；PLAN §5.3 应补 Invalidate 边 | 文档 |

**保持推迟:** Phase 6 FFI、Advanced 页真实控件、完整 IEEE 488.2 寄存器组（`*ESR?`/`*STB?` 等）— Round 2 不做全套 488.2，最多注释 `Opc::bit_set` 不可观测；`docs/SCPI.md` 可开一个最小命令表。

## 性能瓶颈（不阻塞正确性）

- `FETC?` 每样本两次 `write_all` + Grisu；每响应新建 64 KiB BufWriter。
- CSV `time_s` 全精度膨胀。
- GUI 每帧深拷贝 `Project`。Round 2 仅在不扩大冲突时做低成本项。

## Round 2 攻坚重点

1. **必修代码:** B1 spawn 失败收尾、B5 TOCTOU、B4 可中断 pace、B3 stop 可达（或明确文档 + 钩子）。
2. **次必修:** B6 错误队列；B8/B9/B10 若无冲突一并修。
3. **文档:** 最小 `docs/SCPI.md`（命令表 + Apply/Load 丢弃采集归 Idle）。
4. **测试:** 为 B1/B4/B5/B6 加失败即红的回归；全量 `cargo test --workspace` + clippy。
5. **SOTA 复审:** 确认 R1 修复无回归；评估剩余差距是否可进 Round 3。

## 边界风险

- `--scpi-port` 覆盖仍可与设备口冲突（当场 Bind 失败，有意不改 0=OS 分配）。
- SCPI 口无鉴权、默认 `0.0.0.0`；export 路径可遍历 — 传统仪器语义，Round 2 不做绑定/沙箱除非低成本加 `--bind`。
- 并发编辑：Round 2 opus-A 独占 `quickvib-engine`；opus-B 独占 `quickvib-device` + `errors.rs` 若 A 不碰 errors。
