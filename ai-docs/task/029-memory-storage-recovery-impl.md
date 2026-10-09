# 029 — 实现：记忆版本存储与恢复

- 状态：in-progress
- 依赖：007、013、022
- 优先级：P2
- 创建 / 更新：2026-10-06 / 2026-10-10

## 目标与背景

实现 007 阶段 A 的持久化基础：不可变记忆正文、SQLite manifest、素材身份与快照、幂等操作状态机、因果路径核验、故障冻结及有界恢复。该 crate 供 engine 后续装配使用；本任务不接模型候选、IPC、界面或 006 投影热路径。

## 必读

[记忆规则与工程协议](../architecture/memory.md) · [算法与标定](../architecture/memory-algorithms.md) · [存储基建](../architecture/storage.md) · [记录引擎](../architecture/record-engine.md) · [阶段机](../architecture/turn-state-machine.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与边界

- 新增 `src-rust/aoidos-memory`，复用 `aoidos-store` 的路径、原子发布、有界读取与 applied 回执；通过 engine 注入的共享 SQLite 连接和因果端口校验记录边界，不自行打开第二套数据库。
- 保存 run / 会话绑定、冻结策略、materials / batches / spans、entries / versions / source / evidence 关联、effects、operations / targets、processed 标记、clock receipts 及持久故障标记。
- 实现准备、写正文、应用提交、重复提交核验、来源回退重建、损坏祖先恢复、分页诊断与依赖安全清理。
- `aliases` / `questions`、模型批次调用、费用账本、前端与 IPC 留给后续任务。
- Character 的 `Effect` 在现有 schema 没有主体绑定 `KnowledgeEvidence` 前必须拒绝；030 扩展 schema 后也必须在最终提交、审计和恢复处核验权限，不能只依赖上游调用方。

## 已实现的边界

- 正文与 SQLite 双写采用可恢复的 `prepared → ready → applied` 协议；相同身份 / 内容可幂等重放，冲突内容视为损坏。应用状态、targets、effects 与 `store_applied` 在事务内提交。
- 输入及持久化 BLOB 都有长度 / 数量限制，先查询 BLOB 长度再复制；恢复按真实计划、正文和来源 / 证据关联字节扣减页预算，游标绑定 run 与历史修订。
- 已应用操作若回执、计划摘要、完整效果 / 素材集合、来源去重索引或目标审计不一致，写入 `memory_run_faults` 并冻结该 run 后续写入；已应用目标缺失按损坏处理，效果消费前按关联操作去重审计，仅显式审计修复可解除。
- 029 实现不意味着产品运行时已连接：engine 仅定义端口/装配基础，无模型提议、IPC 命令、窗口事件或记忆上下文注入。

## 非目标

不实现 Provider、语义分类、别名与确认问题、完整记忆门控、费用控制、高级设置、轮回注入、节点收束、产品界面、另一套世界分支或生产算法标定。不能以容量 / 算法开发值宣称生产参数已验证。

## 验收标准

- [x] schema、持久 DTO、版本与来源/evidence 哈希均 fail closed；不兼容版本不使用默认值。
- [x] 原子正文发布、双 CAS、幂等应用、回执核验、因果回退与祖先恢复均有 SQLite / 文件故障测试。
- [x] BLOB 长度在复制前检查；单次扫描的行数、字节数、正文、快照、计划及关联关系均有硬上限与预算耗尽恢复测试。
- [x] 已应用状态任何审计不一致均可持久冻结；后续写路径统一拒绝，健康只读仍可用。
- [x] Character effects 在缺少主体绑定获知证据校验前拒绝；与 007 / 022 边界一致，依赖图无环。
- [ ] Rust 代码、注释、类型、架构文档和本 task 同步；`bun run lint:rust:fix`、`bun run lint:rust`、`bun run verify` 通过，CI 复验通过。
- [x] 覆盖率缺口由 subagent 修复；完成后由独立 subagent 整体 review，问题修复后复核。

## 验证计划与结果

| 日期       | 检查                                           | 结果                                          |
| ---------- | ---------------------------------------------- | --------------------------------------------- |
| 2026-10-10 | `cargo test -p aoidos-memory`                  | 115 项通过；9 个源码文件行覆盖均为 100%       |
| 2026-10-10 | `bun run lint:rust:fix`、`bun run lint:rust`   | 均通过                                        |
| 2026-10-10 | `bun run verify`                               | 13 项全部通过                                 |
| 2026-10-10 | `cargo test --workspace --no-default-features` | 全工作区通过；loopback 测试在受限环境重跑通过 |
| 2026-10-10 | `cargo tree -p aoidos-memory -e normal`        | 未发现 engine / llm 依赖，依赖方向无环        |
| 2026-10-10 | 独立 subagent 最终 review                      | 未发现新的 P1 / P2 阻塞                       |
| 2026-10-10 | `bun run test:native --test ipc-platform`      | 真实 macOS Webview 端到端通过                 |
| —          | CI 三平台                                      | 待提交后验证                                  |

## 风险与恢复

文件系统与 SQLite 不共享原子事务。任何不确定结果都依据已登记身份、hash 与回执核验；不能猜测、静默覆盖或重发收费请求。来源撤回只影响有效路径及派生效果，不删除不可变正文或原始记录。无法验证时保留数据、冻结写入并返回脱敏错误，等待显式修复。

## 工作记录

- 2026-10-10：PR #93 首轮 Linux / macOS 的 Rust 门禁通过，但原生 IPC 夹具仍固定断言迁移版本 1；改为核验后端目标版本、completed 与无错误，真实 macOS Webview 复验通过，独立 review 确认未放宽完成条件，等待三平台复验。
- 2026-10-10：整体独立评审通过 public API 复现效果去重索引审计遗漏、额外效果 / 素材集合遗漏及已应用目标缺失未冻结；修复后补齐效果直接消费审计和 operation 索引，独立复验均阻断损坏数据及后续写入，未发现剩余 P1 / P2 阻塞。
- 2026-10-10：完成 memory crate / schema / 版本存储 / 幂等状态机 / 因果恢复初版；根据 review 增加预复制 BLOB 上限、持久 run freeze、Character effect fail-closed 与实际关系字节预算。`bun run verify` 13 项、无默认特性工作区测试、crate 全行覆盖和独立 review 均通过；等待 CI 复验。
- 2026-10-09：开始 029；依赖 007、013、022 已满足。memory 只依赖 store / json 等基础库，由 engine 提供记录有效性端口并借用共享 SQLite 连接，避免依赖环。

## 完成摘要

尚未完成验收。全工作区测试、覆盖率、`bun run verify`、独立 review、CI 与最终文档同步完成后更新状态。
