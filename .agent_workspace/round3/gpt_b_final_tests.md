MODEL_SLUG: gpt-5.6-sol-xhigh-fast

# R3-gpt-B 回归测试报告

## 产出

- `crates/quickvib/tests/bind_configuration.rs`
  - 默认仍绑定 `0.0.0.0`
  - `--bind 127.0.0.1` 与 `--bind=127.0.0.1`
  - `--bind` 缺值/空值拒绝，且 help 中列出该参数
  - `server.bindHost` 驱动 SCPI、device 两个 listener
  - CLI `--bind` 覆盖项目 `server.bindHost`
- `crates/quickvib-project/tests/bind_host.rs`
  - 缺省 `bindHost` 为 `0.0.0.0`
  - camelCase 字段解析、序列化及 round-trip
- `crates/quickvib/src/scpi_server_timeout_tests.rs`
  - unit-level 建立真实 TCP pair，经 session accept 路径后直接读取 socket
    `write_timeout()`，断言为正且非 `None`
  - `scpi_server.rs` 仅增加四行 `#[cfg(test)]` 模块声明
- `crates/quickvib-device/tests/refusal_reasons.rs`
  - 锁定 `Refusal::SocketError` 必须存在且与两种业务拒绝原因不同

共新增 10 个测试；未修改 `engine.rs`。

## 当前验证

- `cargo fmt --all`：通过
- `git diff --check`：通过
- 定向测试已运行；同级实现尚未落地时按预期为红：
  - project 测试：`Server` 尚无 `bind_host`
  - device 测试：`Refusal` 尚无 `SocketError`
  - bind 集成测试：`--bind` 仍为 unknown flag，项目 bindHost 尚未接线
  - timeout unit 测试：accepted session 的 `write_timeout()` 仍为 `None`
  - bind 默认兼容性测试已通过

未执行 commit、push 或 PR 操作。
