# TS / Rust 职责边界

更新日期：2026-10-06。均为项目约定。状态：**部分落地**——业务 crate `src-rust/mythos-store`（013）与 IPC 基座（015–017）已按本文分工落地并通过质量门禁（业务 crate 不依赖 tauri、装配 `lib.rs` 不计覆盖、域逻辑 100% 行覆盖）；后续 crate 与域落地时继续按本文检验并回写。工程纪律部分借鉴 [Herta 调查](../research/001-herta.md)。

## 原则

- **TS 只做 UI**：视图编排、展示状态、调用命令、渲染事件流。不写业务规则，不直接触碰文件系统、path、shell、SQL 或网络。
- **Rust 拥有一切底层与领域能力**：path / 文件 IO、shell 与进程、存储（SQL、文件持久化）、状态机（回合与阶段流转）、LLM 客户端、对局引擎、记忆系统、密钥管理。
- **命令层薄**：`src-tauri` 的 `#[tauri::command]` 只解参数、转发、回包，不写业务；业务逻辑在 workspace 的业务 crate 里，命令层只做转发。
- **内存域结构归 Rust**：图（技能树 / 关系网）、索引、缓存等运行时领域结构在业务 crate 内存中维护（候选库如 petgraph，仅在需求真实出现时经 task 引入）；SQLite 与文件只做持久化与查询，重启时由持久层重建内存状态。详见[存储基建](storage.md)「三层模型」。

域 → 任务覆盖：LLM 与密钥 → [task 005](../task/005-llm-design.md)；记录与上下文 → [006](../task/006-record-design.md)；记忆 → [007](../task/007-memory-design.md)；存储 / path / IO / shell 进程边界 → [011](../task/011-storage-design.md)；回合与阶段状态机（含骰判） → [012](../task/012-turn-state-machine-design.md)；通信契约 → [010](../task/010-ipc-contract-design.md)。

## 工作区布局

```text
src-tauri/            # 仅 Tauri 装配：Builder、命令层、capabilities，不沉淀业务
src-rust/<crate>/     # 业务 crate，按域拆分（如 mythos-llm / mythos-engine / mythos-store）
```

- 业务 crate 放 `src-rust/` 下，出现真实需求时创建，不预建空 crate；创建即加入根 `Cargo.toml` 的 `members`。
- 依赖方向：`src-tauri` → 业务 crate；业务 crate 之间单向依赖、禁止成环；**业务 crate 不得依赖 tauri**（保证可独立 `cargo test`，也方便未来复用）。
- `src-tauri` 的单测只覆盖命令层解参与转发；域逻辑的单测跟随业务 crate。

## 通信契约

命令 / 事件 / 错误码的命名与形状细则（含看门狗预算表、重试单层化、后台任务门槛的可检查清单）见[通信契约](ipc-contract.md)。

- 命令：已落地的 store 薄调用和载荷类型在 `src-web/api/store.ts`；前端 `invoke(name, args)` ↔ Rust command；参数 / 返回类型 Rust 定型后 TS 立即声明同型（沿用 [Rust 约定](../standards/rust.md)）。
- 事件：长流程（LLM 流式输出、对局推进、后台任务进度）由命令层 `emit_to("main", …)` 推给前端。业务 crate 只通过不含 Tauri 类型的进度出口交出载荷。前端不轮询、不用 setTimeout 凑实时。信封与序号见[通信契约](ipc-contract.md)。
- 错误：形状与目录见[通信契约](ipc-contract.md)。前端按 `code` 分支。
- 大数据（对局记录、记忆文本）不塞 IPC 返回值——Rust 侧落盘，IPC 只回句柄 / 路径 / 摘要，前端需要时再按命令取分页。

## 工程纪律（借鉴 Herta）

- **持久化一律原子写**：普通文件用唯一 tmp 名 + rename + 半截文件自愈。SQLite、实例锁和 JSONL 受控追加 / 截断的例外见[存储基建](storage.md)。
- **密钥隔离**：API key 只存 Rust 侧（OS 凭据库优先），IPC 只传「已设置 + 尾号 hint」，明文不进 webview、不进前端状态。尾号长度和降级文件权限见[通信契约](ipc-contract.md)。
- **网络调用分层看门狗**：连接建立（头阶段）与流式空闲分别设限；重试策略单层化——传输层与业务循环不叠加重试。
- **后台任务默认关**：烧用户 quota 的自动化任务（记忆蒸馏、后台总结）默认关闭，启动前过多重门槛，用户返回时可在边界让位。

## 门禁

业务 crate 全部纳入现有 Rust 门禁：`cargo test --workspace` 与 `coverage:rust`（已是 `--workspace` 口径，行覆盖 100%）；装配用 `lib.rs` 沿用不计覆盖的约定，域逻辑文件必须足额。
