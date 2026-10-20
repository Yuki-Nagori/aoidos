# 060 — 已开周目选择与安全删除

- 状态：planned
- 依赖：013、022、023、029、035、058、059
- 优先级：P1
- 创建 / 更新：2026-10-20 / 2026-10-20

## 目标与背景

按[059 已开周目管理设计](059-run-management-design.md)实现桌面端与 CLI/TUI 共用的周目目录、选择 / 恢复和安全删除流程。目录以 `sessionId` 为主身份，显式保存独立的 `memoryRunId` 与 canonical `billingRunId`；历史私有记忆请求可能错误地使用 `memoryRunId` 作为账本范围，须按确切 session 绑定迁移并合并。历史缺少模型配置的周目必须显式绑定，不能套用当前 profile。玩家可辨认并打开以前的周目，确认目标后清除本地周目数据；费用仍有未决义务时拒绝删除，跨周目刻痕、账本和备份按 059 处理。

## 必读

- [059 已开周目管理设计](059-run-management-design.md)
- [Rust / Web / 测试依赖方向](../standards/layering.md)
- [存储架构](../architecture/storage.md)
- [记录引擎](../architecture/record-engine.md)
- [回合状态机](../architecture/turn-state-machine.md)
- [计费架构](../architecture/billing.md)
- [记忆架构](../architecture/memory.md)
- [Rust 规范](../standards/rust.md)
- [测试规范](../standards/testing.md)

## 范围与非目标

- 实现有界周目列表、摘要读取、选择 / 恢复和每周目的 profile 冻结关联。
- 实现 059 决定的显式删除确认、共享生命周期服务、关联数据清理及单向前进的可恢复操作 journal。
- 在 Tauri 与 TUI 中接入选择 / 删除；键盘与鼠标走同一动作，危险操作须显示可辨认的周目标题和删除影响。
- 删除不发送 LLM 请求，不因列表或预览操作启动周目，不清理 API key / profile 定义。
- 不在本任务重新设计回合、计费或记忆规则；依设计约束挂起中的费用结算和恢复流程。

## 前置条件与待决策

- 059 设计评审通过；目录 migration 和分批旧数据扫描须保持旧存档可读，并标明扫描未完成状态。
- 费用账本身份为 `sessionId`，记忆运行身份为独立 UUID；所有删除查询使用核验后的显式映射，不按 UUID 猜测。
- 所有新私有记忆请求统一使用 session canonical `billingRunId`。迁移旧账本同时查 `sessionId` 和经 `memory_run_sessions` 证明的 `memoryRunId` scope，合并 request / outbox / warnings；预算冲突时阻止收费直至玩家保存新的 canonical 上限。
- 已结算的本局费用明细随玩家确认删除；任意 `reserved` / `unconfirmed` 请求或待结算 outbox 均阻止删除，不得丢弃未知义务。已有完整持久 usage 可走现有本地结算恢复；缺 usage 且无可核验证据时保留该局，本 task 不提供伪造费用或手工改状态的入口。
- 跨周目来源刻痕与动态实体身份按 059 失效；剧本代数高水位、全局价格 / 配置、其他周目与全库备份保留。
- 所有清理操作由共享 app / engine / store 生命周期 API 执行；CLI、Tauri 命令和 UI 不直接操作文件路径或 SQL 级联。

## 实施步骤

1. 建立会话目录、profile 非秘密快照、三身份关联校验、canonical billing run 修复与有界 legacy 扫描；状态不完整或损坏时隔离单条记录。
2. 接入只读分页 / 预览、显式 profile 绑定、切换与恢复；SQLite active-session 是权威，先校验目标再关闭旧 owner，最后更新权威选择并修复文件镜像。
3. 实现可幂等重试的删除 journal、同卷文件隔离、run-scoped SQLite 清理及启动前向恢复；落实未结算费用拒绝规则。
4. Tauri 与 CLI/TUI 接入相同 typed API，完成列表、恢复、确认删除与失败反馈。
5. 以 SQLite、文件系统和进程中断夹具覆盖旧数据扫描、三身份冲突、profile 缺失、正常删除、费用拒绝、cycle mark 失效、重复请求、部分失败和恢复。
6. 更新架构文档、CLI 使用说明与 task；独立评审并运行仓库验收。

## 预计改动

- `src-rust/aoidos-app`、`src-rust/aoidos-engine`、`src-rust/aoidos-store`、`src-tauri`、`src-cli` 与集成测试。
- 可能新增 app / storage API、SQLite schema migration、删除操作 journal 和 Tauri IPC 类型；以 059 的最终设计为准。
- 同步 `storage.md`、`record-engine.md`、`billing.md`、`memory.md`、`ts-rust-boundary.md`、README / CLI help 与本 task。

## 验收标准

- [ ] 列表按固定游标分页，只读校验摘要；legacy 扫描最多处理 128 个遍历项 / 批、单 header 遵守 2 MiB 格式上限且总读取不超过 8 MiB，显式报告不完整，损坏项隔离而不阻断其他周目。
- [ ] 新局冻结公开 profile 快照且不含凭据；旧局缺失 profile 时需要玩家明确绑定；切换恢复原剧本 / profile，不自动发送请求。
- [ ] 切换先校验目标，忙碌时拒绝；关闭旧 owner、恢复目标、更新 SQLite 权威活动选择和修复文件镜像顺序符合 059，提交不确定可读回判定。
- [ ] 删除需要对具体周目显式确认；未决费用义务阻止删除，运行中的周目先安全关闭，直接来源刻痕失效且代数高水位保留。
- [ ] 缺失完整 usage 的 `unconfirmed` 请求保持不可删除；不把它伪装成 settled、不提供无证据的手工核销。
- [ ] transcript、partial、修复备份、memory / entity / applied、billing 和目录关联按设计清理；其它周目、全局价格 / 配置、全库备份不误删。
- [ ] 删除 transcript 清单上限为 8,192 项 / 4 MiB，超限在移动前失败；memory run 目录作为单一同卷项隔离，递归清理每批最多遍历 128 项且不跟随符号链接；删除过程幂等、可崩溃恢复，启动时 journal 只前向完成，已 tombstone 目标不会重新可见或打开。
- [ ] Tauri 与 TUI 只消费共享生命周期 API，不直接拼接路径或执行 SQL；依赖图无环。
- [ ] 确认界面说明全库备份可能保留历史字节，产品不宣称安全擦除。
- [ ] 行为测试覆盖列表分页 / 损坏隔离、周目选择、profile 绑定、删除确认 / 拒绝、重复与中断恢复。
- [ ] 文档、类型、IPC、task 与实现一致；独立评审问题已处理，验收命令通过。

## 验证计划与结果

| 日期 | 环境 / 命令      | 预期                                | 实际结果 |
| ---- | ---------------- | ----------------------------------- | -------- |
| —    | 待设计通过后登记 | 依照 059 和 affected selectors 验证 | 未执行   |

## 风险与回退

删除会跨越 SQLite 与文件系统，任一边界失败都可能留下半完成状态；必须先持久化操作意图，再推进可恢复步骤。迁移失败时禁止静默套用其他 profile 或显示为已删除，保留诊断并允许安全恢复。

## 决策与工作记录

- 2026-10-20：作为 058 TUI 设置缺口审查发现的独立生命周期功能创建；先由 059 定稿数据源、profile 绑定与安全删除协议。

## 完成摘要

未开始。依赖 059 设计定稿后实施。
