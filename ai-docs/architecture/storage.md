# 存储与文件基建

更新日期：2026-10-21。设计稿 v1（[task 011](../task/011-storage-design.md) 已定稿，[task 013](../task/013-storage-impl.md) 已实现存储基建）。对话记录、受控追加与 UI 偏好已由 022 实现；034 将语言偏好表并入 engine 的共享 SQLite 迁移；029 已实现记忆 manifest / 正文存储与恢复，030 接入批次状态；032 增加轮回刻痕来源与节点进度 / 提示迁移；043 将权威实体身份及 applied 回执放入共享 SQLite，并从 JSONL 因果记录恢复。周目目录与删除协议由 [059](../task/059-run-management-design.md) 定稿，060 正在实施。导入导出另行推进。工程纪律见[职责边界](ts-rust-boundary.md)。

## 选型决策

**混合存储：SQLite 管业务状态，纯文件管记录与资产。**

| 数据域                                    | 载体                    | 理由                                                                    |
| ----------------------------------------- | ----------------------- | ----------------------------------------------------------------------- |
| 对局记录（JSONL，append-only）            | 文件                    | 天然顺序写读，崩溃恢复语义简单（截到上一完整行）；导出 / 同步就是拷文件 |
| 记忆 manifest、世界状态、剧本库索引       | SQLite                  | 需要查询、聚合、事务与部分更新                                          |
| 剧本包资产（立绘 / theme.css / 设定文本） | 文件（只读）            | 随剧本包分发，不进库                                                    |
| 密钥                                      | OS 凭据库，否则私有文件 | 不落 SQLite；权限与 hint 见[通信契约](ipc-contract.md)                  |

决策依据：rusqlite（bundled feature，免系统依赖）+ 手写迁移，不引 ORM / 查询构建器——桌面单机规模下重量依赖只有成本。**翻转条件**：当记录需要随机访问查询（跨局检索）时，把记录索引进 SQLite（FTS），记录文件本身仍是事实源——文件格式不变，迁移只加索引。

## 关系复杂度的边界

SQLite 是完整的关系数据库——join、递归 CTE、窗口函数、JSON 函数、FTS5 均可用；多对多、层级、图状关系都能建模。真正的边界只有两条：单写者（桌面单机 + 本仓已约定单写者，无碍）与超大规模图遍历的工程便利性——这类逻辑不放数据库，放引擎内存。

### 三层模型：剧本文件 ⇄ 引擎内存结构 ⇄ SQLite

- **剧本文件（静态规则）**：技能树形状、加点表定义、成长曲线、关系网初值——随剧本包分发，只读。
- **引擎内存结构（运行时领域状态，Rust 所有）**：把剧本规则实例化后的图（技能树 / 关系网；图结构候选库如 petgraph，**仅在需求真实出现时经 task 引入，当前不预置**）、索引与缓存（派生数值、分配历史）。复杂关系的建模、遍历与派生计算全部发生在这一层，内存是运行期的唯一高速真相。
- **SQLite + 文件（持久层）**：只做持久化与查询——把内存状态的变更事实落盘，启动时从持久层重建内存结构。数据库不存「算出来的真相」，也不承担遍历逻辑。

落库的是「事实」（谁在何时把点分给了谁），内存里算的是「现状」（当前属性面板）；两者以[记录引擎](record-engine.md)的事实意图、applied 标记与恢复规则衔接；JSONL 和 SQLite 不是跨文件原子事务。

职责三分法，以「加点」为例：

- **静态规则数据**（技能树形状、每点加成、成长曲线）→ 剧本包文件，不进库。
- **运行时状态**（玩家把点分到哪、何时）→ SQLite 窄表，保存已应用分配事实的查询投影；关键变更的因果事实源是 012 注册的 settlementPlanned / sceneAdvanced / sessionEnded system 块，WorldMutation 是其承载的类型化变更，不是另一个日志码，不形成两份独立真相。以下仅为点位建模示意，不是已实现 schema；实际事务还须带 applied 幂等标记：

```sql
CREATE TABLE point_allocations (
  character_id TEXT NOT NULL,
  node_key     TEXT NOT NULL,     -- 剧本定义的属性 / 技能键
  points       INTEGER NOT NULL CHECK (points >= 0),
  round        INTEGER NOT NULL,  -- 回合号，可回溯重算
  PRIMARY KEY (character_id, node_key, round)
);
```

- **派生数值**（加点后的属性面板）→ Rust 纯函数从分配状态计算，数据库不存「算出来的真相」——可测试、可回放、改规则不迁移数据。

人物关系网（Persona / 记忆）同理：节点 + 边表入库，遍历在引擎内存完成。

## 访问层与迁移

- SQLite 基建及通用 applied 原语归 store，业务 schema / SQL 归对应领域。`aoidos-engine::Storage` 持有唯一业务连接，领域模块经其门禁与受控访问借用，不自行打开第二套连接；装配层持有服务，不持业务连接。
- Schema 迁移：`PRAGMA user_version` + 按版本号排列的内嵌 SQL 切片；每次迁移用 IMMEDIATE 事务，执行或提交失败由 RAII 回滚，`user_version` 与 schema 同事务推进。SQL 不得自行 BEGIN / COMMIT / ROLLBACK；每个迁移配套回填测试。
- 业务 crate 不依赖 tauri：数据根路径由装配层（src-tauri）解析后注入。

`current_version` 使用 READ_ONLY 打开读取已持久化版本，缺失库或父目录为 0，不创建数据目录 / 库。缺失分支检查现存祖先是否为目录，避免 Windows 把文件挡住父路径的 NotFound 当成新安装。读取与迁移打开共用路径归一化和 busy 预算。`open_with_progress` 在每步事务提交成功后回调 `{ from, to }`，失败步骤不回调；之前成功步骤不撤销，无需迁移时无回调。回调是同步纯数据出口，迁移事件 / 运行期快照协议见[记录引擎](record-engine.md)与[通信契约](ipc-contract.md)，命令层发送及状态保存由 [022](../task/022-record-engine-impl.md) 按 006 定稿协议落地。备份的 IPC 字段与限制统一见[通信契约](ipc-contract.md)。

## 目录与路径规范

数据根由装配层调用 Tauri `app_data_dir()` 取得，当前应用标识为 `com.yuki.aoidos`，没有自定义目录覆盖。默认 macOS 为 `~/Library/Application Support/com.yuki.aoidos/`，Windows 为 `%APPDATA%\com.yuki.aoidos\`，Linux 为 `$XDG_DATA_HOME/com.yuki.aoidos/`（未设置时使用 `~/.local/share/com.yuki.aoidos/`），不使用 `~/.aoidos`。实际系统重定向由 Tauri 解析，不在业务层拼接用户主目录。

`llm/profiles.json` 保存配置；密钥优先进入 OS 凭据库（service 为 `aoidos`，user 为 providerId），`llm/credentials.json` 保存后端指针 / 墓碑及私有权限降级值，降级值当前以明文保存，仅使用文件权限保护，没有应用层文件加密；OS 后端正常时文件只保留指针等元数据。对话记录及两字段 UI 偏好已由 022 实现；029 实现会话记忆版本目录，批次结果由 030 接入。020 的调试正文仅存内存，产品持久化使用独立 OutputWriter。

```text
<app-data>/                       # tauri PathResolver::app_data_dir，注入业务 crate
├── storage.sqlite                # 业务状态（含 WAL/SHM）
├── storage.lock                  # InstanceLock：OS 独占锁，释放不删除
├── active-session.json           # 024 活动会话 / profile / script 身份，原子提交确认
├── llm/                          # 019 配置与凭据
│   ├── profiles.json             # profile 集合；业务 schema 由 aoidos-llm 管理
│   └── credentials.json          # 权威后端指针 / 墓碑及私有降级值
├── migrations/…                  # SQL 迁移（编译期内嵌，目录仅调试导出）
├── workspaces/<script-id>/       # 每剧本一个工作区
│   ├── transcript/<session-id>.jsonl
│   ├── transcript/<session-id>.<turn-id>.partial.jsonl  # 唯一活跃安全增量日志
│   ├── memory/                   # 029：不可变记忆正文；manifest 在 storage.sqlite
│   │   └── runs/<run-id>/        # 会话记忆版本正文与 030 批次结果文件
│   │       ├── entries/<entry-id>/<version-id>.json
│   │       └── batches/<batch-id>/result.json
│   └── exports/                  # 导出物（唯一允许被 shell 打开的目录）
└── backups/                      # 迁移 / 覆写前的自动备份
    └── storage-v<N>-<stamp>.sqlite  # SQLite 迁移备份；最近 3 份
```

`script-id` 是剧本注册时冻结的权威身份，遵循[剧本格式规范](../standards/scenarios.md)的合法值约束，不由显示标题重新消毒、截断或静默改名。`script_id_from_name` 只供尚未冻结身份的命名候选使用；已登记资源与存档路径必须验证原身份。

- **路径规则**：装配层注入系统数据根，业务相对路径由 `join_under_root` 拼装——拒绝 `..`、分隔符、盘符，消毒 Windows 非法字符与保留名。该工具只校验组件文本，不解析符号链接；数据根及子目录必须由应用控制。不可信剧本包的完整解包防护与 Windows 长路径前缀尚未实现。`normalize` 只在 Windows 转换正斜杠，保留操作系统原始编码；Unix 反斜杠是合法文件名字符。

兼容活动选择文件有界 4 KiB，包含 version=1、sessionId、profileId、scriptId，拒绝额外字段、非法身份或版本；损坏时报 store.corrupt，不猜测或覆盖旧元数据。060 新增 SQLite `active_session` 权威选择行，文件降为可重建镜像；打开流程须先登记 actor / writer 和周目元数据，再提交 SQLite 选择，最后原子更新镜像。重开按 header 的原文修订和静态前缀核验，不将新资源静默应用到旧周目，也不启动生成。

### 周目目录与生命周期（059 已定稿，060 实施中）

`sessionId` 是周目 / transcript 主身份；记忆 `memoryRunId` 是经 `memory_run_sessions` 读取的独立身份；059 设计确定 canonical `billingRunId=sessionId`。当前新会话与私有记忆请求均以 `sessionId` 作为账本范围；060 的历史 scope 迁移仍未完成。目录遇到经映射核实的 `memoryRunId` legacy 账单数据时必须标为 reconciliation required，不能把周目显示为未配置或可继续收费；迁移须合并 request / reservation / settlement 并保留未决义务。没有任何预算 / 请求行才是合法的“尚未配置”。新目录显式保存并交叉核验三项身份，不能按 UUID 字面值猜测关联。`active-session.json` 仅是最后选择的兼容镜像；SQLite 会话目录负责有界分页和生命周期状态，并新增 SQLite 活动选择权威行；transcript header 仍是会话事实源。

目录 schema 已包含 SQLite 会话摘要、权威活动选择、revision 和删除 journal；启动时逐项比对注册的表 / 索引定义。新创建周目登记 `sessionId`、独立 `memoryRunId`、canonical `billingRunId=sessionId` 与公开 profile 快照。公开快照通过 profile 文件读取，不访问密钥库。分页使用固定排序和 revision 绑定的 keyset 游标，响应不含 transcript 与路径；旧周目的 profile 绑定使用目录 revision 做 CAS，并将 legacy 账本 scope 保守标为待核对。活动文件写入失败后，重启读取 SQLite 权威行并修复兼容镜像。玩家可显式推进旧 transcript 扫描；扫描按每批最多 128 个遍历项、最多 8 MiB header 字节和单行 2 MiB 格式上限读取，只校验首行而不打开或恢复会话。扫描以深度有界的目录栈遍历，并在打开目录和读取 header 前复核文件类型及规范路径。每次新扫描先标记未完成并清除上一轮诊断；异常项以相对路径哈希记录，最多保留 256 项，`scanIssueCount` 达上限时饱和，不会阻断其他条目。缺少 `memory_run_sessions` 映射的旧记录因无法核验独立记忆身份而进入 `needs-repair`，不允许通过 profile 绑定解锁。列表 / 预览本身不扫描，发现完成前持续报告 `scanIncomplete`；header 文件 I/O 在 SQLite 全局锁外执行。

目录分页和预览不得调用 `Session::open`（其恢复可修复文件）或 `builtin::open`（会初始化记忆）。旧局缺 profile 时须标记需绑定，不使用当前新局选择代替。切换、账本 scope 迁移、删除及启动恢复仍按[059](../task/059-run-management-design.md)实施，当前实现尚未提供这些操作。

删除经显式确认并由 engine journal 前向恢复，覆盖单局记录、partial / repair backup、run-scoped 记忆 / 实体及本局已结算账本。transcript 隔离清单最多 8,192 项 / 4 MiB，memory run 目录作为单一同卷项隔离后分批递归清理，不跟随符号链接。存在 `reserved` / `unconfirmed` 请求或待结算 outbox 时拒绝删除；若 `unconfirmed` 缺少完整 usage 且无可核验证据，当前没有任意费用核销入口，只能保留该局。直接引用被删局来源或动态实体的跨周目刻痕失效，但剧本轮回代数高水位与全库 migration backups 保留；文件 unlink 不代表安全擦除。不得依赖外键级联清理共享业务库。

## 原子写工具（全仓唯一实现）

- API：`write_atomic(path, bytes)` / `write_text_atomic(path, text)`（UTF-8）。普通文件覆写经此二函数；凭据使用 `write_atomic_private`，在空 tmp 阶段经平台权限函数确认后才写正文，共用原子发布 / 清理 / fsync 管线。SQLite 事务与在线备份、实例锁文件、JSONL 受控追加 / 尾行截断由各自模块原地写，不经这里。JSONL 追加原语已由 022 在 store 中实现，全仓复用，不在 engine 再写一套 IO。
- 步骤：同目录唯一临时名 `<name>.<pid>.<counter>.tmp` → 排他创建 + 写入 + fsync → `rename` 覆盖目标 → 同步父目录。临时名冲突时既有文件不被覆盖或清理。重试次数与退避见[通信契约](ipc-contract.md)看门狗表。三端的 `WouldBlock`、`ResourceBusy`、`ExecutableFileBusy` 进入退避。Windows 上原始码 5（ACCESS_DENIED）和 32（SHARING_VIOLATION）同样算占用；Unix 上同号是 EIO / EPIPE，保持 `io`，不重试。目录目标三端立即 `io`，不进入退避：Windows 把「文件 rename 到目录」也报成 ACCESS_DENIED，若先按占用重试，目录会在 Windows 上等满退避。耗尽报 `locked` 并清理本次 tmp。父目录同步只吞掉 `PermissionDenied`、`InvalidInput`、`Unsupported`；其它同步错误在 rename 已经发布后仍返回，调用方不能把该错误当成「目标未更新」。
- 自愈：读取目录时清理残留 `.tmp`；JSONL 尾行不完整时截断到上一完整行；完整行仍须校验 JSON / 身份，中间损坏不得删。partial 不是 .tmp，不由通用临时清理删除；保留已提交前文并封中断块的恢复及 buffered / high 耐久边界见[记录引擎](record-engine.md)。
- 并发与锁：单写者约定；打开数据库前持有数据根 `storage.lock` 上的 `InstanceLock`。OS 独占锁阻止多开写入，PID 仅作诊断，不用于存活判断；进程退出自动释放，释放时不删除锁文件。同一目录只放一个业务库，备份共用 `backups/`。
- 有界文本读取：`read::read_text_bounded` 先以字节读取上限 + 1，检查长度后才解码 UTF-8；超限及非法 UTF-8 均为 `store.corrupt`，不输出文件正文。真正缺失返回 `None`；与 SQLite 共用现存祖先目录检查，父路径被文件占据时显式报错，不当首次安装。profile 与凭据各自解析 JSON / 版本，读取上限均为 512 KiB。
- 逻辑读改写：原子写只保证单次覆写的物理完整，**不串行化读-改-写周期**——同一文件的此类周期（如 profile 集合的增删改、降级凭据的设置 / 清除）由调用方在命令层持进程内锁串行（019：llm profile 与 key 各一把；等待用户交互的原生输入不得持锁）。应用 setup 在注入路径 / 开放命令前获取数据根 InstanceLock，并由 Tauri managed state 持有到退出；后续 SQLite 服务复用此所有权，不另取第二把实例锁。凭据状态读取同样持 key 锁，避免跨 OS / 文件更新期间读到中间状态。

## shell 与进程边界

- **允许**：经 opener 能力打开 `https?` / `mailto` 链接；用系统默认应用打开 `exports/` 目录内的导出文件。
- **allowlist**：URL scheme 白名单 + 路径白名单（仅 exports 目录），由 Rust 侧校验，前端传入的任何目标都必须过白名单。
- **禁止**：前端拼接命令或任意路径；业务 crate 暴露通用 spawn 原语（如未来确需剧本脚本执行，单独立 task 做沙箱设计，v1 明确不做的沿用 001 的边界）。

## 备份与升级

- SQLite：每次迁移前经 SQLite backup API 生成 `backups/storage-v<N>-<stamp>.sqlite`；N 为迁移前 schema 版本，stamp 为 Unix 时间戳纳秒整数。备份清理按规范文件名解析时间戳 / 版本整数排序，保留最近 3 份；非规范命名不占名额且不删除。备份查询只返回普通文件，跳过目录和符号链接，载荷限制见通信契约。
- 文件域：覆写前备份（对齐 007 遗忘回写的备份要求）；版本字段 + 读取时迁移，旧文件归档不删除。
- 剧本导入 / 导出：zip 约定（`manifest.json` + `assets/` + `theme.css`），导入时逐条消毒路径并校验 manifest；导出永远来自消毒后的规范路径。

## 错误与对接

- 裸码由 `StoreError::code()` 返回。IPC 的 `store.` 前缀、中文 `message` 与 `detail` 见[通信契约](ipc-contract.md)。磁盘满、权限、锁定超时、损坏、路径非法各占独立码。
- 对接：006 用原子写 + JSONL 截断恢复；029 已实现 007 的 SQLite manifest、不可变文件正文与恢复，产品记忆流程尚未接入；cycle 与依赖式清理边界见[记忆设计](memory.md)。SQLite 迁移备份与文件覆写备份遵守本文约定；019 的配置及私有凭据文件使用 `llm/`，schema 与权威后端规则见[通信契约](ipc-contract.md#工程纪律可检查版)。

## 实现状态与后续范围

- 已标定（task 013）：WAL，备份保留最近 3 份，OS 独占实例锁。rename 的重试预算见[通信契约](ipc-contract.md)看门狗表，013 已按该表实现。记录 append 已由 022 实现；业务 schema 与导入导出随相应任务实现。

## 业务存储所有权

`aoidos-engine::Storage` 统一打开 `storage.sqlite` 并核验共享 applied 表，保存真实迁移状态；主题 / 计费 / 记忆模块复用此所有者及迁移编号，不创建第二套业务连接生命周期。记忆正文由文件承载；029 manifest、030 批次收据、032 `memory_cycle_*` 刻痕与节点派生状态均存 SQLite。`aoidos-store::applied` 提供 operationId + contentHash 幂等提交原语，业务 schema 和解释器由消费模块所有。

043 的 `entity_registry_runs` / `entity_registry_entries` 由 engine 持有 schema 和迁移。周目身份、动态 entityRevision、源 mutation / hash / recordSeq 与 `store_applied` 回执在同一 SQLite 事务写入；JSONL 的 `settlementPlanned` 仍是意图事实源，SQLite 注册行是已应用状态。启动恢复只重放原 mutation；记忆入口在 session → 数据库锁顺序下审计源记录、应用回执及有效分支，再生成有界只读快照。分支回退不会删除身份审计行，但外分支身份不会进入当前快照。

`ui-preferences.json` 在系统应用数据根保存版本 1 的 `panelPinned` / `diceMode`。文件缺失返回默认值，坏格式或未知版本明确拒绝，不静默覆盖；单文件上限 4 KiB，保存经原子写成功才确认。主题和语言偏好仍遵循各自架构，不扩充这两个字段。

`locale_preference` 是 `storage.sqlite` 中独立的单行表，由 `aoidos-locale` 定义 schema，engine 的统一迁移 / 连接负责建立与核验。缺行表示 `system`，读取不隐式写入；显式选择通过 SQLite IMMEDIATE 事务原子替换。它不占用或扩展 `ui-preferences.json`，损坏记录报 `store.corrupt` 且不会自动覆盖。

`journal` 的已知 write / flush 失败回退至此前确认边界；sync 或回退结果不确定时冻结写者。尾行修复先保留原文件副本再截断；正式行与 partial 扫描均有界，未知记录保留原字节并只读。会话关闭不删除原记录，应用退出时关闭全部登记会话。
