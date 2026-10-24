# 056 — 实现：开局生成目标与标签驱动结局

- 状态：in-progress
- 依赖：018、019、020、023、027、032、035、039、040、041、045、052、054
- 优先级：P1
- 创建 / 更新：2026-10-19 / 2026-10-22

## 目标与背景

实现周目初始化、任务引导、标签求值和周目落幕的一条完整流程。功能发布后，每个新周目都由 LLM 根据当前剧本生成一条最终目标，以及用于终局求值的语义标签目录和规则；标签与规则不由剧本预置。最终目标在游戏开局由首个引导 NPC 交代，并作为左上角任务栏的主线目标。任务栏随后可以根据已提交的剧情进展增加普通任务和连续任务；已完成任务划线并移入默认折叠的已完成区。旧周目沿用原有恢复语义，不追补开局目标或标签。

引导 NPC 优先使用剧本明确登记的固定角色；剧本没有登记时，才由 LLM 根据剧本的风格、背景和世界观生成一名周目专属 NPC 的姓名与种族。UI 和任务记录是引擎状态的投影：模型与前端都不能创建未经授权的世界事实、宣布周目结束或改写有效历史。

本任务承接 [054](054-ending-flow-design.md) 的权威边界、终局提交、计费、恢复、标签求值与非权威结局叙事协议；按 2026-10-22 的产品决策，**新周目的标签目录和规则统一在开局生成，不采用剧本预置标签或预置规则**。剧本仍需声明可安全用于规则生成的状态字段、类型和值域等输入约束。剧本文本提供风格、背景和世界设定，不等于可信规则或事实来源。实现时须同步修订 054、[剧本格式](../standards/scenarios.md)及相关架构契约，消除其“作者预设结局规则”的旧选项。

## 对照设计

`aoidos-art` commit `9df5c5c1f4562c2887ffc9b2043ee5f87a6e35fd` 已实现任务栏状态稿，作为任务栏视觉和交互基准：默认展开时主线置顶、普通任务随后、任务链显示链名和当前进度、已完成区默认折叠；收起后显示主线摘要与额外任务数；另有旧周目空态、开局首次交代、分支回退提示和结局达成 / 未达成状态。实现沿用这些状态关系与舞台左上角布局，动态文案和实际数据以 engine 冻结状态为准，不照搬静态样例文本。`ending-approach.html`、`cycle-init.html`、`cycle-init-failed.html`、`ending-pending.html`、`ending.html`、`ending-narrating.html` 与 `ending-narration-failed.html` 可供结局初始化和落幕参考；页面状态不得先于权威 engine / storage 协议实现；`ending-choice.html` 的静态选项不构成玩家选结局的授权。

## 必读

[054 结局生成与周目落幕设计](054-ending-flow-design.md) · [结局架构](../architecture/gameplay/cycle/ending-flow.md) · [回合与阶段状态机](../architecture/gameplay/turn-state-machine.md) · [记录引擎](../architecture/data/record-engine.md) · [剧本格式](../standards/scenarios.md) · [计费架构](../architecture/ai/billing.md) · [记忆架构](../architecture/data/memory.md) · [通信契约](../architecture/core/ipc-contract.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：开局生成的结局标签目录 / 规则、最终目标、初始任务及必要的 NPC 身份；剧本固定引导 NPC 的登记与选择；生成 NPC 的周目身份和展示资料；任务的动态提议、依赖链、完成归档和分支投影；初始化计费 / unknown / 恢复；结局规则求值、冻结事实和确定性摘要；AI 结局叙事；UI 状态 / IPC；032 刻痕衔接和跨层验收。

非目标：玩家从固定清单选择结局；模型自行创造隐藏属性、谓词、世界状态、人物身份 ID 或已提交事实；把任务完成状态直接当作结局事实；把任务历史或 AI 结局叙事作为记忆证据；在本任务实现剧本编辑器、通用任务管理器或无界的自然语言规则执行器。

## 前置条件与待决策

054 的规则快照、严格校验、终局锚点、标签冲突、确定性摘要、AI 文本边界、失败恢复和事务图继续作为安全基线，但实现前需按本任务决策修订其生成来源：功能发布后每个新周目都通过受限的开局生成得到最终目标、标签目录与规则；不再提供作者预置标签 / 规则或静态结果回退。已存在的历史周目不补造开局快照。所有可新建周目的剧本必须提供可登记、可版本化的隐藏状态字段 schema（稳定 ID、类型、范围和允许比较方式）；LLM 只能引用该 schema，不能以 Markdown 描述推断或增添字段。剧本 schema 未就绪时须先完成剧本接入，不能让该剧本绕过生成流程创建无最终目标的新周目。开局生成失败或结果不确定时，周目保持不可玩并给出可恢复错误，不静默降级。

将剧本固定角色表示为 `cycle-input-vN.json` 中的资料，并以 `characterId` 引用实体目录中的 Character。v2 另以 `storyCharacterIds` 明确登记本周目剧情角色；角色 `truth` 是规范个人真值，`initialKnowledge` 表示该角色初始知道哪些真值字段，`presentation` 是角色在玩家交互中的表面资料；它们不能互相覆盖。每个有剧情作用的角色（主角和配角）都必须有 run-scoped 个人档案；作者没有固定主角档案时，开局生成并冻结，不将操作者的控制身份当作角色全知。固定 guide 不能同时是玩家操控的主角。只有 `isGuide: true` 才选为固定引导者；未标记或 `false` 的资料不自动担任引导者，零个固定引导者时由 LLM 生成，多个标记则拒绝剧本。不得运行时搜索正文或让模型猜测人选。资料不随应用 locale 改变，也不授予角色场景在场、真实关系或额外获知权限；这些由可信初始化路径显式处理。开场职责与当前叙事关系分别使用 `openingRole`、`openingRelationship`，不写入永久世界关系。无固定角色时，开局生成 NPC 资料，内部 ID 由引擎创建，并与剧本版本一起冻结为本周目身份。固定和生成角色都必须由可信初始化路径加入初始场景 / 实体注册，不能仅凭对白创建世界实体；脚本固定实体、生成角色与 [045 场景在场与可信获知事件](045-scene-presence-dialogue-impl.md) 的授权和来源边界需保持一致。

主角创建由剧本自己的、版本化的模板决定，不设全局统一的玩家档案问卷。模板可提供完整固定主角，也可针对姓名、性别、年龄、技能、性格等字段配置有限选项或受约束的自定义输入，并支持固定与自定义混合。玩家选择 / 输入经可信代码校验后以 `player-authored` 来源合入主角真值并冻结；开局 LLM 不得代选或改写。模板需明确可见与隐藏字段、允许值、默认项和来源，保证创建 UI 只显示获准字段。此能力不属于当前 cycle-input v2，须通过后续 schema revision 与 run-local 主角身份注册实现，不得将输入混装进 NPC 资料或用 LLM 生成替代玩家自定义。

开局目标与 UI 任务属于引导状态，不是新的世界事实或终局判定器。最终目标应描述本周目的核心方向，并在结构化快照中声明可由哪些生成标签表示达成、未达成或未定；这些标签的状态仍由引擎根据已提交事实和冻结规则判定。主线目标文本、NPC 交代和任务栏都来自同一份冻结的初始化结果，NPC 对白可以润色但不能改变目标含义。仅当 `endingResolved` 已 applied，任务栏才依据冻结结果把主线标为已达成或未达成；LLM 不能直接完成主线。普通 / 连续任务可以帮助玩家分解过程，但任务自身的完成、未完成或模型措辞不得独立结束周目。

动态任务只在有意义的已提交剧情节点后提出，禁止每轮机械追加。模型只返回有界、版本化的任务提议；优先复用已有回合请求中的专用结构化字段，若现有协议不支持，则只在里程碑节点通过单独、显式计费的请求生成，不为每一轮额外调用模型。任务建议失败或结果不确定时不阻断对局、不自动重试，也不添加未经确认的任务状态。引擎负责校验任务 ID、依赖、数量、长度、重复项、引用的有效记录 / 历史分支，以及前置任务状态，再写入当前有效分支。未提交、失败、重生成、重复请求或被回退的回复不产生任务变化。任务状态不是世界事实：完成状态只表示任务栏中的引导进度，不能授权世界变更、满足结局条件或写入跨周目记忆。任务变更必须与已提交历史修订绑定，重启 / 重放可重建；切换历史分支后不能把另一分支的任务提议、完成状态带入当前投影。

开局请求需要协调结局规则、最终目标、初始任务和按需生成的 NPC 人设。优先使用一个用途明确、输出有严格大小上限的版本化结构化请求，让目标、引导和结局语义在同一冻结快照内一致；若实现上拆成多个请求，必须分别持久化 request 身份、计费与 unknown 状态，并且只有所有必需产物均校验并冻结后才能激活周目。生成失败 / unknown 时不得自动换模型、换提示词或重发；玩家显式重试遵循 027 / 035 的物理请求与预算语义，不能丢弃原请求费用或不确定义务。

实现前核对 023 世界状态写入与恢复、032 刻痕周期、040 解析器、041 内置剧本、045 实体登记、052 Vue 舞台、035 账本接口和 Git 式历史分支回退语义；新增 crate / module 依赖须保持架构无环。自然语言的语义一致性无法只靠 schema 证明，权威任务目标与结局规则分别保留结构化字段并展示其产品边界；逐句事实保证仍由引擎数据或确定性模板承担。

## 实施步骤

1. 修订 054、剧本标准和相关架构文档：移除作者预置标签 / 规则模式；定义所有新周目必需的规则输入 schema 和固定 guide NPC 资料/显式选择，明确旧周目恢复语义，并在开放新周目生成前为所有可用剧本补齐 schema。
2. 定义带版本的周目初始化快照：剧本 / schema 版本、生成身份和 requestId、规则 / 标签、最终目标及其达成状态标签映射、初始任务、guide NPC 固定引用或生成资料、输出 hash、locale / 模板版本、计费关联和初始化状态。按 054 的两阶段流程先保存不可玩的 run 与原币预算，再发生成请求；完整快照持久化后方可 playable。
3. 实现严格生成响应 schema 和白名单校验。标签 / 规则只能引用已登记字段、受限比较运算和值域；目标、标签说明、任务标题 / 描述、NPC 姓名 / 种族均须有长度 / 字符控制；由可信代码产生持久 ID 与顺序。整份输出校验失败则不部分应用。
4. 实现 guide NPC 选择与开局呈现：剧本明确登记时直接复用固定角色；否则冻结开局生成的姓名 / 种族，并以可信初始化路径登记为周目专属角色。首次引导叙事从冻结的最终目标生成 / 校验，避免 UI 主线和 NPC 交代相互矛盾。
5. 定义任务 DTO、来源、类型、依赖关系、状态转换和分支身份。初始任务数量有限；动态任务仅在有效提交的里程碑后生成提议，优先复用已有回合请求的专用结构化字段，否则以独立计费请求处理；失败不阻断对局、不自动重试。通过引擎校验后幂等持久化。完成项保留在历史中，默认折叠展示，不物理删除；历史回退按目标修订重投影任务状态。
6. 在可信终局触发点基于已提交世界状态求值冻结的生成规则；冻结标签 ID、规则 ID / version、历史修订和结果摘要。未知字段、缺失字段和冲突依照 054 明确语义拒绝或走明示规则，绝不默认零值。
7. 用确定性模板从允许公开的冻结事实生成规范摘要并原子提交结束事实；AI 只消费该摘要生成可选非权威叙事，失败不会阻止规范周目结束，显式重试复用同一冻结结果。
8. 定义生成 unknown / 已扣费未持久化、数据库失败、重开、分支回退、重复命令、任务提议重放和新周目入口的状态恢复；结局叙事从普通 memory narration / candidate 投影中排除，任务历史不作为事实记忆写入。
9. 接入 Tauri 命令、TS 类型、界面开局 / 生成失败 / 任务栏 / 结束状态及 AI 叙事标识。任务栏左上主线突出显示，活跃普通 / 连续任务分层展示，完成项划线并进入可展开的折叠区；响应式布局不得遮住主要输入或游戏操作。`ending-choice.html` 不建立选择结局命令。
10. 用故障注入和跨层行为测试覆盖主路径、并发、幂等、账单 unknown、旧周目、回退分支隔离、实体身份冲突、隐私和无环依赖；更新状态机、记录、剧本、记忆、IPC、architecture README、本 task，并完成独立评审。

## 预计改动

`resources/scripts/` 的规则输入 schema / guide NPC 元数据、`src-rust/aoidos-engine/src/game/` 的初始化 / 任务 / ending resolver、必要的 `src-rust/aoidos-script/`、Tauri commands、TypeScript API / Vue game feature、locales、SQL migration 与 Rust / native / Vitest 测试；同步 `ai-docs/architecture/gameplay/cycle/`、`gameplay/characters/`、`gameplay/turn-state-machine.md`、`data/record-engine.md`、`data/memory.md`、`ai/billing.md`、`standards/scenarios.md`、架构领域入口、054 task 与 task-index。

## 验收标准

- [ ] 功能发布后每个新周目都在 playable 前生成并冻结最终目标、标签目录和规则；不存在新周目绕过生成、剧本预置标签 / 规则路径、静默降级或历史规则补造。所有可用剧本均已登记供规则引用的类型化状态字段与合法运算符 / 值域。
- [ ] 开局结构化输出经过版本化 schema、完整白名单、长度 / 数量上限、跨字段一致性及规范化校验；最终目标的达成状态只引用同一快照中的合法标签。未知字段、越界、引用不存在字段、无效互斥组或无效依赖导致整份初始化失败，不部分生效。
- [ ] 生成请求只有在新周目身份和预算持久化后才能发送；physical request / requestId / confirmed / unknown 状态及重试符合 027 / 035。校验和全部必需产物冻结之前，周目不可玩；恢复不丢费用、不自动重发、不换规则。
- [ ] 固定 guide NPC 由资料数组中的稳定 ID 与 `isGuide: true` 唯一选择并保留人物资料；零个选择时才生成引导角色，多个选择时拒绝剧本。生成角色拥有引擎创建的稳定 run-local 身份，经可信初始化与实体目录授权登记；恢复 / 分支回退不重新抽人，不发生身份碰撞。
- [ ] 主线目标在左上任务栏突出展示，首次引导 NPC 在开局交代同一目标；模型对白不能改写 UI 冻结目标。只有 applied 的 ending resolution 能将主线标记为达成 / 未达成。窄窗口下任务栏不遮挡输入与舞台交互。
- [ ] 普通 / 连续任务具有明确依赖和有界生成；只从已提交的有效剧情里程碑追加，校验幂等、去重、依赖和当前分支。独立任务提议请求正确计费，失败和 unknown 不阻断对局、不自动重试、不改变任务状态。未提交回复、失败、重生成、重复请求或已回退分支不会污染当前任务投影。
- [ ] 已完成任务保留、划线并进入默认折叠区；折叠只影响 UI 投影，历史回退可恢复当时任务状态。任务完成不直接写世界状态、不触发结局、不进入跨局记忆或规则上下文。
- [ ] 终局只由可信游戏条件触发；标签由冻结规则从已提交数值确定，携带来源版本、组合 / 冲突语义和 revision；缺失状态不伪装为零。
- [ ] 标签确定性地产生权威事实摘要；隐藏原始数值和私有标签不进入 UI 或叙事请求。AI 叙事明确标记为非权威，不更新世界状态、普通 narration、记忆素材 / evidence 或后续规则上下文；事实摘要不依赖 AI 成功。
- [ ] 所有有剧情作用的角色（含主角）在 run 冻结快照中有个人真值档案；角色认知、对外主张、玩家可见事实各自有独立权限投影。真值不进入通用 DTO / 日志 / 无关模型请求，假身份和谎言只能形成 claim，不能覆写真值。
- [ ] 主角配置按剧本模板决定固定档案、受限选项或受约束自定义字段；允许姓名、性别、年龄、技能、性格等模板化差异，也允许直接使用完整固定主角。玩家输入必须经可信校验并以正确来源冻结；模板 schema 及 run-local 身份注册兼容性完成前，不宣称当前 v2 已支持此能力。
- [ ] 结束 / 初始化 / 显式 retry / restart / rollback / replay / new-cycle 的 Tauri / Rust / TS 类型同型且有行为与故障注入测试；任务和 NPC 状态按有效分支恢复。
- [ ] 未完成的旧周目按既有 legacy 语义恢复，不补造最终目标、NPC、任务、标签、规则、endingResolved 或历史分支数据。
- [ ] 代码、中文注释、类型、文档及 task 同步；独立评审发现处理完成；最终 `bun run verify` 全项通过。

## 验证计划与结果

2026-10-24 开局 provider 请求契约切片：冻结请求按严格类型解析，并在创建 journal 写入前核对模型、调用形态、采样参数与冻结 profile 一致；可选 session routing id 必须属于当前周目，输入大小 / 数量和 stop 列表有界，sampling 子对象也拒绝未知字段。回归用例覆盖模型、模式、采样、session routing、空输入、额外采样字段和 stop 上限。`bun run test:affected`、`bun run format:affected`、`bun run lint:affected` 与 `git diff --check` 通过；独立评审指出的嵌套字段与测试覆盖问题已修复并待复核。该边界为后续 coordinator 使用已保存请求提供可校验输入，但仍未派发 Provider 请求或接入新局入口。

首个创建 journal 切片的受影响检查已通过：`bun run test:affected`（554 passed、3 ignored）、`bun run format:affected`、`bun run lint:affected`。2026-10-24 当前结构化输出切片通过 `bun scripts/cargo.mts test -p aoidos-engine --lib cycle_output::tests -- --nocapture`（6 passed）及 `bun scripts/cargo.mts check -p aoidos-engine --lib`；独立 6.1-sol 评审未发现阻止提交的问题。提交前 `bun run format:affected`、`bun run test:affected`（engine 560 passed、3 ignored）和 `bun run lint:affected` 均通过。它还没有生成全部角色档案 / guide NPC，也没有接入 Provider、账本、存储或 playable 转换，因此不是完整开局快照，不能用于启用新局入口。仍待验证并实施：固定 / 生成 guide NPC、完整结构化输出与初始化冻结、WorldPort / 周目恢复、027 / 035 物理计费与 unknown 对账、032 单向对接、任务分支投影与回退、跨平台 IPC、Vitest、更多故障注入及完整 `bun run verify`。

2026-10-24 guide 档案校验切片通过 focused 测试（8 passed）、`bun run test:affected`（562 passed、3 ignored）、`bun run format:affected`、`bun run lint:affected` 和 `git diff --check`；独立 `gpt-6.1-sol` 静态评审未发现阻止提交的问题。它确认固定 guide 不可由模型替换、生成档案有界且不接入生产 playable 路径。仍未接 Provider、账本、存储或 playable 转换。

2026-10-24 剧情角色名册切片将 `cycle-input` 升至 v2，使用 `storyCharacterIds` 明确列出本周目所有需要档案的角色；模型响应必须精确覆盖缺少固定档案的 ID，固定档案按名册顺序原样冻结，guide 改为引用唯一档案，避免重复保存真值。Mistbell 登记主角与洛苇。旧 v1 输入作为兼容资源保留，并验证既有当前 v1 script revision 仍可重建实体目录。Focused assets/output 测试与受影响门禁通过；完整 `bun run verify` 通过（Vitest 549 项、Rust workspace 覆盖率 94.51%、其余 typecheck / lint / format / build / docs 门禁通过），独立 `gpt-6.1-sol` 评审无阻断项。该切片仍不接 Provider、账本、存储或 playable 转换；主角模板 / 玩家自定义输入、完整角色信息隐私隔离及旧输入初始化语义仍待实现。

2026-10-24 创建 journal 切片：新增 `session_creation_ops` 保存不可变 operation、header / profile 快照和 hash；通过 header-only inactive record 写入、恢复时幂等补齐 `recovery-required` 目录行。legacy scanner 排除所有 journal 所有的 sessionId。该接口刻意不创建普通 owner、memory run、预算 / 费用预留或 Provider 请求，也尚未切换产品新局入口。恢复窗口和重复创建测试已通过；后续仍需与初始化输入、预算、物理 attempt 和完成后 owner 激活串成一个可恢复 coordinator。

2026-10-24 初始化准备切片：`session_creation_preparations` 保存版本化、限长的冻结 script body / entity manifest / cycle-input 快照、profile / script revision 绑定、requestId 和用户确认的双币预算。provider request body 作为不可变草稿保存，但在专用 coordinator 完成协议校验前不得 dispatch；locale、prompt version 和输出 schema version 限定为当前实现值。header-only record 写入后，单个 SQLite `IMMEDIATE` 事务登记 `recovery-required` catalog、canonical billing runId、预算及审计、initializing 快照和 pending attempt，再关闭创建 journal；事务故意不 dispatch、不预留费用、不创建 owner / memory run、不激活周目。旧 journal 经新迁移后仍无 preparation，保留原恢复语义。精确重启重放、无效快照拒绝与 attempt 插入故障注入证明幂等和跨表回滚；该切片尚未接入 Provider / 账本 dispatch coordinator 或产品新局入口。

同日独立只读评审指出原草稿没有随 run 冻结可独立校验的 cycle-input / entity 资源，且 locale / schema 只做了宽松格式检查；按建议补齐脚本原始快照与 script revision 重算校验，将 locale 限定为当前支持的两种、prompt 固定为 `init-v1`、输出 schema 固定为 `2`，并明确草稿不能 dispatch。评审建议的注册后实际 product open / provider gate 仍由现有 `require_playable` 门禁覆盖；准备测试现已明确断言 initializing preparation 被拒绝。独立评审未发现事务原子性、旧 journal 恢复或误开放 playable 的阻断问题。

2026-10-24 初始化提交边界加固：`cycle_initialization::finalize` 不再接收任意 JSON 字节，只接受 `cycle_output::validate_and_normalize` 返回的私有构造 `ValidatedOutput`；校验结果绑定周目、物理 requestId、locale 及精确 entity / cycle-input 快照。提交事务重新校验初始化 input hash、严格冻结 envelope、catalog 的剧本 / profile 身份、profile hash、prompt / schema 版本与冻结剧本资源，再允许转为 playable。覆盖错误身份绑定、未派发拒绝、成功提交与幂等重放；这只封住 finalize 的校验绕过面，尚未接 Provider、账本 dispatch coordinator 或产品新局入口。

2026-10-24 初始化 dispatch 候选读取切片：新增 `Storage::initialization_dispatch_candidate`，只从已关闭创建 journal 的 `registered` 记录构造 typed ProviderRequest，并重新核验创建 / profile / catalog / initialization / preparation 快照、哈希、双币预算和尚未领取的 pending attempt；requestId 与 revision 原样返回，后续仍须通过 initialization billing port 原子领取派发权并预留费用。独立评审发现候选读取与删除状态变化间存在竞态，以及正常状态变化不应映射为 corruption；现由领取派发权的同一 `IMMEDIATE` 事务核验目录仍为 recovery-required、billingRunId 与 session 一致且 memory owner 尚未创建，正常失败 / 重试 / deleting 状态返回 conflict。行为测试覆盖有效读取不改 dispatch 状态、失败终态分类、读取候选后目录删除会阻止 claim、重复 claim 仍拒绝。Focused 测试通过（storage / cycle initialization / billing port 各 1 passed）；最终 `bun run test:affected` 通过（engine 570 passed、3 ignored，相关 Rust consumers 通过）；`bun run format:affected`、`bun run lint:affected` 和 `git diff --check` 通过。独立复核完成，原子删除门禁与状态分类无剩余阻断项；曾发现的 lifecycle 测试夹具已修正并复测通过。本切片仍不构造 Provider、不发请求、不做 playable 激活。

2026-10-24 canonical initialization prompt 切片：新增从冻结 script / entity / cycle-input 快照及 profile 确定性构造 Completion 或 Chat 开局请求的 builder，移除固定角色真值和初始认知字段，并给输入加统一字节上限；创建准备校验会重算请求并拒绝任何与持久化 provider request 不一致的快照。覆盖两种调用形态、冻结输入及隐私投影和身份版本不匹配。独立评审补齐模型输出档案结构、字段白名单以及任务 key / 连续任务依赖契约。已有预算 dispatch coordinator 尚未由 Product 新局入口串联，因此此切片只规范和持久化请求，不触发 Provider 调用或 playable 激活。

## 风险与回退

开局生成的规则、最终目标、任务初始态和生成 NPC 都是本周目的不可变初始化输入；剧本、模型、提示词或 schema 更新不能漂移既有周目。模型可能产出结构合法但语义不合理的目标 / 标签，因此要把可验证规则限制为白名单状态比较，并在发布前用剧本样例验收可达性和叙事一致性。任务提议属于可回退的引导投影，不升级为正史；如果模型把任务标记错误，修复只能提交新的有效分支任务状态，不得改写旧提交记录。生成或存储失败时保留不可玩初始化状态与费用义务，提供明确恢复 / 放弃路径；不能通过旧流程静默绕过最终目标。

## 决策与工作记录

- 2026-10-19：从 052 的页面映射登记。当前无结局状态 / IPC；052 只记录缺口，不显示静态假结局。
- 2026-10-19：054 定稿方案采用“剧本声明 → 周目前冻结规则 → 引擎求值标签 → 权威事实摘要 → AI 叙事”；初始实现任务据此规划，尚未实施。
- 2026-10-22：用户决定新周目的结局标签与规则全部由 LLM 在开局生成，不采用剧本预置标签；剧本仅提供规则生成可引用的可信状态 schema 和叙事背景。此项覆盖 054 原有的“作者预设规则”选项，实施时需同步修订 054 和剧本标准。用户提出每局有最终目标，由首个引导 NPC 交代；任务栏固定于左上角并突出主线，随有效剧情增加普通 / 连续任务，完成项划线后进入折叠区。
- 2026-10-22：固定 guide NPC 由剧本登记时优先复用；未登记时才按剧本风格、背景和世界观让 LLM 生成其姓名与种族。2026-10-23 明确改为角色资料数组 + `isGuide` 显式标记，最多选择一名；人物资料、开场职责与关系分字段登记，普通角色资料不能被自动挑成引导者。NPC 身份须由可信初始化路径登记，恢复 / 回退不重生成。任务状态用于引导，不直接构成世界事实或结局权限。
- 2026-10-23：开始实施。先修订 054 / 剧本 / 架构中的旧“作者预设标签”路径并确定可信输入 schema 与引导 NPC 表示，再从现有开局、计费与实体登记边界实现初始化快照；本任务按阶段提交，未通过的验收项保持未勾选。
- 2026-10-23：首个 engine 持久化切片新增独立 `cycle_initializations` / `cycle_initialization_attempts` 表、受限状态与 revision/requestId CAS，持久化输入 hash、attempt 历史、失败 / unknown、显式 retry、可重入 finalize 和放弃状态；初始化必须在已有 profile 与双币预算可用后才能登记。`open_target` 对显式初始化行做 fail-closed 门禁，legacy 无记录周目不受影响。此切片尚未切换新局创建入口，也未接入 Provider 预留、结构化输出校验、owner / scene 注册或恢复 UI；后续必须将目录、预算、初始化行和物理计费身份原子接线，并封闭所有活跃 owner 入口后方可启用。
- 2026-10-23：新增 LLM 调度原语 `run_generation_once_with_output`，复用调用方持久化的规范 requestId，并强制单次物理调用、不重试、不升温；测试覆盖失败时仅预留 / 调用 / 结算一次，以及无效 requestId 在预留前拒绝。该原语尚未接入初始化账本与真实 Provider 创建入口。
- 2026-10-24：增加可恢复的新局创建 journal。存储在写 header 前持久化 intent，header-only record 按冻结身份幂等检查；启动恢复能把中断阶段收敛为 `recovery-required` 目录登记，未结束的创建 journal 也会被 legacy scanner 跳过。该阶段不切换活动周目、不注册 owner 或开始计费 / 模型调用；后续 coordinator 负责将其推进为完整初始化 run。
- 2026-10-24：新增纯开局输出校验器，严格解析 v1 目标、标签规则和初始任务，拒绝重复 JSON key / 未知字段 / 未登记或矛盾规则 / 非法互斥组和依赖，并由 engine 以 session + request + 类别 + key 派生稳定 ID。空规则列表明确表示永不命中。它只校验目标 / 标签 / 任务投影，不包含缺失角色档案或 guide NPC，不读取 run ledger、不触碰 Provider 或存储，也不激活周目；完整冻结前必须扩展输出并与初始化账本原子接线。
- 2026-10-24：扩展纯输出校验：固定 guide 必须来自已验证的 cycle-input，模型不得替换；没有固定 guide 时，必须提供完整且有界的真值、初始认知、表面资料、开场职责 / 关系，由 engine 派生 run-local 身份。此切片仍不补齐其他剧情角色，也不访问实体目录、Provider、账本或存储，完整快照和 playable 接线未完成。
- 2026-10-24：独立只读评审覆盖当前 diff 并对照 054 / `scenarios.md`；未发现阻止此切片提交的问题。评审特别确认该纯校验器无生产调用点，因此不能形成不完整初始化产物转为 playable 的新路径；未来接线仍须保证输入来自 run 冻结清单、完整角色档案已校验且 attempt 身份匹配。评审为静态审查，未执行测试；验证由主代理完成。
- 2026-10-24：任务栏视觉与状态流对齐 `aoidos-art` commit `9df5c5c1f4562c2887ffc9b2043ee5f87a6e35fd`，覆盖展开 / 紧凑摘要、主线置顶、任务链、已完成折叠、旧周目空态、开局引导、回退和结局状态；状态内容由 engine 冻结数据驱动。
- 2026-10-23：继 #215 后继续实现 dispatch 围栏：初始化 attempt 增加持久 `dispatch_started` CAS；初始化预算端口在一次 SQLite `IMMEDIATE` 事务中领取该权并登记同 requestId 的费用预留，避免账本预留的幂等语义意外授权 Provider 重放。当前只完成围栏与事务内 reserve 原语及 focused test；尚未接入完整开局 coordinator、record 创建 journal、初始化恢复流程或产品新局入口，不能启用新流程。
- 2026-10-23：登记 `cycle-input-v1.json` 并由 engine 严格验证版本、字段数量 / 类型 / 范围 / 比较符、已注册来源及固定 Character 引用；固定角色显式标注 `isGuide: true`，角色档案拆分真值、初始认知、对外呈现和开场职责。每个剧情角色含主角需在 run 冻结资料中有个人档案；无作者固定档案时由开局初始化补齐。Mistbell 固定使用已登记的洛苇，并显式绑定主角实体；不改正文原始字节，避免历史 v1 script revision 变化。唯一已接入状态源从当前有效历史分支派生已结算的角色内回合数，空历史时值缺失。新 script revision 纳入输入资源和修订；旧 v1 revision 仍能重建原实体目录。完整开局请求、不可玩初始化账本和目标 / 标签快照仍待实现。
- 2026-10-24：继续初始化准备事务切片，并按 #223 同步角色档案 / 结局架构 / 本任务文档的 v2 语义。新 preparation 只接受 `cycle-input-v2.json`，历史 v1 仍可恢复旧 script revision；新增匹配历史 hash 的 v1 拒绝测试。当前资源已采用显式 `cycle-input-v1.json` / `cycle-input-v2.json` 文件名，代码无无版本 `cycle-input.json` 引用。#219 所需 crate 清单、更新日期和 JSON 共用能力锚点已在基线解决。
- 2026-10-24：增加 `ProfileFactory::dispatch_initialization_once`，从已登记的冻结候选恢复 profile 凭据和 Provider，创建绑定同一 session / revision / requestId 的初始化预算端口，并调用无重试的一次性调度器。输出只进入 128 KiB 私有缓冲区；正常完成后先严格规范化，再与 `playable` 状态在同一 CAS 事务冻结。每次调用记录自身是否成功预留并取得派发权；只有派发权持有者能结束 attempt，避免并发输家或预取消调用中止正在运行的请求。取消、调度失败和输出校验失败分别保留 failed / unknown 状态，不自动重发。当前 `Product::open(start_new)` 尚未改为创建并派发准备候选，也未接入 IPC / UI，因此新周目尚未走完整流程；该入口仍需后续切片接线并补端到端故障测试。

## 完成摘要

实现进行中。本任务覆盖开局生成目标 / 标签、NPC 引导、动态任务栏与结束流程；本阶段先落实可信剧本输入、冻结快照和初始化生命周期，后续阶段再接入动态任务、结局求值与展示。未完成验收项仍保持未勾选。
