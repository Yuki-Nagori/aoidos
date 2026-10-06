# 存储与文件基建

更新日期：2026-10-06。设计稿 v1（[task 011](../task/011-storage-design.md) 已定稿，[task 013](../task/013-storage-impl.md) 已实现存储基建）。业务记录、记忆与导入导出仍按各自任务规划；工程纪律前提见[职责边界](ts-rust-boundary.md)。

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

- 所有 SQL 集中在存储 crate；域 crate（记录 / 记忆 / 引擎）经 trait 访问，不直接持连接。
- Schema 迁移：`PRAGMA user_version` + 按版本号排列的内嵌 SQL 切片；每次迁移用 IMMEDIATE 事务，执行或提交失败由 RAII 回滚，`user_version` 与 schema 同事务推进。SQL 不得自行 BEGIN / COMMIT / ROLLBACK；每个迁移配套回填测试。
- 业务 crate 不依赖 tauri：数据根路径由装配层（src-tauri）解析后注入。

`current_version` 使用 READ_ONLY 打开读取已持久化版本，缺失库或父目录为 0，不创建数据目录 / 库。缺失分支检查现存祖先是否为目录，避免 Windows 把文件挡住父路径的 NotFound 当成新安装。读取与迁移打开共用路径归一化和 busy 预算。`open_with_progress` 在每步事务提交成功后回调 `{ from, to }`，失败步骤不回调；之前成功步骤不撤销，无需迁移时无回调。回调是同步纯数据出口，迁移事件 / 运行期快照协议见[记录引擎](record-engine.md)与[通信契约](ipc-contract.md)，命令层发送及状态保存由 [022](../task/022-record-engine-impl.md) 按 006 定稿协议落地。备份的 IPC 字段与限制统一见[通信契约](ipc-contract.md)。

## 目录与路径规范

```text
<app-data>/                       # tauri PathResolver::app_data_dir，注入业务 crate
├── storage.sqlite                # 业务状态（含 WAL/SHM）
├── storage.lock                  # InstanceLock：OS 独占锁，释放不删除
├── migrations/…                  # SQL 迁移（编译期内嵌，目录仅调试导出）
├── workspaces/<script-id>/       # 每剧本一个工作区
│   ├── transcript/<session-id>.jsonl
│   ├── transcript/<session-id>.<turn-id>.partial.jsonl  # 唯一活跃安全增量日志（022 待实现）
│   ├── memory/                   # 007 文件正文；manifest 在 storage.sqlite（029 待实现）
│   │   ├── runs/<run-id>/        # 会话记忆版本正文与批次结果（029 / 030 待实现）
│   │   │   ├── entries/<entry-id>/<version-id>.json
│   │   │   └── batches/<batch-id>/result.json
│   │   └── cycle/                # 轮回刻痕，独立生命周期（032 待实现）
│   └── exports/                  # 导出物（唯一允许被 shell 打开的目录）
└── backups/                      # 迁移 / 覆写前的自动备份
    └── storage-v<N>-<stamp>.sqlite  # SQLite 迁移备份；最近 3 份
```

- **script-id 规范**：`[a-z0-9-]{1,64}`，由剧本名消毒生成（小写、空格转连字符、去非法字符）；超长截断后追加 8 位短 hash 防碰撞。
- **路径规则**：装配层注入系统数据根，业务相对路径由 `join_under_root` 拼装——拒绝 `..`、分隔符、盘符，消毒 Windows 非法字符与保留名。该工具只校验组件文本，不解析符号链接；数据根及子目录必须由应用控制。不可信剧本包的完整解包防护与 Windows 长路径前缀尚未实现。`normalize` 只在 Windows 转换正斜杠，保留操作系统原始编码；Unix 反斜杠是合法文件名字符。

## 原子写工具（全仓唯一实现）

- API：`write_atomic(path, bytes)` / `write_text_atomic(path, text)`（UTF-8）。普通文件覆写经此二函数。SQLite 事务与在线备份、实例锁文件、JSONL 受控追加 / 尾行截断由各自模块原地写，不经这里。JSONL 追加原语由 022 在 store 中实现，全仓复用，不在 engine 再写一套 IO。
- 步骤：同目录唯一临时名 `<name>.<pid>.<counter>.tmp` → 排他创建 + 写入 + fsync → `rename` 覆盖目标 → 同步父目录。临时名冲突时既有文件不被覆盖或清理。重试次数与退避见[通信契约](ipc-contract.md)看门狗表。三端的 `WouldBlock`、`ResourceBusy`、`ExecutableFileBusy` 进入退避。Windows 上原始码 5（ACCESS_DENIED）和 32（SHARING_VIOLATION）同样算占用；Unix 上同号是 EIO / EPIPE，保持 `io`，不重试。目录目标三端立即 `io`，不进入退避：Windows 把「文件 rename 到目录」也报成 ACCESS_DENIED，若先按占用重试，目录会在 Windows 上等满退避。耗尽报 `locked` 并清理本次 tmp。父目录同步只吞掉 `PermissionDenied`、`InvalidInput`、`Unsupported`；其它同步错误在 rename 已经发布后仍返回，调用方不能把该错误当成「目标未更新」。
- 自愈：读取目录时清理残留 `.tmp`；JSONL 尾行不完整时截断到上一完整行；完整行仍须校验 JSON / 身份，中间损坏不得删。partial 不是 .tmp，不由通用临时清理删除；保留已提交前文并封中断块的恢复及 buffered / high 耐久边界见[记录引擎](record-engine.md)。
- 并发与锁：单写者约定；打开数据库前持有数据根 `storage.lock` 上的 `InstanceLock`。OS 独占锁阻止多开写入，PID 仅作诊断，不用于存活判断；进程退出自动释放，释放时不删除锁文件。同一目录只放一个业务库，备份共用 `backups/`。

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
- 对接：006 用原子写 + JSONL 截断恢复；007 使用 SQLite manifest + 文件正文；不可变版本 / cycle 目录及依赖式清理见[记忆设计](memory.md)，SQLite 迁移备份与文件覆写备份遵守本文约定；密钥的降级文件目录由本规范预留。

## 实现状态与后续范围

- 已标定（task 013）：WAL，备份保留最近 3 份，OS 独占实例锁。rename 的重试预算见[通信契约](ipc-contract.md)看门狗表，013 已按该表实现。业务 schema、记录 append 与导入导出随相应任务实现。
