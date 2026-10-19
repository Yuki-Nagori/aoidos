# 055 — 实现：记忆检索与共用搜索输入

- 状态：planned
- 依赖：031、034、052、053
- 优先级：P2
- 创建 / 更新：2026-10-19 / 2026-10-19

## 目标与背景

实现 [053](053-memory-query-search-design.md) 定稿的全量有界记忆检索，并在 Vue `shared/` 提供无领域语义的受控搜索输入组件。搜索结果必须覆盖当前周目固定快照的所有页，不得只过滤已加载列表或泄露来源正文。

## 对照设计

`aoidos-art` 基线 `1f19504ebc7390b7d9954b1c597fc3d98b6b7fde` 中的 `memory-list.html`、`memory-pending.html`、`settings.html#memory-list` 与快捷键速查筛选是视觉 / 操作参考。检索语义仍以 053 的冻结协议为准：全量有界快照、明确未完成游标、仅匹配获准字段。设置页候选确认继续复用既有 API；不能为匹配截图伪造结果或扩大返回字段。

## 必读

[053 记忆检索设计](053-memory-query-search-design.md) · [记忆架构](../architecture/memory.md) · [通信契约](../architecture/ipc-contract.md) · [界面架构](../architecture/ui-shell.md) · [分层规范](../standards/layering.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：SQLite 派生检索投影及迁移 / 修复、记忆领域查询与 engine 可信来源校验、Tauri 命令和 TypeScript 同型类型、shared 搜索输入、memory feature 查询状态与本地化界面及关键行为测试。

非目标：不修改记忆事实、别名授权、来源正文存储、普通条目分页语义；不实现拼音 / 模糊 / 语义向量搜索、跨周目全局检索或后台轮询。

## 前置条件与待决策

053 的查询字段、归一化、硬预算、cursor 绑定及错误语义为冻结输入。实现前检查现有 SQLite FTS / 索引依赖、memory schema migration 和同事务操作边界；实现选择不得改变对外字面子串语义或放宽 053 预算。新 schema 通过已有迁移版本追加，旧库可从当前有效版本重建检索投影；不可验证数据时失败关闭并保留诊断。

## 实施步骤

1. 在 `aoidos-memory` 实现类型化 query / filters / cursor 与版本化派生投影；在目标记忆版本、状态或已确认别名变更时原子维护 `searchRevision`。
2. 实现独立 entry / name 投影及双层 cursor；每页最多扫描 256 个 entry 行、1024 个名称行、读取 256 KiB 原始名称文本 / 1 MiB 原始投影字节 / 4 MiB 归一化工作字节，最多返回 32 项、单项 JSON ≤32 KiB、完整响应 ≤64 KiB。读取字符串前用 `length(CAST(value AS BLOB))` 核对 UTF-8 字节数；合法最大转义摘要必须能进入单项响应。超限损坏返回 `store.corrupt`。cursor 固定 run、history / search revision、query / filters hash、两类 rowid 上界与位置。详情读取仍走现有来源有效性核验。
3. 经 `aoidos-engine` 完成历史修订与来源边界校验，在 Tauri 新增 `memory_search_entries`，并同步 IPC 文档和 `src-web/api/memory.ts`。
4. 实现 `src-web/shared/components/SearchInput.vue` 的受控输入 / label / clear / 焦点行为；memory feature 自行持有防抖、筛选、分页、generation、加载 / 错误 / 空态。
5. 补充迁移、游标失效、隐私、Unicode、分页续扫、迟到响应及 IME 行为测试；更新架构与本 task，再由 subagent 独立评审并处理发现。

## 预计改动

`src-rust/aoidos-memory/`、`src-rust/aoidos-engine/`、`src-tauri/src/memory_commands/`、`src-web/api/memory.ts`、`src-web/shared/components/`、`src-web/features/memory/`、`locales/`、对应 Rust / Vitest 测试；同步 `ai-docs/architecture/memory.md`、`ipc-contract.md`、`ui-shell.md`、`architecture/README.md` 和本 task。

## 验收标准

- [ ] Rust search 只返回当前有效 run / revision 中 active / archived 当前版本；坏来源、unavailable 和损坏条目不绕过原详情校验；Unicode 小写膨胀、BLOB 字节长度和最大合法 JSON 单项均有测试。
- [ ] 过滤覆盖全快照并可分页；entry / name 行、名称文本、总读取字节和 JSON 响应分别遵守 053 固定预算；响应预算不足时续页无遗漏 / 无重复，零命中但仍有游标时明确未完成。
- [ ] 新增 / 更新 / 别名和分支变化让旧游标失效；跨 run、修订、query / filters 的游标拒绝续读；名称扫描中途续页不会跳过别名。
- [ ] 搜索只匹配批准字段，不返回素材正文、证据正文、路径或原始响应；索引迁移失败关闭，旧库兼容、超长损坏投影与读取字节预算有测试。
- [ ] Rust / Tauri / TypeScript 载荷同型；错误与 053 一致；共享输入不承载请求、分页、筛选或结果语义。
- [ ] 搜索变更丢弃旧 UI 结果，空结果与未扫完状态区分，组件卸载清理 timer；IME 交互和可访问名称有行为测试。
- [ ] 本 task 的代码、测试、类型、注释和架构文档同步；subagent 独立评审通过；最终 `bun run verify` 全项通过。

## 验证计划与结果

待实施后填写：定向 Rust / SQLite migration 测试、Tauri IPC DTO 测试、Vitest、受影响检查和完整 `bun run verify`。

## 风险与回退

检索投影属于派生数据，可从可信当前版本重建；重建不改变已发布版本、操作回执或正文文件。若索引迁移无法校验，禁用搜索并保留普通只读查询 / 诊断，不能回退为仅搜客户端已加载页。

## 决策与工作记录

- 2026-10-19：根据 053 定稿登记实现。用 SQLite 派生投影和有界扫描避免逐页读取正文文件；共享组件只管理输入交互，领域查询仍归 memory feature。

## 完成摘要

未开始。
