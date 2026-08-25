# Round 1 自动化基线探针（R1-gpt-A）

## 范围与环境

- 起始分支：`cursor/review-optimize-f6c8`
- 探针基线提交：`d3a0c03cd674531ce8d9674dc50b5dc9bac9bace`
- 工具链：`rustc 1.83.0`、`cargo 1.83.0`
- 所有 Cargo 命令均设置 `CARGO_TERM_COLOR=never`。
- 首个无特性工作区测试在干净的 `/workspace` 上运行。随后有并发代理持续写入并提交产品源码，因此其余基线命令在同一提交的隔离 worktree 中运行，避免读取半写入文件；使用独立 target 目录，不存在 Cargo 锁等待。

## 命令结果

| 命令 | 退出码 | 结果 | 墙钟时间 |
|---|---:|---|---:|
| `cargo test --workspace` | 0 | 495 passed；0 failed；0 ignored | 14.414 s |
| `cargo test --workspace --features gui` | 0 | 495 passed；0 failed；0 ignored | 40.800 s |
| `cargo test -p quickvib-ui` | 0 | 56 单元测试 + 1 doctest passed；0 failed | 0.335 s |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | 通过；0 warnings | 2.842 s |
| `cargo clippy --workspace --all-targets --features gui -- -D warnings` | 0 | 通过；0 warnings | 11.632 s |
| `ci/check-deps.sh` | 0 | `dependency policy OK` | 0.313 s |

`quickvib-ui` 的 form/controller 视图模型始终编译和测试；`gui` 特性只启用 `eframe` 图形窗口栈。为同时覆盖无窗口 UI 逻辑与图形栈编译，本次按要求运行了工作区 `gui` 变体和单独的 `quickvib-ui` 包测试。两种工作区测试选择的测试总数相同。

## 失败、警告与并发说明

- 提交基线上的必跑测试、Clippy 和依赖策略检查均无失败、无警告。
- 首轮测试结束后，一次用于提取简洁计数的补充运行遇到并发工作区的临时不完整改动，以退出码 101 失败：`crates/quickvib-engine/src/state.rs` 新增 `Event::Invalidate` 后，`transition` 的 match 尚未覆盖该事件，触发 `E0004`。该并发改动随后由另一代理完成并提交为 `064d172`；它不属于 `d3a0c03` 基线失败。
- 本代理未修改 `crates/`，也未执行 commit、push 或 PR 操作。

## 性能备注

- 无特性首跑包含依赖下载和约 9.90 s 的编译，总墙钟 14.414 s。
- `gui` 首跑包含图形依赖下载/编译，总墙钟 40.800 s；缓存后的计数复跑为 5.962 s。
- 默认测试输出不提供逐测试耗时，只能可靠报告测试二进制/套件耗时。最慢的普通套件是 `quickvib/tests/recording.rs`（约 1.02–1.03 s）和 `quickvib-engine` 单元测试（约 1.02 s）。`gui` 复跑中 `quickvib-ui` doctest 套件约 1.49 s（首跑约 1.89 s，包含 doctest 编译开销）。

## Windows 交叉构建

跳过 `cargo build --release --target x86_64-pc-windows-gnu`：环境中既没有 `x86_64-w64-mingw32-gcc`，也没有安装 Rust 的 `x86_64-pc-windows-gnu` target。

## 复跑脚本

Linux 测试、Clippy 和依赖策略子集可通过以下脚本复跑：

```text
.agent_workspace/round1/probes/baseline.sh
```
