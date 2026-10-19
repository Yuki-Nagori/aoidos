# 架构总览

更新日期：2026-10-19。

Aoidos 是 AI 驱动的剧情跑团桌面应用，使用 Vue、TypeScript 与 Tauri 构建界面和桌面壳，底层业务由 Rust 工作区承载。本文是架构文档入口，帮助定位模块职责、接口契约和设计对接；产品目标见[根 README](../../README.md)，实施进度见[任务索引](../task-index.md)，工程约定见[规范索引](../standards/README.md)。

## 主题导航

| 主题         | 文档                                   | 查阅内容                                                                                                |
| ------------ | -------------------------------------- | ------------------------------------------------------------------------------------------------------- |
| 技术选型     | [技术栈](tech-stack.md)                | 依赖、版本口径与 IPC 通道                                                                               |
| 工程组织     | [目录规划](repository-layout.md)       | 目录、模块归属与命名                                                                                    |
| 分层职责     | [职责边界](ts-rust-boundary.md)        | TS / Rust 分工及依赖约束                                                                                |
| 跨端接口     | [通信契约](ipc-contract.md)            | 命令、事件、错误码及公共预算                                                                            |
| LLM 设计     | [LLM 接入与护栏](llm.md)               | Provider、护栏、配置与回合协调；实际能力及后续接线边界                                                  |
| 计价与额度   | [计价与费用预算](billing.md)           | 原币价格、单局预算、持久账本、周期报表；035 已实现，实机展示待验收                                      |
| 记录与上下文 | [记录引擎](record-engine.md)           | 块语法、流式持久化、投影预算、恢复与迁移；022 已实现并验收                                              |
| 记忆设计     | [记忆系统](memory.md)                  | 三域与门控；029–033 引擎端流程已接入，生产参数与实测仍待验收                                            |
| 记忆算法     | [记忆算法与标定](memory-algorithms.md) | 匹配 / 强化 / 衰减候选、正确性约束与对照实验；算法与数值待标定                                          |
| 回合与判定   | [阶段机](turn-state-machine.md)        | 五阶段、三档判定、场景推进、中断恢复与因果回退；023 已实现并验收                                        |
| 界面结构     | [界面交互](ui-shell.md)                | 舞台覆盖层、面板四态、记录呈现与输入；025 已实现、平台验收待完成；051 拆分前端架构，052 移植 aoidos-art |
| 国际化       | [界面国际化](i18n.md)                  | zh-Hans / en、语言偏好、首窗原生文案与精确格式化；034 已验收                                            |
| 主题与皮肤   | [主题架构](theming.md)                 | 平级主题 ID、偏好防闪、CSS 子集与失败回退；026 已三平台验收                                             |
| 存储基建     | [存储基建](storage.md)                 | 数据目录、SQLite、实例锁与原子写                                                                        |
| 开发与交付   | [构建与开发](build-and-development.md) | 本地命令、质量门禁、打包与图标                                                                          |

依赖方向与 crate 分层见[分层规范](../standards/layering.md)。当前 Rust workspace 无环；基础与独立领域 crate 向上由领域服务、engine 与 Tauri 壳消费，原生测试位于末端。CI 通过 Cargo 元数据核验 crate 依赖边，并按变更范围选择测试、格式和 lint；具体规则与保守回退策略见[测试规范](../standards/testing.md)。

## 工程现状

存储、LLM 配置 / 凭据、共享回合协调、记录持久化及前端恢复已由 013–022 交付。023 已接入阶段机与八个产品命令；实际剧本 / 配置联调由 024 承接，完整界面由 025 承接。024 已交付最小正式入口，PR #88 已合并；026 主题运行时经 [PR #105](https://github.com/Yuki-Nagori/aoidos/pull/105) 合并，三平台 CI 与本机全量 verify 通过。034 已交付界面语言、持久偏好与本地化格式；后续补齐英文消息 schema 基准、主题名和剧本目录展示名本地化。[PR #107](https://github.com/Yuki-Nagori/aoidos/pull/107) 三平台 CI 与本机全量 verify 均通过。025 已实现舞台、四态面板、有界记录呈现、composer、偏好及窗口恢复；本机 `bun run verify` 全项通过。跨平台原生 WebView、DPI、IME 与辅助技术验收仍未完成，任务保持 implemented。证据见[任务索引](../task-index.md)。

记忆、界面、主题、国际化与计费设计已定稿，实施按依赖推进。029 已交付记忆存储 / 恢复基础 crate，030 提供需显式授权的 private 批次执行器；031 已交付配置版本、周目策略冻结、待确认与记忆查询 IPC、设置和查询界面，以及只读批次状态恢复。032 已交付轮回刻痕授权 / 原子完成边界、节点进度、提示领取接口及生命周期恢复。033 已接入显式记忆处理链路：按当前周目和因果分支收集素材、冻结批次、构造候选 / Prompt，并由玩家明确授权发送；算法生产参数、真实样本与长对局验收仍待完成。批次继续遵守费用授权和游戏 lease 边界；设计定稿或引擎接口完成都不代表相应产品能力已上线。

## 设计与实施对应

设计定稿表示协议和交互已评审，不表示功能已经上线。下表列出主责；任务范围、依赖及验收证据以链接中的 task 为准。

| 设计文档                               | 设计任务                                        | 实施主责                                                                                                                                            |
| -------------------------------------- | ----------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| [LLM 接入与护栏](llm.md)               | [005](../task/005-llm-design.md)                | 018–021 接入 / 恢复，024 产品联调                                                                                                                   |
| [计价与费用预算](billing.md)           | [027](../task/027-llm-cost-control-design.md)   | [035](../task/035-llm-cost-control-impl.md) 统一计费与展示，030 / 033 消费                                                                          |
| [记录引擎](record-engine.md)           | [006](../task/006-record-design.md)             | [022](../task/022-record-engine-impl.md) 持久化、投影与迁移                                                                                         |
| [记忆系统](memory.md)                  | [007](../task/007-memory-design.md)             | 设计已评审；029–033 承接实施，当前不接 006 投影热路径                                                                                               |
| [记忆算法与标定](memory-algorithms.md) | [007](../task/007-memory-design.md)             | 与记忆系统共同定稿；033 承接标定和运行期验证                                                                                                        |
| [回合与阶段机](turn-state-machine.md)  | [012](../task/012-turn-state-machine-design.md) | [023](../task/023-turn-state-machine-impl.md) 产品入口、判定与恢复                                                                                  |
| [界面结构与交互](ui-shell.md)          | [008](../task/008-ui-shell-design.md)           | [004](../task/004-tailwind-v4.md) 样式底座；[025](../task/025-ui-shell-impl.md) 完整界面，[046](../task/046-scene-presence-ui-impl.md) 在场角色选择 |
| [界面国际化](i18n.md)                  | [028](../task/028-i18n-design.md)               | [034](../task/034-i18n-impl.md) 底座 / 原生，025 / 031 消费                                                                                         |
| [主题与剧本皮肤](theming.md)           | [009](../task/009-theming-design.md)            | [004](../task/004-tailwind-v4.md) 默认样式 / 映射，[026](../task/026-theming-impl.md) 运行时                                                        |

LLM 实施顺序为 [018](../task/018-llm-provider-guard-impl.md) Provider / 护栏 → [019](../task/019-llm-profile-credentials-impl.md) 配置 / 凭据 → [020](../task/020-llm-turn-ipc-impl.md) 回合协调；[021](../task/021-llm-web-recovery-impl.md) 承接前端恢复，[024](../task/024-llm-engine-integration.md) 在记录与阶段机完成后验证产品链路。

025 依赖 004、008、024、026、034，024 经 021 / 023 覆盖业务依赖；024 保留最小提交链路和故障联调范围，完整界面验收归 025。026 提供受控 main 构建与主题 bootstrap，025 在该入口接窗口几何恢复；面板 / 骰判偏好存储归 022，回合冻结骰判偏好归 023，避免重复接线。

007 已完成设计评审。029 已交付独立存储 / 恢复 crate，依赖 007、013、022。[030 门控批次](../task/030-memory-gates-batches-impl.md)已持久绑定候选与操作，可从原响应本地恢复 deferred 候选，并以 applied / rejected 终态核验批次完成；升级前的完成回执保持可读。030 的显式执行器复用 020 `Coordinator` private lease 与请求携带的 027 预算端口，发送前登记物理请求身份，成功响应进入 received，未知结果不重发；没有自动触发，也不增加 012 回合调用次数。

043 提供 Rust 权威实体注册表：Mistbell `entities.json` 是只读种子，动态身份经 023 世界意图提交到共享 SQLite；恢复核验 applied 与因果记录，快照只包含当前有效分支身份。045 提供 Rust 权威场景在场及 observed / rumor 证据端口：观察 v1 只接受来源时同场、主体在场的已完成公开旁白区间；传闻逐跳核验来源作者和传播链。获知事件必须 applied、属于成功完整回合并位于当前有效路径。`Storage::with_memory` 接收活动目录，在会话 → SQLite 锁内装配实体快照并绑定到 `SourcePort`；仓储执行证据校验时将共享连接显式传入。缺少完整实体目录、场景修订或在场策略时失败关闭。存储入口已能校验 `told`、`observed` 与 `rumor`；033 已接入基于冻结上下文的记忆候选 / Prompt、显式批次发送和本地提交 / 恢复。046 已接入只读在场快照 IPC 与定向输入 UI，031 已接入配置、问题 / 条目查询和批次状态恢复；033 的生产参数、真实样本和长对局验收仍待完成。

正式引擎 / Tauri 入口已支持显式指定当前场景内的固定登记角色，接收目标贯穿接纳、恢复、角色提示与持久回复。046 已将 `engine_get_scene_presence`、composer 角色选择和 revision 校验接入产品链路；场外输入保持旁白路由，接收者列表排除玩家自身，并使用 044 定义的稳定错误原因。031 已接入应用默认 / 剧本覆盖的版本化配置读写、新周目策略冻结、待确认问题与条目查询 / 回答、批次状态只读恢复、设置与查询界面，`engine_open_session` 返回 `memoryRunId`。记忆批次由 033 显式驱动，并复用 030 private 执行边界、020 lease 与 027 费用授权；031 不提供未授权触发器。别名支持玩家确认后的登记、撤销及按当前有效来源重建，自动明确指向仍未接入；[032 轮回节点](../task/032-memory-cycle-nodes-impl.md)已提供 Rust 类型、仓储、规则适配器，以及授权上下文准备和原子完成入口，并已由 033 接入候选提交。034 国际化已完成；030 / 033 可使用 035 的费用端口开展本地联调；收费批次和真实费用实验仍须等待 035 完成三平台实机验收并获授权。

027 已定稿[计价与费用预算](billing.md)。035 已接入精确金额、不可变价格登记、按 profile 选择计价币种、每局 CNY / USD 独立预算、物理请求预留 / 结算、双币种用量与月报、80% 提醒及分页明细；参考 Token 只生成同币种建议值。历史恢复限完整 usage 和原冻结价格，余额查询仅支持 DeepSeek 显式只读请求；三平台实机展示留证仍待补，验收状态见 [035](../task/035-llm-cost-control-impl.md)。全应用月报不设总额度。

028 定稿 zh-Hans / en、独立偏好与精确格式化；034 以英文资源为消息 schema 基准，复用 026 的首窗入口和 013 的共享存储，已完成验收。025 / 031 / 035 消费后续界面与金额格式化能力。语言不隐式改写玩家原文或计价币种。

## 分层和依赖方向

```text
src-web（Vue + TypeScript：展示、交互与 IPC 薄调用）
  │ invoke()
  ▼
src-tauri（Tauri 装配、命令注册与平台事件适配）
  │ 调用业务 API
  ▼
src-rust/（独立业务 crate：engine 按单向依赖组合 billing / memory / llm / store）
```

### Rust 工作区依赖图

下图只列当前已登记的生产 crate 依赖；新增 crate 或调整依赖时同步更新，不按 task 编号维护一次性说明。实现进度仍以任务索引为准，目录存在不等于已经完成验收。

```text
src-tauri
  ├─→ aoidos-engine
  │     ├─→ aoidos-billing
  │     ├─→ aoidos-memory（029 存储 / 恢复，030 批次，032 轮回 / 节点原语）
  │     │     ├─→ aoidos-store
  │     │     └─→ aoidos-json
  │     ├─→ aoidos-llm
  │     │     ├─→ aoidos-store
  │     │     └─→ aoidos-json
  │     ├─→ aoidos-store
  │     ├─→ aoidos-theme
  │     │     └─→ aoidos-store
  │     ├─→ aoidos-locale
  │     │     └─→ aoidos-store
  │     ├─→ aoidos-script（纯原文解析，无业务 crate 依赖）
  │     └─→ aoidos-json
  ├─→ aoidos-billing
  ├─→ aoidos-llm
  ├─→ aoidos-store
  ├─→ aoidos-theme
  │     └─→ aoidos-store
  ├─→ aoidos-locale
  │     └─→ aoidos-store
  └─→ aoidos-json
```

原生测试 crate 是测试入口，通过 dev-dependencies 消费被测 crate，不被生产 crate 反向依赖。033 已接入记忆产品处理链路，真实样本与生产参数验收仍在进行；计费运行时由 engine 持有账本并实现 LLM 的预算端口。端口定义与实现归属见[职责边界](ts-rust-boundary.md#回合协调与端口依赖)，不能靠反向依赖解决类型复用。

前端 IPC 调用放在 `src-web/api/`；功能私有逻辑留在 `features/<feature>/`，跨功能共用逻辑放在 `shared/`；组件负责展示和编排。命令层保持薄，业务逻辑进入独立 Rust crate；业务 crate 通过普通数据交出进度，由平台层适配窗口事件。跨端类型在 Rust 定型后同步声明 TS 类型，`invoke` 负责透传。

### JSON 共用能力

`aoidos-json` 基于 serde / serde_json，供 engine、llm、memory 和 Tauri 单向消费，只依赖第三方序列化库。公共 API 使用 `serde_json::Value`；消费方显式依赖 serde_json，不由工具 crate 转导出其类型或宏。

共用能力包括按 UTF-8 字节限制输入、递归拒绝重复键、完整值解码、类型转换、紧凑编码及显式规范化。错误只保留静态类别，区分容量、UTF-8、JSON、schema 与编码失败，不携带原文。`InvalidShape` 统一覆盖缺字段与类型错误，不匹配第三方错误字符串。

领域 DTO、未知字段策略、JSONL 行规则、原始记录 hash 与 I/O 由消费模块负责。普通编码保持字段顺序，规范化只用于明确采用排序规则的摘要；已存记录按原字节保留。实现及验证见 [036](../task/036-json-tools-impl.md)。

### 前端回合消费链路

```text
useLlmTurn（Vue 身份 / 订阅 / 重连 / scope 清理）
  ├─→ api/llm（同型载荷、invoke / 主窗口 listen 薄调用）
  └─→ turn-recovery（单在飞读取、有界缓存、恢复代次）
        └─→ turn-consumer（正文 / 序号 / 终态的纯消费规则）
```

纯消费层只从 API 模块导入类型，不引用 Vue 或 Tauri。记录视图与迁移消费链路见下节；后续产品界面接入时继续更新实际链路，保持生命周期与消费规则分层，不把 UI 的恢复错误写成 Rust 回合失败。

### 记录与存储消费链路

```text
store_ipc（lib.rs 宏装配）→ store_commands（完整参数 DTO、阻塞调度）
  → engine::Storage（迁移、偏好、Records 登记表）
  → record::Session（单写者、partial、正式记录、恢复）
  → store::journal / applied / db（文件原语与 SQLite 事务）

Session → WorkingSet → 纯 Projector → PromptPlan / GuardSpec
Session → RecapCandidate → Compression（同一 Coordinator lease）→ Session

useRecordView / useMigration / useLlmTurn → listener-group（逐项持有资源）
  → API 薄调用 + 各域 recovery 纯逻辑（有界读取、缓存和代次）

resources/scripts/<scriptId>/（编译期登记的 Markdown + 固定实体种子）
  → engine::game::assets（有界严格解码、静态资源 revision）
    ├→ game::builtin::SceneCatalog（精确规范实体查询）
    └→ EntityDirectory seed
         ├→ 023 WorldPort → EntityRegistry（共享 SQLite；事务化写入 applied + 动态实体）
         └→ Storage::with_memory（活动 SceneCatalog）→ EntityRegistry snapshot + SQLite + Session
              └→ SourcePort（只读、当前分支、有界；told / observed / rumor 校验）
              └→ game::recovery::derive_evidence_path（复用 023 回合因果状态机）

memory_get/save_*_config（Tauri IPC）→ engine::Storage → aoidos-memory 配置仓储
  → SQLite 版本行 + head CAS（应用默认 / 剧本显式覆盖）
engine_open_session → 正式记录登记 → 合并并校验配置 → 冻结 memory_runs.policy + hash
  → OpenedSession { sessionId, memoryRunId, ... }

033 显式处理链路：玩家发起 → `memory_collect_materials` 本地分页扫描并登记当前周目有效未处理素材
  → `memory_prepare_batch` 冻结 queued 批次 → 玩家明确选择发送
  → `memory_send_batch` 构造 Prompt 并调用 Provider（复用 020 lease 与 027 预算）
  → 保存响应、校验候选并生成操作计划 → 本地完成或显式恢复

授权记忆批次：冻结输入 + 显式授权 → process_authorized_batch
  → Coordinator 空闲检查 / private lease → 027 预算预留 → SQLite dispatch(turnId, requestId)
  → 一次 Provider 请求 → SQLite received(response)；不确定结果 → unknown（不重发）

Runner 确认 SceneAdvanced / 场内 RoundEnded
  → 先写有界 memory_cycle_events 待处理快照 → 再提交来源记录
  → 释放 Session / Domain 锁 → CycleRuntime 按来源序号消费
  → Storage::with_memory / SourcePort 校验当前分支与记录事实
  → aoidos-memory 原子建立节点基线或推进完成回合
Context 打开 / 历史回退 → 核验并清理失效队列 → 幂等补建当前节点基线
```

032 已接入本地节点生命周期和崩溃恢复队列，并提供消费 023 规则匹配、原子领取固定提示以及在同一记忆请求中准备 / 完成刻痕的引擎接口；领取前先恢复节点事件，待恢复时拒绝按旧进度展示。提示接口只校验传入规则结果的版本一致性，最新 applied 世界匹配仍由调用方负责。032 不新增 LLM 请求或 lease。033 已接入普通候选 / Prompt 构造、显式发送与提示请求 / 展示；内置场景尚无提示规则，当前不会展示虚构提示。生产参数、语义样本与长对局验证仍待完成，真实供应商实验须等 035 三平台实机验收及明确授权。

记录事实、估算与压缩属于 engine；存储原语属于 store，供应商 / 传输仍属于 llm。生产依赖保持单向，测试夹具不进入生产链路；023 接领域决策，024 接最小产品联调，025 接完整界面。

### 主题启动与皮肤链路

```text
Tauri setup → aoidos-store 主题偏好读取 → Rust 冻结 ThemeBootstrap
  → Webview initialization_script → HTML head theme-bootstrap.js → 生产主题 CSS → Vue

src-web composable → theme API → theme_commands → aoidos-theme 目录 / CSS 校验
  → aoidos-store 偏好 / 受控文件访问 → 同型 IPC 结果 → 有界 CSSStyleSheet 生命周期
```

默认主题文件由 `aoidos-theme/assets/themes/*.css` 提供，生成 CSS 提交在 `src-web/styles/generated/`；Rust 集成测试逐主题对照生成值和 Tailwind aliases。皮肤结果只包含经校验的常量与基础 token 引用，UI 不解析原始 CSS。macOS 本机及 Windows / macOS / Linux CI 的首模块样式、生产 CSP 与开发 HMR 验收证据由 [026](../task/026-theming-impl.md) 维护。

### 语言启动与展示链路

```text
engine::Storage（共享 SQLite 迁移 / 连接）← aoidos-locale（偏好与系统语言归一化）
  → Tauri locale_commands（保存、应用原生文案与返回状态）
  → 单一 WebView 初始化脚本（LocaleBootstrap）→ locale-bootstrap.js
  → 每次文档加载、Vue 挂载前读取 Rust 偏好 → html lang / vue-i18n

src-web/i18n/startup → locale API → locale_commands → 同型 IPC 结果
  → 首次挂载前对齐持久偏好并注入 useLocale
src-web/features/locale/composables/useLocale → 最新意图与回读确认
  → 当前视图代次确认 → html lang / 界面消息
src-web/shared/utils/format → Intl + 字符串 / BigInt 精确格式化
```

`aoidos-locale` 只依赖 `aoidos-store`，由 engine 和 Tauri 单向消费；偏好与主题 / 游戏 UI 偏好分别持有。Task 034 的三平台与最终 verify 证据记在[任务](../task/034-i18n-impl.md)。

### 阶段机基础链路

```text
game::runtime（有界 actor / 控制队列）→ game::execution（共享 lease / 副作用提交）
  → game::input / proposal / reducer（纯转换与 effect 身份）
game::catalog → ConfirmedWorldView（确认世界及有效记录依据）
game::dice → CheckPlan / Dice / Check（冻结规则与确定性采样）
record::Session → game::recovery / control（事实投影、恢复锚点与合法回退边界）
game::publication → PhaseSnapshot / PhaseEvents（原子确认与投递分离）

useEnginePhase → api/engine + listener-group
  → phase-recovery（有界缓存、单在飞快照与恢复代次）
    → phase-consumer（独立事件基线与跨流修订）

app/App.vue（跨功能组合）
  ├─ app/components/SetupPanel（统一设置抽屉；仅在显式打开时挂载）
  │    ├─ features/billing/BillingPanel（费用与供应商用量）
  │    └─ features/memory 的设置与查询由 app 插槽装配
  ├─ app/components/WelcomeScreen（未开局摘要与显式开局）
  └─ features/game/GameWorkspace
       ├─ useGameView / useGameInput / useGameControls
       ├─ useUiPreferences 由 app 创建并注入
       ├─ usePanelState → GameStage（舞台与面板展示）
       └─ RecordFeed + ActionComposer（记录与玩家输入）
```

`game::domain` 定义可信世界解释器、冻结请求构造器和只读 completed 通知的内部端口，不引用 Tauri，也不虚构世界属性。实现与验证状态见 [023](../task/023-turn-state-machine-impl.md)，生产 crate 依赖保持单向。

## Web UI 分层与设计移植

```text
index.html / main.ts
  → app（启动、根布局、跨功能状态组合）
    → features/<feature>（页面、功能组件、功能 composable 与私有规则）
      → shared（跨功能展示原语、生命周期能力、纯逻辑）
      → api / i18n / styles（独立的 IPC、文案和视觉基础设施）

aoidos-art（固定版本的静态设计输入）
  → 051：整理现有 Vue 职责和依赖方向
  → 052：按设计状态移植为 Vue 页面 / 组件
  → 使用 Aoidos 已有 Rust / IPC 状态；尚未实现的能力不伪装为可用
```

目录边界和实现顺序由 [051](../task/051-web-ui-architecture-refactor.md) / [052](../task/052-aoidos-art-vue-port.md) 跟踪。052 按设计 HTML 的主要 DOM 层级、排布与响应式断点用 Vue 重实现；设计 HTML、预览工具和生成文案不进入产品运行时，不能以自由改版代替视觉移植。正式 token、locale 与交互状态继续分别由主题、i18n 和产品架构持有。App 层可以组合 feature，但 shared 不依赖 feature，feature 不循环依赖。

## 文档职责

架构文档维护模块边界和协议设计，规范记录长期工程规则，task 保存范围、决策与验证证据。依赖版本集中在技术栈，跨端接口与公共预算集中在通信契约，专题文档通过链接引用，避免同一规则出现多份口径。未排期构想放在 [ideas/](../ideas/)，调查依据放在 [research/](../research/)。

剧本格式及独立解析器由 [039](../task/039-script-format-design.md) / [040](../task/040-script-parser-impl.md) 承接；`aoidos-script` 只解析 Markdown 原文与校验结构，engine 单向消费可信映射，不让解析器依赖业务执行 crate。024 先使用内置最小剧本。

### 最小正式入口

```text
app/App.vue → features/product/useProduct → api/engine → game_ipc
  → Product（活动会话与默认资源登记）
    ├─→ assets → aoidos-script（原文校验；固定编译资源）
    ├─→ builtin（最小探索规则；原文不自动成为世界操作）
    └─→ ProfileFactory → ProfileSource（壳中的配置 / 凭据锁）
          → ProfileGeneration → aoidos-llm（正式 Provider / 代理 / 护栏）
Service（同 factory / Coordinator）→ record 持久化 → 窗口事件 / 快照
  → useEnginePhase / useLlmTurn / useRecordView
```

活动选择保存在数据根的 `active-session.json`；正文仍由 records 的 workspace / transcript 路径维护。打开前预检配置但不发 HTTP，重开只恢复磁盘事实；生成须提交行动或显式恢复。035 已将费用预算接入正式生成端口，并提供受限历史恢复与 DeepSeek 主动余额查询；三平台实机展示证据由 035 保留待验收。完整作者稿的多幕玩法与完整界面分别由后续协议和 025 承接。
