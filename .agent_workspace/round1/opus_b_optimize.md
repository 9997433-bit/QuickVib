# R1-opus-B — 引擎状态机之外的优化与缺陷修复

- **代理:** R1-opus-B（本地）
- **模型 slug:** `claude-opus-5-thinking-high-fast`
- **分支:** `cursor/review-optimize-f6c8`
- **日期:** 2026-08-25

## 结论

审查了 `quickvib-measure`、`quickvib-project`、`quickvib-scpi`、`quickvib-device`
（framer / listener / mock / sink / stream）、`quickvib-ui`（form / i18n / ports / controller /
status / window）、`quickvib-core` 与 `quickvib` CLI 组装层，落地 **3 处高置信度缺陷修复**，
每处都配了回归测试。三处都是逻辑缺陷，不是风格调整；未做任何整文件重排版，未触碰
`crates/quickvib-engine/src/engine.rs`。

三处缺陷有一个共同点：**都是"校验/判定写在了错的层，或者根本没写"**，因此都能在正常操作下
静默通过，直到更晚的时刻才以一个看起来无关的错误暴露出来。

---

## 修复 1 — 工程校验漏掉了 SCPI 端口与设备端口冲突

**文件:** `crates/quickvib-project/src/validate.rs`

`validate()` 是工程文件的最终校验闸门：`ProjectStore::load`、`MMEM:LOAD:STAT`、GUI 的**应用**
全部经由它。它逐条检查了 `scpiPort ≠ 0` 与 `device.port ≠ 0`，却**从未比较过两者是否相等**。

后果不是一个立刻可见的错误，而是一枚延时雷：

1. 操作员（或 UTS 通过 `MMEM:LOAD:STAT`）加载一份 `scpiPort == device.port` 的工程；
2. 校验通过，工程被采用，当前进程一切正常——因为两个监听器早在启动时就已用旧端口绑好了；
3. 下一次启动时 `AppBuilder` 先绑 SCPI 再绑设备口，第二次绑定撞上
   `Address already in use`，得到一个 `StartupError::Bind`。

也就是说，**报错时机与报错内容都指不回真正的原因**——那份工程文件。GUI 表单侧本就有
`Issue::DuplicatePort` 这条规则（见修复 2），说明"两个端口必须不同"是既定的产品规则，只是
从未下沉到 `validate()`，于是绕过 GUI 的任何一条路径都不受约束。

修复是在 `scpiPort ≠ 0` 之后补上相等检查，沿用同一个 `ProjectError::invalid`
（映射到 SCPI `-224 Illegal parameter value`），错误文本带上冲突的端口号：

```rust
if project.server.scpi_port == project.device.port {
    return Err(ProjectError::invalid(
        "server.scpiPort",
        format!("must differ from device.port, both are {}", project.server.scpi_port),
    ));
}
```

**测试:** `the_scpi_port_and_the_device_port_must_differ` —— 相等时报
`IllegalParameterValue` 且错误信息点名 `device.port`；改回不同端口后校验通过（确认新规则没有
顺手把别的东西也拒掉）。

默认值 `DEFAULT_SCPI_PORT = 5025` 与 `DEFAULT_DEVICE_PORT = 9123` 本就不同，`samples/Test.proj`
同理，因此该规则不会让任何既有工程文件失效——全工作区测试通过也印证了这一点。

## 修复 2 — GUI 的重复端口判定比的是字符串而不是端口

**文件:** `crates/quickvib-ui/src/form.rs`

`ProjectForm::to_project` 原先这样判重：

```rust
if self.scpi_port.trim() == self.device_port.trim() {
```

两个方向都是错的：

- **漏报：** 端口是数字，不是文本。设备口为 `9123` 时，SCPI 口填 `09123`、`+9123` 都会解析成
  同一个端口，但字符串不相等，于是 GUI 放行。在修复 1 之前，这种工程能一路存盘落地；即便有了
  修复 1，也只是从"友好的、指向具体控件的本地化提示"退化成一条通用的校验错误。
- **误报：** 两个字段都留空时，`"" == ""` 成立，于是在两条"这不是合法端口"之外再叠一条
  `DuplicatePort`——用一个不存在的冲突去污染一份本就说得很清楚的错误列表。

根因是 `port()` 解析失败时返回哨兵值 `1`，把"解析失败"和"端口就是 1"混为一谈，导致调用方拿
不到可靠的数值去比较。改为让它返回 `Option<u16>`，把失败显式表达出来：

```rust
fn port(errors: &mut Vec<FieldError>, field: &str, text: &str) -> Option<u16> {
    match text.trim().parse::<u32>() {
        Ok(value) if (1..=65_535).contains(&value) => Some(value as u16),
        _ => { errors.push(/* BadPort */); None }
    }
}
```

判重随之改为只在两侧都解析成功时按数值比较：

```rust
if matches!((scpi_port, device_port), (Some(scpi), Some(device)) if scpi == device) {
```

占位赋值仍写 `unwrap_or(1)`，行为与改动前完全一致；且 `to_project` 在 `errors` 非空时返回
`Err(errors)`，占位值不会外泄到任何一份真实工程里。

**测试:**

- `a_duplicate_port_is_caught_however_it_is_spelled` —— `"09123"` / `"+9123"` / `" 9123"`
  三种写法都必须撞上设备口并报 `Issue::DuplicatePort`。
- `two_unparsable_ports_do_not_look_like_a_duplicate` —— 两个空字段只产出 2 条错误，且其中
  没有 `DuplicatePort`。

## 修复 3 — mock 故障注入会把跑完的流报成故障

**文件:** `crates/quickvib-device/src/mock.rs`

`MockFault::ShortStream(n)` 与 `MockFault::LinkLostAfter(n)` 的语义都是"先发 n 个采样，然后
开始捣乱"。但 `stream()` 的主循环结束后无条件执行：

```rust
self.fault_outcome(delivered)
```

主循环正常跑完只有一种含义：**request 要的采样一个不少地发完了，途中没有触碰任何故障阈值**
（真触碰了会在循环内部经由 `fault_outcome` 提前返回）。于是当故障阈值大于本次请求的采样数
时——例如 `LinkLostAfter(5000)` 配上一个 250 采样的 run——一次完整、正确、全额交付的采集，
会被报成 `StreamOutcome::LinkLost` 或 `ShortStream`。

这个方向的错误特别隐蔽：mock 是测量算法与异常路径测试的"标准答案"（README §9），一个
**假阳性的故障**会让本该失败的负向测试变绿，也会让"故障阈值设大一点以关闭故障"这种自然的
测试写法悄悄失效。

修复是让主循环出口直说事实：

```rust
// Reaching here means every requested sample was delivered, so no fault limit was
// crossed on the way: the loop returns through `fault_outcome` when one is.
Ok(StreamOutcome::Completed)
```

同时把 `fault_outcome` 的文档注释补成"只在阈值确已触达时调用"，免得下一个读者重新引入同样的
无条件调用。

**测试:** `a_fault_limit_beyond_the_request_never_fires` —— 对 `ShortStream(5000)` 与
`LinkLostAfter(5000)` 各跑一次 250 采样的 run，断言采样数为 250 且结果为
`StreamOutcome::Completed`。

---

## 改动文件

| 文件 | 改动 |
| --- | --- |
| `crates/quickvib-project/src/validate.rs` | 新增 `scpiPort ≠ device.port` 校验 + 1 测试 |
| `crates/quickvib-ui/src/form.rs` | `port()` 改返回 `Option<u16>`；判重改为数值比较 + 2 测试 |
| `crates/quickvib-device/src/mock.rs` | 全额交付的流返回 `Completed`；补 `fault_outcome` 文档 + 1 测试 |
| `docs/PLAN.md` | §12 校验规则清单补上新规则 |
| `README.md` | 中英文两张工程字段表的 `server.scpiPort` 行注明须与 `device.port` 不同 |

未触碰 `crates/quickvib-engine/src/engine.rs`；未 commit / push / 开 PR；未涉及 Phase 6 M300
FFI，也未加入任何假 DLL。

## 验证

| 命令 | 结果 |
| --- | --- |
| `cargo test -p quickvib-project` | 39 unit + 2 integration + 1 doc-test 通过 |
| `cargo test -p quickvib-ui` | 58 unit + 1 doc-test 通过 |
| `cargo test -p quickvib-device` | 67 unit + 1 doc-test 通过 |
| `cargo test --workspace` | 全部通过，0 failed（含 CLI 集成测试，无回归） |
| `cargo clippy --workspace --all-targets` | 0 warning |
| `cargo fmt --all --check` | 通过 |

## 审过但有意不改的地方

按"宁可 0–3 处有测试的小修，也不要一次大重构"的取舍，以下均记录而不动：

1. **CLI 覆盖仍可造成端口冲突。** `--scpi-port 9123` 配上工程里的 `device.port = 9123` 绕过了
   修复 1（`validate()` 看的是工程文件，不是解析后的实参）。但这一条会**当场**以
   `StartupError::Bind` + `Address already in use` 失败，报错时机和内容都指得回原因，与修复 1
   针对的"延时雷"性质不同。另外 `--scpi-port 0 --device-port 0` 是合法的（0 表示让 OS 各分配
   一个空闲端口，现有测试正依赖这一点），任何 CLI 层判重都必须给 0 开口子，风险高于收益。
2. **`listener.rs`：`set_nonblocking(false)` 失败被报成 `Refusal::PeerNotAllowed`。** 确实是错
   误映射——建立连接后的 socket 配置失败，与"对端不在白名单"是两回事，会误导排障。干净的修法
   需要新增一个 `Refusal::SetupFailed` 变体并打通到日志与 UI 文案，改动面超出"小修"范畴，留给
   后续轮次。
3. **`sink.rs` 的 `received()` 文档与实现措辞不一致**、**`lexer.rs` 关于引号感知切分的注释不准
   确**、**`cli.rs` 关于非 UTF-8 路径的注释有误导性**：均为文档漂移，不影响行为，不构成逻辑
   缺陷，未纳入本轮。
4. **热路径上的微小开销**：`mock.rs` 每采样重复计算三角函数、`scpi/format.rs` 的 `write_samples`
   多次 `write_all`（外层已有 `BufWriter`）。实测量级可忽略，改了反而增加读的成本，未动。
5. **`stats.rs` 的 `remove_dc` 在样本含 `inf` 时均值为 `NaN`**：这属于设备已经送出坏数据的场
   景，正确的处置点在采集侧而非统计侧，不在本轮范围。
6. **R1-opus-A 报告里留给本代理的第 3 条**（应用工程成功后在 GUI 提示"上一份采集已丢弃"）：需
   要改 `controller.rs` / `window.rs` 的 `ApplyOutcome` 路径，与 R1-opus-A 正在改的
   `adopt_project` 语义直接相邻。任务明确要求"避免与 adopt_project 冲突"，故留待其修复合入后
   再做。
