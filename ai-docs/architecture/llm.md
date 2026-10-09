# LLM 接入与护栏

更新日期：2026-10-09；能力资料核验日期：2026-10-05；模型目录核验日期：2026-10-09。task 005 的已评审设计，依据 [issue #8](https://github.com/Yuki-Nagori/aoidos/issues/8) 补齐边界后定稿。018 / 019 已实现供应商适配、护栏、调度、配置、凭据与代理；020 / 021 已接入回合协调、本地夹具 IPC 与前端恢复；022 已实现记录持久化及投影，已通过最终验收；023 已交付阶段机和产品提交入口，024 已接最小正式剧本 / 配置入口，本地验证已完成，三平台 CI 仍待本分支复验。实际能力与验证证据见「实现承接」及[任务索引](../task-index.md)。跨端载荷、错误码和公共预算以[通信契约](ipc-contract.md)为唯一来源。

## 架构与职责

采用薄 Provider trait、reqwest 和独立 SSE 解析器。aoidos-llm 负责供应商适配、传输、取消、护栏与单层请求策略，不依赖 Tauri；引擎负责回合接纳、记录投影和持久化；src-tauri 只适配命令及窗口事件。正常产品入口是 engine_submit_input，llm_submit 仅用于内部调试，并与引擎共用在飞回合门禁。

```text
engine_open_session：选择已保存 profile + 内嵌剧本 → 原文 / 修订预检 → 持久活动选择
engine_submit_input：冻结配置 / 凭据 / 代理 → 记录投影 + 正式 GuardSpec
  → aoidos-engine 接纳操作 / 回合，分配子调用 turnId，复用同一 lease
  → aoidos-llm：Provider → SSE → 增量护栏 → 安全文本
  → 预留事件序号 → 提交记录 → 更新快照基线 → 窗口事件
  → useEnginePhase / useLlmTurn / useRecordView 消费
```

Provider 用可作为 trait object 的异步接口，返回 Send 的 boxed future / stream；若锁定的 Rust 版本不能对 async trait 直接做动态分派，使用显式装箱，不把某个 SDK 类型暴露给业务层。下表约定语义边界；实现时可调整装箱和所有权表达，但不能改变请求次数及输出含义：

| 操作 / 类型                         | 职责                                                                           |
| ----------------------------------- | ------------------------------------------------------------------------------ |
| capabilities(model, mode)           | 返回明确的模型 / 端点能力，不发网络请求                                        |
| start(request, cancellation)        | 一次 HTTP 尝试，无隐式重试或自动重连；返回规范化流                             |
| Completion { prompt }               | 裸文本续写，默认叙事形态；v1 不传 suffix / echo                                |
| Chat { messages, assistantPrefix? } | 普通 chat；有 prefix 时要求 adapter 明确支持，不伪造支持                       |
| ProviderDelta                       | Text、Reasoning、Usage、Finish；只有 Text 进入目标护栏，正文与内部提议共用机制 |
| ProviderError                       | 结构化类别、HTTP 状态和可诊断来源；敏感正文不外传                              |

业务结构至少包括以下字段；所有 byte 上限计算 UTF-8 长度，token 上限由 adapter 能力约束，不能互相替代：

| 结构                 | 必需字段 / 不变量                                                                                                                |
| -------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| ProviderCapabilities | model、支持的调用形态、thinking 支持、按形态的 stopLimit / maxOutputTokens、contextLimit?、temperatureEffective；未知不当作 true |
| GenerationRequest    | 冻结 profile、input、GuardSpec、outputLimits；凭据是 Rust 内部引用，不可序列化到 IPC                                             |
| CompletionInput      | prompt 非空；其 open tag 由 006 投影生成；生成结果不包含 prompt 或 echo                                                          |
| ChatInput            | 非空 messages；role 仅 system / user / assistant；assistantPrefix 单独表达并由 adapter 注入，普通 chat 不能忽略该字段            |
| Sampling             | temperature、maxTokens、emptyTemperatureSteps；thinking 与 temperature 能力冲突时禁用阶梯并省略无效参数                          |
| GuardRule            | id、非空 pattern、Anywhere / LineStart、priority、serverEligible；没有任意用户正则                                               |
| AttemptState         | 物理请求数、传输重试是否已用、温度阶梯位置、首交付是否发生；状态跨 adapter 保留                                                  |
| TurnState            | turnId、冻结 profileId、text、三事件基线、可选 outcome / finishReason / error；终态不可逆                                        |

Provider 的一次 start 可先失败，也可返回尚无 Text 的流；调用方不得把两者都包装进额外循环。adapter 将 completion 的 choices[0].text 与 chat 的 choices[0].delta.content 转成同一种 Text；reasoning 和 usage 单独传递，不用字符串启发式猜测。单个 Text 可以为空，但不能改变首交付状态。

profile 在提交时冻结 provider、model、调用形态、thinking、采样、代理和凭据引用。运行中换设置只影响下一回合；接纳时取得不可变的凭据版本 / Secret 句柄，重试不能重新解析到热更新的密钥。clear / 换 key 不暗中取消已有回合，要停止生成使用 cancel。Provider 不负责生成 turnId、投递窗口事件、写数据库或组织 prompt。GenerationRequest 属于协调器；传给 Provider 的 ProviderRequest 已选定调用形态，并只带适用的服务端 stop 子集，不把 GuardSpec 匹配状态交给 adapter。

下列 Rust 签名示意表达所有权与动态分派要求；实际 crate 类型名可在实现中定型，语义不能改成 adapter 自带重试：

```rust
type ProviderStream =
    Pin<Box<dyn Stream<Item = Result<ProviderDelta, ProviderError>> + Send>>;
type StartFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderStream, ProviderError>> + Send + 'a>>;

trait Provider: Send + Sync {
    fn capabilities(&self, model: &str, mode: RequestMode) -> ProviderCapabilities;
    fn start<'a>(
        &'a self,
        request: ProviderRequest,
        cancellation: CancellationToken,
    ) -> StartFuture<'a>;
}
```

request 拥有该次尝试所需输入；返回流拥有响应句柄，不借用栈上的 prompt。Reasoning 只用来识别协议，不累积进 TurnState，不写记录或日志；Usage 可统计各次物理请求成本，即使重试正文被丢弃也不能抹去已发生的供应商用量。

v1 不把 async-openai / rig / genai 作为主路径，原因是本项目需要直接控制 completion、护栏和请求次数；不据此断言这些库都缺少能力。所有候选依赖以实现时的锁定版本和实测为准，不在设计提交引入依赖。

## 供应商能力与初始 profile

首版提供 DeepSeek V4.1 Flash（`deepseek-flash`，默认）与 V4 Pro（`deepseek-v4-pro`）两项选择，不手动填写模型名；2026-10-09 核验 [官方模型列表](https://api-docs.deepseek.com/quick_start/pricing/)。叙事质量仍按黄金剧本的人设稳定性、玩家前缀泄漏、延迟及用量标定。其他 OpenAI 兼容端点只能按已声明能力使用，不自动假定支持 completion / prefix。

官方资料确认了 Beta FIM 与 Chat Prefix 两条路径，FIM 为非 thinking；Chat API 默认启用 thinking，因此叙事 adapter 要显式关闭，而不能依赖服务端默认值。[模型能力](https://api-docs.deepseek.com/quick_start/pricing/)、[FIM](https://api-docs.deepseek.com/guides/fim_completion/)、[Chat Prefix](https://api-docs.deepseek.com/guides/chat_prefix_completion/)、[thinking](https://api-docs.deepseek.com/guides/thinking_mode/)

| Adapter 形态           | 路径 / 能力                                                                        | 服务端 stop 上限     | temperature        |
| ---------------------- | ---------------------------------------------------------------------------------- | -------------------- | ------------------ |
| DeepSeek Completion    | /beta/completions；两个候选模型，非 thinking                                       | 16                   | 非 thinking 生效   |
| DeepSeek Chat / Prefix | /chat/completions；prefix 用 /beta/chat/completions，末条 assistant 带 prefix=true | 16                   | thinking 下无效    |
| 其他兼容 adapter       | 显式声明 completion、chat、prefix、thinking 与输出上限；未知能力视为不支持         | adapter 声明，可为 0 | adapter 按模式声明 |

stop 数量依据 [FIM API](https://api-docs.deepseek.com/api/create-completion/) 和 [Chat API](https://api-docs.deepseek.com/api/create-chat-completion/)。初始叙事 profile：Completion、thinking=false、temperature=1.0、maxTokens=2048；不另设 top_p。FIM 指南目前给出输出 4K 上限，不能拿通用模型输出上限替代端点上限；实现前核验并测试 capability 表，超范围本地拒绝。

正式降级顺序是 Completion → Chat Prefix → Chat，但由配置能力选择，提交前完成选择。006 为同一记录提供不同投影，不能把 completion prompt 生硬塞成一条 user 消息并宣称等价。不因 401 / 402 / 429 / TLS / 超时而自动改模型或端点，也不在同一回合上额外试探接口；能力失配返回错误，修正 profile 后由用户重新提交。这样降级不会成为第二套重试或暗中改变费用。

## Stop 目录与增量护栏

005 冻结匹配机制，006 冻结记录语法和具体字符串。006 必须产出同一个 GuardSpec 供投影和护栏使用；玩家台词共用稳定、非空的保留前缀，不能依赖动态玩家姓名枚举。prompt 的 open tag、闭合标签和 stop 由同一语法生成，不手写两份。

| 规则                    | 锚定            | 服务端适用性 / 优先级                                      |
| ----------------------- | --------------- | ---------------------------------------------------------- |
| 当前生成块的闭合标签    | Anywhere        | 优先加入；终止当前块，不把标签写进生成正文                 |
| 玩家台词保留前缀        | LineStart       | 客户端必配；只有不扩大匹配范围的字符串变体才加入服务端     |
| 记录外层 / 系统控制边界 | 按 006 语法声明 | 防止生成跨块控制文本，优先于普通分隔符                     |
| 跑题逃逸前缀            | LineStart       | 006 明确保留的 heading / delimiter；不把合法叙事标题全禁掉 |

GuardSpec 的 pattern 使用与输出相同的 LF 规范形式，编译时拒绝空串、含 CR 的模式及重复 id；同锚定 / 同 pattern 合并，保留最高优先级。规则去重并保持稳定顺序；服务端取适用规则中优先级最高的前 capability.stopLimit 项，客户端执行全部规则。服务端只能按子串 stop，不能表达行首锚定；不能把仅行首有效的玩家标记当作 Anywhere stop 发出，误伤正文引用。端点不支持 stop 时省略参数，客户端规则仍有效。无 GuardSpec 或必需保留前缀缺失时，叙事提交返回 app.bad-request，不以无护栏模式继续。

传输 adapter 先增量 UTF-8 解码、解析 SSE 和 JSON，再交出有效 Text；护栏只处理正文字符串，不匹配 SSE / JSON 原文。护栏顺序是 Text → CRLF / CR 规范为 LF → 匹配 → 交付。规范化后的字节序列同时用于事件、快照和持久化；不额外 trim，不向输出补入 prompt prefix。LineStart 仅指输出起点或 LF 后的第一位置；要拦带缩进的玩家前缀，应由 006 显式声明变体。

每次只交付不可能再成为 stop 的安全前缀：保留末尾与任一有效规则前缀重合的最长后缀，并保留待判定的 CR；UTF-8 字节尾部由传输 decoder 扣留。规则跨网络包、SSE event、模型 delta 的分割不影响结果。按字符推进，在最早出现完整匹配的结束位置截断并停止上游；同一结束位置多个命中取最长规则，再按稳定优先级裁决；不再交付命中点和后文。正常结束后仍是 stop 前缀的尾部保守丢弃，取消 / 错误时也不冲刷待判定尾部；可能少量截短合法尾文，这一取舍用黄金样例固定。

最大规则字节长度限制为 512，规则最多 32 条（含全部变体），护栏只扣住可能匹配的尾部，不缓存整行。数值为 v1 本地资源上限，配置越界本地拒绝；实现时用跨分片测试和内存上界断言验证。该机制只约束生成语法，不保证模型的事实正确、角色一致或抵御所有 prompt injection；世界状态和玩家行为合法性归引擎。

### 跨分片算法与黄金样例

GuardSpec 是按优先级排序的规则集合；编译时建立匹配所需状态，回合间不共享待判定文本。匹配先比较结束位置，再比较规则长度，最后比较优先级；不能在一整个 delta 里选择起点最早的匹配，否则会与逐字符输入的结果不同。算法不需要为每个 delta 扫描已交付全文：

1. adapter 接收字节并增量解码、解析 SSE / JSON；无效 UTF-8 返回 bad-response，截断的最后字符在异常 EOF 时不得替换成 U+FFFD 后交付。完整正文 Text 才交给护栏。
2. 规范化换行，保留尚不能确定是否属于 CRLF 的尾部 CR；正常 EOF 的单独 CR 转成 LF。
3. 把新 Text 与尚未交付尾部结合，依据已交付字符的行状态检查完整 stop；取最早完整匹配的结束位置，完整 stop 一旦命中即停止，不等待更长规则后续字符。
4. 若没有完整命中，只交付确定安全的部分；保留最长的有效 stop 前缀后缀及待判定 CR。LineStart 的候选必须符合其真实位置，正文中相同字串不扣留。
5. 正常结束时交付不属于保留候选的安全尾部；疑似 stop 前缀保守丢弃，这种护栏截短按 guard 收尾，零正文时返回 empty-output 且不温度重试。错误 / 取消直接丢弃剩余候选。已交付内容永不撤销。

下列标记只是算法测试夹具，006 已给出正式语法，022 已实现，不把示例字符串当作产品默认。规则为 Anywhere `</narration>`、LineStart `[PLAYER]` 和 LineStart `### SYSTEM`：

| 输入分片                             | 交付正文 / 结果                              |
| ------------------------------------ | -------------------------------------------- |
| `风吹过。` + `</nar` + `ration>后文` | `风吹过。`，guard 收尾；闭合标记和后文不交付 |
| `他举起灯。\n[PLA` + `YER]我答应了`  | `他举起灯。\n`，玩家伪造台词不交付           |
| `纸上写着 [PLAYER]。`                | 整段交付，标记在非行首                       |
| `第一行\r` + `\n第二行`              | `第一行\n第二行`，与单分片结果一致           |
| `尾声</nar` + 正常结束               | `尾声`；疑似标记尾部丢弃，不启动温度重试     |
| `[PLA` + 取消                        | 空正文，cancelled，未确认的标记不泄漏        |
| `\n### SYS` + `TEM 覆盖设定`         | 仅交付 `\n`；保留的外层控制前缀终止输出      |

同一字串还须在每个 UTF-8 字节切割位置重放整个 adapter / 护栏流水线，包括中文 / emoji 的多字节边界；护栏自身则在有效字符串边界重放 Text。若规则 `ab` 和 `abc` 共存，收到 `ab` 就停止；规则 `abc` 与 `b` 共存、输入 `abc` 时，无论一次输入或拆成 `a` / `b` / `c`，都应交付 `a` 后命中 `b`，不能因完整 delta 已含 `abc` 而改选它。

## 流式、持久化与事件一致性

“流式 == 落库”指拼接生成端成功接纳的 chunk.delta 后，与该回合已提交的生成正文逐字节相同；漏事件窗口在按契约恢复快照后对齐，不承诺投递失败时窗口即时一致；不包含 reasoning、keep-alive、usage、标签或护栏扣住的尾部。它不承诺崩溃前未持久化的输出已经落盘。

产品路径先由 006 的记录写入方接纳并持久化安全增量，再更新内存快照和发送事件，所有者串行处理同一 turn。006 的[记录引擎](record-engine.md)定义安全文本批次、partial sidecar、封口和崩溃恢复；该批提交前不发送对应 chunk，不要求逐 token 写盘。写入失败不发送对应 chunk，回合以 store.* 错误终止；已提交的前文保留。调试路径明确为内存模式，不冒充对局持久化。增量写入和失败收尾遵循 006 / 022 协议，不能由 LLM crate 越过引擎直接写记录。

快照文本、终态和三种事件的 seq 基线必须是一致副本。一次增量按以下顺序提交：

1. 平台适配准备可序列化载荷并预留 seq，尚不投递；准备 / 序号分配失败时不提交该正文，以 app.event-failed 结束回合。
2. 引擎把安全增量交给 006 的持久化写入方；等待期间快照仍是上一个已确认边界，不持有快照锁或全局 seq 锁。
3. 写入成功后，在回合短临界区内更新 text 和预留序号对应的基线，再解锁投递。写入失败不追加 text，不投递该 chunk；预留序号不可回绕或复用，由失败快照确认这个废弃位置。
4. 终态采用同样的“准备 / 预留 → 持久化 → 快照更新 → 投递”顺序。序号机制不可用而无法投递终态时，仍保存本地 failed 快照并释放门禁；事件恢复限制见下文。

投递失败时快照仍含已提交增量和已确认的 seq；不重发相同 delta、不重新生成、不回滚记录，下一条事件的缺口使监听者重新取快照。020 已将原有发送路径替换为 EventPort 的 prepare / deliver / retire 适配；引擎仅依赖普通端口，Tauri 负责主窗口投递。

监听顺序修正为：先注册三个监听器并缓存事件 → 获取快照 → 以快照替换文本与终态 → 按每事件基线消费缓存 → 转入实时监听。seq <= 对应基线丢弃，非连续序号重新取快照，仍遵循契约的不重放规则。缓存最多 32 个事件，溢出丢弃缓存并重取快照；重新取快照期间继续使用同一有界缓存。每个回合最多一个快照请求在飞，溢出合并成待恢复标记，不能每个事件再发一次请求；回合切换或恢复代次变更时忽略旧响应，同一回合不能应用基线低于已确认位置的旧快照。先取快照再订阅会丢掉两者之间的最后一条事件，不能作为实现方案。

021 已实现这一消费链路。同一 UUID 的物理读取跨恢复代次仍串行，不能把旧 Promise 标过期后立即重复发读；不同身份可以分别读取，未完成身份最多 32 个，达限只产生恢复错误并保留确认状态。一次恢复最多一个初始读取和一个合并补取；若终态在补取发出后才到达，再允许一次终态后的读取，最多三次。响应已覆盖观察需求时立即停止，不按事件条数追加请求；仍未追平或读取失败则保留最后确认副本并等待新事件或主动恢复，不持续轮询。过期响应、倒退基线、改写前文及改变已确认终态均不能覆盖当前状态。恢复错误独立于回合错误；确认 cancelled 或切换身份时不保留上一终态的 finishReason；重复取消已经结束的同一回合仍以 Rust 返回的既有 outcome 为准。

终态仅写一次。cancel、正常完成、传输失败竞争同一终态转换：先被串行所有者接纳者胜出；done / failed 二选一且恰一次，不再发 chunk。完整 guard 命中仅取消该次请求的 child token，作为 completed/guard 收尾；它不能复用“用户取消”标志而变成 cancelled。进程 / 用户取消通过回合级 token 中止 HTTP、退避、护栏和队列等待；丢弃未交付尾部，保留已提交前文。取消响应等待所有者完成提交 / 封口及终态确认；取消胜出时发送 done/cancelled，若原终态已胜出则返回已有 outcome，不追加取消事件。

## 看门狗与请求次数

预算数字只维护在通信契约。首字节是第一个安全字符被共享输出写入方接纳的时刻，与是否存在窗口监听器无关；持久化在先时，以两者最早发生为不可重试边界。网络包、SSE 注释、usage、reasoning 都不是首字节。

首字节前的头阶段截止时间覆盖连接、HTTP 头、SSE 读取和护栏等待，不因收到 keep-alive 或 reasoning 重置。首字节后空闲计时只随新的护栏安全增量被输出写入方或内部提议收集器接纳而重置；心跳不能让请求无限存活。不可把 reqwest 的 connect_timeout 当成完整头预算，也不可只靠按网络 read 重置的 read_timeout 实现业务空闲看门狗。[reqwest 超时语义](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html)

这意味着 thinking 请求若迟迟没有可交付正文，也会命中头阶段截止时间；v1 叙事默认关闭 thinking。若将来确需长推理，必须先在通信契约设计独立预算，不能在 adapter 悄悄放宽。

空输出温度重试启用，默认候选阶梯 [1.1, 1.2]，组装时仅保留高于初始温度且在供应商合法范围内的项；显式配置仍完整校验，不静默修正。阶梯可配，但最多两步、严格递增、在 provider 合法范围内且高于初始温度。只有第一尝试按 stop 正常结束、护栏后零字符、未命中客户端 stop、未被拒绝 / 截断、尚未进行传输重试，而且该模式 temperature 生效时，才进入例外路径。

两种重试互斥，由一个调度器拥有请求计数：

| 路径                            | 允许的后续行为                                   |
| ------------------------------- | ------------------------------------------------ |
| 首次遇到可重试传输 / HTTP 失败  | 走契约的传输重试；之后即使为空也不再启动温度阶梯 |
| 首次正常空输出                  | 进入温度阶梯；所有阶梯请求的传输重试预算为 0     |
| 阶梯按 stop 正常空输出          | 下一步；耗尽报 llm.empty-output                  |
| 阶梯发生网络、限流、TLS 等错误  | 立即以对应错误结束，不继续阶梯                   |
| 任一尝试已交付字符 / 持久化增量 | 不能重新提交请求；失败保留前文并结束             |

任一路径都不超过契约的单层总请求上限；模式降级、SDK 重试和 SSE 重连也不能在其外再发请求。每步使用相同冻结 prompt / stop / 模型，仅改变 temperature；重试沿用契约退避与抖动，等待可取消。只有空白仍算有字符，不因 trim 变成可重试空输出；客户端护栏或 length 截断的空结果都不启动温度阶梯；failed 快照 / 事件带实际 finishReason，不能把所有空输出伪装成 stop。

reqwest 当前默认会重试协议 NACK，因此必须显式配置 retry(never())；不是换成 reqwest 就自然没有自动重试。[默认行为](https://docs.rs/reqwest/latest/reqwest/struct.ClientBuilder.html)、[never](https://docs.rs/reqwest/latest/reqwest/retry/fn.never.html)

## 统一调用预算与任务优先级

剧情生成、推进所必需的上下文整理及已授权记忆处理共用一套以费用为控制单位的 LLM 调用预算，不单设记忆专属额度。主流程优先；可选后台工作默认关闭，授权后也不得抢占产品请求。额度不足以执行必要请求时暂停推进并提示补充额度或调整配置，可选后台请求则跳过；本地来源校验、状态读取和恢复不因此停止。

共用预算不代表每回合必须整理记忆，也不代表后台记忆全部成为推进前置条件。哪些上下文处理属于当前推进必需，由记录引擎明确计划，仍遵守 006 的投影硬限、压缩授权与 020 单在飞规则，不在纯函数 project 中发请求。007 v1 不接检索热路径的边界保持不变。

统一额度与单次输入 / 输出上限、看门狗、物理请求次数限制分别解决总消耗和单次失控问题，后者继续使用通信契约，不因额度统一撤销。用量可按任务记录以便诊断，但共同计入总消耗；缺失 usage 不当零。预留 / 结算边界已确认：已知价格的请求发出前按输入估算及输出上限换算费用并预留，不足则不发；未登记模型或缺少必要价格信息时按下方必填规则处理；获得完整 usage 后结算实际消耗并释放未使用预留。失败、超时或缺 usage 时标记消耗未确认，不按零消耗退款。请求身份与预算状态持久保存，崩溃恢复 / 事件丢失不能自动重发收费请求；重试也占预算且受既有次数上限约束。

预留不是对供应商账单的保证；精确预留 / 结算状态机（含未确认消耗的核对处置）、持久字段与本地额度不足的 `budget.*` 错误形状已在[计价与预算](billing.md)定稿（027），035 实施前不是可用 API。没有可验证 usage 时不能声称实际消耗已结算，也不能让旧回答 / 旧请求回调重复结算。不能将新规则写成现有可用 API，不将本地策略拒绝冒充供应商 llm.quota。

### 费用额度作用域（已确认）

用户以金额查看额度、已用与剩余；Token 保留为底层请求计量、上下文 / 输出限制及高级用量统计，不设置独立的累计 Token 消费额度。

| 作用域 | 上限与刷新                                              |
| ------ | ------------------------------------------------------- |
| 每周目 | 固定但可修改的费用上限，新周目开启新的额度周期          |
| 全应用 | 固定费用月上限，所有剧本 / 周目共用，按自然月开启新周期 |

同一物理请求须同时满足周目与月度费用限制，预留 / 结算对应同一请求身份，不能只扣一边或因记入两个作用域将统计总消耗加倍。新周目不重置全应用月用量；段切、剧情回退、正文恢复和上限修改不返还已发生消耗。刷新只切换周期，不删除历史账本或未确认请求记录。

### 自然月、预算时区与跨月请求（已确认）

请求按持久发送意图时刻绑定周目与月份，跨月迟到结算只更新原周期；月切换不释放旧预留或清零未知用量。预算时区初取系统并保存，设备变化不自动刷新，手动时区调整下一预算月生效。[计价架构](billing.md)定义唯一周期、跨区 transition、DST / 回拨与原子切换，不借设置产生额外月额度。

### 预算币种与语言默认（已确认）

首版 CNY / USD，首次创建预算按 resolvedLocale 冻结：zh-Hans 为 CNY，其他为 USD；手动选择优先。界面语言 / 预算时区 / 币种分别保存，语言变化不改已存币种、额度或剧情语言。[国际化](i18n.md)负责精确显示；[计价架构](billing.md)负责逐周期币种与显式变更、冻结汇率、旧账不重算。

### 默认金额与计价（027 已评审，尚未实现）

默认上限由参考输入 / 输出 Token × 对应参考模型价格生成固定金额，后续模型切换不重算额度。每次物理尝试冻结价格 / usage 映射 / 汇率版本，先原子预留双作用域，完整 usage 后结算；未知用量占用预留，不按零退款。未登记 / 缺必要价或汇率先补齐并校验，不发 HTTP；显式零价与缺失不同。

价格配置登记后缓存 / 持久恢复，凭据仍由 019 管理。历史未计价可按明确范围补价，不改原始 usage / 原周期或已结算请求。用户同时看费用和 Token，按供应商 + 模型 / 周目 / 月份查明细；80% 提示去重，未知 / 预留单列，不能把双作用域当两次消费。参考默认值、不可变版本、精确计算、时段 / 汇率、周期切换、恢复和可选余额的唯一细则见[计价与预算](billing.md)。

[027](../task/027-llm-cost-control-design.md) 已完成设计，[035](../task/035-llm-cost-control-impl.md) 实施统一端口与费用设置 / 明细，030 / 033 等待其完成。018 / 020 已提供可注入的物理尝试生命周期与预算端口：每次成功预留对应唯一 requestId / turnId，正常终结显式结算，Future 异常丢弃按取消且用量未知结算一次。usage 缺失或只报告部分字段保留未知，不能伪造零消耗；明确报告的零值与未知区分。价格、缓存 / 推理 token 归一化及周期账本由 035 接入。这些接入机制通过本地夹具验证；未接真实账本前不以产品联调完成代表付费预算能力上线。

记忆的有界批次策略与待标定项见[记忆设计](memory.md)。

## 内部提议与游戏回合

012 的[阶段机](turn-state-machine.md)区分 roundId 与单次 LLM turnId。020 的同一协调器支持 public 正文输出和 private 提议输出；private 复用 Provider / 护栏 / 唯一重试调度器及首交付定义，收集器有界，不创建公开 turn ring、不发 llm:turn:*、不写正文 partial。解析 / 规则校验归引擎，Provider 不采骰、不选世界补丁。现有 TurnSnapshot 与事件字段无需增加 purpose；前端只消费 public 输出。

游戏回合持有共享 lease，内部提议 / 叙事子调用借用它并分别分配新 turnId；不能对子调用再次获取门禁，不能让后台任务插入回合间隙。内部 JSON 被收集器接纳后同样禁止自动重发；schema 失败不触发额外纠错请求。012 冻结提议目标、上限与每游戏回合调用预算；单次物理请求、超时、空输出 finishReason 仍唯一遵循通信契约。以下正文快照 / 事件规则描述 public 输出，private 失败由引擎操作终态和脱敏诊断收尾。

007 的[记忆工程协议 v1](memory.md)复用 private 通道处理独立授权批次；不是游戏回合的第四次子调用，不创建另一套重试、费用账本或公开 turn ring。该协议已评审，记忆请求尚未实现。

## IPC 与回合生命周期

精确命令 / 事件 / 快照类型见通信契约，不在本文维护第二份载荷表。游戏提交接纳后返回 operationId / roundId，子调用 turnId 由阶段快照发现；调试提交返回 turnId。两者不等待完整生成，命令 Err 仅表示未接纳。调试 submit 使用已保存 profile、受类型约束的 input 与 Rust 记录语法登记的 guardSpecId，不允许任意 endpoint、key、stop 由 TS 传入。

全应用同一时间最多一个前台回合，包括调试调用；原子检查和占用门禁先于启动任务，重复调用返回 app.busy。turnId 由 Rust 生成不可复用的 UUID。未知 profileId / guardSpecId 返回 app.not-found，形态与能力不符返回 app.bad-request，校验失败不占用回合门禁。调试入口只在开发构建注册；生产发布包不暴露原始 prompt 的提交命令。

进行中的 outcome 字段省略，不使用 null，也不增加 streaming 值；终态为 completed / cancelled / failed。非空正文的正常 stop、客户端 stop 或 length 收尾可 completed；护栏丢弃疑似标记尾部也属于 guard。零正文属于 empty-output，失败快照与 failed 事件必须以 finishReason 区分 stop / guard / length；这是对外诊断字段，不只是内部重试状态；content_filter、未知协议和工具调用（v1 不启用工具）不能伪装成功。服务端 aborted / insufficient_system_resource 按交付边界映射失败，不能冒充用户取消。失败快照保留已提交文本与脱敏错误；llm.empty-output 的 finishReason 必有且与 failed 事件一致，其他失败未知原因时省略，取消不带 finishReason。

仅缓存进行中回合与最近结束的 ring buffer，默认 16，配置 1–128；未知 / 已驱逐 turnId 返回 app.not-found。只驱逐已结束且生产者退出的回合，旧 id 永不重新使用。驱逐时同步清理 016 的 seq 条目；020 已实现清退接口并在驱逐时调用，不能只清文本而留下无限增长的序号表。

内存不能仅按回合数约束：每回合已交付 UTF-8 文本最多 256 KiB，单个未完成 SSE event 最多 1 MiB，单个 chunk.delta 最多 8 KiB（按 UTF-8 边界切分），消费队列最多 32 个增量并施加背压。超过正文 / SSE 上限报 llm.bad-response，停止上游并保留已接纳前文；满队列等待可被取消和看门狗打断。数值是 v1 初值，黄金样例和慢消费者压测核验后才能在实现任务调整。

### 回合状态与更新顺序

内部阶段可以是 Accepted、Preparing、AwaitingFirstText、Delivering、Terminal，但它们不扩展 IPC outcome 枚举。终态转换记录由串行回合所有者管理，HTTP reader 只能交出规范化输入或失败，不能自行发送 done。

| 当前状态 / 触发                    | 状态变化                                   | 可观察结果                              |
| ---------------------------------- | ------------------------------------------ | --------------------------------------- |
| 未接纳 / 参数失败、缺密钥、busy    | 不创建回合                                 | 命令 Err，不发回合事件                  |
| 校验通过并占用门禁                 | 创建 text 为空、三个 seq 基线均为 0 的回合 | 命令返回 turnId；快照已可读             |
| Text 产生但护栏仍扣留              | 状态不暴露文本                             | 不发 chunk，不重置首交付截止时间        |
| 安全 delta 准备 / 预留、持久化成功 | 原子更新 text + chunk 基线                 | 随后发 chunk；失败投递仍可由快照恢复    |
| 合法 finish，正文非空              | 预留 done 序号后提交终态 completed         | 更新快照后投递 done                     |
| 首次正常空输出且允许例外           | 保留同一 turnId，清空本次解析 / 护栏状态   | 内部重试，不发空 chunk 或虚假终态       |
| 重试耗尽 / 不可重试错误            | 写入终态 failed，保留前文                  | 只发 failed，快照含脱敏错误             |
| cancel 被接纳                      | 写入终态 cancelled，保留前文               | 命令返回 cancelled；只发 done/cancelled |
| 已终态 / 再次 cancel               | 终态不变                                   | 返回已有 outcome，不重发终态事件        |

序号预留先于记录提交，但还不对外确认；记录提交后，内存 text / seq 基线在短临界区内一起更新。快照基线只涵盖已经确认成功或废弃的预留位置，不暴露尚在写入的增量。不得持有全局 seq 锁或快照锁跨网络 / 写盘 await。窗口投递在解锁后执行，但同一回合由同一发送者串行发送，以防 seq 分配顺序与实际投递顺序不同。

006 的记录事务一旦提交不能因 cancel 回滚；若 cancel 在提交中到达，由所有者先完成提交 / 状态更新，再决定终态。取消后的结果可能包含刚提交的前文，快照、磁盘和已发 chunk 必须一致，不能为了取消把持久化事实从 UI 快照中删掉。产品路径的终态记录由 006 正式 JSONL 封口并同步成功后再发送；若终态持久化失败，展示 failed/store.* 并保留已确认文本，不伪造 completed，记录恢复规则归 006。

初始三个 seq 基线必须显式提供 0；这表示该快照确定没有发送过对应事件，而不是监听者自行推断。快照读完后使用同一 turnId 的已缓存事件，不把旧回合的 chunk 接到新回合。终态事件必须带 chunkSeq。三个事件各自计数，所以收到 done 的 seq=1 并不能证明 chunk 完整；监听者比较终态的 chunkSeq 与自己的 chunk 基线，不相等时先恢复快照，再显示终态。快照中包含完整已提交文本，才能正确结束 UI。

如果最后所有事件都投递失败且没有下一条事件，seq 缺口本身无法唤醒监听者。窗口恢复 / 重新连接或用户主动恢复时重新取快照，不宣称“没有下一条事件也能自动检测”；首个实际发送方必须测试这一平台失败场景，不能靠隐藏的持续轮询补齐契约。

关闭应用时触发进程级取消，等待写入方完成已开始的原子提交后释放门禁；不承诺所有窗口仍能收到最终事件。重启不恢复旧 turnId，006 按[记录引擎](record-engine.md)从 partial 保留完整安全增量、封中断块并写 orphan 说明，并要求用户主动开始新回合；没有自动收费的续跑。

## 代理、密钥与网络配置

代理模式 system / none / manual：system 使用启动时形成的环境代理快照（不隐式读环境）；none 显式禁用；manual 使用显式 HTTP(S) 或 SOCKS5 代理并禁用自动系统代理叠加（由构造方式保证：客户端统一经 `proxy::build_client` 显式组装，全程不让 reqwest 隐式读环境），客户端已启用 reqwest 的 `socks` feature。代理三模式、CONNECT 隧道、禁用重定向与经代理的 TLS 分类已由本地代理夹具验证（019）。生产 endpoint 必须 HTTPS，禁止 URL 内嵌凭据，禁止自动重定向携带请求（redirect none）；仅测试 / 用户明确配置的回环本地服务可 HTTP。代理用户名 / 密码由 Rust 凭据引用取出，不存进 URL、普通配置、日志或 CmdError.detail；关闭原始 HTTP trace。端点和代理错误仅返回脱敏类别，不输出 response body、请求头或完整 prompt。

`llm_set_key` 在 Rust 原生窗口中输入密钥，命令 / 快照只返回状态；取消保留旧值。`platform/` 以同型接口提供 Windows CredUI、macOS AppKit 与 Linux GTK 密码控件，命令经 UI 主线程调度，无桌面会话显式报错。三平台真实确认 / 取消与 OS 凭据读写清已有验证，环境、提交及最终复验结果以 [019](../task/019-llm-profile-credentials-impl.md) 为准。

OS 凭据库由 keyring 的三平台后端承担；降级文件采用 Windows 受保护 DACL / Unix 0600。权威后端指针、删除墓碑、私有发布与 hint 的唯一规则见[通信契约](ipc-contract.md#工程纪律可检查版)；读取不凭 OS 旧值推断文件值过期。`SecretString` 管理密钥生命周期，脱敏 Debug 仍不能代替 OS 权限。若将来允许设置表单处理密钥，须先修订职责边界与通信契约。

## 错误分类与标定

全部 llm.* 触发条件集中在通信契约；新增 llm.quota 对应余额 / 配额不足，401 为 auth，未设置凭据为 missing-key，不混为一类。[DeepSeek 错误码](https://api-docs.deepseek.com/quick_start/error_codes/)

TLS 不按错误字符串 contains 匹配。锁定 reqwest / TLS backend 后，对 error source 链做可测结构分类；实现时标定证书过期、主机名不符、不受信任、自签名代理 CA、握手与 DNS / TCP 失败的 fixture。明确 TLS 不重试；无法可靠区分的建立连接失败默认不可自动重试，避免把未知证书错误映射成可重试 network。用户自装可信代理 CA 使用正常证书验证，不关闭验证绕过。

SSE adapter 需要接受 content / reasoning 分离、usage-only chunk 和合法空 choices；只抽 index=0，不启用 n>1。成功要求 adapter 识别合法 finish 和结束标记，异常 EOF 不直接视为正常空输出。SSE / JSON / UTF-8 损坏是 bad-response，不因未交付文字就重试协议错误；断网按交付边界区分 network / aborted。

## 实现验收与待标定项

018 实测选型（探测结论，不把候选当已选定）：reqwest 0.13（`retry(never)` 关闭默认协议 NACK 重试）+ tokio-util CancellationToken + secrecy 已采用；**eventsource-stream 0.2 无事件大小上限且事件边界无法从外部观测，无法在输入侧可靠限流，按预案换成 crate 内自建的有界增量 SSE 解析器**（单事件 1 MiB 上限、CRLF / CR / LF 统一、字节级可重放）；**reqwest 0.13 的 `rustls` feature 绑定 aws-lc-rs（Windows 需 CMake + NASM），改用 `rustls-no-provider` 并在构造客户端前显式安装 ring provider**；**wiremock 无法表达字节级分片与中途断连，改为自建 tokio TcpListener 夹具**（脚本化响应段、按连接轮换、RST 断连、自签名 TLS）。SSE 解析器只接收既有字节流，自动重连禁用；错误链 TLS 分类以 reqwest → hyper_util → io → rustls 的实测链形为标定依据，不做字符串匹配。

| 验收              | 方法 / 必须覆盖的断言                                                                                               |
| ----------------- | ------------------------------------------------------------------------------------------------------------------- |
| 护栏              | Text 字符边界及 adapter 字节边界、Unicode、CRLF、行首 / 非行首、不同起点的重叠 stop、EOF / cancel；切片不改变结果   |
| 一致性            | 事件拼接 == 快照文本 == 006 已提交正文；写入失败不发该增量；失败不清空前文                                          |
| 快照              | subscribe / snapshot 竞态、旧响应、序号准备失败不写盘、写入失败废弃预留、缓存溢出合并请求、投递失败恢复；终态恰一次 |
| 重试              | 本地服务器计数，初次传输失败后空结果不进入阶梯；guard / length 空结果零阶梯；阶梯网络失败立即结束；首字节后零重发   |
| 看门狗 / 取消     | 虚拟时钟，只有心跳 / reasoning / stop 片段不延寿；连接、读流、退避、队列等待均可取消                                |
| 协议              | usage-only、空 choices、正常 finish、异常 EOF、content_filter、服务端 aborted、超大 event                           |
| 资源              | 文本 / event / 队列上界、慢消费者、ring 驱逐连同 seq 清退；未知 id 返回 not-found                                   |
| 凭据 / 代理 / TLS | 三平台权限与 hint、取消设置保留旧值、代理 fixture、TLS source 分类，无明文日志                                      |
| 模型质量          | 两候选模型、两调用形态、固定黄金剧本；记录格式违规率、人设一致性、首字符延迟和账单用量                              |

TLS source 分类、SSE 解析器兼容性已由 018 的本地夹具标定落地（自签名证书、连接拒绝、中途断连、异常 EOF、字节级分片重放）；019 的凭据与代理已落地并夹具验证（含三平台原生输入与 OS 凭据实测，证据归 019），模型质量（024）仍为待标定项，上表给出方法；失败必须修正 adapter / profile 或显式拒绝能力，不以未经证实的降级掩盖。006 的[记录引擎](record-engine.md)承接 GuardSpec / 输出写入方约束并定义正式记录语法，012 定义引擎阶段与产品输入规则。

## 实现承接

实现顺序与状态以[任务索引](../task-index.md)为准。018 已交付 `aoidos-llm` crate（`providers/` 适配层 + `guard` / `schedule` / `sse` / `decode` / `error`，不依赖 tauri，本地夹具全覆盖、行覆盖 100%）；019 已落地配置 / 凭据 / 代理（`config` / `credentials` / `proxy` / `platform/`）及对应命令；三平台原生能力验证与最终复验状态见任务记录。020 已实现纯 Rust `aoidos-engine` 协调器、提交确认端口、public / private 共享 lease 和真实主 Webview 夹具；最终验收状态见 020。021 已交付纯消费规则、注入式恢复协调和 Vue 监听生命周期，实际产品消费者由真实 Webview 夹具验证；验收状态见 021，022 已实现持久化与估算诊断；023 已交付阶段机与产品提交入口，024 已接真实 Provider 与内置原文，本地验收已完成，三平台 CI 推送后复验。

| 任务                                                          | 承接边界                                                      |
| ------------------------------------------------------------- | ------------------------------------------------------------- |
| [018 Provider 与护栏](../task/018-llm-provider-guard-impl.md) | 单次传输、协议解析、安全输出、唯一请求策略                    |
| [019 配置与凭据](../task/019-llm-profile-credentials-impl.md) | 冻结 profile、Rust 原生输入、代理与凭据版本                   |
| [020 回合协调与 IPC](../task/020-llm-turn-ipc-impl.md)        | 共享门禁、终态、事件准备 / 投递、有限快照；先用内存调试写入方 |
| [021 前端恢复](../task/021-llm-web-recovery-impl.md)          | 同型 API、订阅与快照消费、缺口 / 旧响应处理                   |
| [022 记录引擎](../task/022-record-engine-impl.md)             | 006 定稿后的持久化写入方、投影和迁移运行期协议                |
| [023 阶段机](../task/023-turn-state-machine-impl.md)          | 012 定稿后的阶段流转、骰判和产品提交入口                      |
| [024 产品联调](../task/024-llm-engine-integration.md)         | 真实窗口、持久记录与失败恢复的三平台验证                      |
| [035 计价与费用控制](../task/035-llm-cost-control-impl.md)    | 计价、统一预算端口与费用设置 / 明细（018 已预留可注入端口）   |

018 的 GuardSpec 使用测试夹具验证匹配机制；产品语法使用 006 的[正式 GrammarSpec](record-engine.md#正式-prompt-语法与-guardspec)，由 022 的 grammar 同源实现，023 已消费，024 的最小入口接实际剧本 / 配置，不能把样例标记当默认协议。020 的内存调试仅验证协调机制，产品路径必须接入 022 的持久化写入方。007 记忆与 008 完整游戏 UI 各自另有设计任务，不混入本轮 LLM 接线验收。

### 模型窗口与估算诊断

`ProviderCapabilities.context_limit` 是已核验的模型窗口，未知模型返回 None，投影在本地拒绝。当前 DeepSeek 已登记模型按官方 1M context 文档设置保守的 1,000,000 Token；输出上限仍按各形态能力校验。[DeepSeek 官方能力与价格](https://api-docs.deepseek.com/quick_start/pricing/)。切换模型须重新冻结预算，不用前一个模型窗口。

PreparedGeneration 的估算诊断包装真实 BudgetPort，每次物理请求记录冻结估算与真实 usage，重装包装不会重复统计。缺失 usage 不补零，自动暂停与恢复规则见[记录引擎](record-engine.md)。当前调试 IPC 使用 local-fixture provider 标识；024 正式入口按实际 providerId 统计供应商诊断，费用账本由 035 注入，不把夹具用量当真实费用。

### 最小正式产品入口（024）

`engine_list_scripts` 只返回内嵌正文摘要与署名；`engine_open_session` 接收 profileId、scriptId、startNew，禁止 Webview 提供端点、路径或密钥。可信资源固定编译嵌入；`aoidos-script` 保留原文，业务执行配置由 engine 单独登记。默认 Mistbell 只接最小探索章节，不自动执行作者稿的多幕计数 / 结局或 D&D d20。

配置源复用现有 profile / credential 锁；`ProfileFactory` 每轮冻结一次，`ProfileGeneration` 按能力选择 Completion / ChatPrefix / Chat，并在一个 round 内复用不可变 Provider、采样和预算。预检不发送请求；活动会话重新打开不会自动续行，手动确认计划不重掷。无可推进候选时本地确认 stay，省略场景提议请求。

主窗口使用已有阶段、正文与记录消费者；重连 / 主动恢复不加入持续轮询。最后事件全部丢失仍需要玩家主动恢复；取消及网络失败保留已提交前文。凭据只通过三平台系统输入设置，明文不进入前端。

当前预算端口使用无账本实现，035 的价格登记 / 金额额度尚未上线。联调仅访问本地合成 HTTP/SSE 服务，不替代真实模型叙事质量、温度与用量标定；未经 API / 费用授权不自动执行付费测试。
