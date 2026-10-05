# 通信契约（IPC）

更新日期：2026-10-05。设计稿 v1（[task 010](../task/010-ipc-contract-design.md) 产出，评审意见已回写）。适用范围：`src-tauri` 命令层 ↔ `src-web`。谁拥有什么见[职责边界](ts-rust-boundary.md)。错误形状、事件信封和看门狗预算以本文为准。状态：**部分落地**——015 已提供命令错误映射，016 已提供序号与信封，017 已提供只读 store 命令和迁移回调。前端界面仍是 greet 占位；真实事件发送方、在飞状态和快照对齐随 005 / 006 / 012 接入。

## 总则

- 传输两种：**命令**（前端 `invoke(name, args)`，请求 / 响应）与**事件**（命令层 `emit_to("main", event, payload)`，单向推送）。窗口标签是 `src-tauri/tauri.conf.json` 的 `main`。
- 命令只回答「提交是否被接受 + 同步可得的即时结果」；长流程（生成、迁移、后台任务）的中间态一律走事件，不在命令里阻塞等待。
- 载荷一律 JSON，字段 camelCase；Rust 侧载荷结构体标 `#[serde(rename_all = "camelCase")]`，不逐字段手写 rename。
- 所有命令返回 `Result<T, CmdError>`，`CmdError` 固定序列化为 `{ code, message, detail? }`（见「错误码目录」）。
- JS number 接收的整数字段必须在安全整数范围内；纳秒时间戳等大整数用十进制字符串传输。
- 不兼容变更只换事件名。桌面应用前后端同包发布，不做字段级兼容层，信封不带版本号。

## 命令命名

- `<域>_<动词>[<宾语>]`，snake_case；域 = 消费的业务 crate：`store` / `llm` / `engine`。
- 动词约定：`get_` 取单值、`list_` 取列表、`set_` 替换一项配置、`create_ / update_ / delete_ / save_` 写实体、`submit_` 提交长流程、`cancel_` 取消在飞流程。
- 形参：Rust snake_case；Tauri 默认把前端 camelCase 键映射到 snake_case 形参。**两侧固定「Rust snake_case ↔ 前端 camelCase」**，不使用 `rename` 特例。
- 分页：可能超过一页的 `list_*` 使用 `{ cursor?, limit? }`，返回 `{ items, nextCursor? }`。省略 `limit` 时为 50，最大 200；`0` 或大于 200 返回 `app.bad-request`。cursor 是不透明字符串，前端只透传不解析。文档写明硬上限不超过 50 的列表可以不带分页，例如 `store_list_backups {}` 返回 `{ items }`。
- 正例：`llm_set_key`、`engine_submit_input { text }`、`store_list_backups {}`。
- 反例：`getScriptsData`（无域前缀）、`do_thing`（动词无信息量）、`llm_generate_stream`（流式不是命令——提交用 `llm_submit`，增量走事件）。

## 已落地的 store 命令

Rust 的 Serialize 载荷与 `src-web/api/store.ts` 类型同步维护；invoke 只透传，不做运行时校验。命令不要求前端传入磁盘路径，setup 注入业务库路径；重复注入保留首次值。

| 命令                  | 参数 | 返回                                          | 边界                                                                                 |
| --------------------- | ---- | --------------------------------------------- | ------------------------------------------------------------------------------------ |
| `store_list_backups`  | 无   | `{ items: [{ path, version, nanos, size }] }` | 最新 50 项，新到旧；目录不存在为空；nanos 为 Unix epoch 纳秒字符串，size 为字节数    |
| `store_get_migration` | 无   | `{ from, to, phase: "idle" }`                 | from == to == 持久化 user_version；库或父目录不存在为 0，不创建目录 / 库，不运行迁移 |

快照目前只提供静态版本，不表示迁移正在运行、成功结束或失败，也未提供事件 seq 基线。006 接入真实迁移流时，必须同时实现状态保存、阶段和每事件序号快照，再按下节规则对齐监听者。备份只列普通文件，跳过目录和符号链接；迁移写入保留 3 份，人工放入更多备份时命令最多列最新 50 份。

## 事件命名与载荷

- `<域>:<对象>:<阶段>`，全小写冒号分隔（Tauri 事件名允许 `:`）。
- 只有 `src-tauri` 调用 `emit_to("main", …)`。v1 单窗口，不做全局广播，避免未来多窗口时的载荷泄漏面。业务 crate 不得依赖 Tauri，进度经回调或通道交出普通载荷，由命令层发送。
- 信封统一：`{ seq: u64, data: T }`，序号范围为 1 到 JS 最大安全整数（2^53−1），超过上限报 `app.event-failed`，不回绕。`seq` 在「事件名 + 流标识」内单调递增。`llm:turn:chunk` 与 `llm:turn:done` 各有自己的序号，互不占号。流标识：`llm:turn:*` 用 `turnId`；一次存储打开是一条迁移流；引擎阶段流的标识由 012 定。
- 序号键由事件名和流标识两个独立字符串构成，空流标识仅在同一事件名下共用计数。条目存活到进程退出，空间随流数量增长。同一流由发送方串行投递；分配器仅保证序号分配，不保证并发投递顺序。data 序列化失败不占号，构造成功后投递失败会留下序号缺口。
- 以下为真实事件消费方接入时必须满足的快照规则，当前还没有真实发送方：
- 监听者先取该流的快照。快照给出这个流上每个事件名各自的最后 `seq`，监听者只取自己订阅的名字作为基线。之后该名字上 `seq <= 基线` 的事件丢掉；`seq == 基线 + 1` 才应用；出现更大的缺口就再取快照，不重放。没有基线时，第一条事件也按缺口处理，不从 0 推断。快照里的状态必须足够重绘，不能只给出最后一条 `delta`。

| 事件                                            | 快照命令              | 状态至少包括                                                    |
| ----------------------------------------------- | --------------------- | --------------------------------------------------------------- |
| `llm:turn:*`                                    | `llm_get_turn`        | 该 turn 已累积的文本、outcome（completed / cancelled / failed） |
| `store:migration:*`                             | `store_get_migration` | `from`、`to`、阶段                                              |
| `engine:phase:changed`、`engine:scene:advanced` | `engine_get_phase`    | 当前阶段与场景                                                  |

`engine_get_record_page`（006）是记录分页，不承担上表的对齐。

- 预留事件（仓库里还没有发送方；未冻结的字段标在条目上）：
  - `llm:turn:chunk`，data `{ turnId, delta }`（005 可加字段，不能删这两项）
  - `llm:turn:done`，data `{ turnId, outcome: "completed" | "cancelled" }`
  - `llm:turn:failed`，data `{ turnId, code, message }`（错误码见下）
  - `store:migration:progress` / `store:migration:done`，data `{ from, to }`。`open_with_progress` 和 `MigrationProgress` 已就绪，每步提交成功后回调，失败步骤不回调、无需迁移时不回调；此前成功步骤不会撤销。命令层 emit 适配、done / failed 收尾与运行期快照随 006 接入，当前没有真实发送方。
  - `engine:scene:advanced` / `engine:phase:changed`（012 冻结 data）
- 流式期间发生错误：以 `*:failed` 事件收尾；命令本身的 `Err` 只表示「提交被拒绝」，两者不重复携带同一错误。

## 错误码目录

- 形状：`{ code, message, detail? }`。`code` 是机器分支的唯一依据；`message` 是可展示中文，不参与分支；`detail` 可选结构化补充（如被拒的路径）。
- 命名空间 `<域>.<错误>`：`store.*` **已落地**——`src-tauri/src/ipc.rs` 的 `From<StoreError> for CmdError` 产出 `format!("store.{}", code())` 形态的前缀码与中文映射。命令层不得把 `code()` 的返回值再当成已带前缀。中文 `message` 由命令层映射器编写，不用 `Display`（`Display` 是英文诊断）。`llm.*` 与 `engine.*` 的码名在本文预留（映射随 005、012 的实现任务落地），触发条件分别由 005、012 冻结。`app.*` 属于命令层。
- 通用：`app.bad-request`（参数校验失败，含分页越界）、`app.not-found`（命令参数里的 id 不存在，如剧本、场景、回合）、`app.event-failed`（载荷序列化、序号分配或平台投递失败；真实监听者以快照对齐，不重试发送）、`app.not-ready`（存储尚未初始化——早于 setup 的调用；桌面正常流程不会出现）。存储路径或文件缺失只用 `store.not-found`。
- `app.busy`：命令层在进入引擎之前拒绝第二个在飞回合。引擎内部可以拒绝，对外仍映射成这一个码。不另设 `engine.turn-in-flight`。
- store：`store.invalid-path` `store.already-running` `store.locked` `store.migration` `store.disk-full` `store.permission` `store.not-found` `store.corrupt` `store.io`。
- llm 预留（005 冻结每个码的触发条件）：`llm.missing-key` `llm.auth` `llm.rate-limited` `llm.network` `llm.tls` `llm.stalled` `llm.empty-output` `llm.bad-response` `llm.aborted`。用户 `cancel_` 成功时命令返回成功，并发送 `llm:turn:done`，`outcome` 为 `cancelled`。`llm.aborted` 只表示首字节之后的传输中断或空闲看门狗，不表示这次取消。
- engine 预留（012 冻结触发条件）：`engine.no-scene` `engine.invalid-phase`。
- 前端分支**正例**：

```ts
if (err.code === "llm.rate-limited") showRetryLater(err);
else if (err.code.startsWith("llm.")) showLlmError(err);
else showGenericError(err);
```

- 前端分支**反例**：`err.message.includes("rate")`（文案漂移即坏）；`switch (err.code)` 不写 default。
- 新增码的流程：先在本目录登记（域 + 语义 + 触发条件），再在映射器中实现，并按 [注释规范](../standards/comments.md) 为映射器补直测（参照 `mythos-store` 的 `err_*` + stage_mappers 模式）。

## 工程纪律（可检查版）

每条 = 怎么做 + 怎么检查；动机见[职责边界](ts-rust-boundary.md)与 [Herta 调查](../research/001-herta.md)。预算数字只维护在本节。

1. **原子写**：普通文件覆写只用 `write_atomic` / `write_text_atomic`。步骤、哪些错误进入退避、目录目标，以及父目录同步失败时目标可能已经更新，以[存储基建](storage.md)为准。不经该 API 的写盘只有三处：SQLite 事务与在线备份、实例锁文件、JSONL 尾行截断。检查：全仓 grep 写目标文件的路径，除这三处外必须调用 `write_atomic`；`atomic` 单测覆盖成功、占用耗尽、目录目标。
2. **看门狗预算表**：

| 阶段                                                       | 预算                 | 超时动作                                                                 |
| ---------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------ |
| LLM 连接头（首字节之前）                                   | 30s                  | 取消；按可重试网络错误处理                                               |
| LLM 流式空闲                                               | 90s                  | 取消；不可重试（部分输出已被消费）                                       |
| SQLite `busy_timeout`：open、pragma、begin、commit、backup | 5s                   | 报 `store.locked`                                                        |
| 迁移 SQL 体内的 `SQLITE_BUSY`                              | 同一连接上的 5s      | 报 `store.migration`（事务回滚，`user_version` 不推进；原因放 `detail`） |
| rename 目标占用                                            | 5 次 / 25ms 指数退避 | 报 `store.locked`，清理本次 tmp                                          |

3. **重试单层化**：传输层只对尚未交付任何字节的请求重试网络错误、429 和 5xx，不重试 `llm.tls`。次数是 1 次初始请求加 2 次重试，间隔 500ms 指数、±25% 抖动。第一个已交付字节（或第一条已持久化的 delta）之后，5xx、429 和断线都不再重试。业务循环不得对同一请求再包一层重试。005 若保留空输出的温度重试，必须写成这一条的显式例外，并且不得与传输层重试叠加，也不得发生在已交付首字节之后。检查：评审计数同一请求的最大重试层数；测试断言请求次数上限，并断言首字节之后的 5xx 不再发起下一次请求。
4. **密钥隔离**：优先 OS 凭据库。凭据库不可用时，写入仅当前用户可读的文件：Unix 模式 `0600`；Windows 用只含当前用户的 ACL，不把 `0600` 当成 Windows 权限。IPC 只暴露 `{ set, hint }`，`hint` 是密钥末尾 4 个字符；短于 4 个字符时 `hint` 为空，只报告已设置。明文不进前端状态、日志、错误 `detail`。检查：序列化载荷与日志的测试不含明文；文件模式或 ACL 由写入降级文件的存储测试断言。
5. **后台任务门槛**：默认关闭；显式开启后须同时满足「闲置时长 + 距上次尝试冷却 + 素材量」门槛；用户返回时在任务边界让位；进度走事件。检查：每个后台任务在文档中列出门槛参数表。

## 与设计任务的对接位

已在本文冻结的预算、错误形状、信封、密钥规则和取消分界，下游任务只引用，不另写一套。

- **005**：[task 005 — LLM 接入与护栏](../task/005-llm-design.md)：冻结每个 `llm.*` 的触发条件、`llm_submit` / `llm_cancel` 的其余参数，以及是否启用空输出温度重试。
- **006**：[task 006 — 对局记录与上下文](../task/006-record-design.md)：定义记录追加事件，以及 `engine_get_record_page` 的分页载荷。该命令不是阶段快照。
- **012**：[task 012 — 回合与阶段状态机](../task/012-turn-state-machine-design.md)：冻结 `engine.no-scene`、`engine.invalid-phase` 的触发条件，阶段事件的 data，以及 `engine_get_phase` 的快照载荷。单回合拒绝码用本文的 `app.busy`。
