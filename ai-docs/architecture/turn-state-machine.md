# 回合与阶段状态机

更新日期：2026-10-09。[task 012](../task/012-turn-state-machine-design.md) 的设计定稿，承接 [issue #10](https://github.com/Yuki-Nagori/mythos/issues/10)，实现由 [023](../task/023-turn-state-machine-impl.md) 承接，已实现并验收。本文维护转移、判定与恢复规则；精确 IPC 类型以[通信契约](ipc-contract.md)为准。LLM 请求策略归 [005](llm.md)，记录持久化及世界状态双写归 [006](record-engine.md)。

实现位于 `mythos-engine::game`，串行 actor 通过共享协调器执行记录、生成、判定与恢复，Tauri 适配八个产品命令。前端阶段消费与恢复链路见[架构总览](README.md#阶段机基础链路)，验证证据见 [023](../task/023-turn-state-machine-impl.md)。世界 / 请求端口由可信 Rust 域初始化，实际剧本与界面接线由 024 / 025 承接。

## 边界与身份

使用手写 Rust enum + match。状态转换函数（reducer）为 `State::step(&self, Event) -> Result<Decision, Fault>`，返回新状态、至多一个待执行副作用及过期回执是否被忽略；副作用携带 ownerEpoch / roundId / effectId 身份，只有对应提交回执才能推进。异步驱动层（driver）执行网络、随机采样、记录和投递操作，再向唯一串行所有者交回类型化回执。网络读取方不得直接改变阶段。

| 身份        | 含义 / 生命周期                                                          |
| ----------- | ------------------------------------------------------------------------ |
| sessionId   | 对局 UUID；阶段、场景和操作终态事件按它分流，跨回合连续                  |
| roundId     | 一次玩家行动的游戏回合 UUID；允许先提议判定，再生成叙事，最后选场景      |
| turnId      | 一次 LLM 逻辑调用 UUID；各调用分别受 005 重试上限约束，不能复用已终态 id |
| operationId | 一次接纳的产品命令 UUID；用于快照说明异步结果，不是命令重试幂等键        |
| effectId    | 内部副作用身份；连同 ownerEpoch、roundId 校验返回结果是否仍属于当前任务  |

全应用共享 020 的门禁。游戏回合从接纳至结束持有占用凭证（lease），内部 LLM 子调用借用它，依次分配新 turnId；调试、其他对局和后台 recap 均受同一门禁约束。取消先完成已开始的记录提交，再释放或移交 lease；锁不跨网络 / IO await。

每个已打开 session 只缓存当前 operation 和最近 16 个已结束 operation / round 的脱敏元信息，驱逐后的 id 不重新加入缓存。持久检查点另从记录加载；关闭 session 且生产者退出后清退阶段事件 seq。

## 两条轴与状态数据

公开阶段仅 `idle / generating / awaitingCheck / settling / advancing`。章 / 幕 / 场是数据轴：`scene.path` 为有序 `{ kind, id, title }[]`，默认 chapter → act → scene；可信剧本注册层级 kind，最大深度 16，id 唯一且 path 须符合父子关系。显示名用于展示，场景选择按登记 id 和条件校验。

内部 `EngineState` 至少持有当前阶段、ScenePosition、活动 round、有效历史边界、已确认 world revision、pending 写入 / 恢复状态及当前 effect 身份。round 保存冻结的玩家输入 / profile、已验证 CheckPlan、diceSeq / checkSeq、生成目标及已完成步骤。`generating` 内部区分 checkProposal 和 narration，`advancing` 内部区分 sceneProposal 与提交，均用内部 enum 表达。

副作用类型为 CommitFacts、StartProposal、StartNarration、RollDice、ApplySettlement、ApplyHistoryFork、CancelChild、Publish、NotifyRoundEnd；完成回执携带 effectId 和输入 revision，写入失败交回 TypedError。driver 按 effectId 去重。持久化重试复用已登记 operation / mutation 身份；采样和付费请求不随回执重试。

phase 由已提交因果事实和当前活动任务派生，不另存阶段表。Reducer 可先产生候选 state；涉及持久事实的转移必须等待记录 / applied 回执，确认后才发布状态。

## 转移表

事件中携带的模型输出必须先由 driver 做长度 / schema 校验，再送 reducer；所有未列出的外部命令拒绝，内部不匹配的完成回执按身份丢弃并清理资源，不当成新输入。

| 当前阶段                                                | 事件 / 前提                                           | 下一阶段               | 有序 effect / 确认边界                                                                                                     |
| ------------------------------------------------------- | ----------------------------------------------------- | ---------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| idle                                                    | Submit；角色内、活动场景、记录 / 世界可写、取得 lease | generating             | 先提交 playerSpeech + roundAccepted，再接纳成功，启动 checkProposal；骰判关闭时提交 checkSkipped/disabled 后直接 narration |
| idle                                                    | Submit；角色内、可信规则强制判定                      | awaitingCheck          | 先提交 playerSpeech / roundAccepted / checkPlanned；不用付费提议；按冻结 diceMode 等待确认或执行 RNG                       |
| idle                                                    | Submit；场外、活动场景、记录可写、取得 lease          | generating / narration | 先提交 playerSpeech / roundAccepted / checkSkipped/outOfCharacter；直接旁白回答，不启动判定提议                            |
| generating / checkProposal                              | NoCheck；规则校验通过                                 | generating / narration | 提交 checkSkipped，调用一个旁白或指定角色生成；phase 不变；公开 turnId 确认后发布状态通知                                  |
| generating / checkProposal                              | CheckProposed；规则白名单通过                         | awaitingCheck          | 提交 checkPlanned；manual 等待提交判定，auto 执行 RNG / 写 dice                                                            |
| awaitingCheck                                           | SubmitCheck；匹配 roundId / planId，未开始执行        | awaitingCheck          | 标 rolling，按计划采样并提交 dice；重复请求不再次执行                                                                      |
| awaitingCheck                                           | DiceCommitted                                         | settling               | 根据已提交 dice 计算并写 check；有 dice 无 check 时只补计算，绝不重新采样                                                  |
| settling                                                | CheckCommitted；无 pending                            | settling / narration   | 投影带骰判结果的上下文，启动新的叙事 turn；公开 turnId 确认后发布状态通知，phase 仍为 settling                             |
| generating / narration 或 settling / narration          | NarrativeCommitted；非空 completed                    | settling               | 封口已由 006 确认；按可信规则生成并提交有限 WorldMutation，零变更也必须确认步骤完成                                        |
| settling                                                | SettlementApplied                                     | advancing              | 提交 roundSettled；筛选场景候选，再发起 sceneProposal 或确定性留场 / 结束                                                  |
| advancing                                               | SceneValidated                                        | advancing              | 校验候选和世界 revision，提交 sceneAdvanced / sceneStayed / sessionEnded，确认 SQLite applied 状态                         |
| advancing                                               | AdvanceApplied                                        | idle                   | 提交 roundEnded/completed，更新快照、发布终态，释放 lease；触发只读回合结束钩子                                            |
| generating / narration 或 settling / narration          | 非空 completed/length                                 | settling               | 接受为截断叙事，保留 finishReason=length，不自动再发“补完”请求                                                             |
| 任一在飞阶段                                            | Cancel / 非持久化异步失败                             | 检查点投影阶段或 idle  | 先停止当前 child，完成已开始提交；保留正文 / 骰判，写 roundEnded/cancelled 或 failed，再发布终态                           |
| 任一在飞阶段                                            | IO 不确定、pending mutation / 控制意图                | 当前确认阶段           | 冻结推进；快照 needsRecovery=true、operation failed；停止网络、释放 lease，修复前拒绝新行动                                |
| generating 或 settling / narration                      | Interrupt(text)；本 round 可取消                      | generating（新 round） | 持有同一 lease，旧 round 按取消封口 / 结束；新 playerSpeech + roundAccepted 提交成功后再启动新调用                         |
| idle 或无 lease 的 awaitingCheck / settling / advancing | Resume / Regenerate；合法持久检查点                   | 检查点对应阶段         | 显式新 operation / round / turn；复用有效 dice，不恢复旧 HTTP 请求                                                         |
| idle 或无 lease 的 awaitingCheck / settling / advancing | Rewind；合法目标、重放可用                            | 目标投影阶段           | 执行下述 fork 控制协议；不启动 LLM，不重新掷骰                                                                             |

无 lease 的暂停阶段接纳 Submit 时，先提交 abandonCheckpoint，再复用 idle 转移；旧骰不进入新行动，已应用世界变更继续保留。先解析并分流场外输入；角色内行动的强制判定优先于模型提议，NoCheck 不能跳过它。

接纳前校验所需 profile；输入与 roundAccepted 提交后返回，不等待 LLM。接纳前失败返回 Err，接纳后失败由 engine:operation:failed 和快照报告。

008 / issue #11 将骰判交互定为默认 manual、可选 auto。`awaitingCheck` 包含 waiting 和 rolling 子状态：manual 持有 round lease 等待 `engine_submit_check`，auto 在计划提交后直接进入 rolling。偏好只在回合接纳时冻结；等待用户期间没有 HTTP 请求，不运行 LLM 首字节 / 空闲计时。重启后只恢复暂停检查点，不自动掷骰或收费。叙事 completed/guard 只说明生成块护栏正常封口，不能当作世界补丁。取消 / 失败不会触发结算和场景推进；已完成的独立世界意图保留，需回退才能撤销。

## 玩家输入模式与判定确认

`parse_input(text)` 是 Rust 纯函数；UI 仅预览，业务使用 Rust 结果。按原文首部识别 `/ooc` + 至少一个空白 + 非空内容，或整条 `((…))` 且内部非空为 outOfCharacter；其他普通文本为 inCharacter。首部 `\/` 为字面 slash，未知 / 未完成 slash 或空 wrapper 拒绝 app.bad-request。局部括号和 `/oocx` 不匹配场外语法；默认不 trim 事实原文。

playerSpeech 保存 raw text、可选 mode 和 contentRange（UTF-8 字节闭开区间），省略时为 inCharacter / 全文；解析视图可去掉模式前缀 / 外壳，复制和导出仍为原文。范围必须落在字符边界且只覆盖语法确认的内容。roundAccepted 记录 mode，prompt 将其作为只读玩家上下文，不变成可信系统指令。

outOfCharacter 是场外问答 / 表达偏好，不执行判定、WorldMutation 或场景切换；提交 checkSkipped/outOfCharacter，生成旁白回答，确认空结算后直接写 sceneStayed，不启动 sceneProposal 或依据场外正文结束对局。需要改变行动 / 世界时使用正式角色内输入或已冻结控制命令，不能从场外正文推导 SQL。角色身份仍由 Rust 登记，不允许前缀指定任意 actor。

manual 待判定提供可信 plan 摘要（ruleId、actorId、expression、modifierTotal），UI 不传 seed、骰值、修正或规则覆盖。`engine_submit_check` 在同一串行所有者下将 waiting 改为 rolling；同 round / plan 的重复请求返回已有接纳结果，不能再次采样。auto 与手动请求竞争也由此标记裁决。请求响应只确认接纳，骰值由记录和阶段流交出；存储失败保留 / 核验实际事实，按既有 pending 协议处理。

resume 到骰前时，新 round 重新校验合法计划并使用当前偏好冻结 diceMode；manual 仍须明确点击。已有 dice 的恢复 / 重生成不受偏好影响，始终复用。等待时停止 round 可释放 lease；新裸输入仍 busy，不把漫长等待当作自动取消授权。

## 判定提议通道与请求预算

普通旁白 / 角色正文始终只生成选定正文块，不扫描它提取判定或状态指令。另设 Rust 内部 `ProposalRequest<CheckProposal | SceneProposal>`，复用 005 Provider、取消、护栏、冻结 profile 和唯一重试调度器；不暴露新的调试 invoke，不启用 tool_calls。

提议使用独立目标 `[MYTHOS:CHECK-PROPOSAL]` 或 `[MYTHOS:SCENE-PROPOSAL]`，仅用于请求投影，不是正式 JSONL kind。目标闭合标记 Anywhere stop，并沿用 006 的行首 outer / forged-close 拦截；GrammarSpec 登记该目标及生成 GuardSpec，不能在 adapter 硬编码第二套规则。普通 Chat 明确要求仅返回 JSON，Completion / Prefix 输出 JSON 后结束在目标闭合处。玩家 / 历史的保留括号仍只在 prompt 副本转义。

内部收集器最多 8 KiB UTF-8；逐增量接纳也构成 005 首交付边界，之后不可自动重发。合法 finish 为 stop 或完整目标 close 所致 guard；length、其他标记导致 guard、半个 JSON、未知字段、重复键和 schema 不符均报 llm.bad-response，不发修复 prompt。空输出仍沿用 llm.empty-output 与真实 finishReason。解析成功只是候选，必须经过可信规则校验。

```json
{"kind":"check","ruleId":"search","actorId":"player-a","reason":"在浓雾中寻找出口"}
{"kind":"noCheck"}
{"kind":"scene","sceneId":"harbor"}
{"kind":"stay"}
```

CheckProposal 禁止 expression / rolls / dc / modifier / worldPatch；reason 最多 512 字节且仅作为审计数据。actorId 必须是本回合可行动角色，ruleId 必须在当前场景规则白名单；候选过期或条件不满足按无效提议失败，不临时创造规则。SceneProposal 同样不能创造场景、路径和结束条件。

内部提议不进入玩家正文 / partial，不发 llm:turn:*，不加入公开 llm_get_turn ring；使用同一协调器的 private 输出策略，只有类型化结果和脱敏失败传给引擎。首交付 / 超时 / 用量统计仍适用，不因“内部调用”重置计数。公开 turn 快照只描述旁白 / 角色或开发调试输出。

每个游戏回合最多三个 LLM 逻辑调用：一次 checkProposal、一次 narration、一次 sceneProposal；各次物理请求上限仍按通信契约，不能在三次外增加纠错 / 自动续写。强制规则或骰判关闭省掉提议；无场景候选省掉场景请求。前次已交付文本后也不重发该次请求。sceneProposal 失败将回合标 failed、保留已提交结算；主动恢复只重试未完成步骤，不再生成叙事或重结算。模型质量 / 成本需 024 标定，调用次数只是硬上界，不保证账单金额。

## 骰子与规则表

引擎构造 `CheckPlan`：planId、ruleId / ruleVersion、actorId、规范 expression、modifiers、resultPolicy、三档后果分支。修正项 `{ value, source: { kind, id } }` 来自可信角色 / 处境 / 剧本状态，模型只能建议 ruleId。计划保存输入 world revision 及 hash；掷骰前再次确认条件，变化则失败，不拿旧能力继续掷。

v1 默认注册规则为 `pbta-2d6-v1`：2d6 + 修正之和，修正之和限定整数 −3 到 3；total >= 10 为 success，7–9 为 costlySuccess，<= 6 为 failure。三分支均须注册合法后果：代价与失败后果由规则表选择并交给叙事，失败推进新的处境，不自动终结游戏。结束只有可信 sessionEnded 条件能决定。规则可关闭骰判；暴击 / DC 判定是注册扩展，不改变五阶段。

骰式的 K 是 modifiers 的来源分解之和：total = sum(rolls.value) + K，不能再把 modifiers 加第二次。骰式 v1 只支持 `NdS±K`：N 1–20，S 2–1000，K 整数 −100 到 100；PbtA 默认仍固定 2d6，修正规则更严格。拒绝爆炸骰、无限重掷、任意函数和非有限数。骰式解析先用小子集，caith / tyche 留作候选，不在未验证可注入 RNG 及限制前接入；[caith 官方说明](https://github.com/Geobert/caith)包含更丰富骰式，不能直接视为本项目合法输入。

023 已采用 rand_chacha 的 ChaCha20 确定性生成器，具体版本由根 Cargo.lock 锁定；黄金向量冻结 `chacha20-v1` 的 seed 长度、字节序与无偏映射（[官方文档](https://docs.rs/rand_chacha/latest/rand_chacha/)）。

每个新 plan 由系统熵生成 32 字节 seed，保存小写 64 位十六进制 seed 与 startCounter / endCounter 十进制字符串、algorithm、mappingVersion=1；

初始 stream=0、word position=0，startCounter=0；原始流按连续 little-endian u32 字消费。对 S 面骰取 L=floor(2^32/S)*S，丢弃 x>=L 的字，接受时 value=(x mod S)+1；counter 计实际消耗字数，不以骰子个数代替拒绝采样次数。复现用已冻结算法核验，恢复的业务结果直接读取已记录 rolls / total，不靠升级后的库重新采样。

单次采样最多消耗 4096 个原始字，耗尽返回 invalid-phase，整组骰子不提交；系统熵获取失败不使用备用 seed。零 seed 的前两个原始字为 `0xade0b876 / 0x903df1a0`，2d6 拒绝采样结果为 `[1, 1]`，endCounter 为 `2`。计划摘要覆盖除 planHash 自身之外的完整 CheckPlan，按键排序的 JSON 对象序列化并包含末尾 LF；执行前重核规则版本、表达式、修正来源、身份形状和冻结 worldRevision。

dice 包含 `planId`、`rng`、`modifiers`；check 包含 `planId`，result 包含 costlySuccess，dc 为可选：PbtA 省略，DC 注册规则必有有限 dc。dice.source 指向规则 / plan，check.diceSeq 必须引用当前有效因果路径上的既有 dice。修正之和、各骰值和 total 校验一致；没有 check 但已有 dice 时重算分档，不能再掷。022 已实现类型化读写，023 的默认 PbtA 已加入计划和采样基础；当前执行器只支持 `pbta-2d6-v1`，DC / 暴击仅保留格式扩展位，后续须增加并登记相应执行器，不能从历史或模型字段自动启用。

| 走查       | 输入 / 骰值                                         | 结论                                                |
| ---------- | --------------------------------------------------- | --------------------------------------------------- |
| 成功       | 2d6 = 6 + 4，modifier=0                             | 10 → success；成功分支，叙事不能改成失败            |
| 有代价成功 | 3 + 3，modifier=+1，来源为角色能力                  | 7 → costlySuccess；须包含已注册代价，不能省掉判定行 |
| 失败推进   | 1 + 2，modifier=−1，来源为场景处境                  | 2 → failure；进入失败后果，不能自行标 sessionEnded  |
| 边界       | total=6 / 7 / 9 / 10                                | failure / costlySuccess / costlySuccess / success   |
| 非法       | 21d6、modifier 总和 +4、未知 ruleId、模型提交 rolls | 提交判定前拒绝；不采样、不落非法 dice               |
| 提交中断   | dice 已提交，check 尚未提交                         | 补写同 plan 的 check，所有 rolls / seed 不变        |

## 因果事实与重启投影

006 的 system 使用下表注册的 code 保存可审计、可重放的因果事实。公共 version=1，有回合时 roundId 必有，接纳 / 控制操作有 operationId，roundAccepted 保存 mode（inCharacter / outOfCharacter）及 diceMode（manual / auto），所有引用均须在当前有效路径可解。profileRevision 仅是非敏感配置身份，不保存密钥或原始请求。roundEnded.outcome 与 completedSteps 是结果 / 已完成步骤的审计摘要，不是另存 phase。

| code              | data 必需内容（公共字段之外）                                                                                                                                         |
| ----------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| roundAccepted     | inputSeq、mode、diceMode、profileId、profileRevision、target `{ kind: narration 或 characterSpeech, speakerId? }`、sceneId；恢复时 sourceRoundId / checkpointSeq 必有 |
| checkPlanned      | diceMode、完整 CheckPlan，包括 planId、ruleId、ruleVersion、actorId、expression、modifiers、resultPolicy、三档 branchId、worldRevision、planHash                      |
| checkSkipped      | reason（disabled / noCheck / outOfCharacter）、worldRevision                                                                                                          |
| settlementPlanned | narrativeSeq、worldRevision、最多 32 个有序 WorldMutation，每项固定 mutationId 和版本化已注册 data                                                                    |
| roundSettled      | narrativeSeq、settlementSeq、appliedThroughMutationId?；无变更时省略最后一项                                                                                          |
| sceneAdvanced     | previousSceneId、完整 ScenePosition、worldRevision、mutationId                                                                                                        |
| sceneStayed       | sceneId、worldRevision                                                                                                                                                |
| sessionEnded      | previousSceneId、已注册 reasonId、worldRevision、mutationId                                                                                                           |
| roundEnded        | outcome（completed / cancelled / failed）、throughSeq、completedSteps[]；failed 必有脱敏 error，其他省略                                                              |
| abandonCheckpoint | sourceRoundId、checkpointSeq；之后不能自动沿用旧骰判                                                                                                                  |
| historyFork       | 下节冻结的 mode、父控制引用、targetSeq 与 prefixHash；无 round 时省略 roundId                                                                                         |

sceneAdvanced / sessionEnded 的世界变更只允许对应注册类型，沿用 006 mutationId / applied 协议；上述表不建立世界属性 schema。completedSteps 固定来自 proposal / check / narration / settlement / advance，不接收模型任意字符串；它是派生的审计摘要，读取时与对应记录引用核验，不能单靠它越过缺失事实。恢复 roundAccepted 的 target 沿用检查点，profile 是新操作冻结值。

`derive_phase(EffectiveHistory, AppliedWorld, LiveOwner?)` 为纯函数。正常打开先由 006 核验 / 封 partial、恢复已知 pending mutation，再对有效因果路径投影：

| 优先匹配的持久事实                                 | 无 live owner 时的阶段 / 操作                                 | checkpoint.stage / throughSeq                                          |
| -------------------------------------------------- | ------------------------------------------------------------- | ---------------------------------------------------------------------- |
| 有 pending / 未知控制操作                          | needsRecovery；先恢复，禁止收费和修改世界                     | 省略 checkpoint；禁止从未确认事实推导可恢复位置                        |
| 当前回合 completed 或明确 abandon                  | idle；保留最近终态                                            | 省略 checkpoint                                                        |
| roundSettled，但场景提交 / roundEnded 未完成       | advancing + resumeRequired；只补缺步骤                        | advance / 已 applied 的场景事实 seq；尚无场景确认时取 roundSettled.seq |
| completed 叙事已封口，尚未结算（含无骰回合）       | settling + resumeRequired；只补结算                           | settle / settlementPlanned.seq；尚无计划时取 completed 叙事 seq        |
| dice 已提交，check 缺失                            | settling + resumeRequired；计算分档，不重掷                   | check / dice.seq                                                       |
| check 已提交，completed 叙事缺失                   | settling + resumeRequired；生成新正文                         | narration / check.seq                                                  |
| manual checkPlanned 已提交，尚无 dice              | awaitingCheck + resumeRequired；先 resume，再确认掷骰         | check / checkPlanned.seq                                               |
| historyFork 停在 checkPlanned，尚无 dice           | awaitingCheck + resumeRequired；显式 resume 按偏好等待 / 采样 | check / 目标 checkPlanned.seq                                          |
| historyFork 停在 checkSkipped，尚无叙事            | idle + resumeRequired；显式 resume 生成正文                   | narration / 目标 checkSkipped.seq                                      |
| accepted / planned 无 dice，进程中断（非上述暂停） | 幂等写 roundEnded/failed 后 idle；保留输入与 orphan           | 省略 checkpoint                                                        |
| 无未完成行动，场景有效                             | idle；可提交新输入                                            | 省略 checkpoint                                                        |
| sessionEnded 或未载入场景                          | idle，省略 scene；新输入返回 no-scene                         | 省略 checkpoint；有待收尾 advance 已由前列优先匹配                     |

checkpoint 只在 resumeRequired=true 且已核验可恢复时出现。stage 表示下一项缺失工作：check 包括未掷骰或缺分档，narration 表示缺 completed 正文，settle 表示缺结算确认，advance 表示缺场景 / 回合收尾。throughSeq 是上表指定的已确认事实锚点，不是物理最后 seq、事件基线或合法 rewind 目标的自动推断。roundEnded/failed、orphan 和 historyFork 控制行自身不能替代这个锚点。

sourceRoundId 指拥有待恢复工作的最近 roundAccepted.roundId，复用旧 dice / 正文时按 sourceRoundId / checkpointSeq 引用链解析其事实。throughSeq 必须在当前有效路径中，相关 planId / diceSeq / narrativeSeq / applied 标记一致；任何缺失 / 版本未知先 needsRecovery，不猜一个阶段。部分结算以 settlementPlanned 为锚，逐项 applied 状态另外核验。

按表从上到下取第一个匹配项。roundEnded/failed 或 cancelled 保留未完成检查点；resumeRequired 表示等待用户主动恢复，此时没有请求在飞。outcome 描述操作结果，phase 描述当前执行位置。

用户也可 rewind，或提交新输入放弃检查点：先记录 abandonCheckpoint，未结算骰判留在只读历史，已 applied 的世界变更保留。

partial 恢复严格承接 006：保留完整已提交安全前文，封为 engine.interrupted 的失败块并写 orphan；仅丢弃半行和未提交尾文。恢复不会自动发起付费续写。

resume 使用新 roundId / turnId、sourceRoundId 和 checkpoint 引用；一 LLM turn 一个正文块。需要沿用中断前文时将其标为只读“中断片段”上下文，后续另起正文，不能给旧失败块补 delta 或伪装 completed。

已封 completed 的叙事存在而结算尚未确认时，resume 直接结算，不重复生成；settlementPlanned 的固定 mutationId 与 applied 标记逐项核验，只补未应用项，不重新计算一套后果。恢复 / 换回合事实保存 sourceRoundId 与已提交步骤引用，禁止仅靠阶段 enum 猜测缺哪一步。

状态派生从当前有效检查点和尾部增量进行；首次打开 / rewind 可扫描整个因果文件，不能宣称恒定开销。replayCache 是可验证的重放加速缓存，其 history hash / 控制版本不符就重放，丢失缓存不丢事实；它与 IPC 中的暂停 checkpoint 不同。

恢复新 operation 按当前已保存 profile 冻结设置，不从历史还原已清除密钥；原 round 的 profileId / revision 只用于审计。没有可用配置时接纳前拒绝，不暗用备用模型。场景、世界初始化需要可信 session 基线；无基线 / 无世界 mutation 重放解释器的会话可以查看，禁止 rewind / 世界修改，不在本设计虚构世界属性 schema。

## 中断、重生成与回退

engine_interrupt 是在持有本 round lease 时的控制操作：只允许正在 checkProposal 或 narration 的 generating / settling；校验新输入后请求取消旧 child。旧 turn 的终态竞争沿用 005，已开始提交先完成；取消若输给当前 child 自然终态（即使公开 phase 尚未变化），则插话 operation 以 engine.invalid-phase 失败收尾，不偷偷开第二回合。切换子状态期间第二个 interrupt 返回 app.busy，不能重复取消并接纳两个新输入。新 round 接纳前保留同一 lease；旧块封口失败时新输入不落盘，operation failed，不边修复边生成。

engine_cancel_round 取消本 round，重复取消返回其已有结果；round 已不在有限快照中为 app.not-found。直接 llm_cancel 只结束 LLM child，driver 将其视为游戏回合取消，不能让 Settling 假装叙事完成。普通 engine_submit_input 在任何活跃 lease 下仍 app.busy；暂停检查点的新输入按上述放弃协议处理。

rewind 与 regenerate 都用追加式 `system code=historyFork`，不用尚未定义的 tombstone / supersede。data 固定 `{ version, operationId, mode, parentControlSeq, targetSeq, sourceRoundId?, reusedDiceSeq?, prefixHash }`；mode 为 rewind / regenerate，parentControlSeq 为父分支控制 seq（根为 0），targetSeq 必须在父分支有效因果路径上。prefixHash 是父路径截至目标的有效普通块按 seq 顺序拼接原始行（含 LF）的 SHA-256；父路径由 parentControlSeq 链另行校验，不能把无效物理行或未 applied 控制算入该 hash。新分支身份就是此控制块 seq，后续普通块可带 branchSeq，根省略；物理 seq 始终递增，不从 targetSeq 重新编号。

回退只允许可重放检查点：回合接纳前的已结束边界、已提交 checkPlanned 后但 dice 前的边界、或 check / checkSkipped 已提交后但叙事前的边界。拒绝 partial 内部、dice 与 check 之间、pending mutation 及任意自由行切片；目标非有效路径或未注册控制为 bad-request / invalid-phase。回退到骰前须主动 resume 或提交输入；新 plan 按冻结 diceMode 等待点击或采样；回退到 check 后只能复用该 dice。

重生成 v1 只针对当前有效路径的最近一个已终态回合，fork 到该回合的 check 后 / 无判定时 checkSkipped 后，恢复此时世界状态并复用骰值；新 narrative 使用新 roundId / turnId。旧叙事与其后世界结算、场景推进留在物理历史但从有效上下文排除。先撤销后生成需要同一 lease；因重放 / 记录失败未成功 fork 时不启动请求。任意旧回合 swipe、多候选并行生成不属 v1。

控制提交顺序：

1. 校验目标路径和重放解释器，预备目标 WorldView。
2. 追加并 fsync historyFork 意图。
3. 在 SQLite 单事务中重建世界投影和 applied 控制标记。
4. 更新有效路径 / historyRevision，发布记录与状态事件。

SQL 失败或提交结果不确定按 006 pending 协议核验，needsRecovery 阻止新操作；恢复只重放已提交控制意图，不再追加同 operationId，不发送 LLM、不重复掷骰。不能先删数据库再发现历史不可重放。

有效路径 = 父路径截至 targetSeq 的因果前缀 + 本分支的新块；读取物理 historyFork 不等于其已 applied。recap 只有其覆盖范围全部仍有效且 sourceHash 匹配才可使用，否则从 prompt 排除；不会因回退重写旧 recap。历史 / 导出保留全部原行；记录分页用当前有效路径，lastRecordSeq 仍为物理最大 seq，不能当有效回退位置。fork applied 后更换 record viewEpoch，使旧 cursor / bodyRef 和异步响应失效；重新取有界 view，避免旧页拼进新分支。historyRevision 为最新已 applied 控制 seq，根为 0，phase 快照也带它。

007 的未来记忆必须绑定来源因果路径，回退后排除无效来源；回退重放不自动触发新的记忆收费、回合结束钩子或 recap。原始历史导出由存储层处理，不添加无界全历史 IPC。

## 场景推进与世界视图

可信剧本提供 SceneCatalog、父子结构、入口 / 结束条件、规则白名单及世界只读字段优先级。角色内回合的 driver 在已确认结算后，以 world revision 筛选有限候选；最多 32 个，超量按可信优先级和 id 稳定截取并注明，模型从该候选集合选 sceneId 或 stay。模型依据世界状态选取候选，剧本条件约束合法范围。

场景提议失败不随机替模型选，不重复发纠错请求。无符合候选时按可信规则留场；满足 sessionEnded 条件则持久化结束。接受候选时再次校验当前世界 revision、父子路径与条件；未改变场景发 sceneStayed 因果事实，不发 engine:scene:advanced。切换 / 结束先提交事实与世界投影，再发事件；header.staticPrefix 不热替换，新场景信息放动态 WorldView。世界只读投影预算沿用 006，不允许任意 SQL / 全数据库注入 prompt。

### 场景规则视图与只读求值端口

SceneCatalog 是可信剧本注册域；023 负责交付 SceneRuleView 及登记规则求值器，032 在其上维护派生进度 / 停滞计数和提示选择。以下是已接入并经本机测试的内部 Rust 端口，不新增 Tauri 命令、公开阶段或世界属性。

| SceneRuleView 字段        | 注册内容与约束                                                                             |
| ------------------------- | ------------------------------------------------------------------------------------------ |
| sceneId / catalogRevision | 已登记场景身份与规则目录修订，固定本次读取边界                                             |
| advanceRuleIds            | 当前场景可用的晋级条件规则，目标仍由 SceneCatalog 的父子 / 入口条件限制                    |
| progressRuleIds           | 可信进展条件规则；匹配及新的有效依据供 032 判断进展，持续匹配不等于每回合新进展            |
| hintRules                 | hintId、whenRuleId、固定文本、priority 与目标范围；文本 / 权重由剧本注册，模型不可自由填写 |
| exitRuleIds               | 已登记合法出口 / 结束条件；命中不代替正常晋级校验与提交                                    |

端口为 `get_scene_rule_view(sceneId, catalogRevision)` 与 `evaluate_scene_rule(sceneId, ruleId, ConfirmedWorldView, currentWorldRevision, currentHistoryRevision) -> Result<SceneRuleMatch, Fault>`。输入世界视图必须是当前已确认的不可变快照；结果含 ruleId、matched、catalogRevision、worldRevision、historyRevision 和 evidenceRefs。evidenceRefs 区分已注册剧本事实（剧本 / 规则版本、键、值 hash）与已提交记录事实（sessionId / recordSeq / 内容 hash），由注册解释器输出，不接受模型伪造引用。

规则 ID 必须属于对应场景的已登记规则集合，条件由 023 实现的统一类型化条件求值层处理，场景晋级与 032 共用这一层；未知规则 / 解释器、过期 revision、缺失依据返回类型化拒绝，不能以 matched=false 掩盖无法求值。函数只读、不提交状态、不采随机、不调用 LLM。角色 / 原文相关引用仍需 022 核验有效路径；匹配结果不得直接当作世界补丁。

内部端口同样有界：每类规则 ID 最多 64、hintRules 最多 32、单提示最多 512 字节，SceneRuleView 完整序列化最多 64 KiB；单次 SceneRuleMatch 的 evidenceRefs 最多 32。超过上限拒绝目录 / 求值结果，不截掉依据后宣称成功；这些限制已由登记与求值层验证，032 的消费接线仍待实施。

023 验证规则目录与只读求值结果，032 使用其确认身份幂等消费 completed 钩子；032 不复制条件解释器、不扩展权威状态。无 progress / hint / exit 配置的场景返回空登记集合，不自动制造进展或出口。原有公开五阶段、每回合三个 LLM 调用、场景提议与提交顺序保持不变。

## IPC、错误与消费

engine_get_phase 可独立重绘阶段、场景、活动 round / 可公开 turn、暂停检查点、最近 operation 结果；正文从 llm_get_turn / 记录 view 获取，不把整份历史放阶段快照。精确字段、命令、阶段 / 场景 / 操作终态四个事件见[通信契约](ipc-contract.md)。内部提议省略 inFlight.turnId。启动公开叙事时先创建可读的空 turn 快照，再确认 inFlight.turnId 并发 engine:phase:changed，之后才启动网络流；即使 phase 未变也须通知。前端收到身份后按 021 的“先订阅、再快照”流程接入正文。

phaseRevision 是本次打开 session 的确认状态修订号，跨四种事件单调递增；各事件 seq 仍独立计数，不能用 revision 替代缺口检测。scene 变更与对应阶段变更在一次短临界区确认，同一 revision 可以出现在不同事件名；快照原子读状态与四基线。stateEpoch 为打开 session 的新 UUID，重开改变，旧响应不能覆盖新 epoch。发布顺序仍为准备 / 预留 → 提交事实 / applied → 确认状态与基线 → 投递；阶段的纯内存转移无新事实时直接确认再投递。投递失败不回滚事实、不启动新请求。

无场景时 rewind 仍可恢复到有场景的合法检查点；cancel / 查询也不要求场景存在。submit / interrupt 检查活动场景；resume / regenerate 校验检查点中的场景，允许从已提交 sessionEnded 的未完成 advance 步骤补收尾，或从 ended 历史恢复到有场景的检查点。错误优先级：参数结构 / 限额校验（bad-request）→ id 存在性（not-found）→ 与其他 lease 冲突（busy）→ 无活动场景（no-scene）→ 目标阶段 / 检查点 / pending 不允许（invalid-phase）。interrupt / cancel / submit_check 是本 lease 的受控例外，不归为第二个在飞回合；resume / regenerate / rewind 不能抢占活动 lease。存储损坏仍为 store.corrupt，底层 IO 为对应 store.*，不统统包装成 invalid-phase。engine.interrupted 仅是重启后的历史错误。

监听者先订阅四事件、缓存最多 32 条，再取 phase 快照；每 session 至多一个恢复请求，新事件只合并恢复标记；具体载荷上限引用通信契约，不在消费方重复维护数值。处理独立 seq 缺口后再按 phaseRevision 应用状态，旧 revision 只推进对应事件基线，不倒退 UI；同 revision 合并完整状态数据。切换 session / stateEpoch / historyRevision 后忽略旧请求，卸载释放全部监听。最后所有事件丢失仍需重连 / 主动恢复，没有持续轮询；与 005 / 006 相同限制如实保留。

## 对接与实现验收

| 调用方 → 被调用方  | 时机 / 责任                                                                                                              |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| 023 → 020          | 接纳取得同一 lease；子调用复用协调器输出策略 / 取消 / UUID，不增第二套门禁                                               |
| 023 → 022          | 每个事实、partial 封口、世界意图 / applied 和 fork 提交；确认后才能推进                                                  |
| 023 → 018 / 019    | 通过 020 使用冻结 profile、Provider 与 GrammarSpec 生成的护栏；schema 不由 Provider 决策                                 |
| 023 → 007 未来钩子 | roundEnded/completed 确认后送 `{ sessionId, roundId, historyRevision, throughSeq }` 一次；当前仅接口，不生成记忆、不收费 |
| 023 → 032          | 交付 SceneRuleView / SceneRuleMatch 的只读内部端口；032 维护派生计数及提示，不反向改场景条件                             |
| src-tauri → 前端   | 适配普通载荷发 main 窗口，TS 与 Rust 同型；025 按 008 设计区分骰判行、正文、暂停恢复与脱敏失败                           |

007 的[工程协议 v1](memory.md)推荐 completed 钩子只幂等登记素材与逻辑时钟；收费处理在回合释放 lease 后作为独立授权批次运行，保持本回合最多三个逻辑调用。记忆回退 / 恢复依赖本节有效路径和记录回执，不把后台候选直接送入阶段 reducer；当前仍未实现记忆钩子消费。

023 用表驱动测试全部合法 / 非法转移，专项覆盖以下边界：

- 判定：三档及阈值，dice 已提交而 check 缺失时不重掷。
- 并发：重复 / 过期 effect、子调用借 lease、cancel 与封口竞争。
- 恢复：partial 前文保留，已 applied 结算不重复，公开 turn 切换可发现。
- 事件：阶段 / 场景跨流乱序、旧快照 / cursor 淘汰。
- 回退：fork 各提交故障点、recap 失效、缺解释器时不改世界。

023 已执行状态机、确定性 RNG、世界重放和故障恢复测试，验证证据见对应 task；模型质量与真实剧本后果由 024 标定，世界属性 schema 由可信注册域提供。三平台结果按实际 CI 记录；没有注册解释器的操作必须明确拒绝，不能以 TODO 绕过验收。
