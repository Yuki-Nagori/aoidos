# 045 — 实现：场景在场与可信获知事件

- 状态：done
- 依赖：023、043、044
- 优先级：P1
- 创建 / 更新：2026-10-12 / 2026-10-13

## 目标与背景

044 定稿场景在场与获知事件协议后，将固定 / 动态角色成员关系和观察 / 传闻交付写入可恢复的引擎因果记录，并提供按当前有效路径读取的有界快照。030 消费这些已提交事件并自行校验记忆权限；`game/presence/` 业务模块不依赖记忆 crate，记忆证据由 engine `record/memory` 适配层通过只读端口提供。

## 必读

[场景角色在场与定向交互设计](044-scene-presence-dialogue-design.md) · [阶段机](../architecture/gameplay/turn-state-machine.md) · [记忆系统](../architecture/data/memory.md) · [通信契约](../architecture/core/ipc-contract.md) · [实体注册表](043-rust-entity-registry-impl.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：按 044 交付 Rust 权威在场状态、固定 / 动态角色进入与离开、具名且版本化的观察 / 传闻获知事件、持久化 / 恢复 / 分支过滤及只读有界 evidence-source port。

非目标：不实现角色自主排期、模型生成实体权限、记忆候选解释 / 提交、定向对话选择 UI、记忆专属 IPC；不从自由文本推断观察范围或传闻传播；不回扩 023、025 既有任务。

## 前置条件与待决策

044 必须已评审通过；023 提供有效因果记录 / 恢复边界，043 提供完整稳定的固定 / 动态实体注册表。本任务产出的事实由 engine / record / entity registry 提交；`game/presence/` 不依赖 `aoidos-memory`、Tauri 命令或 UI。记忆证据通过 engine `record/memory` 适配 `aoidos-memory` 的只读端口，030 作为下游消费方。

## 实施步骤

1. 已实现绑定 `scriptRevision` 的 `SceneCatalog` presence policy：配置初始成员、每个场景的 entry roster 及版本化变更规则。`actor_ids`、Markdown、`entities.json` 和 `entityCreated` 不会自动使角色在场；变更作为 `SettlementPlanned` 中的 mutation，按 applied / 当前有效路径重建。
2. 已实现 observed 版本化 mutation。v1 规则只允许引用绑定场景内已完成的公开旁白；UTF-8 字节区间标出依据，并核验主体在来源时刻处于同一场景，不能从玩家输入、角色私语或其他场景正文推导获知。
3. 已实现 rumor delivery，校验接收者与规则要求的告知者在场、交付正文作者及其来源时刻在场、原始来源、前一跳接收者 / informant 对应、root source 和 scope 一致、父事件先行、根告知者与接收者集合无环及最多 8 跳。规则不会将主张提升为世界事实。
4. 已实现 Rust 只读快照及 evidence source：snapshot 限 512 人 / 256 KiB；evidence 查询每次最多读取最近有效路径的 4096 条记录 / 16 MiB，仅返回 applied、有效分支上的成功完整回合事件，来源正文和父链须在同一预算窗口并属于成功完整回合。恢复按原有序 mutation 组校验并只重放未应用 intent，支持组内已部分应用的父子事件；不从 partial 补事件，未知事件与不完整链失败关闭。
5. 已接入内置 domain 对 presence / observed / rumor mutation 的登记校验和幂等 applied 回执；engine presence 模块不依赖 memory、Tauri 或 UI。`SourcePort::with_evidence_context` 接受显式有效目录、catalog 和共享数据库上下文；正式 `Storage::with_memory` 产品消费仍只使用 `told`，由 030 承接后续接线。
6. 已覆盖策略完整性、初始快照、在场进入 / 过期 revision、跨场景观察来源拒绝、观察主体缺席、失败回合来源拒绝、传闻作者身份、部分应用恢复、持久化损坏链和成功终态。规则累计上限及跨调用身份冲突也有回归测试。

## 预计改动

engine 的 scene presence / trusted-evidence source、023 record facts、043 entity registry 消费接口及共享持久化；不新增 Tauri / TypeScript 命令。030 通过只读端口消费 trusted-evidence source，046 再把 presence snapshot 接入角色选择 UI。

## 验收标准

- [x] 初始在场与进入 / 离开只由可信规则提交；NPC 注册本身不构成在场，固定 / 动态身份使用同一稳定 ID 约束。
- [x] presence 仅按 applied / effective path 重建；observed / rumor 作为记忆证据还须来源回合成功完整、mutation 已 applied，恢复补写不会从 partial 创造事件。
- [x] observed 有可验证范围和触发记录；rumor 每跳保留 informant、原始主张、交付来源、规则版本及不确定性。
- [x] 缺少主体、场景 revision、规则、因果位置或恢复回执时只读端口失败关闭；presence 最多 512 人 / 256 KiB，单次来源扫描最多 4096 记录 / 16 MiB，传闻最多 8 跳。
- [x] `game/presence/` 不依赖 memory / Tauri / UI；记忆证据通过 engine `record/memory` 适配层暴露只读端口；架构、类型、注释和 044 一致，独立 review 通过。

## 验证计划与结果

覆盖 engine 因果路径和 SQLite 故障 / 恢复测试、有效分支过滤、事件源只读验证及全量 `bun run verify`。

| 日期       | 环境 / 命令                                                    | 预期                             | 实际结果                                                  |
| ---------- | -------------------------------------------------------------- | -------------------------------- | --------------------------------------------------------- |
| 2026-10-13 | `cargo clippy -p aoidos-engine --all-targets -- -D warnings`   | engine lint 无警告               | 通过                                                      |
| 2026-10-13 | `cargo test -p aoidos-engine --lib game::presence::tests`      | 在场 / 观察 / 传闻与恢复测试通过 | 52 项通过                                                 |
| 2026-10-13 | `cargo test --workspace`                                       | workspace 单元与文档测试通过     | 通过                                                      |
| 2026-10-13 | `bun run lint:rust:fix` / `bun run lint:rust`                  | 自动修复后 lint 无警告           | 两项通过                                                  |
| 2026-10-13 | `cargo test --manifest-path Cargo.toml -p aoidos-engine --lib` | engine 全量库测试通过            | 402 项通过                                                |
| 2026-10-13 | `bun run coverage:rust`                                        | Rust coverage 门禁通过           | 113 个库目标通过；报告 2 行 LLVM 映射未覆盖，未超配置预算 |
| 2026-10-13 | `bun run verify`                                               | 所有门禁通过                     | 通过                                                      |

## 风险与回退

在场或获知事件若无法沿 023 的有效分支重建，记忆可能将已撤销或无法证实的内容授予角色。读取或恢复不完整时拒绝输出相关证据；不得降级为正文推断或普通目录快照。

## 决策与工作记录

- 2026-10-12：按交叉评审重定范围为 engine 权威状态 / 事件源；不依赖 030、UI 或 Tauri，避免与 030 双向等待。角色选择 UI 改由 046 承接，记忆 IPC 仍归 031。
- 2026-10-13：按 issue #140 收窄 crate 依赖边界表述：`game/presence/` 不依赖 memory；engine `record/memory` 适配层仍依赖 `aoidos-memory` 并提供证据只读端口。代码无需修改。

## 完成摘要

实现与验证已完成；023、043 与 044 前置均已完成。当前分支提供引擎事件源，正式 030 消费与 046 快照 IPC / UI 接线仍由各自任务承接。独立 review 修复了知识规则跨调用累计上限 / ID 冲突问题，未发现未解决的 P0/P1/P2。完整 `bun run verify` 通过。
