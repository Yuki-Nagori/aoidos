# 060 — 已开周目选择与安全删除

- 状态：done
- 依赖：013、022、023、029、035、058、059
- 优先级：P1
- 创建 / 更新：2026-10-20 / 2026-10-21

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
- 在 Tauri / Web 与 TUI 中接入选择、账务迁移和删除；键盘与鼠标走同一动作，危险操作须显示可辨认的周目标题和删除影响。
- 删除不发送 LLM 请求，不因列表或预览操作启动周目，不清理 API key / profile 定义。
- 不在本任务重新设计回合、计费或记忆规则；依设计约束挂起中的费用结算和恢复流程。

## 前置条件与待决策

- 059 设计评审通过；目录 migration 和分批旧数据扫描须保持旧存档可读，并标明扫描未完成状态。
- 费用账本身份为 `sessionId`，记忆运行身份为独立 UUID；所有删除查询使用核验后的显式映射，不按 UUID 猜测。legacy scope reconciliation 检查预算、物理请求和 target 精确匹配的 `run-budget-update` 审计，不能因预算行缺失而忽略残留账务审计。
- 新会话与私有记忆请求已经统一使用 session canonical `billingRunId`。历史数据仍可能留有以 `memoryRunId` 为范围的预算和请求；目录发现此 scope 下任何账单数据都必须标记 `billing-reconciliation-required`，不能误报为未配置或可继续收费。迁移旧账本同时查 `sessionId` 和经 `memory_run_sessions` 证明的 `memoryRunId` scope，合并 request / outbox / warnings；预算冲突时阻止收费直至玩家保存新的 canonical 上限。
- 已结算的本局费用明细随玩家确认删除；任意 `reserved` / `unconfirmed` 请求或待结算 outbox 均阻止删除，不得丢弃未知义务。已有完整持久 usage 可走现有本地结算恢复；缺 usage 且无可核验证据时保留该局，本 task 不提供伪造费用或手工改状态的入口。
- 跨周目来源刻痕与动态实体身份按 059 失效；剧本代数高水位、全局价格 / 配置、其他周目与全库备份保留。
- 所有清理操作由共享 app / engine / store 生命周期 API 执行；CLI、Tauri 命令和 UI 不直接操作文件路径或 SQL 级联。

## 实施步骤

1. 建立会话目录、profile 非秘密快照、三身份关联校验、canonical billing run 修复与有界 legacy 扫描；状态不完整或损坏时隔离单条记录。
2. 接入只读分页 / 预览、显式 profile 绑定、切换与恢复；SQLite active-session 是权威，先校验目标再关闭旧 owner，最后更新权威选择并修复文件镜像。
3. 实现可幂等重试的删除 journal、同卷文件隔离、run-scoped SQLite 清理及启动前向恢复；落实未结算费用拒绝规则。
4. Tauri / Web 与 CLI/TUI 通过 `aoidos-app::RunLifecycleService` 调用相同生命周期 API，完成列表、恢复、旧账务核对 / 迁移、确认删除与失败续跑；入口不直接读写周目文件或数据库。
5. 以 SQLite、文件系统和进程中断夹具覆盖旧数据扫描、三身份冲突、profile 缺失、正常删除、费用拒绝、cycle mark 失效、重复请求、部分失败和恢复。
6. 更新架构文档、CLI 使用说明与 task；独立评审并运行仓库验收。

## 预计改动

- `src-rust/aoidos-app`、`src-rust/aoidos-engine`、`src-rust/aoidos-store`、`src-tauri`、`src-cli` 与集成测试。
- 可能新增 app / storage API、SQLite schema migration、删除操作 journal 和 Tauri IPC 类型；以 059 的最终设计为准。
- 同步 `storage.md`、`record-engine.md`、`billing.md`、`memory.md`、`ts-rust-boundary.md`、README / CLI help 与本 task。

## 验收标准

- [x] 列表按固定游标分页，只读校验摘要；legacy 扫描最多处理 128 个遍历项 / 批、单 header 遵守 2 MiB 格式上限且总读取不超过 8 MiB，显式报告不完整与损坏项数量，诊断每轮重置且最多保留 256 项，损坏项隔离而不阻断其他周目。
- [x] 旧 transcript 的 session、memory 与账务身份仅按显式映射核验；缺少 `memory_run_sessions` 映射或映射冲突进入 `needs-repair`，不能经 profile 绑定转为可运行状态；重复扫描与 profile 绑定事务均重新核验已登记身份。
- [x] 新局冻结公开 profile 快照且不含凭据；旧局缺失 profile 时需要玩家明确绑定；切换恢复原剧本 / profile，不自动发送请求。
- [x] TUI 恢复后始终使用该局冻结 profile 做回合与账单预检；当前 profile 的修改或删除不改变已开周目。
- [x] 切换先校验目标，忙碌时拒绝；关闭旧 owner、恢复目标、更新 SQLite 权威活动选择和修复文件镜像顺序符合 059；只对提交结果不确定的错误读回判定，明确 revision / 状态冲突不得被吞；镜像写入失败须在恢复与普通重开结果中报告。
- [x] 删除需要对具体周目显式确认；未决费用义务阻止删除，运行中的周目先安全关闭，直接来源刻痕失效且代数高水位保留。
- [x] 历史账务迁移在额度无冲突时可直接核验；冲突时要求玩家明确提交最终 CNY / USD 上限。Web 与 TUI 均能从目录状态触发迁移，迁移不发送模型请求。
- [x] 缺失完整 usage 的 `unconfirmed` 请求保持不可删除；不把它伪装成 settled、不提供无证据的手工核销。
- [x] transcript、partial、修复备份、memory / entity / applied、billing 和目录关联按设计清理；其它周目、全局价格 / 配置、全库备份不误删。
- [x] 删除 transcript 清单上限为 8,192 项 / 4 MiB，超限在移动前失败；memory run 目录作为单一同卷项隔离，递归清理每批最多处理 128 项且不跟随符号链接。manifest 前缀游标通过独立追加 migration 持久化；中断后从剩余项继续，启动时 journal 只前向完成，已 tombstone 目标不会重新可见或打开。
- [x] 递归清理预算包含每个实际检查的文件和目录；逐项确认设备 / 卷身份，不穿过嵌套挂载点。持久化有界原生路径栈，跨批继续深层目录；单项 journal 恢复失败保留稳定诊断码并隔离该目标，不阻断其他周目启动，成功重试后清除诊断；完成记录压缩为目标身份哈希墓碑，仍支持相同 operation ID 幂等重试。
- [x] Tauri 与 TUI 只消费共享生命周期 API，不直接拼接路径或执行 SQL；依赖图无环。
- [x] Web 目录可恢复、迁移与删除；分批删除未完成时即使关闭确认框仍显示续跑入口，并复用同一 operation ID。
- [x] 确认界面说明全库备份可能保留历史字节，产品不宣称安全擦除。
- [x] 行为测试覆盖列表分页 / 损坏隔离、身份映射缺失、跨批次目录栈上限、诊断重置与存储上限、周目选择、profile 绑定、删除确认 / 拒绝、重复与中断恢复。
- [x] 文档、类型、IPC、task 与实现一致；独立评审问题已处理，验收命令通过。

## 验证计划与结果

截至 2026-10-21，`bun run verify` 全项通过：Web 473 项测试、Rust workspace 覆盖率 94.72%、视觉回归及构建、格式、lint、文档检查均通过。独立评审复核了完整改动，未发现阻断问题；深目录删除通过持久化路径栈跨批推进，恢复诊断码会保存在 journal 并显示于 Web / TUI。PR #191 的共享质量门禁及 Ubuntu、macOS、Windows 验证全部通过；Windows job 用时 20 分 57 秒。

删除链路定向测试覆盖账单门禁、journal、manifest 冻结、owner 关闭、文件隔离、数据库清理和隔离文件清理。集成用例以 130 个 transcript sidecar 验证 128 项游标跨批续跑；store 用例覆盖递归预算、身份变化、重放和符号链接拒绝。数据库测试验证目标账本和刻痕清理、非目标刻痕及剧本代数保留、失败回滚与幂等重试。store / engine 定向测试与 Clippy 通过；PR #191 三平台 CI 全部通过。

## 风险与回退

删除会跨越 SQLite 与文件系统，任一边界失败都可能留下半完成状态；必须先持久化操作意图，再推进可恢复步骤。迁移失败时禁止静默套用其他 profile 或显示为已删除，保留诊断并允许安全恢复。

## 决策与工作记录

- 2026-10-20：从 058 的周目管理缺口拆出本任务，由 059 定稿目录、profile 绑定和安全删除协议。
- 2026-10-21：完成目录 schema、三身份校验、公开 profile 快照、旧局显式绑定与有界 transcript 扫描。账务迁移按已核验的 session / memory 映射合并预算、请求、outbox 和审计；存在冲突时要求玩家提供最终双币种上限。
- 2026-10-21：接入共享恢复服务及 Tauri、Web 类型和 TUI 的扫描 / 恢复入口。恢复使用周目冻结的 profile，不发送模型请求；SQLite 活动选择为权威值，文件镜像失败会显式返回待修复状态。
- 2026-10-21：删除后端已实现费用门禁、幂等 journal、owner 安全关闭、活动选择清理、限界 manifest 冻结、store 文件身份 / 列举 / 隔离与有界递归清理，以及 `files-quarantined → database-purged → files-purged → completed` 阶段。manifest 游标用独立追加 migration 持久化；数据库失败整体回滚，文件清理跨批次可重放，并保留墓碑、全局配置、其他周目和剧本代数高水位。
- 2026-10-21：共享 `RunLifecycleService`、Tauri 删除命令和 TUI 二次确认 / 重试已接入；Tauri 与 CLI 启动时前向恢复未完成 journal。Web 已提供目录分页、旧局扫描、profile 绑定、恢复、删除确认与分批续跑；历史账务冲突时展示双币额度选择。额度无冲突时先尝试无参数迁移，冲突后才要求玩家填写最终上限。TUI 也可从目录触发迁移并按需填写最终双币额度。
- 2026-10-21：PR #191 的共享质量门禁及 Ubuntu、macOS、Windows 检查通过；Windows job 用时 20 分 57 秒。修复了 Windows `OsString::from_wide` 所需的 UTF-16 切片，并将 Unix 原始字节 / Windows UTF-16LE 的本机路径编解码统一移入 `aoidos-store::paths`，保留既有持久化编码格式。

## 完成摘要

实施与验收已完成；目录、旧局扫描、共享恢复 / 删除服务、启动前向恢复、Tauri / Web 周目管理与 TUI 选择 / 恢复 / 账务迁移 / 删除均已接入。独立评审、`bun run verify` 与 PR #191 三平台 CI 均通过。
