# 018 — 实现：LLM Provider、流式护栏与请求策略

- 状态：done
- 依赖：005
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-07

## 目标与背景

LLM 设计已冻结，但仓库尚无供应商调用与安全输出流水线。交付不依赖 Tauri 的 aoidos-llm 核心，以可重复的本地夹具验证生成行为。

## 必读

[LLM 规则与结构](../architecture/llm.md) · [通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md) · [Rust 规范](../standards/rust.md) · [前端规范](../standards/frontend.md) · [注释规范](../standards/comments.md) · [测试规范](../standards/testing.md) · [提交规范](../standards/commits.md)。依赖任务及后续设计定稿文档从[任务索引](../task-index.md)查阅；规则数值以架构 / 契约原文为准，不在 task 重写。

## 范围与非目标

- 薄 Provider、Completion / Chat / Prefix 能力声明与 DeepSeek adapter；规范化 SSE、UTF-8、JSON 和结构化错误。
- GuardSpec 编译、换行规范化、跨分片扣留与终止；使用正式机制和测试语法，具体记录语法由 006 / 022 提供。
- 一个调度器管理传输重试、互斥温度阶梯、看门狗和可取消背压；禁用 HTTP / SSE 隐式重试。

非目标：不包含凭据持久化、窗口事件、回合快照、记录存储或产品 UI。

## 前置条件与待决策

005 评审通过后可开始。先探测锁定解析器的缓冲限制和 TLS source 分类；无法满足设计的能力必须显式拒绝或替换实现。

## 实施步骤

1. 核对官方 API 与锁定依赖，先验证 SSE 内存上限、异常 EOF 和 TLS 分类；不把候选依赖当成已选定。
2. 创建纯 Rust crate，先接一次尝试，再接护栏和唯一请求调度器；能力验证发生在网络请求前。
3. 通过本地服务计数、分片黄金样例和虚拟时钟验证正常 / 失败路径，更新实际选型与能力表。

## 预计改动

待创建 `src-rust/aoidos-llm/` 及测试夹具；更新现存根 workspace、锁文件与 package scripts（新增命令时）。以上为规划，不表示目录或接口已经实现。

## 验收标准

- [x] 每个 UTF-8 字节切割、重叠 stop、行首 / 正文标记、CRLF、EOF 和取消得到设计规定的相同安全文本。（decode 三段切割全位置重放、sse 字符边界重放、deepseek 字节切割全位置重放、guard 黄金样例与 `abc`/`b` 优先级用例、取消丢弃 `[PLA` 待判定尾部）
- [x] 本地服务实测请求次数符合契约；两种重试不叠加，首交付后无重发，重试期间请求配置冻结。（`transport_failure_retries_then_succeeds_with_frozen_config` 断言两次请求体逐字节相同；`mid_stream_abort_after_delivery...` 断言首字节后仅 1 次请求；温度阶梯 [1.0, 1.1, 1.2] 逐请求断言）
- [x] 心跳 / reasoning 不延寿；连接、读流、退避和满队列都能取消；正文与 SSE 缓冲有上界。（`head_stall_with_heartbeat_and_reasoning...`、`no_response_at_all_stalls...`、`cancel_during_backoff...` / `cancel_during_stalled_backoff...`、`full_queue_cancel...`；SSE 1 MiB 上限、护栏扣留 ≤ 最长规则 + CR）
- [x] HTTP、TLS、损坏协议、服务端终止及空输出分类均有失败夹具，不依赖错误字符串匹配。（401/402/429/404/5xx 状态表、自签名证书 / 主机名不符 → `Tls`（error source 链类型下钻，实测链形 reqwest → hyper_util → io → io → rustls）、RST 断连、干净 EOF 无 [DONE]、`[DONE]` 缺 finish、未知 finish、SSE/JSON/UTF-8 损坏、guard / length / stop 空输出）
- [x] 正常 stop 空输出可按条件进入阶梯；guard / length 空输出不重试，调度结果保留最终实际 finishReason。（`empty_stop_enters_ladder_and_recovers`、`guard_empty_output_never_enters_ladder`、`length_empty_output_never_enters_ladder`、`transport_retry_used_then_empty...`；`RunOutcome::Completed` 携带实际 finish，`EmptyOutput` 携带 stop/guard/length）
- [x] 代码、注释、类型、文档与 task 同步；最终状态 `bun run verify` 全项通过。

## 验证计划与结果

使用本地 HTTP / TLS fixture、虚拟时钟与黄金样例；不需要真实 API key 或付费请求。
看门狗语义不依赖绝对时长：测试用缩短的真实预算（初值 80ms，第七轮加固为 250ms，现值见 `fast_policy`）驱动全部超时 / 取消路径，契约默认值（30s / 90s / 500ms / 1+2 / 32 / 3）由 `policy_defaults_match_contract` 单测钉死；未用 tokio paused 虚拟时钟——暂停时钟在真实 IO 等待期间会空转推进时间，可能误触发计时器。

| 日期       | 环境 / 命令                                                             | 预期        | 实际结果                                                                        |
| ---------- | ----------------------------------------------------------------------- | ----------- | ------------------------------------------------------------------------------- |
| 2026-10-06 | `cargo test -p aoidos-llm`                                              | 全部通过    | 147 通过（decode/sse/guard 单测 + deepseek 25 + schedule 33 + testserver 3 等） |
| 2026-10-06 | `cargo llvm-cov -p aoidos-llm --lib --ignore-filename-regex "lib\.rs$"` | 行覆盖 100% | Lines 100.00%（4116/4116），函数 100%                                           |
| 2026-10-06 | `cargo clippy -p aoidos-llm --all-targets -- -D warnings`               | 无告警      | 通过                                                                            |
| 2026-10-06 | `bun run verify`（提交前在最终状态重跑）                                | 十项通过    | 十项全部通过                                                                    |

## 风险与回退

供应商 Beta 与解析器行为可能变更；先完成探测，再选择依赖，保留显式拒绝不支持能力的路径。回退代码时同步撤销接口 / 依赖与文档；已有存档和凭据不因回退删除。

## 决策与工作记录

- 2026-10-05：按用户要求规划 LLM 相关实现任务，明确依赖、范围和失败路径；本次只登记计划，不实施代码。
- 2026-10-06：issue #18 统一 empty-output 的对外 finishReason 规则，同步相关失败路径验收；任务状态不变，尚未开始实现。
- 2026-10-06：issue #44 明确与 035 的单向接入：本任务提供物理尝试身份 / 生命周期与可注入预算端口，035 落地账本后接入；预算未实现阶段仅用本地夹具，不宣称付费产品已受额度控制。
- 2026-10-06：依赖探测定稿——reqwest 0.13.5 文档确认默认 "retry protocol NACKs"，显式 `retry(never())`；eventsource-stream 0.2.3 无事件上限且边界不可外部观测，按设计预案换成自建有界 SSE 解析器；reqwest 0.13 `rustls` feature 绑定 aws-lc-rs（Windows 需 CMake + NASM），改 `rustls-no-provider` + 构造前显式安装 ring provider；wiremock 无法表达字节级分片 / 中途断连，改自建 TcpListener 夹具（脚本段 + 按连接轮换 + RST + 自签名 TLS）。
- 2026-10-06：TLS 错误分类以实测链标定（reqwest::Error kind=Request → hyper_util legacy Error(Connect, io) → io(Other) → io(InvalidData) → rustls::Error），走 source 链与 `io::Error::get_ref` 的类型下钻递归，不匹配字符串；`is_request()` 对传输错误也为真，不能据此判本地构造失败。
- 2026-10-06：实现落地——`decode`（增量 UTF-8，异常 EOF 不合成 U+FFFD）、`sse`（有界增量解析，CRLF/CR/LF、BOM、EOF 冲刷）、`guard`（GuardSpec 编译 + 跨分片护栏，最早结束位置 / 最长规则 / 稳定优先级裁决，扣留 ≤ 最长规则 + 待判定 CR）、`error`（ProviderError 类别 + 交付边界定码）、`provider`（trait / 能力 / 请求与增量类型）、`providers/`（适配层目录：ProviderId 注册 + DeepSeek completion/chat/prefix）、`schedule`（传输重试与温度阶梯互斥、30s/90s 看门狗、500ms±25% 指数退避、可取消背压、BudgetPort 预留-结算生命周期、3 次请求上限）。
- 2026-10-06：用户要求供应商实现独立成适配层目录，`deepseek.rs` 移入 `src/providers/`，`providers/mod.rs` 提供 ProviderId 身份 / 默认端点 / 解析（新厂商 = 新模块 + 登记，不动核心流水线）。
- 2026-10-06：夹具服务器按用户可见行为打磨——测试用缩短真实预算替代 paused 虚拟时钟（暂停时钟在真实 IO 等待期间空转推进会误触发计时器）；退避取消测试用预算端口记账（观察到 S1:Failed 再取消）消除竞态；Windows 下 RST 会清客户端未读缓冲，中断类夹具在 RST 前留读取窗口。
- 2026-10-06：`bun run verify` 十项通过（typecheck 双端、eslint / clippy、prettier / rustfmt、vitest、cargo test、行覆盖 100%、knip），任务与索引标 done。
- 2026-10-06：整体评审后做行为不变优化——删除 `run_generation`→`run` 的无谓中转、删除 `ProviderCapabilities.mode` 只写字段（形态由 `capabilities(model, mode)` 入参给出）、deepseek / schedule 两处重复的 `mode_of` 收编为 `ProviderRequest::mode()`；新增 `stream_is_terminal_after_finish` 契约测试（Finish 之后再 poll 只得 None）。src-tauri / aoidos-store 注释扫描无需改动。
- 2026-10-06：消融验证（单点摘除安全机制 → 跑套件 → 还原，工作区零残留）：护栏命中+扣留摘除 → 24 测试失败；UTF-8 跨包扣留摘除 → 6 失败；SSE 事件上限摘除 → 2 失败；头 / 空闲看门狗摘除 → 挂起（限时中止）；传输重试 / 请求总数上限摘除 → 无限重试挂起；温度阶梯门禁摘除 → 3 失败 + 挂起；首字节后禁止重发摘除 → 失败 + 挂起；适配流终止一次性摘除 → 重复增量无限产出致进程 abort（上述契约测试将此类退化钉为断言级失败）。八项机制均被套件捕捉；`reqwest retry(never)` 摘除在离线夹具下无差异（无法触发协议 NACK 重试），该项由 024 真实端点联调覆盖。
- 2026-10-06：第四轮细粒度消融（子不变量级单点变异，12/12 捕捉）：LineStart 锚定放宽为 Anywhere（3 失败）、命中选择改为最早起点而非最早结束位置（2 失败）、同结束位置改选最短规则（1 失败）、尾部 CR 扣留改立即转 LF（3 失败）、server_stops 资格过滤（1 失败）、传输重试后仍进阶梯（1 失败）、阶梯请求保留传输预算（1 失败）、阶梯配置校验（1 失败）、预算 reserve 门槛（1 失败）、跨尝试 usage 改覆盖不累计（1 失败）、退避指数改恒为基准（1 失败）、SSE 多行 data 不以 LF 连接（4 失败）。其中「跨尝试 usage 累计」原为盲区（无测试断言），本轮补 `usage_accumulates_across_ladder_attempts` 钉住后再消融验证。
- 2026-10-06：第五轮消融发现并修复实现与契约的偏差：`ingest_data` 原取 choices 首元素，未校验 index 字段，与契约「只抽 index=0，不启用 n>1」不符——改为 `find(index == 0)`（index 缺失按 0），补 `only_index_zero_choices_are_consumed`（index=1 候选排前不进正文）。同轮子不变量消融 4/4 捕捉：index 过滤、显式关闭 thinking（2 失败）、prefix=true 标志、空 stop 省略参数。
- 2026-10-06：第六轮残存微不变量消融 3/3 捕捉：decode 非法序列报错（吞掉则 3 失败）、SSE 流首 BOM 剥除、data 值单空格剥除（38 失败——所有携带正文帧的测试共同钉住）。注释评审至此全仓逐文件闭环，零改动。
- 2026-10-06：第七轮测试健壮性审计（生产矩阵已满后转向测试自身）：静态分类全部时序假设——`fast_policy` 的 80ms 看门狗预算是主要抖动源（慢速 CI 上首包晚到会让 idle/head 在正文到达前触发走错分支），RST 前 50ms 读取窗口次之（Windows 会清未读缓冲）；其余 sleep 均为缺失断言或事件驱动，不依赖时序。加固：head/idle 预算 80→250ms、RST 窗口 50→150ms（超时类测试并行执行下总时长几乎不变）。压力验证：aoidos-llm ×10、store / tauri ×3、vitest ×5 共 21 轮全部通过，单轮耗时分布 2.2–2.3s / 0.9–1.6s / 0.3–0.4s / 0.8s，零抖动。
- 2026-10-06：第八轮补角消融（叶子级规则与常量，9/9 捕捉，零代码改动）：guard 空 pattern 拒绝、含 CR 拒绝、512 字节上限、32 条上限、收尾截断标志（2 失败）、deepseek prefix 端点走 /beta、completion 端点走 /beta、SSE EOF 冲刷派发（4 失败）、script_id 64 截断+哈希。至此八轮消融台账闭合：全部机制 / 子不变量 / 叶子规则共 66 项变异，64 项被钉死，1 项离线不可触发（reqwest retry 归 024），1 项非承重防御（锁显式 unlock）。
- 2026-10-06：第九轮补两项从未做过的检查：首次 `cargo doc --no-deps` 构建发现 error.rs 一处断链（code_at_boundary 未限定路径），已修为 Self:: 限定，现零警告；clippy pedantic 信息性扫描 135 条均属风格意见或有意模式（short_hash 低 32 位有专项测试钉住、migrations.len() as u32 为 user_version 目标空间），无可行动项，不据此改门禁。
- 2026-10-07：issue #64 同步更新日期，复核 #55 的全项门禁与 fast_policy 看门狗说明已生效；本次未改变调度策略或重做历史消融。

## 完成摘要

已交付 `src-rust/aoidos-llm`：不依赖 tauri 的纯 Rust crate，含增量 UTF-8 解码、有界 SSE 解析、GuardSpec 编译与跨分片护栏、结构化错误分类（含 TLS 链下钻标定）、Provider 抽象与 DeepSeek 适配层（completion / chat / prefix，能力表按官方文档登记）、单层请求调度器（互斥重试、看门狗、可取消背压、预算端口）。全部行为由本地夹具验证：行覆盖 100%、clippy / fmt 干净、`bun run verify` 全项通过（测试数与门禁项数随后续消融 / 门禁扩张增长，以 CI 为准）；消融验证确认 8/9 安全机制可被套件捕捉（reqwest 隐式重试禁用归 024 联调）。凭据持久化、代理、命令层接线与产品联调按任务索引归 019–024。
