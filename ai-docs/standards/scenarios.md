# 剧本格式与接入规范

更新日期：2026-10-19。项目约定；v1 为最小 Markdown 原文接入、显式结构化实体登记及结局规则 sidecar，不承诺完整规则自动解析或任意剧本包导入。

## 内容真源与资源

剧本在独立内容仓库维护；Aoidos 将已发布原文放入 `resources/scripts/<scriptId>/`。每个目录保存正文、必要署名 / 许可文件，以及显式登记的 `i18n.json` 和 `entities.json`；不随应用打包私有仓库地址、提交号或 SOURCE.json，不在构建 / 运行时隐式读取相邻仓库或联网同步。

默认资源由 engine 的资源模块显式登记，使用编译期 `include_bytes!` / `include_str!` 嵌入正文、许可和本地化展示元数据。固定路径属于发布资源配置，运行时按 scriptId 选择，不依赖编译机器目录；业务模块不自行拼接文件路径。首版不引入目录扫描或任意剧本包注册。

剧本的界面展示名可放在同目录 `i18n.json`，不混入 Markdown 正文。v1 结构为 `{ "version": 1, "displayNames": { "en": "…", "zh-Hans": "…" } }`；当前应用支持的每种语言都必须有非空展示名，名称最多 256 UTF-8 字节；未知字段、语言或版本令已登记资源不可用，新增界面语言时需同步扩展该格式与类型。应用只在剧本选择器等界面使用当前语言对应的展示名，记录、会话标题和剧本正文继续使用 Markdown 原文首行标题。该文件不翻译剧情、不声明剧本生成语言，也不改变 scriptId / revision。

`entities.json` 是剧本作者 / 维护者人工审核的规范实体初始清单，不是运行时实体注册表；不由 Markdown、标题、自然语言或模型输出自动提取。v1 顶层必须且只能包含 `{ "version": 1, "revision": 1, "complete": true, "entities": [...] }` 中的字段：`version` 是 schema 版本，当前只接受 1；`revision` 是正整数目录修订，每次目录语义变更递增；`complete` 表示所登记 scope 的规范实体集合完整，只有 `true` 才能用于精确名称解析；`entities` 为非空数组。每项必须且只能包含 `{ "id": "…", "kind": "…", "scope": "…", "canonicalName": "…" }`，`kind` 只接受 `character`、`place`、`organization`、`item`、`term`。`id` 是稳定实体身份，`scope` 是类别内解析范围，全局范围使用 `global`；同类别、同 scope 的规范名必须唯一。实体身份不得复用，重命名应保留 ID 并提升 revision。

清单上限 1 MiB、4096 项；ID 为 1–64 个 ASCII 字母、数字、连字符或下划线，scope 最多 128 UTF-8 字节，规范名最多 256 UTF-8 字节，名称不得为空、带首尾空白或控制字符。解码拒绝非法 UTF-8、重复 JSON 键、尾随内容、未知字段、未知 kind、缺字段和类型错误；清单任一错误使该剧本不可用，不静默降级成空目录。正文 Markdown 继续原字节保存；新增或修订实体只改元数据，不重写原文。目录只支持精确规范名查询，不自动登记别名、不证明角色获知、不授予世界事实写入权限。`complete` 是维护者对剧本中可作规范精确解析对象集合的显式完整性声明；完整目录还须覆盖引擎登记场景中的所有 Character actor。未列出的普通名词不被解析，也不能从目录缺项推断文本不存在其他歧义。

该清单只提供剧本固定实体的初始数据；Rust `EntityRegistry` 在运行时将其与可信世界操作提交的动态实体合并，不改写打包清单，也不把记忆能力绑定到 Mistbell 专属路径。动态 ID 由 Rust 随机分配，不从名称推导；每条注册记录绑定 session、script / catalog revision、源 mutation、hash 和 JSONL recordSeq。实体行、单调 `entityRevision` 与 `store_applied` 回执在共享 SQLite 的一个事务中提交。恢复时重放既有 mutation，不重新生成身份；注册表快照会核验回执及源记录，只暴露当前有效分支上的实体，分支外记录保留供审计。快照有界且只读，注册身份本身不授予任何角色获知该实体或相关事件的权限。动态实体创建仍须由可信规则显式授权；模型文本、记忆候选或名称相似度不会直接创建或合并实体。

### 结局标签规则

可选 `ending-rules.json` 声明结局属性、标签目录、互斥组及规则来源。它属于可信剧本资源，在构建期显式登记并计算独立的 `endingRulesHash`；不在运行时扫描目录，也不改变现有 `scriptRevision` 的输入。`endingRulesHash` 对 `aoidos_json::canonical_string` 生成的 UTF-8 字节执行 SHA-256，输入前加固定域标签 `aoidos-ending-rules-v1\0` 与 64 位大端字节长度；对象键按库的规范顺序排列、数组保持原顺序，规则数组在校验时按稳定 ID 排序。结构版本未知、属性引用未登记、值域越界、冲突组不合法或未知字段都会使资源校验失败，不能静默降级。

```json
{
  "version": 1,
  "revision": 1,
  "ruleSource": "authored",
  "attributes": [{ "id": "<script-registered-attribute>", "type": "integer", "min": 0, "max": 100, "operators": ["gte", "lt"] }],
  "conflictGroups": [{ "id": "<stable-group-id>", "resolution": "highestPriority" }],
  "tags": [{ "id": "<stable-tag-id>", "visibility": "public", "conflictGroup": "<stable-group-id>", "priority": 10, "summaries": { "en": "<factual statement>", "zh-Hans": "<事实描述>" } }],
  "rules": [{ "id": "<stable-rule-id>", "all": [{ "attribute": "<script-registered-attribute>", "op": "gte", "value": 70 }], "emit": ["<stable-tag-id>"] }]
}
```

`ruleSource` v1 为 `authored` 或 `generated`；顶层还必须且只能包含 `version`、`revision`、`ruleSource`、`attributes`、`conflictGroups`、`tags`、`rules`。v1 只支持整数属性，范围端点为安全整数且 `min <= max`；运算符仅 `eq`、`ne`、`gt`、`gte`、`lt`、`lte`，比较值必须在属性范围内。单文件 ≤256 KiB，属性 ≤128、冲突组 ≤32、标签 ≤256、规则 ≤512；每条规则含 1–16 个 `all` 条件、最多输出 16 个标签。所有 ID 为 1–64 个 ASCII 字母、数字、连字符或下划线；summary 每语言 1–1024 UTF-8 字节，locale 必须覆盖应用支持语言。作者预设资源及其冻结快照均含 1–512 条规则。生成模式使用同一 DTO / schema，但按用途作条件校验：静态资源声明的 `rules` 必须为空；运行时生成结果须通过严格白名单校验后形成冻结快照，`rules` 必须为 1–512 条，且 `version`、`revision`、`attributes`、`conflictGroups`、`tags` 必须与原声明逐字段相同，只能补入合法规则；任一规则或引用非法时整份结果拒绝，不删除非法项后继续接受。不得把空规则声明误当可执行快照，也不得把非空生成结果按静态资源规则拒绝。属性缺失、未提交、类型错误或超界一律阻止本次终局求值，不能当零、跳过条件或用 AI 推断。多条规则按 OR 生效，每条 `all` 是 AND；tag 默认可并存。互斥组只接受 `highestPriority`，组内所有 tag 必须有唯一的安全整数优先级，非互斥 tag 不得携带 `conflictGroup` / `priority`；命中多个时保留唯一最高者，不能用 ID 或模型意见打破同优先级。若使用 LLM 生成，模型只看到字段定义、比较目录与既有 tag allowlist，不读本局运行值，也不能创建属性、标签、事实或任意谓词；物理请求经现有计费端口记录。两种模式最终都由同一 Rust 类型与字段约束校验器、按用途区分的条件校验、规范化和冻结流程处理。`summaries` 是作者审核的事实描述；visibility 决定能否进入玩家摘要和结局叙事请求。此 sidecar 不改变 Markdown 原文及解析器纯原文职责；规则仅由 engine 执行。

`endingRulesHash` 使用 `aoidos-ending-rules-v1\0` 域标签；`ruleHash` 使用独立的 `aoidos-ending-effective-rule-v1\0` 域标签。二者均按“域标签 + 64 位大端规范 JSON 字节长度 + 规范 JSON UTF-8 字节”计算 SHA-256，输出格式为 `sha256:<hex>`。前者覆盖静态 sidecar 全部字段，后者覆盖该周目的来源模式、有效完整规则、tag / conflict 定义和 schema 版本。`resolutionId` 对 `{ runId, terminalMutationId, ruleHash }` 的规范 JSON 字节使用独立域标签 `aoidos-ending-resolution-v1\0`，按“域标签 + 64 位大端规范 JSON 字节长度 + 规范 JSON UTF-8 字节”计算 SHA-256，结果格式为 `sha256:<hex>`；同一终局锚点与规则快照因此得到相同身份。结局快照同时固定 `summaryLocale`、模板版本和精确摘要 UTF-8 字节；`summaryHash` 是对 `aoidos-ending-summary-v1\0` 域标签后以 64 位大端长度前缀封装的 `{ resolutionId, ruleHash, terminalAnchor, summaryLocale, summaryTemplateVersion, publicTagIds, summaryText }` 规范 JSON 求 SHA-256。排序、整数表示与规范编码复用 `aoidos-json`，数组顺序固定；更换应用语言或模板不改写旧摘要，叙事重试复用原语言、模板版本和精确摘要。

配置了 `ending-rules.json` 的新周目使用持久两阶段初始化：先用稳定 `runId` 写入不可玩的 `initializing` 状态、`endingMode=enabled` 及本周目 CNY / USD 预算快照；035 以此 run 创建预算后，才可登记物理生成请求。初始化恢复按原 `runId` 和原预算配置幂等补齐，不因重开生成新账本身份。规则通过与声明模式相符的校验后，在单个 SQLite 事务中保存完整规则快照并将 run 标为 `playable`。失败或响应未知时保留 initializing 状态与费用 / 未确认占用，不开放游戏、不删账、不自动重发；玩家显式重试时创建新的 requestId 并仍计入原 run，或放弃该未开局 run。作者预设模式跳过物理生成请求，但同样先持久化 initializing，再冻结规则并激活。没有配置结局规则的剧本以 `endingMode=disabled` 正常开始，不生成规则、不建立结局专属初始化步骤，并保持既有 `sessionEnded` 行为。旧存档缺少 `endingMode` 时按 disabled 兼容，不为其补生成或迁移出结局规则；之后恢复、重开、分支回退和剧本升级都复用原快照；规则版本升级只作用于新 run。

对 `endingMode=enabled` 的 run，结局事实先于可选 AI 叙事提交。`sessionEnded` applied 后，engine 用同一终局锚点与锁定规则确定性派生 `resolutionId`；再追加已注册的 `endingResolved` 系统记录，并按 022 的 mutation / `store_applied` 协议原子提交标签、来源 rule ID、revision 与规范摘要。启动恢复若看到 enabled run 的已 applied `sessionEnded` 而无 `endingResolved`，必须沿同一锚点补算并提交同一身份；pending 的 `endingResolved` 由 022 恢复，已 applied 则直接读取，绝不重复求值。disabled / legacy-disabled run 沿用原结束行为，不要求也不得补写 `endingResolved`。只有规范终态持久化后才请求 AI 叙事；叙事单独绑定 resolutionId / requestId，失败不撤销结束。

默认示例为 [Mistbell v1 原文](../../resources/scripts/mistbell/Mistbell.md)及[规范实体清单](../../resources/scripts/mistbell/entities.json)。正文和许可按原字节冻结，格式器不改写；更新为显式发布。`header.scriptRevision` 按现有协议对带域标签和长度前缀的 Markdown 原字节、实体清单字节及目录 revision 计算 SHA256（`sha256:<hex>`）；本次不改变其输入。结局规则 sidecar 单独计算 `endingRulesHash`，并与 `scriptRevision` 一起冻结在周目初始化记录中。引擎加正式 prompt 标记后的静态前缀另算 `staticPrefixHash`。原文、实体目录或结局规则资源更新均不覆盖已有周目；重新打开必须匹配原周目登记版本，不仅校验 scriptId。

## 最小原文 v1

- UTF-8 Markdown，单文件 ≤64 KiB；不自动消毒、补全或重编码正文。
- 首行必须为 `# 非空标题`，标题 ≤256 UTF-8 字节；解析器保留正文原文，标题仅展示，不作为身份。
- 文章可以描述世界、人物、场景、玩法和结局；代码块 / 表格 / 自然语言规则均为内容，解析器不执行 Python、SQL、网络或文件操作。
- 正文拒绝 NUL 等控制字符，允许 LF / CR / Tab；禁止嵌入 `[AOIDOS:` 或 `[/AOIDOS:` 结构标记，由引擎统一封装，避免作者正文破坏 prompt 边界。
- scriptId 由注册资源目录提供，不从 prose 猜测。1–64 字节，小写字母 / 数字 / 连字符，首尾字母或数字；跨平台拒绝 con / prn / aux / nul、com1–com9、lpt1–lpt9，不能静默改名。

错误只保留容量、UTF-8、身份、标题和文本的静态类别，不包含正文或路径。

## 署名与执行配置

原创和第三方来源许可随原文保留，采用 SRD 时保留指定英文署名与改编说明；[许可调查](../research/002-dnd-srd.md)不代替实际素材署名。原创身份本身不授予公开分发权，内容仓库应明确许可范围。

[Mistbell 发布许可](../../resources/scripts/mistbell/LICENSE.md)为内容仓库的原字节副本，其中「本仓库」及 `scenarios/` 路径指来源内容仓库。许可中的 `scenarios/Mistbell.md` 在 Aoidos 对应 [resources/scripts/mistbell/Mistbell.md](../../resources/scripts/mistbell/Mistbell.md)；所述 `scenarios/mistbell/minimal.scenario.json` 仅存在于内容仓库，Aoidos 不内嵌该转换 JSON。Aoidos 的署名文件为 [SRD-5.2.1.md](../../resources/scripts/mistbell/SRD-5.2.1.md)，与许可同目录，原文相对链接保持有效。

正文解析与执行配置分开：`aoidos-script` 只解析原文 / 校验结构，不依赖 engine / llm / tauri / store；engine 单向消费普通 DTO，并显式登记场景、规则、actor、RNG 和世界解释器。不能把文章中的“职业 / 金钱 / 法术”等文字自动授予数值执行能力。

024 首版只登记最小章节及现有 `pbta-2d6-v1`，不宣称自动运行完整作者稿、多幕计数或 D&D d20。新规则和世界属性须先定稿、登记并验证，缺能力拒绝，不交给模型代填。结局标签与冻结规则由 [054](../task/054-ending-flow-design.md) 定稿、[056](../task/056-ending-flow-impl.md) 实现；这不等于现有剧本已配置结局数值或标签。后续格式演进沿独立内容仓库迭代，应用只升级已核验的接入版本。

## 两仓库验证

内容仓库负责正文、署名、玩法镜像和 Python 测试；Aoidos 负责原文解析、内嵌字节校验、正式 Provider / IPC 联调及完整 verify。抽样镜像通过不证明全部自然语言路径或实际 LLM 质量；两者分别记录范围与限制。
