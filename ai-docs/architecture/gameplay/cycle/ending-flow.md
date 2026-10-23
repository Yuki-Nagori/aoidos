# 结局规则与周目落幕

更新日期：2026-10-23。设计由 [054](../../../task/054-ending-flow-design.md) 定稿，实施由 [056](../../../task/056-ending-flow-impl.md) 跟进。标签、规则与最终目标不由剧本预置，所有新周目都在开局由 LLM 生成并冻结；剧本只提供可信状态字段 schema 和可选固定引导 NPC 身份。角色真值、认知、对外呈现和引导职责分别见[角色架构](../characters/README.md)。

## 权威边界与生命周期

结局事实由 `aoidos-engine` 决定，Vue 与 LLM 都不能结束周目或提交世界事实。静态结局选择页仅作视觉参考。

```text
已登记剧本原文 + cycle-input.json + 全部剧情角色档案
  → 持久化不可玩的 initializing run、profile 与原币预算身份
  → 035 登记一个物理开局生成请求
  → LLM 生成目标、标签目录、白名单规则、初始任务及按需 NPC 资料
  → engine 对整个响应作版本化 schema / 白名单 / 跨字段校验
  → 完整初始化快照与 hash 原子持久化，run 变为 playable
  → 可信终局事实提交后，engine 按冻结规则求值并写入 endingResolved
  → 032 单向消费已提交周目边界；可选 AI 根据确定性摘要写非权威叙事
```

每个新 run 均启用此流程；无状态字段的脚本也必须得到目标、标签和任务，但其规则不得引用未登记事实。旧 run 缺少初始化快照时维持原 legacy 语义，不补生成目标、NPC、标签或规则。若开局请求失败或结果 unknown，run 保持不可玩；不得静默放开旧体验、换模型、改提示词或自动重发。玩家显式重试沿用 run / budget / profile 身份并使用新物理 requestId。

## 冻结的输入与初始化快照

`cycle-input.json` 是构建期显式登记并嵌入的剧本配置。它声明由 engine 明确注册、可从已提交事实确定性派生的类型化字段及其范围 / 比较运算符，以及可选角色资料；不能声明结局标签、规则或任务。未知状态 source、字段或版本使剧本不可用。角色资料数组通过 `characterId` 引用完整实体清单中的 Character，并分开登记 `truth`、`initialKnowledge` 与 `presentation`；所有剧情角色（含主角）最终都必须有冻结的个人档案。只有 `isGuide: true` 且额外登记 `openingRole` 与 `openingRelationship` 的角色可固定担任首个引导者。未标记的角色不会被自动选中；零个标记时由 LLM 生成引导 NPC，多个标记时剧本不可用。资料注册不授予场景在场、永久玩家关系或未登记的获知能力，这些都由可信初始化单独决定。固定人物的规范名不随 locale 改变，模型不得改写其真值；开场对白使用本周目冻结的 locale。开场角色可以是冒险者公会前台、长期同行伙伴等，由剧本资料决定。生成 NPC 的稳定 run-local identity 由 engine 创建并走可信实体初始化路径，不能仅存在于叙述文本。

开局模型接收当前剧本文本、受限字段 schema、输出版本和应用 locale；不接收本局状态数值。一个有限的结构化响应同时包含最终目标、目标完成 / 未完成时可引用的标签 ID、标签说明和可见性、以白名单字段和比较运算符构成的规则、初始任务以及按需生成 NPC 资料。ID 与顺序由 engine 创建；响应必须整体通过大小 / 数量 / 文本 / locale / ID 唯一性 / 引用 / 互斥组 / 阈值 / 任务依赖等校验，任一字段无效就整体拒绝。

当前实现仍处于分阶段建设：engine 中的纯输出校验器只覆盖目标、标签、规则和初始任务投影，要求调用方传入该 run 冻结的可信 `cycle-input`；它不生成或校验完整人物档案 / guide NPC，不访问账本或 Provider，也不会把 run 转为 playable。此投影不得被误用作完整初始化快照；接入新局入口前须补齐全部必需产物并在同一可信初始化流程中校验、冻结。

冻结快照绑定 runId、schema / prompt / locale 版本、scriptId / scriptRevision、输入 schema hash、profile revision、物理 requestId、标准化完整输出及其 hash。预算与 run 身份在发请求前保存；成功结果只在一个 SQLite 事务中从 initializing 进入 playable。未知或失败保留原费用记录和待处理义务；显式 retry 使用新 requestId，不能重复创建 run / 预算。放弃初始化只停止该 run 并保留费用审计；玩家显式确认完整删除时，沿既有删除 journal 清理该局全部本地数据，包括未结请求和结算 outbox，仍须校验账务身份范围。

任务初始态与后续提议属于引导投影，不是世界事实或终局权限。动态任务只在有意义的已提交里程碑节点提出；优先复用已有回合的结构化响应字段，否则使用独立计费请求。engine 校验来源修订、ID、数量、文本、依赖和当前历史分支后幂等保存；未提交、失败、unknown、重生成、重复回复和被回退回复不改变任务状态。完成项只划线并进入默认折叠区，数据保留以便随有效分支重放；任务完成不写世界状态、不满足结局条件，也不进入记忆证据。

## 结局求值与恢复

只有可信剧本 / 世界状态终局条件已提交后，engine 才按冻结规则求值已提交且类型 / 范围有效的状态。缺失值、未知字段与冲突不静默按零或由 LLM 猜测。生成标签可组合；互斥组和 tie 必须有确定规则。`sessionEnded` applied 后，engine 根据终局锚点与冻结规则稳定派生 `resolutionId`，追加 `endingResolved`，冻结 tag / rule provenance、history revision、公开事实及规范摘要。只有可公开标签和描述能进入 UI / AI 请求，隐藏原始数值与私有标签不外泄。角色档案也按用途投影：真值、角色自身认知、对外 claim 和玩家已获知事实是独立层。谎言只是表面 claim，不覆盖真值；对白记录只证明角色说过什么，不证明其内容成立。提示词不能提供保密保证；使用秘密推理时必须限制输入权限，并将其与面向玩家的公开叙事分开。

恢复时，初始化未完成不能被当作 playable；unknown 不自动重发。已提交 `sessionEnded` 但缺少 applied `endingResolved` 时，依据原终局锚点与原快照确定性补交相同 resolution；已 applied 的快照直接读取。历史回退只投影当前有效分支状态，不改写旧冻结输入。032 只消费已提交周目结束事实，不拥有规则、不触发结局。AI 结局叙事绑定 resolutionId / summaryHash，是可选的独立展示产物；失败不撤销权威落幕，显式重试复用同一摘要，不进入普通 narration、记忆证据或后续规则上下文。

## 模块边界

```text
resources/scripts → aoidos-engine assets / game initialization
profile + billing ports → aoidos-engine initialization lifecycle
engine ending resolver → record / world / run storage ports
engine → aoidos-memory (consume committed cycle-end facts only)
src-tauri → typed commands / DTOs
src-web API → task bar and ending view (render snapshots, never author facts)
```

依赖保持 `aoidos-engine → aoidos-memory / aoidos-llm / aoidos-billing`；memory、billing 和 UI 不反向持有结局权威。精确 schema、事务、测试和验收由 [056](../../../task/056-ending-flow-impl.md) 维护。
