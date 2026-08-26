# Round 3 文档核对报告(R3-fable-B)

- **核对基线**:HEAD `0b4695e5a3b91b0b29ca7ee70875603d38c89278`(其上仅新增 `.agent_workspace/round3/fable_a_accept.md`;全部代码与文档内容与 `77e92ba` 一致,即 Round 3 补丁 `b75b67f`(opus-B)与 `05ae52d`(opus-A)落地之后的状态)。
- **核对范围**:README.md(中英双语两份镜像)、docs/SCPI.md、docs/PLAN.md(重点 §5.3/§5.4/§7.2/§8/§10/§12/§13)、`stop_handle` rustdoc、`bindHost` schema 及其校验链路。
- **方法**:逐条把文档声明与 `quickvib-engine`/`quickvib-device`/`quickvib-project`/`quickvib`(bin)各 crate 的实现对照;未编辑任何 crate,未 commit/push。
- **结论**:Round 3 指定的修复(`stop_handle` 合同、`--bind`/`bindHost`、会话写超时、`*CLS` 措辞、`Invalidate` 边、`is_query`/`IntoAdoptResult` 移除)全部落地且与代码一致;但仍有 **11 处**文档与代码不符或文档缺口,列举如下(按严重程度排序)。

---

## 一、仍然存在的错配

### M1. README:`*CLS` 仍声称清"状态寄存器"(中等)

- **文档**:README.md L313(英文)/ L1015(中文):`*CLS` — "Clear the error queue and status registers" / "清空错误队列与状态寄存器"。
- **代码**:`crates/quickvib-engine/src/engine.rs` L488–492(`clear_status` 只清错误队列与 OPC 状态);`crates/quickvib-engine/src/opc.rs` L11–16 明确说明 IEEE 488.2 状态寄存器**未实现**(无 `*ESR?`/`*STB?`)。
- **对照**:docs/SCPI.md L16 与 docs/PLAN.md L716 已由 opus-B 改为"清错误队列与已挂起的 `*OPC` 请求",唯独 README 两处未同步。
- **建议**:README 两行改为"清空错误队列与已挂起的 `*OPC` 请求(状态寄存器未实现)"。

### M2. README:工程 schema 表 `device.backend` 缺 `"tcp"`(中等)

- **文档**:README.md L452(英文)/ L1151(中文):`device.backend` 允许值只列 `"mock"` 或 `"m300"`。
- **代码**:`crates/quickvib-project/src/serde_enums.rs`(反序列化接受 mock/tcp/m300,错误信息为 "mock, tcp or m300")。
- **对照**:README 自己的 §4 命令行表(L251/L957)与 docs/PLAN.md L967 都正确列出三个值,只有这张 schema 表漏了 `tcp`。
- **建议**:两处改为 `"mock"`、`"tcp"` 或 `"m300"`。

### M3. README:§5 状态图画了 `COMPLETE → ABORTED` 的边(中等)

- **文档**:README.md L296–298(英文)/ L999–1001(中文)ASCII 图:`ABOR / timeout` 一线的箭头把 COMPLETE 下方的竖线也接进了 `▶ ABORTED`,即图面上存在 `COMPLETE --ABOR/超时--> ABORTED`。
- **代码**:`crates/quickvib-engine/src/state.rs` L161 `(Complete, Abort) => Complete` 是显式空操作;Complete 状态也没有任何超时路径(看门狗只在 run 进行中有效)。
- **对照**:docs/PLAN.md §5.3 的 mermaid 图(L245–260)与 §8 `ABOR` 行(L746)均正确。
- **建议**:重画 ASCII 图,让 `ABOR / timeout` 只从 ARMED 与 RECORDING 引出。

### M4. PLAN §5.4:声称 `*OPC?` 标志是"会话级"(中等)

- **文档**:docs/PLAN.md L284(线程表"SCPI session thread"行,Owns 列):"session-local `*OPC?` flag"。
- **代码**:`Opc` 是 `EngineState` 里的**单例**(`crates/quickvib-engine/src/engine.rs`,`guard.opc`;`opc.rs` L8–16)。任一会话发 `*OPC` 会武装全局位,任一会话发 `*CLS` 会把它清掉;`*OPC?` 的阻塞条件 `opc.is_operation_in_progress()` 也是全局的。不存在任何会话级 OPC 状态。
- **建议**:该单元格改为"(无会话级 OPC 状态;OPC 记录在引擎全局 `EngineState` 中)"或直接删去。

### M5. PLAN §7.2:通知发给"启用了通知的会话"——不存在该开关(中低)

- **文档**:docs/PLAN.md L523–524:"the engine pushes the literal line `#REC:DONE` to every connected session **that has notifications enabled**"。
- **代码**:`crates/quickvib-engine/src/engine.rs` L180–187 `broadcast` 无条件复制全部已注册 sink 并逐一 `notify`;代码库中没有任何"启用/停用通知"的机制。
- **对照**:docs/SCPI.md L45–46("pushed to every open session")与 README L345–346 均正确。
- **建议**:删去 "that has notifications enabled",改为 "to every connected session"。

### M6. PLAN/README:5 秒会话写超时完全未见于文档(中等,Round 3 新行为的缺口)

- **代码**(opus-A `05ae52d` 新增):`crates/quickvib/src/scpi_server.rs` L35 `SESSION_WRITE_TIMEOUT = 5 s`;L203–206 每个会话的写半套接字都设置该超时;L241–245 `abandon_on_error`——任何写失败(含超时)立即 `shutdown(Both)` 关闭套接字,同时唤醒该会话的读线程收尾。
- **文档缺口**:
  - docs/PLAN.md L528–530 仍写 "Idle sessions are never timed out by QuickVib; A session thread parked in `read_until` exits **when the peer closes or when shutdown calls `shutdown(Both)`**"——读空闲不超时仍然成立,但会话线程如今还有第三条退出路径:对端在写(响应或 `#REC:DONE` 广播)上停摆超过 5 秒即被服务端断开。§5.4 线程表 L284 也只提"per-session `Mutex<TcpStream>` 写半",未提超时。
  - README L345–348(英文)/ L1047–1048(中文)只描述"每会话写锁",未提"停摆的客户端会在约 5 秒后被断开"。
- **建议**:PLAN §7.2 补一句"每会话写操作带 5 秒超时;写超时/写失败即关闭该会话,防止一个停摆的对端阻塞广播";README §6 通知段落加同样一句。

### M7. 看门狗阻塞上界的 "+1 s" 措辞与代码硬上界 "+2 s" 不符(中低)

- **文档**:README.md L408–409(英文)/ L1109–1110(中文):"`*OPC?`/`REC:WAIT?` 最长不会超过 `duration × timeoutMultiplier + 1 s`";docs/PLAN.md L718(`*OPC?` 行)同样写 "cannot hang past `duration × multiplier + 1 s`"。
- **代码**:看门狗时限本身 = `duration × multiplier + 1 s`(`crates/quickvib-project/src/schema.rs` L422–424);而阻塞查询的 condvar 等待上界是 `guard.watchdog + 1 s`(`crates/quickvib-engine/src/engine.rs` L531、L551),即硬上界为 `duration × multiplier + 2 s`。正常情况下看门狗在 +1 s 处触发并立刻唤醒等待者,但文档所述的"最长不超过 +1 s"并不是代码保证的界。
- **附注(docs-only 注释)**:`engine.rs` L528 的 rustdoc 也写着 "cannot hang past `duration * multiplier + 1 s`",与其下两行自己的实现(`watchdog + 1 s`)不一致——属代码注释问题,本报告仅记录,不改 crate。
- **建议**:三处改为"由看门狗兜底,最长约 `duration × multiplier + 1 s`(容忍最多再加 1 秒的唤醒宽限)",或直接写 `+ 2 s` 为硬上界。

### M8. PLAN §10 与 backend.rs 顶部 rustdoc 的"六个方法"计数过期(低)

- **文档**:docs/PLAN.md L891 "**Six methods total.** If the trait grows past this…";§10 的 trait 草图(L841–865)只有 `is_connected/open/capabilities/stream/stop`,没有 `stop_handle` 和 `close`。
- **代码**:`crates/quickvib-device/src/backend.rs` 里 `DeviceBackend` 现有**七个**方法(Round 2/3 期间新增了 `stop_handle`);其 L126 rustdoc 首句同样写 "Six methods, deliberately"(docs-only 注释,仅记录)。
- **说明**:§10 自我声明为 "trait sketch / not a source file",草图缺方法尚可谅解,但"总共六个方法"的断言以及 PLAN 全文对 `stop_handle` **零提及**(它如今是 `ABOR`/看门狗能唤醒阻塞读的关键机制,§5.4/§7.4 的取消模型描述里都没有)已与实现脱节。
- **建议**:PLAN L891 更新计数并补一句 `stop_handle` 的存在与动机(引用 backend.rs 新 rustdoc 合同即可)。

### M9. PLAN §12:加载期校验规则清单漏了 `bindHost`(低)

- **文档**:docs/PLAN.md L998–1003 "Validation rules enforced on load" 列举了 sampleRateHz/duration/multiplier/decimals/枚举/容量/滤波/量程/端口等规则,未提 `server.bindHost`。
- **代码**:`crates/quickvib-project/src/validate.rs` L137–141 在加载期用 `parse_bind_host` 做语法校验(空串、`host:port`、非法括号、非法主机名一律拒绝,SCPI 视角为 `-224`)。
- **对照**:同节 schema 表 L993 已有 `server.bindHost` 行(opus-A 补的),只是规则清单没跟上。
- **建议**:在该段末尾补 "`bindHost` 必须是 IP 字面量或语法合法的主机名(不做 DNS)"。

### M10. README §2.1:"窗口不显示的字段"清单漏了 `server.bindHost`(低)

- **文档**:README.md L168–171(英文)/ L887–888(中文)枚举 GUI 不显示但原样保留的字段:"看门狗倍数、采集缓冲上限、`*IDN?` 标识块、会话上限、允许的对端地址、mock 信号定义"。
- **代码**:Round 3 新增的 `server.bindHost`(`crates/quickvib-project/src/schema.rs` L209–212)同样不在 GUI 中展示(`quickvib-ui` 无任何 bind 相关控件),经加载/编辑/保存原样保留——它应出现在这份"不显示字段"枚举里。
- **附注**:L177–179 "五项设置需重启"(两个端口、后端、采样率、数据类型)与 `crates/quickvib-ui/src/controller.rs` 的 `restart_required` 一致,**无需**改动——`bindHost` 不是 GUI 可编辑项,不冲突。
- **建议**:两处清单各加一项"监听地址(`server.bindHost`)"。

### M11. 状态图的两处简化边缺失(低;正文均有正确描述)

- **缺 `Armed → Complete`**:`crates/quickvib-engine/src/state.rs` L153 `(Armed, Completed) => Complete`(整段采集一批到齐的 run 不经过 `Recording`)。docs/PLAN.md §5.3 图(L245–260)与 README 两张 ASCII 图都没有这条边。
- **缺 `Armed/Recording → Idle: *RST`**:state.rs L139 `(_, Reset) => Idle`,PLAN L715(`*RST` 行)与 L1179("`*RST` from every state lands in `Idle`")的文字都正确,但 §5.3 图只画了 `Complete/Aborted --*RST--> Idle`。
- **建议**:mermaid 图补 `Armed --> Complete: whole capture in one batch` 与 `Armed/Recording --> Idle: *RST(先中止)`;README ASCII 图受限于排版,可在图注里加一句说明。

---

## 二、复核确认无误的项(Round 3 修复验收)

| 项 | 结论 |
| --- | --- |
| `stop_handle` rustdoc 合同(`backend.rs` L165–201) | ✅ 三条规则(每次 run 结束都会触发、幂等且空闲安全、不得永久废掉传输)与 engine 的注册/取消路径(`engine.rs` L935–938)一致;`MockBackend`/`StreamBackend` 走"默认 `None` + 轮询 token"路线,`stream` 入口清 `stopped` 标志(mock.rs L253、stream.rs L134),符合合同 |
| `--bind`/`bindHost` 文档链 | ✅ README §4 L249/L955、§12 L470/L1169、排障表 L680/L1358,PLAN L993/L1018,与 `cli.rs`(非法主机退出 2、`[::1]` 规范化)、`bind_host.rs`(默认 `0.0.0.0`、RFC 1035 长度限制、加载期不做 DNS)、`app.rs`(绑定失败→退出 3)全部吻合 |
| 会话写超时实现本身 | ✅ 常量 5 s、每会话设置、失败即 shutdown 并唤醒读线程,有测试覆盖——问题只在文档未记载(见 M6) |
| SCPI.md 全文 | ✅ 在 HEAD 上逐行核对命令表、错误表、"加载丢弃采集"节,未发现错配(`*CLS`、`-350` 覆盖最新条目、`*IDN?` 序列号回退、别名、`REC:WAIT?` 语义均正确) |
| PLAN §5.3 `Invalidate` 边 | ✅ opus-B 补齐,与 state.rs L164–165 一致(settled 态→Idle,运行中拒绝为 `-221`) |
| `is_query` / `IntoAdoptResult` | ✅ 已从 `quickvib-scpi`/`quickvib-testkit` 移除,文档无残留引用 |
| 退出码 0/2/3/4/5 | ✅ README L239–240、PLAN L499–501、`cli.rs` usage、`app.rs` 常量四处一致;`--backend m300` 在不支持平台归类 `Unavailable`→退出 2(PLAN L1020 所述正确),后端打开失败→退出 5 |
| 错误队列 | ✅ 32 深 FIFO、溢出替换最新条目为 `-350`,README/SCPI/PLAN 三处描述一致 |
| PLAN §8 `*(proposed)*` 标记 | ✅ 前言 L707 已自定义该标记为"brief 未点名的新增",与实现状态不构成矛盾 |

---

## 三、汇总

| 编号 | 位置 | 严重度 | 一句话 |
| --- | --- | --- | --- |
| M1 | README L313 / L1015 | 中 | `*CLS` 仍说清"状态寄存器" |
| M2 | README L452 / L1151 | 中 | schema 表 `device.backend` 缺 `tcp` |
| M3 | README L296–298 / L999–1001 | 中 | 状态图多画 `COMPLETE→ABORTED` |
| M4 | PLAN L284 | 中 | `*OPC?` 标志并非会话级而是全局 |
| M5 | PLAN L523–524 | 中低 | 不存在"通知启用"开关 |
| M6 | PLAN L284/L528–530;README L345–348/L1047–1048 | 中 | 5 秒写超时无任何文档记载 |
| M7 | README L408–409/L1109–1110;PLAN L718 | 中低 | 阻塞上界写 `+1 s`,代码硬上界 `+2 s` |
| M8 | PLAN L841–892(+backend.rs L126 注释) | 低 | trait 已七个方法,"六个"计数与草图过期;PLAN 全文无 `stop_handle` |
| M9 | PLAN L998–1003 | 低 | 校验规则清单漏 `bindHost` |
| M10 | README L168–171 / L887–888 | 低 | GUI"未显示字段"清单漏 `bindHost` |
| M11 | PLAN L245–260;README 两图 | 低 | 图缺 `Armed→Complete` 与运行中 `*RST` 边 |

以上 11 项全部为**文档侧**修改即可闭合(M7/M8 各含一处 crate 内 rustdoc 注释的同步,属 docs-only comment,遵嘱未改动代码)。
