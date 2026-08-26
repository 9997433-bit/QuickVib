# QuickVib Round 3 终验收 — R3-fable-A

- **模型:** `claude-fable-5-thinking-xhigh`(无降级)
- **验收对象:** 分支 `cursor/review-optimize-f6c8`,最终 HEAD `77e92ba`(本地 == origin;最后一个改代码的提交是 `05ae52d`,`77e92ba` 仅工作区文档)
- **验收性质:** 只读;未改动任何 crate;未 commit/push/PR;本文件与 `probes/fable-accept/` 日志为唯一写入
- **独立验证(本人在 `77e92ba` 上复跑,日志在 `.agent_workspace/round3/probes/fable-accept/`):** `cargo fmt --all --check` 通过;`cargo test --workspace` **555 通过 / 0 失败**;`cargo test --workspace --features gui` **555 通过 / 0 失败**;两组 `cargo clippy --all-targets -- -D warnings` 干净;`ci/check-deps.sh` 通过。测试轨迹:基线 495 → R1 509 → R2 528 → **R3 555**(+27,与新增用例逐个对得上,见 §3)。

## 0. 结论

**五个 Round 3 必修项全部 PASS,零 FAIL;唯一 DEFERRED 是简报本就按合同定夺的 488.2 寄存器组。SOTA 验收通过。**

| # | 必修项 | 判定 | 落地提交 |
| --- | --- | --- | --- |
| ① | `stop_handle` 契约文档 | **PASS** | `b75b67f`(opus-B) |
| ② | B7 SCPI 会话写超时 | **PASS** | `05ae52d`(opus-A) |
| ③ | `--bind` / `server.bindHost` | **PASS** | `05ae52d`(opus-A) |
| ④ | PLAN §5.3 Invalidate 边 + `*CLS` 措辞对齐 | **PASS** | `b75b67f`(opus-B) |
| ⑤ | B12 `Refusal::SocketError` / B13 `Opc` 注释 / B14 `is_query` / `IntoAdoptResult` 死 impl | **PASS**(四项全) | `b75b67f`(opus-B) |
| — | IEEE 488.2 寄存器组(`*STB?`/`*ESR?` 等) | **DEFERRED**(按 PLAN §21.2 新增第 16 问,等 UTS 合同回执) | 简报明令不盲做 |

## 1. 逐项验收证据

### ① `stop_handle` 契约文档 — PASS

R2 报告点名的"最高优先级的一行字"落成了一整节。`crates/quickvib-device/src/backend.rs` 的 `stop_handle` 上新增 `# Contract`,三条规则逐字覆盖简报要求:

1. **每次 run 结束都执行,不只 abort**——明确写出"成功的 run 会 cancel 自己的 token 以放行看门狗,钩子随之触发;到达这里不说明采集失败"。
2. **幂等、空闲时调用安全**——`ABOR` 会同时走 `stop()` 与 token 两条路;钩子可能在 `stream` 尚未 park 或已返回后触发。
3. **不得永久失效传输**——只许打断当前这次读,下一次 `stream` 必须无需 `open` 就能工作;点名"置 `stream` 入口清除的标志位"满足、"持久链路上的 `TcpStream::shutdown`"不满足,并给出两条出路(透明重连或返回 `None` 改轮询)。这正是 R2 §2-B3 预言的 M300 地雷,现在拆在了 trait 文档里。

外加线程约束(任意线程调用、不得阻塞、不需要 `&mut`)。`engine.rs` 的 `finish_run` 在 `cancel.cancel()` 旁补了交叉引用注释——R2 建议的"顺手交叉引用"也做了。核对实现:两个现有后端仍在 `stream()` 入口复位 `stopped`,与契约一致。

### ② B7 会话写超时 — PASS

`crates/quickvib/src/scpi_server.rs`:

- `SESSION_WRITE_TIMEOUT = 5s`(简报允许 1–5s,取上限有测试 `the_shipped_write_timeout_is_modest` 锁在区间内,防止将来被随手改大);`Session::new` 对每个 accepted 会话 `set_write_timeout`,设置失败退回旧的阻塞行为而不拒绝会话——符合仪器语义。
- **实现比"只加超时"多想了一步,且这一步是对的:** `abandon_on_error` 让任何写失败立即 `shutdown(Both)`。理由成立——写到一半超时意味着对端流里躺着半行,继续存活会把下一行拼在半行后变成垃圾;关闭同时唤醒该会话的读线程走正常注销。`write_line` 改为单次 `write_all`(行不会被超时切成两半)。
- **我专门核验了锁形状:** `Engine::broadcast`(`engine.rs:180-191`)先在 sinks 锁内克隆 `Arc` 列表、锁外逐个 `notify`;`finish_run` 在 `notifying = true` 后**先 drop 引擎 guard 再广播**。5 秒停顿不持有任何锁;`REC:WAIT?`/`*OPC?` 等 `notifying` 的最坏情况从"永远"变成"每个卡死会话 ≤5s、8 会话满载 ≤40s",且这些查询自身另有看门狗上界。R2 §3.2-1 说的"全仓最后一个单客户端持久资源泄漏点"已封口。
- 测试三层:`Session` 层(套接字确带超时;对端只连不读时写在有限时间内失败且随后写直接报错)+ accept 路径层(gpt-B 预置的 `every_accepted_session_socket_has_a_write_timeout`,经真实 `ScpiServer::accept` 验证)。`clippy::disallowed_methods` 的豁免只落在测试模块并写明理由(被测对象是 OS 按墙钟执行的套接字选项,不属 D20 注入时钟范围)——豁免面收得住。

### ③ `--bind` / `server.bindHost` — PASS

规模超出简报预估的"约 30 行",但多出来的部分都花在了正确的地方:

- **单一真源** `quickvib-project::bind_host`(新文件):`parse_bind_host`(trim;IPv4/IPv6 字面量裸/带括号皆收并规范化;否则按 RFC 1123 校验;**不做 DNS**——解析失败是启动期 bind 失败退出码 3,不是工程加载失败)+ `join_host_port`(只给 IPv6 字面量加括号,堵住 `::1:5025` 这种无意义拼接)。schema 校验与 CLI 解析共用同一份实现,两个面拒绝理由完全一致。
- **接线完整:** schema `Server.bind_host`(serde default `"0.0.0.0"`,camelCase `bindHost`,`schemaVersion` 不变,旧工程行为不变);`validate.rs` 非法值 → `-224`;`cli.rs` `--bind`/`--bind=` 解析即校验、非法退出 2、usage 收录;`app.rs` 优先级链 **builder 显式 > `--bind` > 工程 > `0.0.0.0`**,SCPI 与 device **两个**监听器都走 `join_host_port`。`AppBuilder::bind_host` 改 `Option` 是必要修正——原来无条件 `"0.0.0.0"` 会盖掉工程字段。
- **默认行为零变化**(`default_bind_host_remains_all_ipv4_interfaces` 锁定),GUI 表单 `..Server::default()` 保证 Apply/保存不抹掉 `bindHost`。
- 文档四处齐:README 中英表 + 退出码 3 排查项、PLAN §12 schema 表、§13 CLI 表(opus-A 报告里说留给交接的两行,实际已在 `05ae52d` 内补上,我逐行确认)。
- 测试 20+:bind_host 单测 9、validate 2、cli 2、app 2(含 IPv6 到达双监听器、无 IPv6 宿主上退化断言 `[::1]:` 而非真空通过)、gpt-B 集成 6 + project 2。opus-A 另做了 `/proc/net/tcp` 人工冒烟。

### ④ PLAN §5.3 / §8 文档对齐 — PASS

- **§5.3** 状态图补三条边(`Complete → Idle`、`Aborted → Idle`、`Idle → Idle`,标注 "project adopted (Invalidate)"),图下新增一段说明。我对照 `state.rs::transition` 核过:`(Idle|Complete|Aborted, Invalidate) => Idle`、`(Armed|Recording, Invalidate) => Err`(即 `-221`),与文字"只存在于 settled 态、检查与写入同一次持锁"逐条一致。R1 修复①、R2 修复 B5 至此有了 normative 图。
- **§8.1 `*CLS`** 原文"clear status/event registers"与实现不符;新措辞"清错误队列 + 已武装的 `*OPC` 请求与 OPC 位,状态字节/标准事件寄存器**未实现**"。对照 `engine.rs::clear_status`(`errors.clear()` + `opc.clear()`)逐字吻合。`*OPC` 行同步改写。`docs/SCPI.md` 两行同口径(原来的 "Clear the error queue only." 其实也不准——漏了 opc,这次一并修对)。
- **顺带的正确裁量:** 简报写的 "§21.1 Q1" 在 PLAN 里并不存在(§21.1 只有 Phase 6 SDK 三问),opus-B 没有照抄错引用,而是在 §21.2 新增第 16 问(488.2 寄存器组是否为 UTS 合同所需)并让 `*CLS` 行引用它——比简报原文更准。

### ⑤ 信息级清理四项 — PASS(全)

- **B12:** `Refusal::SocketError` 新变体;`set_nonblocking(false)` 失败不再谎报 `PeerNotAllowed`;`device_server.rs` 日志映射 `reason=socketError`。**不加 `#[non_exhaustive]`** 的取舍正确——跨 crate `match` 保持穷尽,将来加变体是编译错而非被 `_` 吞掉。listener 单测从"数拒绝次数"升级为"断言拒绝理由序列",理由再写错必红;另有集成测试 `refusal_reasons.rs` 锁三变体互不相等。
- **B13:** `Opc::bit_set` 字段与 `bit()` 访问器都写明"非 SCPI 可见,无 `*ESR?`/`*STB?` 可读回,UTS 用 `*OPC?`/`REC:WAIT?`/`#REC:DONE`",并说明保留理由(须接受 `*OPC`;寄存器模型是纯增量,指向 §21.2 第 16 问)。
- **B14:** 死代码 `Command::is_query`(产品零调用,唯一调用者是自己的测试——自证循环)连同测试整块删除;`quickvib-scpi` 是 `publish = false` 内部 crate,无 API 承诺问题。
- **`IntoAdoptResult` 死垫片:** trait + 两个 impl + 两处 `.into_adopt_result()` 删净,断言语义不变。R2 指出的"签名回退会静默通过而非编译错"的反向风险随之消失。

## 2. 回归安全(R1/R2 修复在最终 HEAD 的存活)

Round 3 对既有产品代码的触碰极克制:`engine.rs` 只加了注释;`state.rs`/`errors.rs`/`mock.rs`/`cancel.rs`/`stream.rs` 零改动;`validate.rs` 只是在 R1 端口判重规则**之前**插入 bindHost 规则、规则本身未动;`form.rs` 只给 `starter_project` 补 `..Server::default()`,R1 的 GUI 数值判重未动。R2 的 528 项回归网全部包含在本轮 555 之内,我在自己的测试日志里抽查了六个哨兵用例(`a_reader_that_cannot_be_spawned_settles_the_run`、`adopting_a_project_mid_run_is_refused_under_the_lock`、`cancellation_interrupts_a_paced_batch`、`a_second_overflow_after_a_partial_drain_is_marked_again`、`abort_reaches_a_backend_that_only_wakes_through_its_stop_handle`、`the_scpi_port_and_the_device_port_must_differ`)——全部在场且通过。

## 3. 测试账目

555 − 528 = **27**,逐项对账:bind_host 单测 9 + validate 2 + cli 2 + app 2 + scpi_server 单测 3 + accept 路径超时 1 + `bind_configuration.rs` 6 + project `bind_host.rs` 2 + `refusal_reasons.rs` 1 − 删除的 `queries_are_classified_correctly` 1 = **27**,精确吻合,无来路不明的增减。gpt-B 按 TDD 先行落测试(实现前定向红),opus-A 按测试期望的 API 形状落地——两份报告的交叉表述一致,且与 diff 对得上。

## 4. 遗留观察(全部不阻塞,供后续排期)

1. `is_host_name` 接受全数字标签(如 `999.999.999.999` 会作为主机名通过工程校验,启动时以退出码 3 失败)。与"加载期不做 DNS"的设计一致,行为有界,不算缺陷。
2. `SESSION_WRITE_TIMEOUT` 是常量而非配置项;`Session::new` 已参数化,将来若需要 `server.writeTimeoutSeconds` 是最小改动。
3. 广播最坏延迟 8 × 5s = 40s(此前无界)。若现场证明太长,降常量即可,无结构改动。
4. `listener.rs` dispatch 中线程 spawn 失败不走 `on_refused`(opus-B 已记录,既有行为,本轮范围外)。
5. `samples/Test.proj` 未加 `bindHost` 行,维持"最小样例 + 默认值";GUI 无 bindHost 编辑框(重启类字段,Round 3 明确不做 Advanced 页)。
6. R3-fable-B(文档与代码交叉核验)的报告在本验收定稿时尚未出现在 `round3/`;其分工与本报告 §1-④ 有重叠,我已独立核验文档对齐,不视为缺口。

简报"不做"清单(Phase 6 FFI、完整 488.2 寄存器、export 沙箱、性能四项、Advanced 页)全部维持不做,且 export 路径遍历的既定缓解(`--bind` + 部署注记)本轮已经落地。

## 5. 最终判定

- **Round 3 必修五项:5 PASS / 0 FAIL。**
- **条件项一项:DEFERRED(488.2 寄存器组,已在 PLAN §21.2 第 16 问立案,拿到 UTS 合同回执前不做是正确决策)。**
- **质量门禁:fmt / 555×2 测试 / clippy×2(-D warnings)/ check-deps 六项,本人在最终 HEAD 独立复跑全绿。**
- **本地与 origin 同步于 `77e92ba`,工作树(除 agent 工作区日志外)干净。**

三轮累计:R1 破题(adopt/Invalidate 矛盾、端口判重、mock 假故障)→ R2 五项深修(spawn 卡死、TOCTOU、不可中断 sleep、stop 不可达、二次溢出)→ R3 封口(契约文档、最后的资源泄漏点、安全不对称、规范图漂移、死代码)。每轮修复都带"修复前必红"的测试,495 → 555 的回归网没有一项是凑数的。**以 mock/tcp 路径 + UTS 集成为验收范围,`cursor/review-optimize-f6c8` 达到 SOTA 验收标准,准予收官。**
