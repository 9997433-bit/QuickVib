# Round 2 结论简报

- **日期:** 2026-08-25
- **最终 HEAD:** `729d376`（含 rustfmt）
- **验证:** `cargo test --workspace` 528 通过 / 0 失败（R1 基线 495）；clippy `-D warnings` 干净；`fmt --check` 通过；`ci/check-deps.sh` OK

## 演进对比（相对 Round 1）

| 项 | Round 1 | Round 2 |
| --- | --- | --- |
| Apply/`FETC?` Complete 矛盾 | 已修 Invalidate | HOLD，且 adopt 检查进同锁 |
| B1 spawn 失败卡 Armed | 未修 | **真修** + FAIL_SPAWN 注入测试 |
| B5 load_project TOCTOU | 未修 | **真修** `adopt_project -> Result`，GUI 仅 Ok 后提交 base |
| B4 paced mock 睡死 | 未修 | **真修**（注入时钟 10ms 切片，优于直接 `wait_timeout`） |
| B3 stop() 不可达 | 未修 | **真修** `stop_handle` + `on_cancel`；契约文档仍缺 |
| B6 二次溢出 | 未修 | **真修** 按队尾判 `-350` |
| Phase 7 `docs/SCPI.md` | 缺失 | **已有**命令表 + Load 丢弃采集 |
| B8/B9/B10/B11 | 未修 | 已修 |

fable 守门：**零 REJECT**。R1 三项修复全部 HOLD。

## 潜在边界风险

1. **`stop_handle` 在正常完成后也会触发**（`finish_run` 的 `cancel.cancel()`）。当前 mock/tcp 在 `stream()` 入口复位 `stopped`，无害；M300 若用 shutdown 实现句柄会切断持久链路。Round 3 **必须**把「幂等、正常完成后也执行、不得永久失效传输」写进 trait 文档。
2. SCPI 默认绑 `0.0.0.0`、无 allow-list；export `MMEM` 可写任意路径（仪器语义，won't-fix；缓解 = `--bind`）。
3. IEEE 488.2 强制命令仍缺：仅当 UTS 合同（PLAN §21.1 Q1）要求 `*STB?`/`*ESR?` 才阻塞；否则接受 `REC:WAIT?`/`#REC:DONE`。
4. `IntoAdoptResult` 垫片死代码；通知写无超时（B7）；`Opc::bit_set` 不可观测。

## SOTA 验收差距（Round 3 范围）

**必修（小成本封口）**
1. `stop_handle` 契约文档（最高优先级的一行字）
2. B7：SCPI session `set_write_timeout`
3. `--bind` / `server.bindHost`（默认仍 `0.0.0.0`）
4. PLAN §5.3 补 Invalidate 边；`*CLS` 措辞与实现对齐
5. 信息级：B12 `Refusal::SocketError`、B13 注释、B14 `is_query`、删 `IntoAdoptResult` 死 impl

**不做：** Phase 6 FFI、完整 488.2 寄存器、export 沙箱、性能四项、Advanced 页。

## Round 3 文件分界

- opus-A：CLI/`app`/`schema` 的 `--bind`/`bindHost`；`scpi_server` 写超时（B7）
- opus-B：`backend.rs` 文档、PLAN.md、B12 listener、B13/B14、测试垫片清理、SCPI.md 微调
- gpt：最终全量测试 + bind/超时回归
- fable：终验收，对照本简报打勾
