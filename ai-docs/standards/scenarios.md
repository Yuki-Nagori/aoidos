# 剧本格式与接入规范

更新日期：2026-10-23。项目约定；v1 为最小 Markdown 原文接入、显式结构化实体和周目初始化输入，不承诺完整规则自动解析或任意剧本包导入。

## 内容真源与资源

剧本在独立内容仓库维护；Aoidos 将已发布原文放入 `resources/scripts/<scriptId>/`。每个目录保存正文、必要署名 / 许可文件，以及显式登记的 `i18n.json`、`entities.json` 和版本化 `cycle-input-vN.json`；不随应用打包私有仓库地址、提交号或 SOURCE.json，不在构建 / 运行时隐式读取相邻仓库或联网同步。

默认资源由 engine 的资源模块显式登记，使用编译期 `include_bytes!` / `include_str!` 嵌入正文、许可和本地化展示元数据。固定路径属于发布资源配置，运行时按 scriptId 选择，不依赖编译机器目录；业务模块不自行拼接文件路径。首版不引入目录扫描或任意剧本包注册。

剧本的界面展示名可放在同目录 `i18n.json`，不混入 Markdown 正文。v1 结构为 `{ "version": 1, "displayNames": { "en": "…", "zh-Hans": "…" } }`；当前应用支持的每种语言都必须有非空展示名，名称最多 256 UTF-8 字节；未知字段、语言或版本令已登记资源不可用，新增界面语言时需同步扩展该格式与类型。应用只在剧本选择器等界面使用当前语言对应的展示名，记录、会话标题和剧本正文继续使用 Markdown 原文首行标题。该文件不翻译剧情、不声明剧本生成语言，也不改变 scriptId / revision。

`entities.json` 是剧本作者 / 维护者人工审核的规范实体初始清单，不是运行时实体注册表；不由 Markdown、标题、自然语言或模型输出自动提取。v1 顶层必须且只能包含 `{ "version": 1, "revision": 1, "complete": true, "entities": [...] }` 中的字段：`version` 是 schema 版本，当前只接受 1；`revision` 是正整数目录修订，每次目录语义变更递增；`complete` 表示所登记 scope 的规范实体集合完整，只有 `true` 才能用于精确名称解析；`entities` 为非空数组。每项必须且只能包含 `{ "id": "…", "kind": "…", "scope": "…", "canonicalName": "…" }`，`kind` 只接受 `character`、`place`、`organization`、`item`、`term`。`id` 是稳定实体身份，`scope` 是类别内解析范围，全局范围使用 `global`；同类别、同 scope 的规范名必须唯一。实体身份不得复用，重命名应保留 ID 并提升 revision。

清单上限 1 MiB、4096 项；ID 为 1–64 个 ASCII 字母、数字、连字符或下划线，scope 最多 128 UTF-8 字节，规范名最多 256 UTF-8 字节，名称不得为空、带首尾空白或控制字符。解码拒绝非法 UTF-8、重复 JSON 键、尾随内容、未知字段、未知 kind、缺字段和类型错误；清单任一错误使该剧本不可用，不静默降级成空目录。正文 Markdown 继续原字节保存；新增或修订实体只改元数据，不重写原文。目录只支持精确规范名查询，不自动登记别名、不证明角色获知、不授予世界事实写入权限。`complete` 是维护者对剧本中可作规范精确解析对象集合的显式完整性声明；完整目录还须覆盖引擎登记场景中的所有 Character actor。未列出的普通名词不被解析，也不能从目录缺项推断文本不存在其他歧义。

该清单只提供剧本固定实体的初始数据；Rust `EntityRegistry` 在运行时将其与可信世界操作提交的动态实体合并，不改写打包清单，也不把记忆能力绑定到 Mistbell 专属路径。动态 ID 由 Rust 随机分配，不从名称推导；每条注册记录绑定 session、script / catalog revision、源 mutation、hash 和 JSONL recordSeq。实体行、单调 `entityRevision` 与 `store_applied` 回执在共享 SQLite 的一个事务中提交。恢复时重放既有 mutation，不重新生成身份；注册表快照会核验回执及源记录，只暴露当前有效分支上的实体，分支外记录保留供审计。快照有界且只读，注册身份本身不授予任何角色获知该实体或相关事件的权限。动态实体创建仍须由可信规则显式授权；模型文本、记忆候选或名称相似度不会直接创建或合并实体。

### 周目初始化输入与引导角色

每个新周目都使用显式登记的版本化 cycle input，不从 Markdown 自然语言推断可执行数值字段。文件声明版本、输入字段 schema、剧情角色名册和可选固定角色资料；不得包含结局标签、结局规则或最终目标。开局模型根据剧本原文、已登记的字段 schema、locale 与输出协议生成目标、标签目录、规则、初始任务及名册中缺少固定资料的角色档案。角色资料用稳定 `characterId` 关联 `entities.json` 中的 Character，并将 `truth`、`initialKnowledge` 与 `presentation` 分开：真值是故事正史，认知声明角色初始知道哪些资料，对外呈现允许角色用另一种表面身份与玩家交互。每个有剧情作用的角色，包括主角，都必须在可玩 run 的冻结快照中有档案；没有剧本固定档案的主角或配角由初始化阶段生成一次。主角控制绑定不能授予玩家查看隐藏真值的权限。是否担任开场引导者由独立的 `isGuide: true` 标记选择，并额外提供 `openingRole` 与 `openingRelationship`。未标记或 `isGuide: false` 的角色资料不会自动成为引导者；零个标记角色时才由 LLM 生成周目专属引导 NPC，多个标记角色使剧本配置无效。固定角色的规范身份和真值由引擎沿用；模型只生成缺失档案与获准的开场对白。生成角色的内部 ID 由引擎分配并通过可信初始化路径注册。角色类型可按剧本写成冒险者公会前台、长期同行伙伴等。不得运行时搜索正文猜测名册或角色。

当前 `cycle-input-v2.json` 顶层必须且只能包含 `{ "version": 2, "revision": 2, "protagonistCharacterId": "…", "storyCharacterIds": [...], "characters": [...], "stateFields": [...] }` 中登记的字段。`storyCharacterIds` 是作者显式确定的本周目剧情角色集合；它必须非空、有界、无重复，且每项必须引用完整实体清单中 `kind=character` 的角色。若提供 `protagonistCharacterId`，它必须属于该集合；固定档案角色也必须属于名册。实体目录可包含仅供背景引用的其他 Character，不要求全量纳入名册。模型必须且只能为名册中缺少固定资料的 ID 生成档案，不得漏项、加项、重复或更换 ID。v1 文件不含名册，只保留历史读取语义，不能用于新的完整角色初始化。

角色项使用 `characterId`、`truth`、`initialKnowledge`、`presentation`，可选布尔 `isGuide`、`openingRole` 和 `openingRelationship`。`truth` 必须明确给出非空 `personalName`、`background`、`identity`、`race`、`age`、`gender`；已能从剧本确定的值不可填 `null`。当前 v2 不支持尚未设定的真值字段，若确需未知状态须先升级 schema；真值已确定但暂不揭晓时，保留真值并用认知和展示层控制披露。`presentation` 明确给出角色初始对外使用的 `displayName`、`identity`、`race`、`age`、`gender` 和 `appearance`，允许与真值不同。`initialKnowledge` 是上述六类真值字段 ID 的唯一值子集，只声明角色知道什么，不代表玩家已知或角色会主动披露。可选 `isGuide` 缺省等同 `false`；只有 `true` 才表示固定引导者，并且必须提供非空 `openingRole` 和 `openingRelationship`。最多一个角色可标记引导者；被 `protagonistCharacterId` 指定的角色不得同时标记为 guide。资料注册不意味着角色已在当前场景、与玩家有真实关系或获知其他事实；这些能力仍须由可信初始化流程单独授权。固定人物真值应与剧本文本及作者资料一致。实体目录的 `canonicalName` 必须是可以公开展示的规范称呼，不得用隐藏真名；展示必须经 `presentation` 或获授权的投影，不得回退读取私有真值。字段项必须且只能包含稳定 `id`、`kind`、可执行的可信来源 `source`、整数 `min` / `max`、非空 `operators` 和两种应用 locale 的简短 `descriptions`。source 必须命中 engine 显式注册、由已提交事实确定性派生的状态投影；未知 source、未经注册的字段、重复 ID、非法范围或非法运算符使剧本不可用。当前字段 ID 固定为 `committed-round-count`，对应 `engine.committed-round-count.v1`；模型只能引用已登记字段与运算符，不能创造状态源或读取当前隐藏值。字段描述帮助模型理解游戏语义，不构成执行代码或事实。

示例：

```json
{
  "version": 2,
  "revision": 2,
  "protagonistCharacterId": "player",
  "storyCharacterIds": ["player", "luo-wei"],
  "characters": [
    {
      "characterId": "luo-wei",
      "isGuide": true,
      "truth": {
        "personalName": "洛苇",
        "background": "苔泉村守灯人，熟悉旧塔、铜钟与山雾的地方传说。",
        "identity": "苔泉村守灯人；尊重玩家自主选择，不替玩家决定行动。",
        "race": "人类",
        "age": "成年，约三十岁",
        "gender": "女性"
      },
      "initialKnowledge": ["personalName", "background", "identity", "race", "age", "gender"],
      "presentation": {
        "displayName": "洛苇",
        "identity": "苔泉村守灯人",
        "race": "人类",
        "age": "成年，约三十岁",
        "gender": "女性",
        "appearance": "三十岁上下的人类女性，身着守灯人的旧式衣物。"
      },
      "openingRole": "守灯人",
      "openingRelationship": "开局时介绍当地情况并提供行动引导"
    }
  ],
  "stateFields": [
    {
      "id": "committed-round-count",
      "kind": "integer",
      "source": "engine.committed-round-count.v1",
      "min": 1,
      "max": 1000000,
      "operators": ["equals", "atLeast", "atMost"],
      "descriptions": {
        "en": "Number of committed player rounds in the current history branch.",
        "zh-Hans": "当前有效历史分支中已提交的玩家回合数。"
      }
    }
  ]
}
```

当前实现上限 128 KiB、64 个字段；字段 ID 为 1–64 个 ASCII 字母、数字、连字符或下划线，描述每语言 1–256 UTF-8 字节。当前已登记来源支持整数 `equals`、`atLeast`、`atMost` 比较，阈值必须落在声明范围内；缺失、未提交、类型不符和越界值不得当作零或跳过条件。无可用状态字段的脚本使用空数组，仍须生成目标和有限标签 / 规则，但规则不能引用未声明字段。

cycle input 使用 `cycle-input-vN.json` 显式命名；新增字段 / 名册语义须提升 schema version 与 cycle input revision。script revision 的哈希输入包括原文、实体清单和 cycle input 字节及其各自修订。当前资源与需要兼容的历史 cycle input 均在构建期显式登记并编译期嵌入，不扫描目录或运行时联网；旧 run 继续使用其冻结的 script revision，不补造角色名册、NPC 或初始化快照。

开局生成结果是 run 私有、版本化、严格白名单校验并冻结的产物，至少包含最终目标、目标状态可引用的标签 ID、tag 目录、受限比较规则、有限的初始任务、所有缺少固定资料的剧情角色档案，以及仅在剧本没有固定 guide NPC 时生成的引导 NPC。人物档案补齐和 guide NPC 的选择是两项独立职责：固定 guide 仍须由 `isGuide` 显式指定，模型不能因为要补全主角或配角资料而把他们选为 guide。引擎自行创建 ID 和顺序，完整结果要通过数量、字符、locale、重复 ID、字段引用、互斥组、阈值、任务依赖和跨字段一致性校验；未知字段或任一部分无效时整份拒绝，不部分应用。应用支持的 locale 必须完整覆盖用户可见目标、任务及公开标签说明。对规范化 JSON 使用 `aoidos-cycle-init-v1\0` 域标签与 64 位大端长度前缀计算 SHA-256，冻结在该 run 初始化快照；规则 hash、终局 resolutionId 与 summaryHash 分别使用 `aoidos-ending-effective-rule-v1\0`、`aoidos-ending-resolution-v1\0`、`aoidos-ending-summary-v1\0` 独立域标签和长度前缀。

当前 engine 纯输出校验阶段覆盖剧情角色档案补齐：有固定档案的角色按名册顺序冻结并拒绝模型覆盖；模型必须精确补齐其余名册 ID，所有档案的认知字段受白名单约束。固定 guide 由唯一 ID 引用；无固定 guide 时才要求模型提交额外完整档案，ID 由 engine 派生。该局部校验不等于完整开局产物，仍未接入可信实体注册、Provider、账本或 playable 转换。

生成请求的物理 requestId、预算和 profile 快照必须先按 027 / 035 持久化。每个新周目先进入不可玩的 initializing 状态，成功校验且完整初始化快照持久化后才变为 playable；失败或 unknown 不自动重发、不换规则、不开放游戏。显式重试使用同一 run / 预算身份和新的物理 requestId。旧存档和明确废弃的初始化 run 沿既有恢复 / 删除协议处理，不静默升级为新周目。

结局事实、可信终局触发、恢复、标签求值、确定性摘要和 AI 非权威叙事边界见 [054](../task/054-ending-flow-design.md)、[056](../task/056-ending-flow-impl.md) 与[结局架构](../architecture/gameplay/cycle/ending-flow.md)。正文解析器只处理 Markdown 原文；初始化和规则都由 engine 明确登记及执行。

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

024 首版只登记最小章节及现有 `2d6-pbta-v1`，不宣称自动运行完整作者稿、多幕计数或 D&D d20。新规则和世界属性须先定稿、登记并验证，缺能力拒绝，不交给模型代填。结局字段输入由 `cycle-input-vN.json` 声明，结局目标、标签和规则按 [054](../task/054-ending-flow-design.md) / [056](../task/056-ending-flow-impl.md) 在开局生成；这不等于现有脚本 schema 已实现或已验证所有结局状态来源。后续格式演进沿独立内容仓库迭代，应用只升级已核验的接入版本。

## 两仓库验证

内容仓库负责正文、署名、玩法镜像和 Python 测试；Aoidos 负责原文解析、内嵌字节校验、正式 Provider / IPC 联调及完整 verify。抽样镜像通过不证明全部自然语言路径或实际 LLM 质量；两者分别记录范围与限制。
