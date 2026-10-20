# 059 — 已开周目管理设计

- 状态：done
- 依赖：022、023、029、035、058
- 优先级：P1
- 创建 / 更新：2026-10-20 / 2026-10-20

## 目标与背景

CLI/TUI 与桌面端需要共用已开周目的浏览、选择 / 恢复和删除语义。当前 `Product` 只持有 `active-session.json` 中的一个选择；记录正文在剧本工作区的 JSONL，记忆、实体和费用则在 SQLite，不能通过枚举或删除 JSONL 文件实现完整生命周期。

本设计把周目目录定义为**会话身份的可验证索引**，明确区分游戏会话、记忆运行与费用账本的关联身份，并规定旧数据发现、旧局 profile 绑定、切换与跨存储删除的可恢复边界。实现由 060 承接；本 task 不改业务代码。

## 必读

- [分层规范](../standards/layering.md)
- [Repository layout](../architecture/repository-layout.md)
- [存储架构](../architecture/storage.md)
- [记录引擎](../architecture/record-engine.md)
- [回合状态机](../architecture/turn-state-machine.md)
- [计费架构](../architecture/billing.md)
- [记忆架构](../architecture/memory.md)
- [CLI / TUI 实现](058-cli-tui-shared-game-runtime.md)

## 核对后的现状

- `active-session.json` 是单一活动选择，不是历史目录；当前 `Product.open` 只能重开该选择或创建新会话。
- 记录路径为 `workspaces/<script-id>/transcript/<session-id>.jsonl`。`Records::open` 会进入 `Session::open`，可能修复尾行 / 处理 partial；`builtin::open` 会建立或恢复记忆运行。因此目录和预览绝不能调用这两个入口。
- 游戏 / transcript 身份为 `sessionId`（UUID）。`memory_run_sessions` 显式把独立的 `memoryRunId` 绑定到 `sessionId`；不能假定二者相同。
- 剧情账本的 `billingRunId` 当前由 runtime 以 `sessionId` 传给 `LedgerBudgetPort`；但 030 的 Tauri 私有记忆请求目前把 `memoryRunId` 传给同一端口，可能形成两套账本。060 必须将新请求修正为唯一 canonical `billingRunId=sessionId`，并显式识别、合并已存在的 legacy scope。
- `active-session.json` 带当前局的 `profileId`，但旧的非活动记录没有该绑定。`FrozenProfile` 含密钥副本，不可序列化；公开 `LlmProfile` 不含供应商密钥。
- `entity_registry_runs` / `entity_registry_entries` 与 `sessionId` 关联；权威回执位于共享 `store_applied`，不是可整表清除的周目私有表。
- 记忆周目表大多按 `memoryRunId` 关联。`memory_cycle_marks` / `memory_cycle_sources` 则按剧本、周目代数及来源快照跨周目保存；`memory_script_cycle_generations` 是不能回退的单调高水位。
- 账本的 `billing_run_budgets.run_id` 有 `sessionId` 与历史 `memoryRunId` 两种已知来源；每局费用义务必须先由 `memory_run_sessions` 建立 scope 集合，再统一 canonicalize。`reserved` / `unconfirmed` 请求及 `billing_pending_settlements` 表示尚未解决的账务义务。除 settlement 子表外，不依赖外键级联清理。
- 记录恢复可能产生 `<session>.recovery-<uuid>.bak`；全库 `backups/storage-v<N>-<stamp>.sqlite` 则包含多个周目，不能按单局删除。

## 已定设计

### 1. 身份与权威目录

目录以 `sessionId` 作为周目主身份，不把 `memoryRunId`、`billingRunId` 简写为同一个含糊的 `runId`。新局目录行显式保存：

| 字段                    | 含义与权威来源                                                                                                 |
| ----------------------- | -------------------------------------------------------------------------------------------------------------- |
| `sessionId`             | UUID；对应 transcript header 及游戏会话的主身份。                                                              |
| `scriptId`、`createdAt` | 由经校验的 transcript header 得出；目录值是可重建索引，不覆盖正文事实。                                        |
| `memoryRunId`           | 由 `memory_run_sessions` 查询并核验的独立 UUID。                                                               |
| `billingRunId`          | 新局 canonical 值为 `sessionId`。目录显式记录此值；旧记忆账本别名仅通过 `memory_run_sessions` 的确切映射识别。 |
| `profileSnapshot`       | 创建时冻结的公开 `LlmProfile` 及其 schema 版本，不包含 `FrozenProfile`、API key 或凭据值。                     |
| `catalogState`          | 目录 / 恢复 / 删除状态，不作为游戏事实。                                                                       |

`storage.sqlite` 中由 engine 所有的会话目录是有界查询和生命周期操作的权威索引；transcript header 仍是会话原始事实源。目录行必须与 header、`memory_run_sessions`、账本预算 / 请求身份逐项交叉核验。发生缺行、重复绑定或身份冲突时标为 `needs-repair` 并隔离该局；禁止按 UUID 字面相等、文件名或剧本名推断缺失关系。没有预算行 / 物理请求是合法的 `billing-not-configured` 状态：可以浏览、选择或删除，不代表身份损坏，只会在发送前由 035 拒绝无预算请求。`active-session.json` 降为最后选择的兼容镜像；新增 SQLite active-session 单行是选择权威，文件只用于兼容并可由权威行修复。

列表采用 keyset 分页，固定排序 `createdAt DESC, sessionId DESC`，每页默认 50、最大 200；cursor 绑定目录 revision 和排序键，目录变化后旧 cursor 返回 staleRevision。返回字段仅包括会话身份、剧本展示名 / id、创建时间、活动选择标记、profile 摘要、可执行状态和必要的待处理计数；不读完整 transcript，不返回正文、密钥或绝对路径。读取与预览不得创建 `Session`、修复文件、初始化 `Product` / memory run、取得游戏 lease 或发起模型请求。

目录状态至少区分 `ready`、`needs-profile-binding`、`billing-not-configured`、`billing-reconciliation-required`、`recovery-required`、`needs-repair`、`script-unavailable`、`settlement-pending`、`deleting`。状态计算只使用有界 header / 目录元数据和针对该局的索引查询；一个损坏条目只能显示局部诊断，不能阻断同页其他周目。

### 2. 新旧局 profile 绑定与恢复

- 新局在建立目录项时同时冻结其完整、非秘密的公开 `LlmProfile`（含 provider / model / mode / sampling / proxy 选项）及 profileId。当前 profile 后续修改、改名或删除不改变已有局的配置快照。
- 快照永不包含 `FrozenProfile`、`SecretString`、API key 或代理凭据；打开 / 请求时按快照中的 provider 身份从当前 OS 凭据库读取凭据。密钥轮换可继续使用；无凭据时允许只读查看，但发送前按现有 LLM 错误阻止请求。
- 存档格式、剧本静态前缀及 script revision 以原 header 为准。恢复时必须能装配其 scriptId 对应的兼容规则；当前资源缺失或 revision 不兼容时标为 `script-unavailable`，只读展示并要求明确修复 / 安装，不能静默套用新剧本。
- 旧局 profile 不存在时标为 `needs-profile-binding`，不能把 active-session 中的 profileId 当成历史配置证明，也不能套用当前新局选择。该 profileId 至多作为候选提示；用户必须明确选定现有 profile 并确认“旧记录未保存原配置，后续请求将使用此配置”；绑定以 revision CAS 写入不可秘密的快照并留操作记录。未绑定局可只读查看，不允许发送新请求。
- 显式选择目标后才执行恢复。恢复先核验目录和身份映射，再按 023 规则打开 / 修复记录、重建 memory / entity 投影并检查账本；不自动提交行动、不恢复后自动发送、不自动重试收费请求。未解决的回合 / 记忆状态继续由原有显式恢复入口处置。

### 3. 切换协议

切换与新建、删除共用一个 app 生命周期互斥门；任何活跃回合、正在派发的 provider 请求或生命周期删除操作期间均返回 `app.busy`。操作不持锁跨网络 await。

1. 在不改变当前状态的前提下校验目标目录、header、三种身份、剧本兼容性及 profile 快照；目标无法恢复时保留当前局。
2. 取得共享 coordinator 的空闲门，关闭旧 Product / Actor owner 并同步旧记录；旧目录和活动镜像此时仍可作为恢复入口。
3. 显式打开目标并完成记录恢复、memory / entity 注册与 profile 装配；该步骤不发送请求。失败时清理新建的内存 owner，保留原 `active-session.json`，旧局可再次打开。
4. 目标成功登记后，以 CAS 事务更新 SQLite authoritative active-session 行；提交结果不确定时读回该行判定。权威行确认目标后，目标 owner 保持打开，`active-session.json` 尽力原子镜像；镜像失败报告可恢复诊断，后续由权威行修复，不回滚选择。权威行仍指旧目标时才关闭新 owner，旧局可再次打开。目录与 transcript 不回滚。

进程启动先读取 SQLite authoritative active-session 行，并核验其目录状态与可用能力；不能以 `ready` 作为唯一可选状态。`billing-not-configured` 可打开但发送须先配置额度；`needs-profile-binding`、`billing-reconciliation-required`、`settlement-pending` 与 `recovery-required` 可进入只读 / 显式恢复能力范围，未解决前不发新请求；`script-unavailable` / `needs-repair` 仅允许记录诊断或 transcript 只读查看；`deleting` 不可打开。活动镜像缺失或陈旧时仅按 SQLite 权威行修复镜像，不改变已提交选择；权威目标不可恢复时保留其身份并显示诊断，须由玩家显式改选，绝不自动切到最近局。尚无 SQLite 选择行的旧库只可在 `active-session.json` 完整身份经 header 核验后迁入一次；否则保持未选择。目录索引缺失 / 旧版存档由有界恢复扫描补建，恢复完成前分页须明确 `scanIncomplete`，不能谎报总数。

### 4. 旧数据发现与索引修复

SQLite schema 迁移只建目录及删除 journal 表，不在迁移事务中扫描文件。engine 以稳定的剧本目录项 / transcript 文件名分批发现旧 JSONL；每批最多处理 128 个遍历条目（包括目录、符号链接、非法或不匹配条目），header 单行读取遵守 record 格式现有 2 MiB 上限，批次 header 总读取预算 8 MiB。应用进程中持有有界 `ReadDir` 游标；重启后从根目录安全重扫，已登记身份以唯一键幂等跳过，不依赖不可移植的目录偏移量。每个批次稳定排序当前读得的候选项；单批进度游标不越过未处理项。拒绝符号链接、非法路径、超限文件和身份不匹配。扫描期间目录查询返回已发现结果并标 `scanIncomplete`，显式“继续发现”推进下一批，不做后台轮询。

每个旧 header 可验证时，独立查询 `memory_run_sessions` 与账本，建立 `sessionId → memoryRunId` 及 legacy billing scope 集合。新账单统一以 `sessionId` 作为 canonical `billingRunId`；旧私有记忆请求可能在 `memoryRunId` scope，必须按明确的 memory session 映射识别。无预算、无请求是 `billing-not-configured`；一对多、无法核验、重复计费 requestId 或预算冲突则进入 `billing-reconciliation-required` / `needs-repair`，不能遗漏为无欠款。

legacy 账本整理先以所有已验证 scope 检查 unresolved 义务，再将 `billing_physical_requests.run_id` 统一迁至 canonical sessionId、保留 requestId / pending-settlement 身份。单一预算可以迁入 canonical；多个预算值一致时合并，值冲突时记录原值并要求用户显式设置本局新限额后才能收费。价格 / 配置选择不迁移。budget warning 按币种合并且不把未读提醒变成已读，预算更新审计保留 operationId 并关联 canonical 身份。任何迁移冲突均阻止新的收费请求；玩家仍可只读或在所有费用义务 settled 后删除。

旧版 `active-session.json` 只保存选择时的 profileId，不含该 profile 的历史内容，不能证明旧局真实使用过该配置；完整身份匹配时至多用作候选提示，仍须经用户显式确认并冻结当前公开 profile。其他 profile 缺失的旧局进入 `needs-profile-binding`。部分文件损坏时保留目录条目并显示可操作诊断；发现流程绝不触发尾行修复或 partial 恢复。

### 5. 删除语义与数据范围

删除是玩家显式确认的不可逆**本地周目清除**，无回收站。确认页列出剧本 / 会话摘要、会清除的记录 / 记忆 / 本地费用明细，以及费用待结算阻止条件。删除不调用 LLM、不访问供应商账单 API、不删除 profile / 凭据 / 公共价格 / 应用设置，也不删除共享 SQLite 全库备份。`billing_audit` 只删除 target 精确等于已核验账本 scope 的周目预算 / 警告操作记录；按物理请求 ID 记账的结算审计，仅在关联请求已核验属于该局且已结算时删除。价格登记、模型选择及无法确定归属的审计记录保留。删除 journal 的精简完成墓碑供幂等重试。

清除目标包括：该会话 JSONL、同会话 partial 与恢复备份；对应 `memoryRunId` 的 memory 正文目录与全部 run-scoped memory 表；该 `sessionId` 的 entity registry 行及可精确归属的 `store_applied` 回执；该周目所有通过目录关联核验的 canonical / legacy billing scope 下的 budget、warning、物理请求和局部审计数据；目录与活动选择引用。未迁移的 legacy scope 也在预检 / journal manifest 中显式列出，不能遗漏。不能按时间 / prefix / `scriptId` 批量删除其他周目数据。

计费删除采用严格门禁：所有已核验账本 scope 中只要有 `reserved` / `unconfirmed` 物理请求、待结算 outbox，或未知的结算恢复状态，就拒绝删除并返回待解决请求数。已有完整持久 usage 的请求可走现有本地 settlement recovery；当前没有任意费用核销入口，若 `unconfirmed` 缺少完整 usage 且无供应商回执等可核验证据，该局只能保留 / 只读，不能靠手工改状态或把未知用量记零来删除。未来要解决此类记录，须另行设计并实现有审计证据的人工核销流程；060 不伪造该能力。所有请求已 settled 后，玩家确认可一并清除本地该局费用明细；服务商侧账单不受影响。

删除直接依赖规则如下：

- `memory_cycle_marks` 不属于单个 memory run 的外键级联集合。删除前枚举来源 `SourceRef.runId == memoryRunId` 或主体引用该局动态实体 ID 的刻痕，删除这些刻痕及来源，使其之后不能再注入；不删除其他剧本 / 周目的无关刻痕。
- `memory_script_cycle_generations` 的每剧本高水位永久保留，删除最高代数也不回退、不复用代数。
- 后续周目的 transcript 是独立提交的世界记录；删除不改写后续对白 / 世界事实。无法从无血缘索引的自由文本推导并清除所有间接叙事影响，因此 UI 与文档只承诺清理本局及可明确追溯的直接派生刻痕，不宣称跨局内容“彻底遗忘”或安全擦除。
- SQLite 全库迁移备份仍按现行策略保留最近 3 份，可能含已清理周目的旧字节；普通文件删除、SQLite checkpoint 或清理不是安全擦除。用户需要了解本地备份仍可能保留历史副本。

### 6. 删除 journal 与崩溃恢复

engine 在 `storage.sqlite` 持有所有权明确的 `session_deletion_ops` journal。每次调用包含 UUID `operationId` 和 `sessionId`；相同 operationId + 同目标重放返回当前 / 已完成结果，不同目标冲突。用户确认前只预览，不写 journal。

状态单向推进，不回滚删除：

```text
requested → owner_closed → files_quarantined → database_purged → files_purged → completed
```

1. 先取得生命周期门、核验目标与账务门禁。在 IMMEDIATE 事务中写 journal 并将目录行置 `deleting`；该事务提交后，任何 open / switch 都拒绝此 sessionId。若目标是活动局，关闭 owner 并清理 SQLite active-session 与兼容镜像。
2. owner 关闭后枚举本局文件；把源 / 隔离目的相对路径、文件类型、校验身份及 manifest hash 写入 journal，状态推进为 `owner_closed`。transcript 主 JSONL、partial 与 recovery backup 清单最多 8,192 项且编码后最多 4 MiB；超限时在移动任何文件前停止并报告容量错误。memory run 目录作为同卷单一目录项隔离，不递归展开到 manifest。清单冻结后不得因重试重新扩大范围。
3. 把 transcript manifest 文件逐项移动至同文件系统 `<data-root>/deleting/<operationId>/transcript/` 隔离目录；把整个 memory run 目录作为单一目录项移动到隔离目录。每项 rename 后同步源 / 目的父目录，再记录该项已移动；两侧均存在或类型异常时停止并进入 repair。全部完成后置 `files_quarantined`。隔离目录和 `deleting` 目录行保持到数据库及文件清理全部完成。
4. 在一个 SQLite 事务中按明示外键顺序移除 run-scoped memory、entity、所有已核验 canonical / legacy 账本 scope、目录关联及直接来源 cycle marks / applied receipts，保留代数高水位，并把 journal 更新为 `database_purged`。物理请求先检查所有 scope 均 settled 且无 pending settlement；否则不能开始删除。
5. 按冻结 manifest 删除隔离文件，并递归移除隔离的 memory run 目录；递归清理每批最多遍历 128 个文件系统条目，不跟随符号链接、不越过文件系统边界，重启后从隔离目录幂等重扫。对每个父目录变更做必要同步，全部完成后置 `files_purged`。最后在同一事务移除仍为 `deleting` 的目录行并把 journal 置为精简 `completed` 墓碑。完成墓碑只保留 operationId、目标身份哈希、完成时间和结果码，不保留正文、profile、剧本标题或费用详情。

进程启动时先分页恢复 journal：`requested` 及之后的记录只前进完成，不恢复可见性；文件部分已移动 / 部分缺失均按冻结 manifest 续做。SQLite 事务中断则整体回滚，重试该阶段；SQLite 已清理但文件仍在隔离目录时继续删文件。相同 operationId + 目标幂等；不同 operationId 请求删除已有未完成目标时返回该活动操作身份或冲突，不建立第二份 manifest。恢复失败只隔离该目标，显示失败码和重试入口，不阻断其他健康周目；此局在完成前始终不可打开。锁顺序为生命周期门 → coordinator → session writer → SQLite；禁止在持有 SQLite 锁期间等待 actor / 网络。

## 分层与接口边界

- `aoidos-store` 只提供 SQLite / 文件系统原语与安全路径，不依赖 engine、app、CLI 或 Tauri。
- `aoidos-engine::Storage` 拥有迁移和领域表入口，负责目录身份核验、分批旧数据发现、run-scoped 清理的事务原语；它不持有 UI 状态。
- `aoidos-app` 提供唯一的 `RunCatalog` / `RunLifecycleService`：分页查询、只读预览、profile 显式绑定、切换、删除预览 / 确认 / 恢复；协调 `Product`、`Coordinator`、`Records` 与 `Storage`。共享应用服务不进入 `aoidos-engine`，不引入反向依赖。
- Tauri IPC 与 CLI/TUI 仅调用 app typed API；两端使用同一 DTO / 错误语义，UI 不拼路径、不执行 SQL、不启动 CLI 子进程。
- 生命周期查询与短事务有界；删除前不自动恢复未决费用，删除和选择均不发生成请求。日志与错误 detail 只暴露 ID / 稳定原因，不包含 transcript、profile 快照或密钥。

## 实施任务与顺序

060 [已开周目选择与安全删除](060-run-management-impl.md) 是唯一实现任务，依赖本设计。它先落目录 schema / 旧数据扫描 / 新局 profile 快照及恢复，再实现切换、删除 journal 与直接依赖清理，最后接 Tauri 和 TUI。059 不扩张 058 的设置 / 新局模型与剧本选择范围。

## 验收标准

- [x] 明确目录权威、分页 / 排序 / 有界摘要、状态隔离和旧数据发现；预览无文件修复 / runtime / memory 初始化副作用。
- [x] 明确 `sessionId`、`memoryRunId`、canonical `billingRunId`、legacy scope 迁移与核验；不根据 UUID 猜关系。
- [x] 定义新局非秘密 profile 快照、旧局显式绑定、密钥轮换、profile 删除和资源版本不匹配行为。
- [x] 定义 SQLite 活动选择权威、文件镜像修复、切换提交顺序和并发门。
- [x] 删除覆盖 JSONL / partial / repair backup、memory 正文 / schema、entity / applied 回执和本局费用；定义费用待结算阻止、cycle mark 失效及代数高水位保留。
- [x] 删除确认、冻结文件 manifest、逐阶段同步 / 恢复、重复请求、并发打开 / 删除和安全擦除限制均有确定语义。
- [x] `storage.md`、`record-engine.md`、`billing.md`、`memory.md`、架构总览、060 与 task-index 同步，依赖图无环。
- [x] 独立评审完成并处理发现；本任务只交付设计，不声称周目选择或删除已实现。

## 验证计划与结果

| 日期       | 环境 / 检查            | 预期                                               | 实际结果                                                                                                                                                                  |
| ---------- | ---------------------- | -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-20 | 源码 / schema 交叉核对 | 覆盖三种身份、恢复副作用、所有者关联及共享数据边界 | 已核对 `Product`、`Records`、memory schema / cycle、entity registry、billing schema / ledger；发现记忆私有请求使用 `memoryRunId`，纳入 legacy scope 合并及 canonical 修复 |
| 2026-10-20 | 独立架构 review        | 检出跨存储生命周期、恢复和误删风险                 | 两轮 review 发现均已处理；最终复审通过，确认设计和 060 / 架构文档一致                                                                                                     |

## 风险与回退

目录是可重建索引但包含 profile 绑定与操作状态；应通过版本化 SQLite migration 写入，回填失败时保留旧文件并标记不完整。切换 / 删除都不允许壳层绕过 app 生命周期服务。跨文件与 SQLite 不能宣称原子事务，删除只能按 journal 前向恢复。已删除周目可能仍存在于迁移备份，产品不得把 unlink 表述为安全擦除。

## 决策与工作记录

- 2026-10-20：基于 CLI/TUI 缺少历史周目选择 / 删除创建本设计任务；发现旧局缺少 profile 绑定，记录与 SQLite 分持久化。
- 2026-10-20：源码核验确认 memory runId 独立；剧情请求以 sessionId 记账，现有私有记忆请求误以 memoryRunId 记账。060 统一新账单身份并迁移 / 合并可明确关联的 legacy scope；同日 review 补充跨周目刻痕、只读恢复副作用、profile 秘密边界、repair backup、账务未知义务及 owner 并发门。

## 完成摘要

设计内容已通过独立复审，可直接由 060 实施。该状态仅表示设计定稿；周目目录、选择与删除功能尚未实现。
