# TS / Rust 职责边界

更新日期：2026-10-09。均为项目约定。存储基建（013）、LLM 适配 / 配置（018 / 019）、协调与记录 / 阶段机（020–023）、JSON 共用工具（036）及 IPC 基座（015–017）已按本文分工落地；验收证据见[任务索引](../task-index.md)。后续模块继续核验单向依赖及职责归属。工程纪律参考 [Herta 调查](../research/001-herta.md)。

## 原则

- **TS 只做 UI**：视图编排、展示状态、调用命令、渲染事件流。不写业务规则，不直接触碰文件系统、path、shell、SQL 或网络。
- **Rust 拥有一切底层与领域能力**：path / 文件 IO、shell 与进程、存储（SQL、文件持久化）、状态机（回合与阶段流转）、LLM 客户端、对局引擎、记忆系统、密钥管理。
- **命令层薄**：`src-tauri` 的 `#[tauri::command]` 只解参数、转发、回包，不写业务；业务逻辑在 workspace 的业务 crate 里，命令层只做转发。
- **内存域结构归 Rust**：图（技能树 / 关系网）、索引、缓存等运行时领域结构在业务 crate 内存中维护（候选库如 petgraph，仅在需求真实出现时经 task 引入）；SQLite 与文件只做持久化与查询，重启时由持久层重建内存状态。详见[存储基建](storage.md)「三层模型」。

域 → 任务覆盖：LLM 与密钥 → [task 005](../task/005-llm-design.md)；记录与上下文 → [006](../task/006-record-design.md)；记忆 → [007](../task/007-memory-design.md)；存储 / path / IO / shell 进程边界 → [011](../task/011-storage-design.md)；回合与阶段状态机（含骰判） → [012](../task/012-turn-state-machine-design.md)；通信契约 → [010](../task/010-ipc-contract-design.md)；主题 / 皮肤校验 → [009](../task/009-theming-design.md) 与 [026](../task/026-theming-impl.md)（设计 / 实现）。

## 工作区布局

```text
src-tauri/            # 仅 Tauri 装配：Builder、命令层、capabilities，不沉淀业务
src-rust/<crate>/     # 业务 crate，按域拆分（如 aoidos-llm / aoidos-engine / aoidos-store）
```

- 业务 crate 放 `src-rust/` 下，出现真实需求时创建，不预建空 crate；创建即加入根 `Cargo.toml` 的 `members`。
- 依赖方向：`src-tauri` → 业务 crate；业务 crate 之间单向依赖、禁止成环；**业务 crate 不得依赖 tauri**（保证可独立 `cargo test`，也方便未来复用）。
- `src-tauri` 的单测只覆盖命令层解参与转发；域逻辑的单测跟随业务 crate。

### 回合协调与端口依赖

依赖按提供能力的方向单向连接；模块之间共享普通类型或端口，不共享 Tauri 状态。

```text
src-tauri（装配、命令、窗口事件适配）
  ├─→ aoidos-engine（lease、回合所有者、快照、输出 / 事件端口）
  │     ├─→ aoidos-script（纯剧本原文解析与格式校验，无业务反向依赖）
  │     ├─→ aoidos-json（共用编解码与静态错误类别）
  │     ├─→ aoidos-store（记录追加与 applied 原语）
  │     └─→ aoidos-llm（Provider、护栏、唯一请求调度器与预算端口）
  │           ├─→ aoidos-json（配置 / 凭据及 SSE 编解码）
  │           └─→ aoidos-store（配置 / 凭据复用的文件基建）
  ├─→ aoidos-llm（配置 / 凭据的薄命令适配）
  ├─→ aoidos-store（实例锁与存储装配）
  └─→ aoidos-json（IPC 值解码与信封编码）
```

- **事件端口**：`EventPort` 和回合载荷由 engine 定义；Tauri adapter 负责序列化、序号预留、主窗口投递与清退。engine 不引用 `AppHandle`、`CmdError` 或前端类型。
- **输出端口**：`OutputWriter` 由 engine 定义；内存实现仅供调试。022 的记录模块实现增量提交 / 封口，并消费 store 原语；文件基建不反向依赖 engine 的终态或业务 schema。022 已交付记录持久化，engine 直接消费 store 的记录日志与 applied 原语；数据库连接、业务格式与恢复状态归 engine，壳只持有服务。
- **调度确认端口**：`OutputPort` 属 llm，engine 通过确认通道接入；LLM 仅在接纳后确认首交付，不能引用 engine 的快照。准备 / 写入错误由 engine 映射为事件和快照，LLM 不承担记录持久化。
- **预算端口**：`BudgetPort` 属 llm，020 提供每个 turn 和物理 request 的唯一身份；035 在调用方注入账本实现，补齐价格 / 周期上下文。LLM 不反向依赖计费模块，engine 不把账本写成第二套调度器。

`aoidos-json` 不依赖项目业务 crate；JSON schema、错误码与持久化仍归消费模块，详见[共用 JSON 能力](README.md#json-共用能力)。

后续计费（035）等模块接入时，先核对上述方向，再声明 Cargo 依赖；不能为复用字段让 store 依赖 engine / llm，或让 llm 依赖 engine / 计费实现。确需共享词汇时放到双方可单向消费的模块；不预建空的公共 crate。提交前检查 workspace 依赖图与 task 依赖图，各自必须无环；task 的实施前置不等同于 crate 的代码依赖。

## 通信契约

命令 / 事件 / 错误码的命名与形状细则（含看门狗预算表、重试单层化、后台任务门槛的可检查清单）见[通信契约](ipc-contract.md)。

- 命令：已落地的 store 薄调用和载荷类型在 `src-web/api/store.ts`；前端 `invoke(name, args)` ↔ Rust command；参数 / 返回类型 Rust 定型后 TS 立即声明同型（沿用 [Rust 约定](../standards/rust.md)）。
- 事件：长流程（LLM 流式输出、对局推进、后台任务进度）由命令层 `emit_to("main", …)` 推给前端。业务 crate 只通过不含 Tauri 类型的进度出口交出载荷。021 的前端恢复分为纯消费 / 注入式协调与 Vue 生命周期，链路见[架构总览](README.md#前端回合消费链路)。前端不轮询、不用 setTimeout 凑实时。信封与序号见[通信契约](ipc-contract.md)。
- 错误：形状与目录见[通信契约](ipc-contract.md)。前端按 `code` 分支。
- 大数据（对局记录、记忆文本）不塞 IPC 返回值——Rust 侧落盘，IPC 只回句柄 / 路径 / 摘要，前端需要时再按命令取分页。

## 工程纪律（借鉴 Herta）

- **持久化一律原子写**：普通文件用唯一 tmp 名 + rename + 半截文件自愈。SQLite、实例锁和 JSONL 受控追加 / 截断的例外见[存储基建](storage.md)。
- **密钥隔离**：API key 只存 Rust 侧（OS 凭据库优先），IPC 只传「已设置 + 尾号 hint」，明文不进 webview、不进前端状态。尾号长度和降级文件权限见[通信契约](ipc-contract.md)。
- **网络调用分层看门狗**：连接建立（头阶段）与流式空闲分别设限；重试策略单层化——传输层与业务循环不叠加重试。
- **后台任务默认关**：烧用户 quota 的自动化任务（记忆蒸馏、后台总结）默认关闭，启动前过多重门槛，用户返回时可在边界让位。

## 门禁

业务 crate 全部纳入现有 Rust 门禁：`cargo test --workspace` 与 `coverage:rust`（已是 `--workspace` 口径，行覆盖 100%）；忽略正则 `lib\.rs$` 对所有 crate 的 lib.rs 生效，入口只放模块声明 / 薄装配；src-tauri 的非忽略文件及域逻辑文件必须足额。
