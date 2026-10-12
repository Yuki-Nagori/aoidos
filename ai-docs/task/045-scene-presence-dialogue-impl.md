# 045 — 实现：场景在场与可信获知事件

- 状态：ready
- 依赖：023、043、044
- 优先级：P1
- 创建 / 更新：2026-10-12 / 2026-10-12

## 目标与背景

044 定稿场景在场与获知事件协议后，将固定 / 动态角色成员关系和观察 / 传闻交付写入可恢复的引擎因果记录，并提供按当前有效路径读取的有界快照。030 消费这些已提交事件并自行校验记忆权限；本任务不依赖记忆 crate，避免形成反向依赖。

## 必读

[场景角色在场与定向交互设计](044-scene-presence-dialogue-design.md) · [阶段机](../architecture/turn-state-machine.md) · [记忆系统](../architecture/memory.md) · [通信契约](../architecture/ipc-contract.md) · [实体注册表](043-rust-entity-registry-impl.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：按 044 交付 Rust 权威在场状态、固定 / 动态角色进入与离开、具名且版本化的观察 / 传闻获知事件、持久化 / 恢复 / 分支过滤及只读有界 evidence-source port。

非目标：不实现角色自主排期、模型生成实体权限、记忆候选解释 / 提交、定向对话选择 UI、记忆专属 IPC；不从自由文本推断观察范围或传闻传播；不回扩 023、025 既有任务。

## 前置条件与待决策

044 必须已评审通过；023 提供有效因果记录 / 恢复边界，043 提供完整稳定的固定 / 动态实体注册表。本任务产出的事实只依赖 engine / record / entity registry，不依赖 aoidos-memory、Tauri 命令或 UI；030 作为下游只读消费方。

## 实施步骤

1. 按 044 将可信 `SceneCatalog.initialPresenceIds` 与 `scriptRevision` 绑定到 session 起始基线；Scene entry rule 的成员快照随 `sceneAdvanced` mutation 建立。未配置初始成员时仅玩家在场；`actor_ids`、Markdown、`entities.json` 与 `entityCreated` 均不自动入场。其余加入 / 离场由 `SettlementPlanned` 中的登记 mutation 承载；presence 状态只按已 applied 且当前有效路径重建，即使后续 round 终态失败也不撤销已提交世界状态。
2. 对 observed 事件绑定角色、显式可见范围、当时场景 revision、触发规则版本及来源记录；scope 必须按 scriptRevision 解析到登记的 SourceRef 范围。同一 settlement mutations 按数组顺序应用，并以发出事件时的临时 presence 状态验证；仅登记规则能发出事件，不从 narration / 同场自动推导。
3. 对 rumor delivery 绑定当前 informant、原始主张来源、当前交付来源、接收角色、传播规则版本和 uncertainty；验证前一跳 recipient 与本跳 informant 相同、root source / scope 完全一致、父事件先于本事件且同 session、无循环，最多 8 跳；默认不自动转发。已登记源头仅证明“某人提出此主张”，不转换为世界事实。
4. 为来源事件提供只读、有界、当前有效因果路径视图；回退排除外分支事件，重开依据 023 完成 applied 恢复。只有成功完整终态、当前有效路径且 mutation 已应用的获知事件能作为 030 证据；presence 重建只要求变更已应用。扫描最多 256 条记录 / 4 MiB；读取 presence snapshot 最多 128 人 / 64 KiB。缺失 / 未知版本失败关闭。
5. 覆盖事务失败、重复应用、分支回退、重启、过期在场 revision、角色离场、观察范围不匹配和传闻链断裂；更新架构依赖图。
6. 候选完成后由 subagent 独立评审，处理发现并复验。

## 预计改动

engine 的 scene presence / trusted-evidence source、023 record facts、043 entity registry 消费接口及共享持久化；不新增 Tauri / TypeScript 命令。030 通过只读端口消费 trusted-evidence source，046 再把 presence snapshot 接入角色选择 UI。

## 验收标准

- [ ] 初始在场与进入 / 离开只由可信规则提交；NPC 注册本身不构成在场，固定 / 动态身份使用同一稳定 ID 约束。
- [ ] presence 仅按 applied / effective path 重建；observed / rumor 作为记忆证据还须来源回合成功完整、mutation 已 applied，恢复补写不会从 partial 创造事件。
- [ ] observed 有可验证范围和触发记录；rumor 每跳保留 informant、原始主张、交付来源、规则版本及不确定性。
- [ ] 缺少主体、场景 revision、规则、因果位置或恢复回执时只读端口失败关闭；presence 最多 128 人 / 64 KiB，来源扫描最多 256 记录 / 4 MiB，传闻最多 8 跳。
- [ ] engine crate 不依赖 memory / Tauri / UI；架构、类型、注释和 044 一致，独立 review 通过。

## 验证计划与结果

覆盖 engine 因果路径和 SQLite 故障 / 恢复测试、有效分支过滤、事件源只读验证及全量 `bun run verify`。

| 日期 | 环境 / 命令                        | 预期                         | 实际结果 |
| ---- | ---------------------------------- | ---------------------------- | -------- |
| —    | 场景 / 恢复 / evidence-source 测试 | 权限、恢复与因果来源边界一致 | 未执行   |
| —    | `bun run verify`                   | 全项通过                     | 未执行   |

## 风险与回退

在场或获知事件若无法沿 023 的有效分支重建，记忆可能将已撤销或无法证实的内容授予角色。读取或恢复不完整时拒绝输出相关证据；不得降级为正文推断或普通目录快照。

## 决策与工作记录

- 2026-10-12：按交叉评审重定范围为 engine 权威状态 / 事件源；不依赖 030、UI 或 Tauri，避免与 030 双向等待。角色选择 UI 改由 046 承接，记忆 IPC 仍归 031。

## 完成摘要

可开始。023、043 与 044 前置均已完成；本任务交付后由 030 消费可信获知事件，046 接入在场快照 UI。
