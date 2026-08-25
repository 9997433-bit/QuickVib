# R1-opus-A — 修复 `adopt_project` 的 `COMPLETE` / `FETC? -230` 语义

- **代理:** R1-opus-A（本地）
- **模型 slug:** `claude-opus-5-thinking-high-fast`
- **分支:** `cursor/review-optimize-f6c8`
- **日期:** 2026-08-25

## 缺陷

`Engine::adopt_project`（GUI **应用** 与 `MMEM:LOAD:STAT` 的共同落点）把 `result` / `outcome`
置空，却没有把 `state` 从 `State::Complete` 迁走。而 `capture()` / `measurements()` /
`export_capture()` 的取数守卫是 `(Some(result), State::Complete)`，于是出现自相矛盾的一对答复：

```
REC:STAT?  -> COMPLETE      # 声称"有一份已完成的采集"
FETC?      -> (无响应)
SYST:ERR?  -> -230,"Data corrupt or stale"
```

这违反了"状态查询不得在无可取数据时报 `COMPLETE`"这一不变式；PR #1 已将其列为遗留项。

## 修复

不变式：**`REC:STAT?` 报 `COMPLETE` ⟺ 存在可取的采集数据。**

代码库要求所有状态迁移都经由唯一的 `state::transition` 函数（"illegal edges are a `match`
arm rather than scattered `if` checks"），因此没有在 `adopt_project` 里直接写
`if state == Complete { state = Idle }`，而是给状态机新增一条边。

### 1. `crates/quickvib-engine/src/state.rs`

新增事件 `Event::Invalidate`——"上一份采集被带外丢弃（例如采用了新工程）"：

| from | `Invalidate` | 理由 |
| --- | --- | --- |
| `Idle` | `Idle` | 无变化 |
| `Complete` | `Idle` | 数据已丢弃，终态不得继续声称完成 |
| `Aborted` | `Idle` | 上一次运行属于旧配置；`result` 本就为 `None`，不存在"复活"被中止的采集 |
| `Armed` / `Recording` | `IllegalTransition`（`-221`） | 运行中的 run 独占状态机 |

`Event::all()` 同步加入新变体，保证穷举表测试继续覆盖全表。

### 2. `crates/quickvib-engine/src/engine.rs`

`adopt_project` 在清空 `result` / `outcome` 之后套用该事件；迁移非法（即运行中）时保持状态
不变，因此不会破坏在飞运行的保护：

```rust
guard.result = None;
guard.outcome = None;
if let Ok(next) = transition(guard.state, Event::Invalidate) {
    guard.state = next;
}
```

因为 `load_project` 内部调用 `adopt_project`，`MMEM:LOAD:STAT` 与 `MMEM:LOAD:AUTO` 自动获得
同样的语义。文档注释也写明了这条不变式。

未改动的部分（有意为之）：错误队列不清空（采用工程不是 `*CLS`）、`any_run_started` 保持不变
（`REC:WAIT?` 在 `Idle` 下本就立即返回 `0`）、`abort_reason` 保持不变（`finish_run` 已消费）。

## 修复后的行为

```
INIT / REC:WAIT?  -> 1
REC:STAT?         -> COMPLETE
<GUI 应用 / MMEM:LOAD:STAT>
REC:STAT?         -> IDLE
REC:WAIT?         -> 0
FETC? / TRAC:POIN? / TRAC:DATA? / CALC:MEAS:*? -> -230
MMEM:STOR:TRAC    -> -230
```

## 新增测试

`crates/quickvib-engine/src/state.rs`

- `invalidating_the_capture_settles_in_idle`
- `invalidating_the_capture_mid_run_is_a_settings_conflict`
- `the_full_state_event_table_is_covered`：拒绝边由 8 条更新为 10 条（新增 `Armed` /
  `Recording` 下的 `Invalidate`）

`crates/quickvib-engine/src/engine.rs`

- `adopting_a_project_clears_the_completed_run_status`——完成一次 mock run → `adopt_project`
  → 状态为 `Idle`，`capture()` / `measurements()` / `export_capture()` 全部返回
  `-230 DataCorruptOrStale`，`wait_for_run()` 返回 `false`
- `loading_a_project_from_disk_clears_the_completed_run_status`——`MMEM:LOAD:STAT` 路径同理
- `adopting_a_project_does_not_resurrect_an_aborted_capture`——PLAN D9
- `adopting_a_project_mid_run_leaves_the_run_in_flight`——在飞运行保护未被破坏

`crates/quickvib-engine/src/dispatch.rs`

- `applying_a_project_stops_the_state_from_claiming_complete`——SCPI 线级回归：`REC:STAT?` 为
  `IDLE`、`REC:WAIT?` 为 `0`、`FETC?` / `TRAC:POIN?` / `CALC:MEAS:ALL?` 均排队 `-230`

R1-gpt-B 独立新增的集成测试 `crates/quickvib-engine/tests/adopt_project_semantics.rs`（本代理
未改动）在本修复后由红转绿，其中包括
`adopting_project_after_completed_capture_must_clear_complete_status`。

## 验证

| 命令 | 结果 |
| --- | --- |
| `cargo test -p quickvib-engine` | 70 unit + 3 integration + 1 doc-test 全通过 |
| `cargo test --workspace` | 全部通过（含 UI / CLI，无回归） |
| `cargo clippy --workspace --all-targets` | 0 warning |
| `cargo fmt --all -- --check` | 通过 |

## 交给后续轮次的遗留项（超出本代理可改文件范围）

1. `docs/PLAN.md` §5.3 的状态图只画了 `*RST` 造成的 `Complete → Idle`，应补上"采用新工程"这
   条边，并在 §5.3 正文说明该不变式。
2. `docs/SCPI.md`（PLAN Phase 7 仍缺）落地时，应把"应用工程会丢弃采集并把 `REC:STAT?` 归位
   `IDLE`"写进 `MMEM:LOAD:STAT` 与 `REC:STAT?` 条目。
3. GUI 侧可考虑在**应用**成功后提示"上一份采集已丢弃"，避免操作员误以为导出仍然可用（该文件由
   R1-opus-B 负责）。
