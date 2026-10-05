# 通信契约（IPC）

更新日期：2026-10-06。设计稿 v1（[task 010](../task/010-ipc-contract-design.md) 产出，评审意见已回写）。适用范围：`src-tauri` 命令层 ↔ `src-web`。谁拥有什么见[职责边界](ts-rust-boundary.md)。错误形状、事件信封和看门狗预算以本文为准。状态：**部分落地**——015 已提供命令错误映射，016 已提供序号与信封，017 已提供只读 store 命令和迁移回调。前端界面仍是 greet 占位；真实事件发送方、在飞状态和快照对齐由 020–023 实现任务承接。LLM 已评审设计见 [LLM 接入与护栏](llm.md)，尚未实现。

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

快照目前只提供静态版本，不表示迁移正在运行、成功结束或失败，也未提供事件 seq 基线。006 负责设计真实迁移流的状态、阶段和每事件序号快照，[022](../task/022-record-engine-impl.md) 在设计定稿后负责一并落地，再按下节规则对齐监听者。备份只列普通文件，跳过目录和符号链接；迁移写入保留 3 份，人工放入更多备份时命令最多列最新 50 份。

## LLM 命令与快照（005 已评审设计，尚未实现）

产品使用 engine_submit_input，llm_submit 只在开发构建注册，并与产品回合共用单在飞门禁。provider / model / 代理 / stop 由 Rust 已保存 profile 与记录语法决定，不作为随意覆盖的 invoke 参数。

| 命令         | 参数                                       | 成功返回                                                      |
| ------------ | ------------------------------------------ | ------------------------------------------------------------- |
| llm_submit   | `{ profileId, input, guardSpecId }`        | `{ turnId }`，已创建空快照后才响应                            |
| llm_cancel   | `{ turnId }`                               | `{ turnId, outcome }`，outcome 为已胜出的终态                 |
| llm_get_turn | `{ turnId }`                               | 下方 TurnSnapshot                                             |
| llm_set_key  | `{ providerId, action: "set" 或 "clear" }` | `{ set, hint }`；set 发起 Rust 原生输入，用户取消时保留旧状态 |

input 是带 kind 的联合类型：`{ kind: "completion", prompt }`，或 `{ kind: "chat", messages: [{ role, content }], assistantPrefix? }`。role 仅 system / user / assistant；非空、能力相容和 GuardSpec 校验在 Rust 完成，失败为 app.bad-request，未知 profile / guardSpec / provider / turn 为 app.not-found。调试 prompt 不进入日志。凭据输入命令属于显式用户交互，可等待原生输入结束；它不承担 LLM 长流程，也不接受明文 key 的 IPC 参数。凭据读取 / 写入失败按 store.* 映射，不伪装为已设置。

成功接纳后才产生 llm:turn:*；接纳后的异步失败由 failed 事件与快照表示，不再改写已经成功的 submit 返回值。取消完成前必须让正在提交的记录达到一致边界；已终态重复取消返回已有 outcome，不重发事件。

```ts
type TurnOutcome = "completed" | "cancelled" | "failed";
type FinishReason = "stop" | "guard" | "length";

interface TurnSnapshot {
  turnId: string;
  text: string;
  seq: {
    chunk: number;
    done: number;
    failed: number;
  };
  outcome?: TurnOutcome;
  finishReason?: FinishReason;
  error?: { code: string; message: string };
}
```

进行中省略 outcome / finishReason / error；不用 null 或 streaming。completed 必有 finishReason；failed 必有脱敏 error，其中 llm.empty-output 必有 finishReason（stop / guard / length），其他 failed 仅在已确定这些收尾原因时携带，不能填默认值；cancelled 省略 finishReason / error；三个 seq 键均必有，未产生对应事件时为 0。done / failed 的 data.chunkSeq 为该回合最后分配的 chunk 序号，监听者与自身 chunk 基线不符时先取快照再收尾，避免独立终态序号无法发现丢失正文。0 是快照确认的初始基线，事件本身仍从 1 开始。seq.chunk 对应 llm:turn:chunk 等，各自计数，不用一个总号替代。先准备载荷并预留序号，随后写盘；写入期间不对快照暴露未确认预留。写入失败废弃的预留及窗口投递失败后的序号在确认边界纳入基线，不回绕或复用；text / outcome / error 与三基线是一个原子读取的一致副本。

text 仅包含护栏后的已接纳正文。产品模式已持久化后再更新快照和投递；调试模式为内存结果。缓存只含进行中与最近已结束回合，容量与驱逐规则见 llm.md；驱逐后的旧 turnId 为 app.not-found，持久记录归 006，不靠本命令跨进程恢复。

## 记录命令与运行期迁移快照（006 设计，尚未实现）

规则与磁盘结构见[记录引擎](record-engine.md)，实现由 022 承接；下列载荷不修改当前 Rust / TS API。本节定稿后，022 在同次实现提交中同步两端类型与调用者，不能只改文档就宣称运行期接口已生效。

| 命令                   | 参数                              | 成功返回                                                                                                       |
| ---------------------- | --------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| engine_get_record_page | `{ sessionId, cursor?, limit? }`  | `{ sessionId, items: RecordItem[], nextCursor?, lastSeq, lastRecordSeq, viewEpoch }`                           |
| engine_get_record_view | `{ sessionId, limit? }`           | `{ sessionId, items: RecordItem[], nextCursor?, lastSeq, lastRecordSeq, viewEpoch, needsRecovery, inFlight? }` |
| engine_get_record_body | `{ sessionId, bodyRef, cursor? }` | `{ text, nextCursor? }`；每段最多 32 KiB，按 UTF-8 边界                                                        |
| store_get_migration    | 无（沿用现有命令）                | 下方 MigrationSnapshot；022 接入时替换当前静态形状                                                             |

记录 page / view 默认新到旧；limit 沿用总则，另有每页 512 KiB 上限。items 不含 header / partial journal，每项 `{ recordSeq, kind, createdAt, body?, bodyRef?, turnId?, outcome? }`；已知且单项能放入页时 body 为该 kind 的类型化块内容（不重复公共字段），超大或未知项仅提供元信息与 bodyRef，未知 kind 用兼容提示显示。生成块的 body 含 speakerId?、text、finishReason? / error?，outcome completed / cancelled / failed 同记录规则；其他 kind 的字段按记录引擎块表定型。nextCursor 缺省表示没有更多历史，不能用空字符串；bodyRef 为 Rust 发出的不透明内容引用，不含路径。

lastSeq 是该 session 的 engine:record:appended **事件信封确认基线**；lastRecordSeq 是正式块最高编号（初始 0），两者不可互换。viewEpoch 为本进程打开视图时生成的 UUID，重启 / 重新打开后改变；cursor 绑定它、sessionId、固定读取边界与方向。未知 session 为 app.not-found；坏 cursor / bodyRef、跨会话 / 内容 hash 失配 / 过期身份、分页越界为 app.bad-request；存在损坏为 store.corrupt。

page 只返回一个固定边界页；其 lastSeq 不表示未返回的内容已被 UI 重建。记录流缺口使用 view 替换有界最新窗口，不把 phase 当记录快照。needsRecovery 表示世界状态双写待恢复，inFlight 可选 `{ turnId, recordSeq }`，正文另取 llm_get_turn；header / 整份记录不塞进返回值。先订阅 → 有界缓存 → view → 按基线合并，旧页和新窗口按 recordSeq 去重；相同 viewEpoch 内基线不倒退。

```ts
type MigrationPhase = "idle" | "running" | "completed" | "failed";
interface MigrationSnapshot {
  phase: MigrationPhase;
  migrationId?: string;
  from?: number;
  to: number;
  current?: number;
  lastStep?: { from: number; to: number };
  failedStep?: { from: number; to: number };
  seq: { progress: number; done: number; failed: number };
  error?: { code: string; message: string };
  deliveryError?: { code: "app.event-failed"; message: string };
}
```

idle 无 migrationId，from == to == current 为静态持久化版本，三基线为 0。流启动后 migrationId 必有，to 是打开时锁定的目标 schema 版本；读取初始版本前可省略 from / current，不能用 0 假装新安装。completed 必有 from / current，current == to；failed 必有 error，current（如果已知）保留最后提交版本，failedStep 仅在步骤已知时出现。lastStep 是最后一个成功步骤。错误文案脱敏；平台失败放 deliveryError，不改变数据库真实终态。

迁移快照可反映当前 running 流及最近终态，旧流 producer 退出后清退 seq。无待迁移也产生 completed + done；只读查询不启动迁移。回调在数据库提交后更新状态 / 预留基线，非阻塞窗口队列满时允许丢投递而保留缺口；失败恢复、缓存 / 队列上界见记录引擎。开始新 migrationId 后，监听者重取当前快照，不把旧流事件应用到新流。

## 事件命名与载荷

- `<域>:<对象>:<阶段>`，全小写冒号分隔（Tauri 事件名允许 `:`）。
- 只有 `src-tauri` 调用 `emit_to("main", …)`。v1 单窗口，不做全局广播，避免未来多窗口时的载荷泄漏面。业务 crate 不得依赖 Tauri，进度经回调或通道交出普通载荷，由命令层发送。
- 信封统一：`{ seq: u64, data: T }`，序号范围为 1 到 JS 最大安全整数（2^53−1），超过上限报 `app.event-failed`，不回绕。`seq` 在「事件名 + 流标识」内单调递增。`llm:turn:chunk` 与 `llm:turn:done` 各有自己的序号，互不占号。流标识：`llm:turn:*` 用 `turnId`；一次存储打开用 migrationId 标识迁移流；引擎阶段流的标识由 012 定。
- 序号键由事件名和流标识两个独立字符串构成，空流标识仅在同一事件名下共用计数。当前条目存活到进程退出，空间随流数量增长；LLM 首个发送方接入时须按 llm.md 在回合驱逐且生产者退出后清退，旧 turnId 永不复用。同一流由发送方串行投递；分配器仅保证序号分配，不保证并发投递顺序。data 序列化失败不占号，构造成功后投递失败会留下序号缺口。
- 以下为真实事件消费方接入时必须满足的快照规则，当前还没有真实发送方：
- 监听者先注册监听并有界缓存事件，再取该流的快照，应用快照后按基线消费缓存，随后转为实时处理；同一流的快照请求串行，缓存溢出合并恢复请求；过期响应不覆盖新回合或更高基线。先取快照再注册监听会漏掉中间事件。快照给出其一致状态涵盖的每个事件名各自的最后 `seq`，监听者只取自己订阅的名字作为基线。之后该名字上 `seq <= 基线` 的事件丢掉；`seq == 基线 + 1` 才应用；出现更大的缺口就再取快照，不重放。没有基线时，第一条事件也按缺口处理，不从 0 推断。快照里的状态必须足够重绘，不能只给出最后一条 `delta`。

| 事件                                            | 快照命令                 | 状态至少包括                                                    |
| ----------------------------------------------- | ------------------------ | --------------------------------------------------------------- |
| `llm:turn:*`                                    | `llm_get_turn`           | 该 turn 已累积的文本、outcome（completed / cancelled / failed） |
| `store:migration:*`                             | `store_get_migration`    | MigrationSnapshot：流身份、目标 / 当前版本、阶段及每事件基线    |
| `engine:phase:changed`、`engine:scene:advanced` | `engine_get_phase`       | 当前阶段与场景                                                  |
| `engine:record:appended`                        | `engine_get_record_view` | 最新窗口、record / 事件位置、viewEpoch、恢复 / 在飞提示         |

记录流对齐使用 `engine_get_record_view`，见上节；`engine_get_record_page` 仍只做历史分页。

- 预留事件（仓库里还没有发送方；未冻结的字段标在条目上）：
  - `llm:turn:chunk`，data `{ turnId, delta }`（005 可加字段，不能删这两项）
  - `llm:turn:done`，data `{ turnId, outcome: "completed" | "cancelled", chunkSeq, finishReason? }`；completed 时必有 stop / guard / length，cancelled 时省略
  - `llm:turn:failed`，data `{ turnId, code, message, chunkSeq, finishReason? }`；llm.empty-output 时 finishReason 必有 stop / guard / length，其余失败仅在原因已确定时出现，与失败快照同值（错误码见下）
  - `engine:record:appended`，data `{ sessionId, recordSeq, kind, turnId? }`；流标识 sessionId，正式块完成提交后发布，不携带整段正文；监听者用 view / page 取有界内容。
  - `store:migration:progress`，data `{ migrationId, from, to, current, target }`；from / to 是本步骤版本，current == to，target 是流目标版本。
  - `store:migration:done`，data `{ migrationId, from, to, current }`；from / to 是整个流初始 / 目标版本，current == to；无步骤时 from == to。
  - `store:migration:failed`，data `{ migrationId, from?, to, current?, failedStep?, code, message }`；to 是目标版本，current 是最后成功提交版本；code 为 store.*，不发送原始 SQLite / IO 错误。失败不发 done，已提交步骤保留。open_with_progress 回调在每步提交后发生，022 必须接入运行期状态和发送；当前仍无真实发送方。
  - `engine:scene:advanced` / `engine:phase:changed`（012 冻结 data）
- 流式期间发生错误：以 `*:failed` 事件收尾；命令本身的 `Err` 只表示「提交被拒绝」，两者不重复携带同一错误。

## 错误码目录

- 形状：`{ code, message, detail? }`。`code` 是机器分支的唯一依据；`message` 是可展示中文，不参与分支；`detail` 可选结构化补充（如被拒的路径）。
- 命名空间 `<域>.<错误>`：`store.*` **已落地**——`src-tauri/src/ipc.rs` 的 `From<StoreError> for CmdError` 产出 `format!("store.{}", code())` 形态的前缀码与中文映射。命令层不得把 `code()` 的返回值再当成已带前缀。中文 `message` 由命令层映射器编写，不用 `Display`（`Display` 是英文诊断）。`llm.*` 与 `engine.*` 的码名在本文预留（映射随 018、022、023 等实现任务落地），触发条件由 005 / 006 / 012 按域冻结。`app.*` 属于命令层。
- 通用：`app.bad-request`（参数校验失败，含分页越界）、`app.not-found`（命令参数里的 id 不存在，如剧本、场景、回合）、`app.event-failed`（载荷序列化、序号分配或平台投递失败；真实监听者以快照对齐，不重试发送）、`app.not-ready`（存储尚未初始化——早于 setup 的调用；桌面正常流程不会出现）。存储路径或文件缺失只用 `store.not-found`。
- `app.busy`：命令层在进入引擎之前拒绝第二个在飞回合。引擎内部可以拒绝，对外仍映射成这一个码。不另设 `engine.turn-in-flight`。
- store：`store.invalid-path` `store.already-running` `store.locked` `store.migration` `store.disk-full` `store.permission` `store.not-found` `store.corrupt` `store.io`。
- llm 预留（005 已评审设计，映射尚未实现）：`llm.missing-key` `llm.auth` `llm.quota` `llm.rate-limited` `llm.network` `llm.tls` `llm.stalled` `llm.empty-output` `llm.bad-response` `llm.aborted`，触发条件见下表。用户 `llm_cancel` 成功时命令返回成功，并发送 `llm:turn:done`，`outcome` 为 `cancelled`。`llm.aborted` 只表示首字节之后的传输中断或空闲看门狗，不表示这次取消。

| 码               | 触发条件                                                                                                                                                                                            | 自动重试边界                                                      |
| ---------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| llm.missing-key  | 提交前该 provider 未保存密钥                                                                                                                                                                        | 不重试                                                            |
| llm.auth         | HTTP 401 或 adapter 明确识别的认证拒绝；不把未设置 key 当成 401                                                                                                                                     | 不重试                                                            |
| llm.quota        | DeepSeek HTTP 402，或 adapter 明确识别的余额 / 配额不足                                                                                                                                             | 不重试，不切模型                                                  |
| llm.rate-limited | HTTP 429；quota 类错误优先归 quota                                                                                                                                                                  | 首交付前且处于传输重试路径时可重试                                |
| llm.network      | 已识别 DNS / TCP / 连接故障、HTTP 5xx；首交付前断网或未收到 HTTP 响应的头阶段超时                                                                                                                   | 仅已排除 TLS 的可重试类别，且首交付前                             |
| llm.tls          | 证书链、主机名、TLS 握手分类确认的失败                                                                                                                                                              | 不重试，不关闭校验                                                |
| llm.stalled      | HTTP 流已建立，但头阶段到期仍无护栏后可接纳正文；心跳 / reasoning 不算正文                                                                                                                          | 首交付前的头阶段超时可按传输路径重试                              |
| llm.empty-output | 合法结束却无已交付正文，或温度阶梯耗尽；全空白不算空                                                                                                                                                | 仅 llm.md 明确的正常空输出例外；guard / length 等截断空结果不重试 |
| llm.bad-response | HTTP 400 / 404 / 422 等能力或请求拒绝、其他未识别非成功响应、SSE / JSON / UTF-8 损坏、超限、content_filter、未启用的 tool_calls、未知 finish；首交付前服务端 aborted / insufficient_system_resource | 不重试；不把协议错误当网络错误                                    |
| llm.aborted      | 已接纳字符或持久化增量后发生断网、提前 EOF、流式空闲超时，或服务端 aborted / insufficient_system_resource                                                                                           | 不重试，保留已提交前文；用户取消不使用此码                        |

HTTP 200 中的合法 usage-only / 空 choices 不属于 bad-response。未带合法 finish / 结束标记的提前 EOF 按网络中断及首交付边界分类；已解析完成的正文后还需检查终止协议。TLS 类型和供应商错误细分按 llm.md 的 fixture 标定，不解析中文 message。诊断 detail 最多携带 providerId / status / 脱敏类别，不包含 key、代理凭据、完整 endpoint、prompt 或供应商原始错误正文。

- engine 预留：`engine.no-scene` / `engine.invalid-phase` 的触发由 012 冻结；006 定义 `engine.interrupted`，仅为重开时已提交生成正文的中断记录错误，不代替实时 llm.aborted，不恢复旧 turnId。
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
| LLM 请求开始至首个护栏安全字符被接纳（含头阶段）           | 30s                  | 取消；按可重试网络错误处理                                               |
| LLM 流式空闲                                               | 90s                  | 取消；不可重试（部分输出已被消费）                                       |
| SQLite `busy_timeout`：open、pragma、begin、commit、backup | 5s                   | 报 `store.locked`                                                        |
| 迁移 SQL 体内的 `SQLITE_BUSY`                              | 同一连接上的 5s      | 报 `store.migration`（事务回滚，`user_version` 不推进；原因放 `detail`） |
| rename 目标占用                                            | 5 次 / 25ms 指数退避 | 报 `store.locked`，清理本次 tmp                                          |

3. **重试单层化**：传输层只对尚未交付任何字节的请求重试网络错误、429 和 5xx，不重试 `llm.tls`。次数是 1 次初始请求加 2 次重试，间隔 500ms 指数、±25% 抖动。第一个已交付字节（或第一条已持久化的 delta）之后，5xx、429 和断线都不再重试。业务循环不得对同一请求再包一层重试。005 的设计启用正常空输出温度重试，触发条件见 llm.md；它与传输重试互斥，每回合共享最多 3 次物理 HTTP 请求，不能各自计数后叠加。首个安全字符被共享输出写入方接纳（与窗口有无监听者无关）或增量持久化，两者任一发生即禁止重发；keep-alive / reasoning 不计交付。模式选择在提交前完成，不在预算之外试探降级。底层 reqwest 重试和 SSE 自动重连必须禁用。检查：评审计数同一请求的最大重试层数；测试断言请求次数上限，并断言首字节之后的 5xx 不再发起下一次请求。
4. **密钥隔离**：优先 OS 凭据库。凭据库不可用时，写入仅当前用户可读的文件：Unix 模式 `0600`；Windows 用只含当前用户的 ACL，不把 `0600` 当成 Windows 权限。IPC 只暴露 `{ set, hint }`，`hint` 是密钥末尾 4 个字符；短于 4 个字符时 `hint` 为空，只报告已设置。明文不进前端状态、日志、错误 `detail`。检查：序列化载荷与日志的测试不含明文；文件模式或 ACL 由写入降级文件的存储测试断言。
5. **后台任务门槛**：默认关闭；显式开启后须同时满足「闲置时长 + 距上次尝试冷却 + 素材量」门槛；用户返回时在任务边界让位；进度走事件。检查：每个后台任务在文档中列出门槛参数表。

## 与设计任务的对接位

已在本文冻结的预算、错误形状、信封、密钥规则和取消分界，下游任务只引用，不另写一套。

- **005**：[task 005 — LLM 接入与护栏](../task/005-llm-design.md)：规则与结构见 [LLM 已评审设计](llm.md)；本文集中维护每个 llm.* 的触发条件和 IPC 载荷，模式 / 护栏与重试例外见该文档。
- **006**：[task 006 — 对局记录与上下文](../task/006-record-design.md)：定义记录追加事件、记录 page / body 与有界 view 恢复，以及 engine.interrupted 的持久记录触发。它们不是阶段快照。另承接存储迁移流的 progress / done / failed、流标识、阶段、失败上下文和 store_get_migration 的每事件 seq 基线设计；实际发送与运行期状态由 [022](../task/022-record-engine-impl.md) 在设计定稿后提供。
- **012**：[task 012 — 回合与阶段状态机](../task/012-turn-state-machine-design.md)：冻结 `engine.no-scene`、`engine.invalid-phase` 的触发条件，阶段事件的 data，以及 `engine_get_phase` 的快照载荷。单回合拒绝码用本文的 `app.busy`。
