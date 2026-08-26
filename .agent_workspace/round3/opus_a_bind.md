# Round 3 · opus-A 报告：B7 会话写超时 + `--bind` / `server.bindHost`

- **日期：** 2026-08-25
- **分工：** 仅动 CLI / `app` / `schema` / `scpi_server`；**未触碰** `quickvib-device/src/backend.rs`、`docs/PLAN.md`、`docs/SCPI.md`（opus-B 范围）
- **未提交、未推送、未开 PR**（按指令）

## 1. B7：SCPI 会话套接字写超时

### 改了什么

`crates/quickvib/src/scpi_server.rs`：

1. 新增常量 `SESSION_WRITE_TIMEOUT = 5s`。
2. 新增 `Session::new(writer, write_timeout)`，在拿到写半边的同时 `set_write_timeout`。`serve_session`
   统一走它，测试可以传一个很短的超时（或 `None` 回到旧的无界行为）。
3. `write_responses` / `write_line` 的失败路径统一走新的 `Session::abandon_on_error`：**任何写失败都
   立即 `shutdown(Both)`**，然后把原错误原样返回。
4. `write_line` 改为把 `line + '\n'` 拼进一个缓冲后**一次** `write_all`，而不是原来的三次
   （payload、`\n`、flush）。
5. `notify` 的注释更新：现在「对端不读」和「对端消失」是同一种处理。

### 为什么是这个形状

- **只设写超时，不设读超时。** 会话空闲是合法状态（UTS 连上后可以静默很久），给读加超时会误杀。
- **5 秒（简报允许 1–5s 的上限）。** `SO_SNDTIMEO` 计的是「一次 `write` 完全没有进展」的时间，不是
  整条响应的传输时间：一个读得慢但**活着**的对端每写出一个字节就把计时清零，因此 5 秒不会误伤
  几 MB 的 `FETC?` 响应；只有真正卡死的对端才会触发。代价上界是「每个卡死会话 5 秒」，8 个会话满
  载最坏 40 秒——相对于原来的「永远」，这是有界的。
- **失败即关闭是本次修复的关键一半。** 只加超时会引入新问题：写到一半超时意味着对端流里躺着半行
  `#REC:DONE`，若会话继续存活，下一行会直接拼在半行后面，UTS 侧解析出的是垃圾。`abandon_on_error`
  关闭套接字，同时也会唤醒该会话的读线程，让 `serve_session` 走正常的注销/清理路径。
- **`set_write_timeout` 失败不拒绝会话**：退回到原来的阻塞行为，比因为一个套接字选项而拒绝 UTS 连接
  更符合仪器语义。

### 测试

`crates/quickvib/src/scpi_server.rs` 单测（模块头新增 `allow(clippy::disallowed_methods)`，并写明理由：
被测对象正是操作系统按墙上时钟执行的套接字选项，不属于 D20 注入时钟的范围）：

| 用例 | 断言 |
| --- | --- |
| `a_session_socket_carries_a_bounded_write_timeout` | `Session::new` 之后套接字确实带着 `SESSION_WRITE_TIMEOUT` |
| `the_shipped_write_timeout_is_modest` | 出厂值落在 1–5 秒区间内（挡住「以后有人随手改成 10 分钟」） |
| `a_notification_to_a_peer_that_never_reads_gives_up_and_closes_the_session` | 对端只连不读，循环写 256 KiB；写必然在有限时间内失败（无超时时该循环永不结束），且失败后套接字已关闭——后续 `write_line` 直接报错 |

另有 gpt 侧预置的 `crates/quickvib/src/scpi_server_timeout_tests.rs`
（`every_accepted_session_socket_has_a_write_timeout`，从 `ScpiServer::accept` 这一层验证），本次实现
使其通过。

## 2. `--bind` / `server.bindHost`

### 新增单一真源：`quickvib_project::bind_host`

`crates/quickvib-project/src/bind_host.rs`（新文件，随 `lib.rs` 导出）：

- `DEFAULT_BIND_HOST = "0.0.0.0"`
- `parse_bind_host(&str) -> Result<String, BindHostError>`：trim；接受 IPv4/IPv6 字面量（`::1` 与
  `[::1]` 都收，返回规范化后的裸地址）；否则按 RFC 1123 主机名语法校验（长度、标签、字符集）。
  **不做 DNS**——名字能不能解析是启动时绑定的事（退出码 `3`），不是工程加载失败。
- `join_host_port(host, port) -> String`：只给 IPv6 字面量加方括号。原来的
  `format!("{host}:{port}")` 遇到 `::1` 会拼出无意义的 `::1:5025`，这个函数就是为了堵住它。

放在 project crate 而不是 core：core 的模块文档明确写着「no I/O, no networking」；而 bindHost 本身就是
工程配置字段，校验规则和 CLI 必须一致，放这里 schema 校验与命令行解析共用同一份实现。

### 接线

| 层 | 改动 |
| --- | --- |
| `schema.rs` | `Server.bind_host: String`，`#[serde(default = "default_bind_host")]`，camelCase 为 `bindHost` |
| `validate.rs` | 新增 `server.bindHost` 规则，错误码沿用 `ProjectError::invalid`（`-224`） |
| `cli.rs` | 新增 `--bind <host>`（`--bind=host` 同样支持），解析即校验，非法值 → `CliError::BadValue` → 退出码 `2`；`usage()` 增加条目 |
| `app.rs` | 新增 `pub fn resolve_bind_host(options, project)`，与 `resolve_scpi_port` 同形；`AppBuilder.bind_host` 由 `String` 改为 `Option<String>`（builder 显式设置 > `--bind` > 工程 > `0.0.0.0`），SCPI 与 device **两个**监听器都改用 `join_host_port` |
| `README.md` | 中英双语：命令行参考表加 `--bind` 行、schema 表加 `server.bindHost` 行、退出码 `3` 排查项补充「主机名解析不了/不是本机地址」、参数说明补充非法 `--bind` 退出 `2` |

优先级链：**`--bind` > `server.bindHost` > `0.0.0.0`**，默认行为与 Round 2 完全一致。
`AppBuilder::bind_host` 改成 `Option` 是必要的：它原来无条件是 `"0.0.0.0"`，会盖掉工程里的
`bindHost`；现在只有集成测试显式调用时才生效（测试一律绑回环）。

### 向前兼容

- `schemaVersion` 仍为 `1`；旧工程文件不含 `bindHost`，靠 serde default 得到 `"0.0.0.0"`，行为不变。
- 未知字段照旧忽略（`unknown_properties_are_ignored` 仍通过）。
- 往返序列化无损：`round_trip_through_json_is_lossless` 与
  `setup_fields_survive_a_round_trip`（已扩展为带 `bindHost`）都通过。
- GUI 表单对未编辑字段是原样保留（在克隆出的 `Project` 上改字段），因此 `bindHost` 不会被应用/保存
  抹掉；`quickvib-ui/src/form.rs` 的 `Server { .. }` 字面量补了 `..Server::default()`。

### 测试

- `bind_host.rs` 模块内 8 个单测：默认值、IPv4/IPv6（裸与带括号）、空白裁剪、主机名、空串、
  `[127.0.0.1]`、`host:port` / `CIDR` / 空格 / 前导连字符 / 末尾点 / `::1%eth0` 一律拒绝、
  标签长度边界、`join_host_port` 加括号规则。
- `validate.rs`：`an_unusable_bind_host_is_rejected`、`a_bind_host_that_names_one_interface_is_accepted`。
- `cli.rs`：`bind_accepts_addresses_and_names_and_canonicalises_them`、
  `a_bind_host_that_could_never_be_bound_is_rejected`，并把 `--bind` 加进「缺值必报错」和「usage 覆盖
  所有 flag」两个既有清单。
- `app.rs`：`the_bind_host_falls_back_from_the_cli_to_the_project_to_every_interface`、
  `an_ipv6_bind_host_reaches_both_listeners`（无 IPv6 的宿主上退化为断言错误信息里的地址是
  `[::1]:…`，避免真空通过）。
- gpt 预置的 `crates/quickvib/tests/bind_configuration.rs`（6 个）与
  `crates/quickvib-project/tests/bind_host.rs`（2 个）全部通过——这两份文件先于实现存在，本次按它们
  期望的 API 形状（`project.server.bind_host` 为 `String`）落地。

### 人工冒烟

`--headless --no-auto-load --bind 127.0.0.1 --scpi-port 15025 --device-port 19123` 启动后，
`/proc/net/tcp` 显示两个监听器都在 `127.0.0.1`（而非 `0.0.0.0`），`*IDN?` 返回
`QuickVib,M300-SCPI,MOCK-0001,1.0.0`。

## 3. 验证结果（本机，含其他子代理并行改动的同一工作树）

| 检查 | 结果 |
| --- | --- |
| `cargo test --workspace` | 555 passed / 0 failed |
| `cargo test --workspace --features gui` | 555 passed / 0 failed |
| `cargo clippy --workspace --all-targets -- -D warnings` | 干净 |
| `cargo clippy --workspace --all-targets --features gui -- -D warnings` | 干净 |
| `cargo fmt --all --check` | 通过 |
| `ci/check-deps.sh` | dependency policy OK |

（R2 基线为 528；差额包含 opus-B 与 gpt 在同一工作树里的并行改动。）

## 4. 交接 / 遗留

1. **`docs/PLAN.md` 需要补两行**，本次刻意没动（PLAN.md 属 opus-B，且它当时正处于被编辑状态，改会撞车）：
   - §12 schema 表：`server.bindHost` | string | no | `"0.0.0.0"` | 两个监听器绑定的地址，
     `app::resolve_bind_host` 按 `--bind` → 本字段 → 默认解析
   - §13 CLI 表：`--bind <host>`；另 R14 行的 flag 列表、§7 架构图里的 CLI 方框文字也可顺带补
2. `samples/Test.proj` 未加 `bindHost`，保持「最小样例 + 默认值」的现状；如果 fable 认为样例应当自文档
   化，加一行 `"bindHost": "0.0.0.0"` 即可，不影响任何测试。
3. GUI 未暴露 `bindHost` 编辑框。它和 `scpiPort` 一样属于「需重启」类字段，加进去要连
   `RestartField` 一起动；本轮范围外，且 Round 3 明确「不做 Advanced 页」。
4. `SESSION_WRITE_TIMEOUT` 目前是常量，没有做成工程字段/CLI 参数。若将来 UTS 现场出现「读得极慢且
   会整段暂停 5 秒以上」的客户端，最小改动是加 `server.writeTimeoutSeconds`，`Session::new` 已经接受
   参数，不需要再改结构。
