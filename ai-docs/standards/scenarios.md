# 剧本格式与接入规范

更新日期：2026-10-12。项目约定；v1 为最小 Markdown 原文接入与显式结构化实体登记，不承诺完整规则自动解析或任意剧本包导入。

## 内容真源与资源

剧本在独立内容仓库维护；Aoidos 将已发布原文放入 `resources/scripts/<scriptId>/`。每个目录保存正文、必要署名 / 许可文件，以及显式登记的 `i18n.json` 和 `entities.json`；不随应用打包私有仓库地址、提交号或 SOURCE.json，不在构建 / 运行时隐式读取相邻仓库或联网同步。

默认资源由 engine 的资源模块显式登记，使用编译期 `include_bytes!` / `include_str!` 嵌入正文、许可和本地化展示元数据。固定路径属于发布资源配置，运行时按 scriptId 选择，不依赖编译机器目录；业务模块不自行拼接文件路径。首版不引入目录扫描或任意剧本包注册。

剧本的界面展示名可放在同目录 `i18n.json`，不混入 Markdown 正文。v1 结构为 `{ "version": 1, "displayNames": { "en": "…", "zh-Hans": "…" } }`；当前应用支持的每种语言都必须有非空展示名，名称最多 256 UTF-8 字节；未知字段、语言或版本令已登记资源不可用，新增界面语言时需同步扩展该格式与类型。应用只在剧本选择器等界面使用当前语言对应的展示名，记录、会话标题和剧本正文继续使用 Markdown 原文首行标题。该文件不翻译剧情、不声明剧本生成语言，也不改变 scriptId / revision。

`entities.json` 是剧本作者 / 维护者人工审核的规范实体目录，不由 Markdown、标题、自然语言或模型输出自动提取。v1 顶层必须且只能包含 `{ "version": 1, "revision": 1, "complete": true, "entities": [...] }` 中的字段：`version` 是 schema 版本，当前只接受 1；`revision` 是正整数目录修订，每次目录语义变更递增；`complete` 表示所登记 scope 的规范实体集合完整，只有 `true` 才能用于精确名称解析；`entities` 为非空数组。每项必须且只能包含 `{ "id": "…", "kind": "…", "scope": "…", "canonicalName": "…" }`，`kind` 只接受 `character`、`place`、`organization`、`item`、`term`。`id` 是稳定实体身份，`scope` 是类别内解析范围，全局范围使用 `global`；同类别、同 scope 的规范名必须唯一。实体身份不得复用，重命名应保留 ID 并提升 revision。

清单上限 1 MiB、4096 项；ID 为 1–64 个 ASCII 字母、数字、连字符或下划线，scope 最多 128 UTF-8 字节，规范名最多 256 UTF-8 字节，名称不得为空、带首尾空白或控制字符。解码拒绝非法 UTF-8、重复 JSON 键、尾随内容、未知字段、未知 kind、缺字段和类型错误；清单任一错误使该剧本不可用，不静默降级成空目录。正文 Markdown 继续原字节保存；新增或修订实体只改元数据，不重写原文。目录只支持精确规范名查询，不自动登记别名、不证明角色获知、不授予世界事实写入权限。`complete` 是维护者对剧本中可作规范精确解析对象集合的显式完整性声明；完整目录还须覆盖引擎登记场景中的所有 Character actor。未列出的普通名词不被解析，也不能从目录缺项推断文本不存在其他歧义。

默认示例为 [Mistbell v1 原文](../../resources/scripts/mistbell/Mistbell.md)及[规范实体清单](../../resources/scripts/mistbell/entities.json)。正文和许可按原字节冻结，格式器不改写；更新为显式发布。`header.scriptRevision` 对带域标签和长度前缀的 Markdown 原字节、实体清单字节及目录 revision 计算 SHA256（`sha256:<hex>`）；引擎加正式 prompt 标记后的静态前缀另算 `staticPrefixHash`。原文或实体资源更新均不覆盖已有周目；重新打开必须匹配已登记版本，不仅校验 scriptId。

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

024 首版只登记最小章节及现有 `pbta-2d6-v1`，不宣称自动运行完整作者稿、多幕计数或 D&D d20。新规则和世界属性须先定稿、登记并验证，缺能力拒绝，不交给模型代填。后续格式演进沿独立内容仓库迭代，应用只升级已核验的接入版本。

## 两仓库验证

内容仓库负责正文、署名、玩法镜像和 Python 测试；Aoidos 负责原文解析、内嵌字节校验、正式 Provider / IPC 联调及完整 verify。抽样镜像通过不证明全部自然语言路径或实际 LLM 质量；两者分别记录范围与限制。
