# 架构总览

Aoidos 是 AI 驱动的剧情跑团桌面应用，使用 Vue、TypeScript 与 Tauri 构建界面和桌面壳，底层业务由 Rust 工作区承载。本文是架构文档入口，帮助定位模块职责、接口契约和设计对接；产品目标见[根 README](../../README.md)，实施进度见[任务索引](../task-index.md)，工程约定见[规范索引](../standards/README.md)。

## 主题导航

| 主题         | 文档                                   | 查阅内容                                                         |
| ------------ | -------------------------------------- | ---------------------------------------------------------------- |
| 技术选型     | [技术栈](tech-stack.md)                | 依赖、版本口径与 IPC 通道                                        |
| 工程组织     | [目录规划](repository-layout.md)       | 目录、模块归属与命名                                             |
| 分层职责     | [职责边界](ts-rust-boundary.md)        | TS / Rust 分工及依赖约束                                         |
| 跨端接口     | [通信契约](ipc-contract.md)            | 命令、事件、错误码及公共预算                                     |
| LLM 设计     | [LLM 接入与护栏](llm.md)               | Provider、护栏、配置与回合协调；实际能力及后续接线边界           |
| 计价与额度   | [计价与费用预算](billing.md)           | 金额额度、不可变价格、持久账本与周期恢复；已评审，待实现         |
| 记录与上下文 | [记录引擎](record-engine.md)           | 块语法、流式持久化、投影预算、恢复与迁移；022 已实现并验收       |
| 记忆设计     | [记忆系统](memory.md)                  | 三域边界、门控、回写与节点收束；029 存储 / 恢复已交付            |
| 记忆算法     | [记忆算法与标定](memory-algorithms.md) | 匹配 / 强化 / 衰减候选、正确性约束与对照实验；算法与数值待标定   |
| 回合与判定   | [阶段机](turn-state-machine.md)        | 五阶段、三档判定、场景推进、中断恢复与因果回退；023 已实现并验收 |
| 界面结构     | [界面交互](ui-shell.md)                | 舞台覆盖层、面板四态、记录呈现与输入；已评审，待实现             |
| 国际化       | [界面国际化](i18n.md)                  | 语言、单源资源、首窗与原生文案、精确格式化；已评审，待实现       |
| 主题与皮肤   | [主题架构](theming.md)                 | token 目录、偏好防闪、CSS 子集与失败回退；已评审，待实现         |
| 存储基建     | [存储基建](storage.md)                 | 数据目录、SQLite、实例锁与原子写                                 |
| 开发与交付   | [构建与开发](build-and-development.md) | 本地命令、质量门禁、打包与图标                                   |

## 工程现状

存储、LLM 配置 / 凭据、共享回合协调、记录持久化及前端恢复已由 013–022 交付。023 已接入阶段机与八个产品命令；实际剧本 / 配置联调由 024 承接，完整界面由 025 承接。024 已交付最小正式入口，PR #88 已合并；[main CI](https://github.com/Yuki-Nagori/aoidos/actions/runs/37950951480) 三平台通过；已有阶段调用链通过独立原生夹具验证；验收证据见[任务索引](../task-index.md)。

记忆、界面、主题、国际化与计费设计已定稿，实施按依赖推进。029 已交付记忆存储 / 恢复基础 crate，尚未接入产品记忆流程；[算法与标定](memory-algorithms.md)仍待实测，设计定稿不等于产品能力已上线。

## 设计与实施对应

设计定稿表示协议和交互已评审，不表示功能已经上线。下表列出主责；任务范围、依赖及验收证据以链接中的 task 为准。

| 设计文档                               | 设计任务                                        | 实施主责                                                                                     |
| -------------------------------------- | ----------------------------------------------- | -------------------------------------------------------------------------------------------- |
| [LLM 接入与护栏](llm.md)               | [005](../task/005-llm-design.md)                | 018–021 接入 / 恢复，024 产品联调                                                            |
| [计价与费用预算](billing.md)           | [027](../task/027-llm-cost-control-design.md)   | [035](../task/035-llm-cost-control-impl.md) 统一计费与展示，030 / 033 消费                   |
| [记录引擎](record-engine.md)           | [006](../task/006-record-design.md)             | [022](../task/022-record-engine-impl.md) 持久化、投影与迁移                                  |
| [记忆系统](memory.md)                  | [007](../task/007-memory-design.md)             | 设计已评审；029–033 承接实施，当前不接 006 投影热路径                                        |
| [记忆算法与标定](memory-algorithms.md) | [007](../task/007-memory-design.md)             | 与记忆系统共同定稿；033 承接标定和运行期验证                                                 |
| [回合与阶段机](turn-state-machine.md)  | [012](../task/012-turn-state-machine-design.md) | [023](../task/023-turn-state-machine-impl.md) 产品入口、判定与恢复                           |
| [界面结构与交互](ui-shell.md)          | [008](../task/008-ui-shell-design.md)           | [004](../task/004-tailwind-v4.md) 样式底座，[025](../task/025-ui-shell-impl.md) 完整界面     |
| [界面国际化](i18n.md)                  | [028](../task/028-i18n-design.md)               | [034](../task/034-i18n-impl.md) 底座 / 原生，025 / 031 消费                                  |
| [主题与剧本皮肤](theming.md)           | [009](../task/009-theming-design.md)            | [004](../task/004-tailwind-v4.md) 默认样式 / 映射，[026](../task/026-theming-impl.md) 运行时 |

LLM 实施顺序为 [018](../task/018-llm-provider-guard-impl.md) Provider / 护栏 → [019](../task/019-llm-profile-credentials-impl.md) 配置 / 凭据 → [020](../task/020-llm-turn-ipc-impl.md) 回合协调；[021](../task/021-llm-web-recovery-impl.md) 承接前端恢复，[024](../task/024-llm-engine-integration.md) 在记录与阶段机完成后验证产品链路。

025 依赖 004、008、024、026、034，024 经 021 / 023 覆盖业务依赖；024 保留最小提交链路和故障联调范围，完整界面验收归 025。026 提供受控 main 构建与主题 bootstrap，025 在该入口接窗口几何恢复；面板 / 骰判偏好存储归 022，回合冻结骰判偏好归 023，避免重复接线。

007 已完成设计评审。029 已交付独立存储 / 恢复 crate，依赖 007、013、022；由 [030 门控批次](../task/030-memory-gates-batches-impl.md) 接入素材与授权流程，[031 高级设置](../task/031-memory-settings-queries-impl.md)、[032 轮回节点](../task/032-memory-cycle-nodes-impl.md) 再接入产品面；[033 标定联调](../task/033-memory-calibration-integration.md)审定生产参数。031 等待 034 国际化，030 / 033 等待 035 费用实施。

027 已定稿[计价与费用预算](billing.md)，035 接不可变价格、双作用域金额额度、持久预留 / 结算、周期恢复及明细 / 可选余额；参考 Token 只生成固定默认金额，不另设累计 Token 额度。当前尚未实施。

028 已定稿 zh-Hans / en、单源资源、独立偏好与精确格式化；034 复用 026 首窗入口交付底座与原生界面，025 / 031 消费，尚未实现。语言不隐式改写玩家原文或计价币种。

## 分层和依赖方向

```text
src-web（Vue + TypeScript：展示、交互与 IPC 薄调用）
  │ invoke()
  ▼
src-tauri（Tauri 装配、命令注册与平台事件适配）
  │ 调用业务 API
  ▼
src-rust/（独立业务 crate：engine 按单向依赖组合 memory / llm / store）
```

### Rust 工作区依赖图

下图只列当前已登记的生产 crate 依赖；新增 crate 或调整依赖时同步更新，不按 task 编号维护一次性说明。实现进度仍以任务索引为准，目录存在不等于已经完成验收。

```text
src-tauri
  ├─→ aoidos-engine
  │     ├─→ aoidos-memory（029 存储 / 恢复）
  │     │     ├─→ aoidos-store
  │     │     └─→ aoidos-json
  │     ├─→ aoidos-llm
  │     │     ├─→ aoidos-store
  │     │     └─→ aoidos-json
  │     ├─→ aoidos-store
  │     │     └─→ aoidos-json
  │     ├─→ aoidos-script（纯原文解析，无业务 crate 依赖）
  │     └─→ aoidos-json
  ├─→ aoidos-llm
  ├─→ aoidos-store
  └─→ aoidos-json
```

原生测试 crate 是测试入口，通过 dev-dependencies 消费被测 crate，不被生产 crate 反向依赖。后续记忆产品流程与计费接入时，补齐实际链路并核对无环；端口定义与实现归属见[职责边界](ts-rust-boundary.md#回合协调与端口依赖)，不能靠反向依赖解决类型复用。

前端 API 放在 `src-web/api/`，可测纯逻辑放在 `utils/`；组件负责展示和编排。命令层保持薄，业务逻辑进入独立 Rust crate；业务 crate 通过普通数据交出进度，由平台层适配窗口事件。跨端类型在 Rust 定型后同步声明 TS 类型，`invoke` 负责透传。

### JSON 共用能力

`aoidos-json` 基于 serde / serde_json，供 engine、llm 和 Tauri 单向消费，只依赖第三方序列化库。公共 API 使用 `serde_json::Value`；消费方显式依赖 serde_json，不由工具 crate 转导出其类型或宏。

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
```

记录事实、估算与压缩属于 engine；存储原语属于 store，供应商 / 传输仍属于 llm。生产依赖保持单向，测试夹具不进入生产链路；023 接领域决策，024 接最小产品联调，025 接完整界面。

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
```

`game::domain` 定义可信世界解释器、冻结请求构造器和只读 completed 通知的内部端口，不引用 Tauri，也不虚构世界属性。实现与验证状态见 [023](../task/023-turn-state-machine-impl.md)，生产 crate 依赖保持单向。

## 文档职责

架构文档维护模块边界和协议设计，规范记录长期工程规则，task 保存范围、决策与验证证据。依赖版本集中在技术栈，跨端接口与公共预算集中在通信契约，专题文档通过链接引用，避免同一规则出现多份口径。未排期构想放在 [ideas/](../ideas/)，调查依据放在 [research/](../research/)。

剧本格式及独立解析器由 [039](../task/039-script-format-design.md) / [040](../task/040-script-parser-impl.md) 承接；`aoidos-script` 只解析 Markdown 原文与校验结构，engine 单向消费可信映射，不让解析器依赖业务执行 crate。024 先使用内置最小剧本。

### 最小正式入口

```text
App.vue → useProduct → api/engine → game_ipc
  → Product（活动会话与默认资源登记）
    ├─→ assets → aoidos-script（原文校验；固定编译资源）
    ├─→ builtin（最小探索规则；原文不自动成为世界操作）
    └─→ ProfileFactory → ProfileSource（壳中的配置 / 凭据锁）
          → ProfileGeneration → aoidos-llm（正式 Provider / 代理 / 护栏）
Service（同 factory / Coordinator）→ record 持久化 → 窗口事件 / 快照
  → useEnginePhase / useLlmTurn / useRecordView
```

活动选择保存在数据根的 `active-session.json`；正文仍由 records 的 workspace / transcript 路径维护。打开前预检配置但不发 HTTP，重开只恢复磁盘事实；生成须提交行动或显式恢复。035 未交付费用账本，当前生成端口不宣称具备金额额度控制。完整作者稿的多幕玩法与完整界面分别由后续协议和 025 承接。
