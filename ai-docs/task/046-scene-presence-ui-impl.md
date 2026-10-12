# 046 — 实现：在场角色选择界面

- 状态：planned
- 依赖：025、030、034、043、044、045
- 优先级：P1
- 创建 / 更新：2026-10-12 / 2026-10-12

## 目标与背景

045 提供 Rust 权威的当前场景在场快照后，在 composer 中让玩家选择对话接收者，并把有界 snapshot revision 与接收者一并提交给 030 已交付的正式入口。该界面只消费可信快照，不从实体目录、剧本文字或生成内容推测谁在场。

## 必读

[场景角色在场与定向交互设计](044-scene-presence-dialogue-design.md) · [场景在场与可信获知事件](045-scene-presence-dialogue-impl.md) · [记忆门控与批次](030-memory-gates-batches-impl.md) · [界面交互](../architecture/ui-shell.md) · [通信契约](../architecture/ipc-contract.md) · [国际化](../architecture/i18n.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：读取有界、版本化的角色选项；在 composer 提供旁白和当前在场角色选择；提交时携带 `expectedPresenceRevision`，服务端在追加玩家输入前拒绝 stale / absent / unknown 目标；处理 session、scene、presence revision、phase 变化和旧请求响应。

非目标：不建立或修改角色在场状态、不生成 NPC、不判定角色是否观察 / 获知、不实现记忆专属查询 / 批次 IPC、不改变 025 的既有界面基线。

## 前置条件与待决策

044 已评审，045 已提供 authority-backed presence snapshot；030 的 `recipientCharacterId` 路由已实现并经验证。025 作为已实现组件消费，034 提供中英文资源；不继承 025 尚待进行的跨平台实机视觉验收结论。

## 实施步骤

1. 调用 `engine_get_scene_presence`，读取仅含当前场景在场、完整目录登记的 Character 快照及不透明 revision；遵守 128 人 / 64 KiB 上限，缺失或恢复不完整时不展示角色选项。
2. 在 composer 提供旁白默认和显式角色选择；选择绑定 session、scene 与 snapshot revision，revision 改变时清理失效选择。
3. 在 `engine_submit_input` 请求中传入 `recipientCharacterId` 和 `expectedPresenceRevision`；Rust 校验当前 revision / 在场身份，成功接纳时将二者写入 `RoundAccepted`，恢复 / 重生成沿用原值；过期或未知选择必须在任何输入记录写入前拒绝。
4. 处理切换 session、scene、phase、presence snapshot 与并发响应；已接纳回合目标按 023 checkpoint 恢复规则保留，不让旧快照覆盖新状态。
5. 补齐简中 / 英文文案、Rust / TS DTO、IPC 契约、架构依赖图和可重复的竞态 / 故障测试。
6. 候选完成后由 subagent 独立评审，处理发现并复验。

## 预计改动

Tauri `game_commands`、`src-web/api/engine.ts`、composer UI、英文与简中文案及相应测试；Rust 只消费并复核 045 的快照，不将 UI 提升为状态来源。

## 验收标准

- [ ] 角色选项只来自当前在场且完整登记的 Character；目录身份本身不显示为可交互对象。
- [ ] 提交携带 snapshot revision 并与目标一起持久写入 `RoundAccepted`，恢复 / 重生成保留该值；stale / absent / unknown 目标在玩家输入追加前失败。
- [ ] scene / session / phase / revision 更新会清理当前未提交选择并丢弃旧异步响应；已接纳目标按回合恢复规则保持。
- [ ] 旁白默认、中英文显示、角色切换、过期选择、离场、并发响应与服务端拒绝均有直接测试。
- [ ] Rust / Tauri / TS 契约、文档与 044 / 045 一致；独立 review 通过；`bun run verify` 全项通过。

## 验证计划与结果

覆盖引擎输入前 CAS、Tauri IPC MockRuntime、composer 状态竞态 / 本地化和全量 `bun run verify`。

| 日期 | 环境 / 命令       | 预期               | 实际结果 |
| ---- | ----------------- | ------------------ | -------- |
| —    | UI / IPC 定向测试 | stale 选择不写输入 | 未执行   |
| —    | `bun run verify`  | 全项通过           | 未执行   |

## 风险与回退

presence 快照不可用或恢复未完成时，隐藏角色选项并保留旁白；不可使用旧快照、实体目录或本地缓存绕过 Rust 的 revision 校验。

## 决策与工作记录

- 2026-10-12：从原 045 拆出产品 UI，令 045 只负责可信引擎状态 / 事件源，从而保持 043 → 045 → 030 → 046 单向依赖。

## 完成摘要

未开始。等待 044、045 及 030 目标后端满足。
