# 060 — 已开周目选择与安全删除

- 状态：in-progress
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
- 在 Tauri 与 TUI 中接入选择 / 删除；键盘与鼠标走同一动作，危险操作须显示可辨认的周目标题和删除影响。
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
4. Tauri 与 CLI/TUI 通过 `aoidos-app::RunLifecycleService` 调用相同生命周期 API，完成列表、恢复、确认删除与失败反馈；入口不直接读写周目文件或数据库。
5. 以 SQLite、文件系统和进程中断夹具覆盖旧数据扫描、三身份冲突、profile 缺失、正常删除、费用拒绝、cycle mark 失效、重复请求、部分失败和恢复。
6. 更新架构文档、CLI 使用说明与 task；独立评审并运行仓库验收。

## 预计改动

- `src-rust/aoidos-app`、`src-rust/aoidos-engine`、`src-rust/aoidos-store`、`src-tauri`、`src-cli` 与集成测试。
- 可能新增 app / storage API、SQLite schema migration、删除操作 journal 和 Tauri IPC 类型；以 059 的最终设计为准。
- 同步 `storage.md`、`record-engine.md`、`billing.md`、`memory.md`、`ts-rust-boundary.md`、README / CLI help 与本 task。

## 验收标准

- [ ] 列表按固定游标分页，只读校验摘要；legacy 扫描最多处理 128 个遍历项 / 批、单 header 遵守 2 MiB 格式上限且总读取不超过 8 MiB，显式报告不完整与损坏项数量，诊断每轮重置且最多保留 256 项，损坏项隔离而不阻断其他周目。
- [ ] 旧 transcript 的 session、memory 与账务身份仅按显式映射核验；缺少 `memory_run_sessions` 映射或映射冲突进入 `needs-repair`，不能经 profile 绑定转为可运行状态；重复扫描与 profile 绑定事务均重新核验已登记身份。
- [ ] 新局冻结公开 profile 快照且不含凭据；旧局缺失 profile 时需要玩家明确绑定；切换恢复原剧本 / profile，不自动发送请求。
- [ ] TUI 恢复后始终使用该局冻结 profile 做回合与账单预检；当前 profile 的修改或删除不改变已开周目。
- [ ] 切换先校验目标，忙碌时拒绝；关闭旧 owner、恢复目标、更新 SQLite 权威活动选择和修复文件镜像顺序符合 059；只对提交结果不确定的错误读回判定，明确 revision / 状态冲突不得被吞；镜像写入失败须在恢复与普通重开结果中报告。
- [ ] 删除需要对具体周目显式确认；未决费用义务阻止删除，运行中的周目先安全关闭，直接来源刻痕失效且代数高水位保留。
- [ ] 缺失完整 usage 的 `unconfirmed` 请求保持不可删除；不把它伪装成 settled、不提供无证据的手工核销。
- [ ] transcript、partial、修复备份、memory / entity / applied、billing 和目录关联按设计清理；其它周目、全局价格 / 配置、全库备份不误删。
- [ ] 删除 transcript 清单上限为 8,192 项 / 4 MiB，超限在移动前失败；memory run 目录作为单一同卷项隔离，递归清理每批最多遍历 128 项且不跟随符号链接；删除过程幂等、可崩溃恢复，启动时 journal 只前向完成，已 tombstone 目标不会重新可见或打开。
- [ ] Tauri 与 TUI 只消费共享生命周期 API，不直接拼接路径或执行 SQL；依赖图无环。
- [ ] 确认界面说明全库备份可能保留历史字节，产品不宣称安全擦除。
- [ ] 行为测试覆盖列表分页 / 损坏隔离、身份映射缺失、跨批次目录栈上限、诊断重置与存储上限、周目选择、profile 绑定、删除确认 / 拒绝、重复与中断恢复。
- [ ] 文档、类型、IPC、task 与实现一致；独立评审问题已处理，验收命令通过。

## 验证计划与结果

| 日期       | 环境 / 命令                                                                                                                  | 预期                                                             | 实际结果                                                                                                                                                                                                                    |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-21 | `bun run test:affected`、`format:affected`、`lint:affected`                                                                  | 生命周期、恢复冻结配置、typed IPC 与消费端契约保持一致           | Rust workspace 通过，aoidos-engine 524 项通过 / 3 项显式忽略，aoidos-cli 27 项通过；466 项 Vitest 通过；格式、分层、i18n、Clippy 与 Web 构建通过                                                                            |
| 2026-10-21 | `bun scripts/cargo.mts test -p aoidos-engine deletion_check`                                                                 | 只读删除门禁核验 revision、身份映射及所有已知账本范围            | 2 项定向测试通过；确认 canonical 未结账单、legacy 未确认请求和结算 outbox 均被检查；未启动删除或写入 journal                                                                                                                |
| 2026-10-21 | `bun scripts/cargo.mts test -p aoidos-engine deletion_request`                                                               | 删除意图与目录封锁同事务提交；重复操作幂等且未结账单不写 journal | 3 项定向测试通过；目录更新失败时 journal 与 revision 均回滚；尚未接入 app 入口                                                                                                                                              |
| 2026-10-21 | `bun scripts/cargo.mts test -p aoidos-engine clearing_`                                                                      | owner 关闭后只清除目标活动选择；失败回滚且不影响非活动目标       | 3 项定向测试通过；SQLite 选择清除与目录 revision 原子更新，journal 保持 `requested` 直至 manifest/hash 冻结；文件镜像和 app 生命周期接入仍未实现                                                                            |
| 2026-10-21 | `bun scripts/cargo.mts test -p aoidos-engine run_catalog::tests`                                                             | 清单路径、身份映射、活动选择、哈希与 journal 阶段原子核验        | 28 项目录测试通过；4 项清单冻结测试覆盖条目上限、非目标活动周目、超范围路径、SHA-256 envelope 持久化和事务失败回滚；aoidos-engine Clippy 通过；尚无可信文件枚举器、镜像清理与前向执行器                                     |
| 2026-10-21 | `bun run test:affected`                                                                                                      | 验证文件系统身份原语及受影响 workspace 行为                      | macOS workspace Rust 测试与 466 项 Vitest 全通过；`bun run lint:affected`、`bun run format:affected` 通过；Windows / Linux 尚待 CI 复验；文件 manifest 枚举与隔离接线仍未实现                                               |
| 2026-10-21 | `bun scripts/cargo.mts test --locked -p aoidos-store filesystem::tests`                                                      | 有界列举原生直接名称并暴露清单是否完整                           | macOS 文件系统测试与 store Clippy 通过；链接名保留供领域层筛选，目标身份单独严格校验；Windows / Linux 尚待 CI 复验；遍历是路径式非原子操作，需持生命周期锁并在移动前复核身份                                                |
| 2026-10-21 | `bun scripts/cargo.mts test -p aoidos-engine deletion::tests`                                                                | 筛选目标周目条目并处理缺席的 memory run 目录                     | macOS 3 项定向测试、engine Clippy 与格式检查通过；无关链接和目录及缺席 memory 目录有本机回归；Linux 专测覆盖非 UTF-8 无关名称；app 生命周期与 SQLite 冻结接线仍未实现                                                       |
| 2026-10-21 | `bun scripts/cargo.mts test --locked -p aoidos-engine game::product::tests`                                                  | 验证活动选择权威状态与 Product 安全关闭、冻结流程                | 22 项通过、3 项长时探索显式忽略；已初始化的空 SQLite 选择不会回退到兼容镜像；活跃 coordinator lease 下拒绝且不登记 journal；活动周目关闭后清理镜像并冻结至 `owner-closed`；镜像清理失败后可用原 operationId 重试            |
| 2026-10-21 | `bun scripts/cargo.mts test -p aoidos-engine storage_freezes_manifest_from_registered_run_and_keeps_absent_memory_directory` | 通过 Storage 数据根将受信任枚举结果冻结到删除 journal            | 1 项集成路径测试通过；文件枚举不持 SQLite 锁，冻结事务推进到 `owner-closed`，无记忆正文时 manifest 身份为 `null`，打开中的 Records writer 会被拒绝；Storage 原语已由 Product 生命周期入口串行协调；该测试只覆盖存储冻结路径 |

## 风险与回退

删除会跨越 SQLite 与文件系统，任一边界失败都可能留下半完成状态；必须先持久化操作意图，再推进可恢复步骤。迁移失败时禁止静默套用其他 profile 或显示为已删除，保留诊断并允许安全恢复。

## 决策与工作记录

- 2026-10-20：作为 058 TUI 设置缺口审查发现的独立生命周期功能创建；先由 059 定稿数据源、profile 绑定与安全删除协议。
- 2026-10-21：实现 SQLite 周目目录 schema、活动选择行、删除 journal 基础结构和 revision 绑定 keyset 页面；启动校验逐项比对表与索引定义。新局目录记录独立的 session / memory 身份、canonical session 账本身份与不含凭据的公开 profile 快照。补充旧局 profile 绑定 CAS：只有玩家显式指定 profile 且目录 revision 匹配时才保存公开快照；发现关联的 legacy 账本数据时保持计费 reconciliation 门关闭。目录遇到尚未迁移的 memory-run 账单 scope 时同样标记待核对；SQLite 活动选择是权威值，重启时据此修复活动文件镜像。旧数据发现、恢复 / 切换、费用迁移、删除执行器及两端界面仍未实现。
- 2026-10-21：新增由玩家显式调用的 legacy transcript 扫描入口，读取 header 而不打开或修复会话；每批遍历上限 128 项、header 总读取上限 8 MiB，单行遵守 2 MiB 格式限制。有效身份按 header 登记，旧局先进入 profile 绑定状态；之后由绑定事务检查 legacy 账本 scope。损坏、符号链接和身份不匹配写入不含路径正文的哈希诊断，列表返回未完成标记与诊断数。Rust 扫描测试覆盖损坏隔离及 130 项跨批次发现。Tauri 命令和 TS API 已接线，产品界面、CLI/TUI 调用、切换 / 恢复、账务迁移及删除流程仍未实现。
- 2026-10-21：独立 review 后将遍历目录改为深度最多 3 层的 DFS 栈，目录打开与 header 读取均复核文件类型和规范路径；header 读取移到数据库互斥锁外，Tauri 扫描命令改由 blocking 执行器承载。每次扫描先清除上一轮诊断并设置未完成标记，诊断表最多保留 256 项，缺失 memory 身份映射的 transcript 进入 `needs-repair`。新增覆盖缺失映射、扫描重置、诊断上限和目录栈深度的测试。
- 2026-10-21：重复扫描现有目录行时重新核对 `memory_run_sessions`、`memory_runs.script_id` 与目录中的三身份关联；映射缺失或损坏会持续保持 `needs-repair`，并在每轮诊断重置后重新报告。新增缺失映射重扫和已登记映射被破坏的回归测试；旧数据扫描相关 8 项测试通过。
- 2026-10-21：独立复审发现 profile 绑定仍信任扫描时缓存的 memoryRunId，以及 header 元数据变化可能覆盖 `deleting` 状态。绑定事务现重新核验 session→memory→script、canonical billing 身份；映射异常会先持久化 `needs-repair` 再拒绝绑定。扫描器在比较 header 前保护删除状态。新增两项回归测试，目录与扫描定向测试 21 项通过。
- 2026-10-21：legacy scope reconciliation 现同时检查预算、物理请求与 target 精确匹配的 `run-budget-update` 审计行；即使预算已不存在，残留审计仍会阻止绑定后进入可收费状态。按 operation kind 收窄查询，避免把 profile / price 审计误认成本局账务。新增 audit-only 历史 scope 回归测试。
- 2026-10-21：核对并修复新请求仍以 `memoryRunId` 计费的问题。普通周目冻结和私有记忆请求现在都使用 `sessionId` / canonical `billingRunId`；回归测试分别确认两条新请求都落入 session 账本，以及旧 scope 有预算时必须先完成核对。历史 scope 迁移仍待实现。
- 2026-10-21：在账本预留事务加入 legacy scope 门禁。仅依据已核验的 `sessionId → memoryRunId` 映射检查旧范围；预算、物理请求或预算变更审计尚存时拒绝新的收费尝试并返回本地化错误码。既有 requestId 的幂等重放先于门禁检查；新增回归确认旧 scope 有零额度预算时不会创建 canonical 预留。
- 2026-10-21：新增 engine 事务内的 legacy 账务迁移原语，并由目录 CAS 操作核验 session / memory / script 映射后同事务更新计费状态。迁移保留 requestId 与 pending settlement，按币种合并提醒且未读状态优先；预算冲突需调用方提供玩家选定的最终双币种上限，否则完整回滚。共享 app service、Tauri 命令及 Web typed API 已接入，并验证 IPC 仅接受精确十进制字符串；产品界面与 CLI/TUI 操作入口尚未接入。
- 2026-10-21：接入显式周目恢复后端：按目录 revision、三身份映射和冻结 profile 校验目标；恢复 transcript 与记忆 owner 后才 CAS 更新 SQLite 活动选择，不发送模型请求。冻结 profile 的 provider / proxy 凭据按引用从当前凭据库读取，不用同 ID 的现行 profile 覆盖历史快照。Tauri 命令和 Web 类型已接入；文件镜像写入失败由 `selectionMirrorPending` 报告，SQLite 权威选择不回滚。扫描与恢复定向测试补齐；CLI/TUI 选择入口、Web 选择界面、删除和启动前向恢复仍未实现。
- 2026-10-21：独立复审后收紧活动选择提交错误处理：仅 SQLite commit 结果不确定时按权威行读回，revision / 状态冲突不会因目标原本已选中而被误判成功。普通重开同样传播镜像待修复状态，并新增该连续恢复路径测试。
- 2026-10-21：显式恢复统一经 `RunLifecycleService` 暴露给 Tauri 与 CLI/TUI；恢复结果带回无凭据的冻结公开 profile，CLI 费用操作使用 canonical `sessionId` 账本范围。TUI 新增已开周目分页、旧存档显式扫描及恢复入口；选择器回归测试确认恢复不调用 Provider。桌面选择界面、迁移 UI、安全删除与启动恢复仍待实现。
- 2026-10-21：修复 TUI 恢复后错误依赖现行 profile 的问题；目录恢复与普通重开均取得周目冻结快照，费用预检和回合请求都使用同一份模型参数，已保存配置修改或删除不影响正在进行的周目。目录项改为单行裁切，鼠标定位与滚动保持一条摘要对应一行；翻页只在成功读取后更新历史游标。存储文档、IPC 命令表和 `selectionMirrorPending` 类型说明同步更新。
- 2026-10-21：新增只读删除计费检查，由共享 app 生命周期服务调用；engine 先按目录 revision 核验 canonical `sessionId`、memory 映射与剧本，再让 billing ledger 统计 canonical 和已核验 legacy scope 的未结请求及 pending settlement。损坏身份、repair / deleting 状态与过期 revision 均失败关闭。此接口不关闭 owner、不生成删除 journal，也不代表删除已可执行；隔离清单、关联数据清理、生命周期串行化和前向恢复仍待完成。
- 2026-10-21：补齐 engine 内部删除意图事务原语：在 `IMMEDIATE` 事务内重新检查 revision / 三身份 / 账单义务，幂等登记 `requested` journal 并将目录状态置为 `deleting`；操作 ID 重放返回现有状态，其他操作不能并发占用同一周目，未结义务不写入任何状态。该原语暂不暴露给 app，待生命周期互斥、owner 安全关闭、冻结 manifest 与前向执行器接通后再开放。
- 2026-10-21：新增 owner 关闭后的 SQLite 活动选择清理原语。调用方完成生命周期门与 owner 关闭后，engine 在单个 `IMMEDIATE` 事务中仅当权威活动选择指向目标时清空选择并递增目录 revision；删除 journal 保持 `requested`，直到 manifest 与 hash 冻结后才能进入 `owner-closed`。测试覆盖目标选择清除、失败回滚、幂等重放和删除非活动周目时保留另一周目的活动选择。该原语不能证明 app owner 已关闭，也不清理 `active-session.json` 镜像；共享生命周期接线前必须一并解决这些边界，删除仍未开放。
- 2026-10-21：新增删除清单冻结原语，清单严格限定在目标剧本 / session / memoryRunId 路径，要求 transcript 与 memory 目录各一项、旁车 UUID 格式正确、来源有序且 quarantine 路径由 operationId 确定；transcript 与 sidecar 合计最多 8,192 项，memory 目录另计一项，完整 envelope 最多 4 MiB。`owner-closed` 阶段将清单及其 SHA-256 摘要 envelope 同事务保存，并复核目录处于 deleting、记忆映射有效且 SQLite 活动选择不再指向目标；哈希覆盖清单规范字节，因此无需为单独哈希列增加数据库迁移。测试覆盖上限边界、保留非目标活动周目、哈希持久化、越界路径和事务失败回滚。实际磁盘枚举和内容身份计算、活动文件镜像清理、隔离 / 清理执行器及 app 生命周期协调仍未接入，删除仍不可用。
- 2026-10-21：在 `aoidos-store` 增加跨平台文件系统对象身份摘要：Unix 使用 `dev` / `ino`，Windows 通过打开末级时启用 `OPEN_REPARSE_POINT` 读取 volume / file ID；公共接口校验数据根内的相对组件，平台分支留在 store。文件或目录改名后摘要保持稳定，便于恢复时确认 quarantine 项仍是冻结对象；摘要不是内容校验，对象删除后 ID 可能复用，因此不跨删除周期复用。路径检查与调用不是原子操作，实际生命周期接线须持锁并在 rename 后复核。定向测试覆盖普通文件和目录改名稳定、同内容不同文件身份不同及链接组件拒绝。该原语尚未接入 engine 枚举、冻结或 rename 恢复，不能据此启用删除。
- 2026-10-21：在同一 store 文件系统模块增加有界的直接名称列举，保留原生文件名，并用 `complete` 明确表示目录是否完整；此层不检查条目类型，领域层筛出目标后再调用类型化身份接口。它只提供路径式遍历，不提供原子目录句柄快照，使用方须持生命周期锁并在移动前复核对象身份。测试覆盖刚好到达上限、超过上限及保留链接名称。
- 2026-10-21：engine 增加目标周目 manifest 构造器，扫描 transcript 共用目录并只纳入该 session 的主 JSONL、UUID partial、recovery backup 与单个 memory run 目录；未知同周目旁车和不完整扫描失败关闭，文件与目录身份由 store 的类型化接口读取。目录扫描上限固定为 65,536 项；构造器要求调用方先完成生命周期锁、owner 关闭与活动镜像清理，尚未接入 `Storage` / app 冻结事务。
- 2026-10-21：独立 review 发现无关周目的异常条目不应阻塞目标清理，且空记忆周目可能尚无磁盘目录。store 的有界枚举现只返回原生直接名称，engine 先用 ASCII session 前缀筛选目标再校验其类型 / 身份；目标旁车仍严格拒绝，扫描上限仍计入所有直接项。memory 目录缺席时在 manifest 中保留路径并记录 `null` 身份，后续执行必须确认其仍缺席才可跳过。回归覆盖 macOS 可验证的 symlink、子目录和缺席目录，以及 Linux 文件系统支持的非 UTF-8 无关名称。
- 2026-10-21：`Storage` 现在持有可信数据根并组合“SQLite 读取目标身份 → 锁外枚举文件 → 事务内复核并冻结 manifest”，避免数据库锁跨越文件 I/O。冻结原语降为 engine crate 内部接口，公开 Storage 方法不接受调用方拼出的路径或清单；Records 仍登记着目标 writer 时拒绝冻结。集成测试用真实登记的周目确认打开中的 writer 被拒绝，关闭并清除活动选择后，无记忆正文目录会以 `owner-closed` 原子保存且 manifest 明确记录空身份；runtime 生命周期协调仍待接入。
- 2026-10-21：Product 新增 `close_and_freeze_run_deletion`，在打开互斥门和 coordinator lease 下登记删除意图、关闭目标 runtime owner 与 Records writer、清除 SQLite 活动选择及指向目标的文件镜像，再调用 Storage 冻结清单。失败保留 `requested` journal，使用同一 operationId 可重试；定向测试验证活动周目完成冻结，以及活跃回合时拒绝且不写 journal。RunLifecycleService、Tauri / TUI 入口、文件隔离、数据库清理与启动前向恢复仍未实现。
- 2026-10-21：活动选择读取现区分“SQLite 行未建立”和“SQLite 已初始化但为空”；后者不再回退到兼容镜像，避免周目删除或显式清空后旧文件恢复出陈旧选择。删除冻结后按权威选择状态清理镜像，并覆盖镜像内容与活动周目不一致的回归场景。

## 完成摘要

实施进行中；目前完成目录 schema、新局登记、只读分页、旧局公开 profile 绑定 CAS、显式旧 transcript 扫描、旧账务原子迁移、Tauri / Web API 接线及 TUI 周目列表 / 扫描 / 恢复入口，尚未满足本任务验收标准。
