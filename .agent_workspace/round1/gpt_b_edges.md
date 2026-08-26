# R1-gpt-B 边界/回归测试报告

## 新增覆盖

新增 `crates/quickvib-engine/tests/adopt_project_semantics.rs`：

- `adopting_project_after_completed_capture_must_clear_complete_status`
  - 先完成一次采集，再调用 `Engine::adopt_project`。
  - 期望：状态不再是 `Complete`，因为采集已被清空。
- `adopting_project_after_completed_capture_makes_fetch_and_measurements_stale`
  - 期望：项目替换后，Engine 的 capture/measurements 都返回 `DataCorruptOrStale`；
    SCPI `FETC?` 与 `CALC:MEAS:ALL?` 都无数据响应并排队 `-230`。
- `abort_discards_fetch_and_measurement_data`
  - 通过 SCPI dispatch 执行 `INIT`/`ABOR`。
  - 期望：状态为 `ABORTED`，Engine 和 SCPI 的原始数据、测量值访问均为 `-230`。

## 测试结果

命令：

```text
cargo test -p quickvib-engine --test adopt_project_semantics
```

最终结果：3 passed，0 failed。

- PASS `adopting_project_after_completed_capture_makes_fetch_and_measurements_stale`
- PASS `adopting_project_after_completed_capture_must_clear_complete_status`
- PASS `abort_discards_fetch_and_measurement_data`

初次断言运行时结果为 2 passed、1 failed，失败测试是
`adopting_project_after_completed_capture_must_clear_complete_status`：`adopt_project` 已将
capture/result 清空，但实际状态仍是 `Complete`。这证明回归测试能捕获
“COMPLETE 但无数据可取”的原缺陷。并行的引擎修复落入工作区后，用相同命令复跑得到
3 passed、0 failed。

首次运行时还短暂遇到并行修改中的 `Event::Invalidate` 非穷尽匹配编译错误；待该并行编辑补齐后，
上述测试可正常编译。

## 已有覆盖复核

以下语义在现有集成测试中已经有直接覆盖，因此未重复新增同义测试：

- 两个 SCPI session 共享一个 Engine，且一端 `INIT` 可被另一端观察：
  `crates/quickvib/tests/sessions.rs::two_sessions_share_one_instrument_model`
- 设备只允许一个活动连接：
  `crates/quickvib/tests/device_link.rs::a_second_device_is_refused_while_one_is_live`
- 设备 peer allow-list：
  `crates/quickvib/tests/device_link.rs::a_peer_outside_the_allow_list_is_turned_away`
- Abort 后 `FETC?` 为 `-230`：
  `crates/quickvib/tests/recording.rs::abort_mid_capture_leaves_no_data_behind`

以上四个已有测试均已定向复跑并通过。

本代理未编辑 `crates/quickvib-engine/src/engine.rs`，也未提交或推送。
