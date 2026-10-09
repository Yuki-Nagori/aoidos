# 通信契约（IPC）

更新日期：2026-10-09。设计稿 v1（[task 010](../task/010-ipc-contract-design.md) 产出，评审意见已回写）。适用范围：`src-tauri` 命令层 ↔ `src-web`。职责边界见[分层约定](ts-rust-boundary.md)；错误形状、事件信封与公共预算以本文为准。

实现状态：015–017 已提供命令错误、事件信封及只读 store 命令；018 / 019 已提供 LLM 基础与配置 / 凭据命令，三平台原生验证证据见 [019](../task/019-llm-profile-credentials-impl.md)。回合发送、在飞状态与快照对齐由 020–023 承接，前端仍为 greet 示例。下文分别标明已实现接口与设计接口。

## 总则

- 传输两种：**命令**（前端 `invoke(name, args)`，请求 / 响应）与**事件**（命令层 `emit_to("main", event, payload)`，单向推送）。窗口标签是 `src-tauri/tauri.conf.json` 的 `main`。
- 命令只回答「提交是否被接受 + 同步可得的即时结果」；长流程（生成、迁移、后台任务）的中间态一律走事件，不在命令里阻塞等待。
- 载荷一律 JSON，字段 camelCase；Rust 侧载荷结构体标 `#[serde(rename_all = "camelCase")]`，不逐字段手写 rename。
- 所有命令返回 `Result<T, CmdError>`，`CmdError` 固定序列化为 `{ code, message, detail? }`（见「错误码目录」）。
- JS number 接收的整数字段必须在安全整数范围内；纳秒时间戳等大整数用十进制字符串传输。
- 不兼容变更只换事件名。桌面应用前后端同包发布，不做字段级兼容层，信封不带版本号。

## 命令命名

- `<域>_<动词>[<宾语>]`，snake_case；域 = 消费的业务 crate：`store` / `llm` / `engine` / `theme`（026 待实现）/ `locale`（034 待实现）/ `budget`（035 待实现）。
- 动词约定：`get_` 取单值、`list_` 取列表、`set_` 替换一项配置、`create_ / update_ / delete_ / save_` 写实体、`submit_` 提交长流程、`cancel_` 取消在飞流程。
- 形参：Rust snake_case；Tauri 默认把前端 camelCase 键映射到 snake_case 形参。**两侧固定「Rust snake_case ↔ 前端 camelCase」**，不使用 `rename` 特例。
- 分页：可能超过一页的 `list_*` 使用 `{ cursor?, limit? }`，返回 `{ items, nextCursor? }`。省略 `limit` 时为 50，最大 200；`0` 或大于 200 返回 `app.bad-request`。cursor 是不透明字符串，前端只透传不解析。文档写明硬上限不超过 50 的列表可以不带分页，例如 `store_list_backups {}` 返回 `{ items }`。
- 正例：`llm_set_key`、`engine_submit_input { sessionId, text }`、`store_list_backups {}`。
- 反例：`getScriptsData`（无域前缀）、`do_thing`（动词无信息量）、`llm_generate_stream`（流式不是命令——提交用 `llm_submit`，增量走事件）。

## 语言偏好命令（028 已评审，034 待实现）

`locale_get_preference {}` / `locale_set_preference { preference }` 的同型载荷、nativeStatus 与失败语义见[国际化架构](i18n.md)。单项写入不替换主题 / UiPreferences；非法参数为 app.bad-request，持久化 / 迁移沿用 store.* / app.not-ready。原生应用失败返回 pending，不冒充持久保存失败；当前未注册这些命令。

## 费用与预算接口（027 已评审，035 待实现）

命令族、精确金额、设置 revision、查询 / 补价、物理请求身份与两作用域结算见[计价架构](billing.md)。金额一律十进制字符串 + 明确币种，不用 JS number / f64；列表复用默认 50 / 最大 200 分页。当前未注册这些 API；035 同 commit 定型 Rust / TS 载荷，不扩张现有回合事件。

## 已落地的 store 命令

Rust 的 Serialize 载荷与 `src-web/api/store.ts` 类型同步维护；invoke 只透传，不做运行时校验。命令不要求前端传入磁盘路径，setup 注入业务库路径；重复注入保留首次值。

| 命令                  | 参数 | 返回                                          | 边界                                                                              |
| --------------------- | ---- | --------------------------------------------- | --------------------------------------------------------------------------------- |
| `store_list_backups`  | 无   | `{ items: [{ path, version, nanos, size }] }` | 最新 50 项，新到旧；目录不存在为空；nanos 为 Unix epoch 纳秒字符串，size 为字节数 |
| `store_get_migration` | 无   | `MigrationSnapshot`（见下方运行期协议）       | 读取当前打开流程诊断及序号基线；查询不创建库、不运行迁移                          |

迁移运行期快照已由 022 接入，反映当前流程或最近终态；监听者按下方运行期协议对齐。备份只列普通文件，跳过目录和符号链接；迁移写入保留 3 份，人工放入更多备份时命令最多列最新 50 份。

## LLM 命令与快照

005 的设计已评审；019 已实现配置与凭据命令，020 已接入回合命令 / 事件 / 内存快照，验收状态见任务记录。

产品使用 engine_submit_input，llm_submit 只在开发构建注册，并与产品回合共用单在飞门禁。provider / model / 代理 / stop 由 Rust 已保存 profile 与记录语法决定，不作为随意覆盖的 invoke 参数。

已落地（019）：profile 管理与凭据命令；`LlmProfile` 的 TS 同型见 `src-web/api/llm.ts`。

| 命令               | 参数                                       | 成功返回                  | 边界                                                                                                                                                                                 |
| ------------------ | ------------------------------------------ | ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| llm_list_profiles  | 无                                         | `{ items: LlmProfile[] }` | 硬上限 50 个、无分页；目录缺失为空列表                                                                                                                                               |
| llm_save_profile   | `{ profile: LlmProfile }`                  | 已持久化的 LlmProfile     | 同 id 覆盖、不存在追加；结构非法（空 id / model、代理 URL 形态）或数量超 50 为 app.bad-request；能力匹配不在此校验（020 提交时）                                                     |
| llm_delete_profile | `{ profileId }`                            | `{ deleted: true }`       | 幂等，不存在也成功                                                                                                                                                                   |
| llm_get_key_status | `{ providerId }`                           | `{ set, hint }`           | 只报设置状态，不读明文；凭据后端不可读为 store.*                                                                                                                                     |
| llm_set_key        | `{ providerId, action: "set" 或 "clear" }` | `{ set, hint }`           | set 发起 Rust 原生凭据输入（Windows CredUI / macOS AppKit / Linux GTK，同型 UI 主线程入口；重复输入 app.busy，无会话 store.io，不降级为 Webview 明文）；用户取消保留旧值；clear 幂等 |

凭据命令的 `providerId` 与 profile 共用结构校验：非空白、最多 256 字节、无控制字符；非法输入在原生交互及后端操作前返回 `app.bad-request`。允许自定义标识符及代理认证引用，不限于内置供应商。

```ts
type ProfileMode = "completion" | "chat";

interface Sampling {
  temperature: number;
  maxTokens: number;
}

type ProxyConfig =
  { mode: "system" } | { mode: "none" } | { mode: "manual"; url: string; authRef?: string };

interface LlmProfile {
  profileId: string;
  providerId: string;
  model: string;
  mode: ProfileMode;
  thinking: boolean;
  sampling: Sampling;
  proxy: ProxyConfig;
}
```

020 已实现 `llm_submit` / `llm_cancel` / `llm_get_turn`。开发提交仅接受 `debug-fixture-v1`，复用已保存配置 / 凭据的冻结与能力校验，但模型执行由本地夹具替换。生产构建不编译或注册 submit；022 持久化已就绪；真实付费联调与费用控制分别等待 024 / 035。

| 命令         | 参数                                | 成功返回                                      |
| ------------ | ----------------------------------- | --------------------------------------------- |
| llm_submit   | `{ profileId, input, guardSpecId }` | `{ turnId }`，已创建空快照后才响应            |
| llm_cancel   | `{ turnId }`                        | `{ turnId, outcome }`，outcome 为已胜出的终态 |
| llm_get_turn | `{ turnId }`                        | 下方 TurnSnapshot                             |

input 是带 kind 的联合类型：`{ kind: "completion", prompt }`，或 `{ kind: "chat", messages: [{ role, content }], assistantPrefix? }`。role 仅 system / user / assistant；非空、能力相容和 GuardSpec 校验在 Rust 完成，失败为 app.bad-request，未知 profile / guardSpec / provider / turn 为 app.not-found。Tauri 边界先解码 JSON 为 Rust 联合类型，未知 role / 字段返回统一 app.bad-request，避免框架透出字符串错误；TS 维持同型联合类型。调试 prompt 不进入日志。凭据输入命令属于显式用户交互，可等待原生输入结束；它不承担 LLM 长流程，也不接受明文 key 的 IPC 参数。凭据读取 / 写入失败按 store.* 映射，不伪装为已设置。

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

021 的 `useLlmTurn` 消费上述载荷；读取错误保存在独立 recoveryError，不改写回合 error。subscribe / snapshot 竞态、终态补正文和最后全事件丢失的实际边界见 [LLM 恢复规则](llm.md)。

## 引擎命令与阶段快照（012 设计，023 已实现）

规则以[阶段机](turn-state-machine.md)为准，023 实现；phase、场景和操作终态事件均用 sessionId 分流。roundId 是游戏回合，turnId 是单次 LLM 调用；内部提议不发 llm:turn:*，不出现在公开 llm_get_turn，旁白 / 角色仍沿用上节。

下列命令已由串行驱动实现，Rust / TS 同型；阶段发布、四事件消费与恢复的验证证据见 [023](../task/023-turn-state-machine-impl.md)。可信场景、世界解释器与配置由 Rust 域登记，实际产品接线由 024 / 025 承接。

| 命令                | 参数                             | 成功返回 / 接纳                                                                |
| ------------------- | -------------------------------- | ------------------------------------------------------------------------------ |
| engine_submit_input | `{ sessionId, text }`            | `{ operationId, roundId }`；输入与接纳事实提交后返回                           |
| engine_interrupt    | `{ sessionId, roundId, text }`   | `{ operationId }`；接纳取消并换回合的请求，新 round 提交后从快照获取           |
| engine_cancel_round | `{ sessionId, roundId }`         | `{ roundId, outcome }`；待已开始提交达到一致边界后返回胜出终态                 |
| engine_resume       | `{ sessionId }`                  | `{ operationId, roundId }`；从暂停检查点显式恢复                               |
| engine_regenerate   | `{ sessionId, roundId }`         | `{ operationId, roundId }`；返回新回合身份，始终复用已有骰值；无骰回合继续无骰 |
| engine_rewind       | `{ sessionId, targetSeq }`       | `{ operationId }`；后续控制提交 / 世界重放走事件                               |
| engine_submit_check | `{ sessionId, roundId, planId }` | `{ roundId, planId, accepted: true }`；同一 plan 重复返回，不重复采样          |
| engine_get_phase    | `{ sessionId }`                  | 下方 PhaseSnapshot                                                             |

text 为非空白字符串，最多 32 KiB UTF-8，保留原文；角色内 / 场外前缀及内容范围由 012 的 Rust 解析，不增加前端可覆盖身份 / 规则的参数，不接受模型 / 骰式 / 状态覆盖参数。所有身份由 Rust 分配，operationId 不是客户端重试幂等键；收到接纳后不自动重发命令。已在飞时新提交 busy；interrupt / cancel / submit_check 仅控制指定的本 round。其他合法性、错误优先级和 rewind 检查点见阶段机。cancel 的重复返回限于本进程有界缓存，旧 round 驱逐为 not-found；暂停恢复由持久检查点决定，不靠 ring 跨进程恢复。

```ts
type EnginePhase = "idle" | "generating" | "awaitingCheck" | "settling" | "advancing";
interface ScenePosition {
  sceneId: string;
  path: { kind: string; id: string; title: string }[];
}
interface PhaseState {
  sessionId: string;
  stateEpoch: string;
  phaseRevision: number;
  historyRevision: number;
  phase: EnginePhase;
  scene?: ScenePosition;
  inFlight?: { operationId: string; roundId?: string; turnId?: string };
  check?: {
    planId: string;
    status: "waiting" | "rolling";
    mode: "manual" | "auto";
    ruleId: string;
    actorId: string;
    expression: string;
    modifierTotal: number;
  };
  needsRecovery: boolean;
  resumeRequired: boolean;
  // 暂停位置的事实锚点；派生表归阶段机，不用最后物理 seq 替代。
  checkpoint?: {
    sourceRoundId: string;
    throughSeq: number;
    stage: "check" | "narration" | "settle" | "advance";
  };
  lastOperation?: {
    operationId: string;
    roundId?: string;
    outcome: "accepted" | "completed" | "cancelled" | "failed";
    error?: { code: string; message: string };
  };
}
interface PhaseSnapshot extends PhaseState {
  seq: {
    phaseChanged: number;
    sceneAdvanced: number;
    operationDone: number;
    operationFailed: number;
  };
}
```

check 仅在 awaitingCheck 的活动 / 暂停计划中出现，摘要由 Rust 核验；暂停时按钮先要求 resume，取得活动 roundId 后才能提交。accepted 只说明判定动作已接纳，不表示骰值已落盘；后续错误归当前 operation 的 failed 与快照，不新增一次操作终态。合法 round / plan 的重复接纳限当前及有界终态缓存；未知身份 not-found，跨 round 的 plan 为 bad-request，非待判定且未曾接纳为 invalid-phase。

所有可选项省略，不用 null；无活动场景省略 scene，不造空路径。inFlight 表示当前 lease 所有者，rewind 可无 roundId，内部提议可无公开 turnId；resumeRequired 表示暂停且无在飞请求，必须有按阶段机派生表核验的 checkpoint（stage 为下一缺失工作、throughSeq 为指定确认事实），不能与 inFlight 同时为真。lastOperation.failed 必有脱敏 error，其他结果省略；新接纳替换上一结果，不保留旧 error。回合终态和当前 phase 独立：骰判后失败可暂停在 settling。needsRecovery 阻止恢复 / 新行动，但 get_phase 与诊断查询可用。PhaseState 的单次事件载荷最多 64 KiB，scene.path 最大深度 16，title 最多 256 字节，脱敏 error.message 最多 512 字节；超限不得预留 / 发布载荷。

stateEpoch 在每次打开 session 时生成 UUID；phaseRevision 初始 0，每次确认可观察状态变化递增；historyRevision 为最新已 applied historyFork 的正式 seq，根为 0。公开叙事先建立空 llm_get_turn 快照，再确认 / 发布 inFlight.turnId，随后启动网络请求。四基线必有，未产生事件为 0；各自对应下文四个事件，不能以 revision 替代信封 seq。同次确认可产生多个不同名字事件并共用 revision，各事件 data 均携带完整 PhaseState；phase 快照不含正文，正文恢复使用 llm_get_turn / record view。

- engine:phase:changed：data 为 PhaseState，公开 PhaseState 变化时发，包括 phase、inFlight 身份、检查点与恢复状态的变化；同阶段新公开 turnId 必须通知，私有内部步骤不改变公开状态时不发。正文 token 增量不触发该事件。
- engine:scene:advanced：data 为 `{ ...PhaseState, previousSceneId? }`；真实切场或 sessionEnded 时发，结束后的 scene 省略；留场不发。
- engine:operation:done：data 为 `{ ...PhaseState, operationId, outcome: "completed" | "cancelled" }`，operationId 标识此次完成；lastOperation 仍描述最近接纳操作，旧操作完成不能覆盖新接纳结果。
- engine:operation:failed：data 为 `{ ...PhaseState, operationId, code, message }`，顶层 code / message 是此次 operation 的脱敏失败；仅当它仍是最近接纳操作时更新 lastOperation/failed 的同值 error。

接纳的长流程 done / failed 二选一恰一次；未接纳只返回 Err。cancel 不另造 operationId，用被取消流程的终态事件；interrupt 接纳后新 operation 负责换回合结果，旧 operation 正常取消收尾。PhaseState 的完整载荷只能在对应事实 / applied 确认后发布，最后事件全部丢失仍需重连或主动 get_phase，不暗加轮询。前端一 session 一个快照请求、最多 32 条缓存；按 seq 对齐，再按 phaseRevision 防止跨事件名乱序倒退。

## 界面偏好（008 设计，022 已实现）

| 命令                     | 参数                        | 成功返回                                       |
| ------------------------ | --------------------------- | ---------------------------------------------- |
| store_get_ui_preferences | 无                          | UiPreferences                                  |
| store_set_ui_preferences | `{ panelPinned, diceMode }` | 已持久化确认的 UiPreferences；完整替换两个偏好 |

UiPreferences 为 `{ version: 1, panelPinned: boolean, diceMode: "manual" | "auto" }`，首次默认为 false / manual；未知枚举、额外可写字段为 app.bad-request，持久化失败为对应 store.*。Rust 拥有持久化，前端不写文件。偏好修改不改变当前 round 的冻结值；任务 022 接入存储，023 在接纳时使用，025 界面实施消费该类型。源 schema / 配置升级由存储规范约束，不能把焦点、草稿或凭据塞进偏好。

## 主题与皮肤命令（009 设计，尚未实现）

[主题架构](theming.md)维护加载、目录、预算和防闪，026 实现。所有返回与 Rust 类型同型，当前不新增实际 invoke 注册。

| 命令                 | 参数                           | 成功返回                                          |
| -------------------- | ------------------------------ | ------------------------------------------------- |
| theme_get_preference | 无                             | ThemePreference                                   |
| theme_set_preference | `{ theme: "dark" 或 "light" }` | 已持久确认的 ThemePreference，仅更新主题项        |
| theme_skin_load      | `{ scriptId }`                 | SkinLoadResult，返回规范常量，不返回原 CSS 或路径 |

```ts
interface ThemePreference {
  version: 1;
  theme: "dark" | "light";
}
// 原生首窗注入的只读值；降级值不是已持久化确认的偏好。
interface ThemeBootstrap {
  version: 1;
  theme: "dark" | "light";
  fallbackReason?: "storageUnavailable" | "invalidPreference";
}
interface SkinToken {
  name: string;
  value: string;
}
interface SkinWarning {
  code:
    | "unknown-token"
    | "protected-token"
    | "invalid-declaration"
    | "invalid-value"
    | "invalid-reference"
    | "cyclic-reference";
  mode?: "dark" | "light";
  token?: string;
  line?: number;
}
interface SkinLoadResult {
  scriptId: string;
  status: "missing" | "valid";
  sourceHash?: string;
  tokens: { dark: SkinToken[]; light: SkinToken[] };
  warnings: SkinWarning[];
  warningsTruncated: boolean;
}
```

ThemeBootstrap 由 026 在窗口创建前注入，非 invoke 返回，也不带原 CSS / 路径 / 错误正文；普通浏览器开发确定性用 dark。首次缺偏好为 dark，未知持久枚举为 store.corrupt；非法 theme / scriptId 参数为 app.bad-request，未登记 scriptId 为 app.not-found。目录合法但 theme.css 不存在为 missing，省略 sourceHash，返回空两套 / 空 warnings；读取失败为 store.*。valid 必有 SHA-256 小写十六进制 sourceHash，空文件 / 全部声明被局部丢弃可返回空映射。结构 / 硬预算错误返回 theme.invalid-skin，无部分 tokens；前端收到失败须清退旧皮肤。

SkinToken.name 只取主题目录，value 只含 Rust 规范化常量，按目录顺序输出，每模式每名称最多一次。warning.token 仅目录名或受限 ASCII 候选（最多 64 字节），非安全名称省略，不泄露原值 / 路径；line 为 1 起安全整数。载荷 / warning / 缓存限额只在主题架构维护。theme 域无后台任务或事件流，响应即是已确认结果，不借主题切换取得游戏回合 lease。

## 记录命令与运行期迁移快照（006 设计，022 已实现）

规则与磁盘结构见[记录引擎](record-engine.md)，实现由 022 承接；以下载荷已由 022 同步实现 Rust / TS API；产品记录创建与阶段决策仍由 023 接入。

| 命令                   | 参数                              | 成功返回                                                                                                       |
| ---------------------- | --------------------------------- | -------------------------------------------------------------------------------------------------------------- |
| engine_get_record_page | `{ sessionId, cursor?, limit? }`  | `{ sessionId, items: RecordItem[], nextCursor?, lastSeq, lastRecordSeq, viewEpoch }`                           |
| engine_get_record_view | `{ sessionId, limit? }`           | `{ sessionId, items: RecordItem[], nextCursor?, lastSeq, lastRecordSeq, viewEpoch, needsRecovery, inFlight? }` |
| engine_get_record_body | `{ sessionId, bodyRef, cursor? }` | `{ text, nextCursor? }`；每段最多 32 KiB，按 UTF-8 边界                                                        |
| store_get_migration    | 无（沿用现有命令）                | 下方 MigrationSnapshot；022 已替换静态形状                                                                     |

记录 page / view 默认新到旧；limit 沿用总则，另有每页 512 KiB 上限。items 不含 header / partial journal，每项 `{ recordSeq, kind, createdAt, body?, bodyRef?, turnId?, outcome? }`；已知且单项能放入页时 body 为该 kind 的类型化块内容（不重复公共字段），超大或未知项仅提供元信息与 bodyRef，未知 kind 用兼容提示显示。生成块的 body 含 speakerId?、text、finishReason? / error?，outcome completed / cancelled / failed 同记录规则；其他 kind 的字段按记录引擎块表定型。nextCursor 缺省表示没有更多历史，不能用空字符串；bodyRef 为 Rust 发出的不透明内容引用，不含路径。

lastSeq 是该 session 的 engine:record:appended **事件信封确认基线**；lastRecordSeq 是正式块最高编号（初始 0），两者不可互换。viewEpoch 为本进程打开视图时生成的 UUID，重启 / 重新打开后改变；cursor 绑定它、sessionId、固定读取边界与方向；historyFork applied 后更换 viewEpoch，分页读取当前有效因果路径，原始物理历史保留供导出。未知 session 为 app.not-found；有效 bodyRef 指向不存在的 recordSeq 为 app.not-found；坏 cursor / bodyRef、跨会话 / 内容 hash 失配 / 过期身份、分页越界为 app.bad-request；存在损坏为 store.corrupt。

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
- 信封统一：`{ seq: u64, data: T }`，序号范围为 1 到 JS 最大安全整数（2^53−1），超过上限报 `app.event-failed`，不回绕。`seq` 在「事件名 + 流标识」内单调递增。`llm:turn:chunk` 与 `llm:turn:done` 各有自己的序号，互不占号。流标识：`llm:turn:*` 用 `turnId`；一次存储打开用 migrationId 标识迁移流；引擎阶段 / 场景 / 操作终态流用 sessionId，跨 roundId 连续。
- 序号键由事件名和流标识两个独立字符串构成，空流标识仅在同一事件名下共用计数。当前条目存活到进程退出，空间随流数量增长；LLM 首个发送方接入时须按 llm.md 在回合驱逐且生产者退出后清退，旧 turnId 永不复用。同一流由发送方串行投递；分配器仅保证序号分配，不保证并发投递顺序。data 序列化失败不占号，构造成功后投递失败会留下序号缺口。
- 以下为真实事件消费方接入时必须满足的快照规则，当前还没有真实发送方：
- 监听者先注册监听并有界缓存事件，再取该流的快照，应用快照后按基线消费缓存，随后转为实时处理；同一流的快照请求串行，缓存溢出合并恢复请求；过期响应不覆盖新回合或更高基线。先取快照再注册监听会漏掉中间事件。快照给出其一致状态涵盖的每个事件名各自的最后 `seq`，监听者只取自己订阅的名字作为基线。之后该名字上 `seq <= 基线` 的事件丢掉；`seq == 基线 + 1` 才应用；出现更大的缺口就再取快照，不重放。没有基线时，第一条事件也按缺口处理，不从 0 推断。快照里的状态必须足够重绘，不能只给出最后一条 `delta`。

| 事件                                                                  | 快照命令                 | 状态至少包括                                                    |
| --------------------------------------------------------------------- | ------------------------ | --------------------------------------------------------------- |
| `llm:turn:*`                                                          | `llm_get_turn`           | 该 turn 已累积的文本、outcome（completed / cancelled / failed） |
| `store:migration:*`                                                   | `store_get_migration`    | MigrationSnapshot：流身份、目标 / 当前版本、阶段及每事件基线    |
| `engine:phase:changed`、`engine:scene:advanced`、`engine:operation:*` | `engine_get_phase`       | PhaseSnapshot：阶段、场景、检查点、操作结果及四基线             |
| `engine:record:appended`                                              | `engine_get_record_view` | 最新窗口、record / 事件位置、viewEpoch、恢复 / 在飞提示         |

记录流对齐使用 `engine_get_record_view`，见上节；`engine_get_record_page` 仍只做历史分页。

- 预留事件（仓库里还没有发送方；未冻结的字段标在条目上）：
  - `llm:turn:chunk`，data `{ turnId, delta }`（005 可加字段，不能删这两项）
  - `llm:turn:done`，data `{ turnId, outcome: "completed" | "cancelled", chunkSeq, finishReason? }`；completed 时必有 stop / guard / length，cancelled 时省略
  - `llm:turn:failed`，data `{ turnId, code, message, chunkSeq, finishReason? }`；llm.empty-output 时 finishReason 必有 stop / guard / length，其余失败仅在原因已确定时出现，与失败快照同值（错误码见下）
  - `engine:record:appended`，data `{ sessionId, viewEpoch, recordSeq, kind, turnId? }`；流标识 sessionId，正式块完成提交后发布，不携带整段正文；监听者用 view / page 取有界内容。
  - `store:migration:progress`，data `{ migrationId, from, to, current, target }`；from / to 是本步骤版本，current == to，target 是流目标版本。
  - `store:migration:done`，data `{ migrationId, from, to, current }`；from / to 是整个流初始 / 目标版本，current == to；无步骤时 from == to。
  - `store:migration:failed`，data `{ migrationId, from?, to, current?, failedStep?, code, message }`；to 是目标版本，current 是最后成功提交版本；code 为 store.*，不发送原始 SQLite / IO 错误。失败不发 done，已提交步骤保留。open_with_progress 回调在每步提交后发生，022 已接入运行期状态与真实窗口发送适配；完成证据见任务记录。
  - `engine:scene:advanced` / `engine:phase:changed` / `engine:operation:done` / `engine:operation:failed`：data 见上方引擎阶段快照，流标识 sessionId，012 定稿、023 已实现。
- 流式期间发生错误：以 `*:failed` 事件收尾；命令本身的 `Err` 只表示「提交被拒绝」，两者不重复携带同一错误。

## 错误码目录

- 形状：`{ code, message, detail? }`。`code` 是机器分支的唯一依据；`message` 是可展示中文，不参与分支；`detail` 可选结构化补充（如被拒的路径）。
- 命名空间 `<域>.<错误>`：`store.*` **已落地**——`src-tauri/src/ipc.rs` 的 `From<StoreError> for CmdError` 产出 `format!("store.{}", code())` 形态的前缀码与中文映射。命令层不得把 `code()` 的返回值再当成已带前缀。中文 `message` 由命令层映射器编写，不用 `Display`（`Display` 是英文诊断）。`theme.*`（026）、`engine.*` 的码名在本文预留（映射随 022、023 等实现任务落地）。`llm.*` 十码分工：传输类 7 码（auth / quota / rate-limited / network / tls / bad-response / aborted）已由 018 的 `aoidos-llm::ProviderError` 落地，`code()` 返回裸码、交付边界定码见 `code_at_boundary`；`stalled` / `empty-output` 由调度层 `RunError::Stalled` / `RunError::EmptyOutput` 变体承载（无 `code()`，020 域错误映射补码）；`missing-key` 是提交前未保存密钥的命令层判定，不在 crate 内。020 域错误与薄命令适配已统一加 `llm.` 前缀。`app.*` 属于命令层。
- 通用：`app.bad-request`（参数校验失败，含分页越界）、`app.not-found`（命令参数里的 id 不存在，如剧本、场景、回合）、`app.event-failed`（载荷序列化、序号分配或平台投递失败；真实监听者以快照对齐，不重试发送）、`app.not-ready`（所需业务存储未开放：初始化尚未完成，或运行期迁移失败后被冻结；后一种由 022 的共享 Storage 门禁实现。普通业务命令拒绝，但 store_get_migration 诊断仍可用；前端主动取该快照展示脱敏迁移失败，不持续轮询）。存储路径或文件缺失只用 `store.not-found`。
- `app.busy`：命令层在进入引擎之前拒绝第二个在飞回合；019 同型原生输入门禁也用此码拒绝第二个密码框，不占用业务 key 锁。引擎内部可以拒绝，对外仍映射成这一个码。不另设 `engine.turn-in-flight`。
- store：`store.invalid-path` `store.already-running` `store.locked` `store.migration` `store.disk-full` `store.permission` `store.not-found` `store.corrupt` `store.io`。
- theme 预留（009 设计，026 待实现）：`theme.invalid-skin` 表示存在的皮肤结构 / 硬预算不合法；缺文件为成功 missing、单条语义失败为 warnings、读取失败为 store.*，不自动重试。
- budget 预留（027 已评审，035 待实现）：`budget.exceeded`（本地费用不足）、`budget.price-missing`（未登记 / 必需价格缺失）、`budget.fx-missing`（无有效换汇）、`budget.invalid-usage`（费用诊断中的非法用量，不撤回合法正文终态）；结构化 detail 与触发语义见[计价架构](billing.md)。不自动重试，不冒充供应商 llm.quota；旧配置 / 游标使用 app.bad-request + detail.reason=staleRevision，存储失败沿用 store.*。
- llm 预留（005 已评审设计；crate 侧类别 018 已落地，020 已接入域错误 / 命令映射）：`llm.missing-key` `llm.auth` `llm.quota` `llm.rate-limited` `llm.network` `llm.tls` `llm.stalled` `llm.empty-output` `llm.bad-response` `llm.aborted`，触发条件见下表。用户 `llm_cancel` 成功时命令返回成功，并发送 `llm:turn:done`，`outcome` 为 `cancelled`。`llm.aborted` 只表示首字节之后的传输中断或空闲看门狗，不表示这次取消。

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

- engine（012 已冻结，023 已实现）：`engine.no-scene` 为未载入 / 已结束而无活动场景；`engine.invalid-phase` 为命令不适用于当前阶段、无合法恢复检查点或 pending 世界 / 控制意图未修复；与其他在飞 lease 冲突仍为 app.busy。详见阶段机的错误优先级。006 定义 `engine.interrupted`，仅为重开时已提交生成正文的中断记录错误，不代替实时 llm.aborted，不恢复旧 turnId。
- 前端分支**正例**：

```ts
if (err.code === "llm.rate-limited") showRetryLater(err);
else if (err.code.startsWith("llm.")) showLlmError(err);
else showGenericError(err);
```

- 前端分支**反例**：`err.message.includes("rate")`（文案漂移即坏）；`switch (err.code)` 不写 default。
- 新增码的流程：先在本目录登记（域 + 语义 + 触发条件），再在映射器中实现，并按 [注释规范](../standards/comments.md) 为映射器补直测（参照 `aoidos-store` 的 `err_*` + stage_mappers 模式）。

007 的[拟实施记忆端口](memory.md#高级设置与跨端接口)尚未注册 Tauri 命令，不改变本目录现有 API。参数 / 过期版本使用 app.bad-request、未知查询身份 app.not-found、门禁 app.busy、持久损坏 store.corrupt，网络错误沿用 `llm.*`；内部候选拒绝 reason 不作为供应商 quota。所需旧策略不可用时，拟端口返回 app.not-ready，保留原数据并开放只读诊断，不用 store.corrupt 指代仅版本不受支持。具体 DTO / 命令由后续实施同 commit 展开到本目录，遵循 Rust / TS 同型，不新增 `memory:*` 事件流或持续轮询。

## 工程纪律（可检查版）

每条 = 怎么做 + 怎么检查；动机见[职责边界](ts-rust-boundary.md)与 [Herta 调查](../research/001-herta.md)。预算数字只维护在本节。

1. **原子写**：普通文件覆写只用 `write_atomic` / `write_text_atomic`。步骤、哪些错误进入退避、目录目标，以及父目录同步失败时目标可能已经更新，以[存储基建](storage.md)为准。原地写例外及 JSONL 受控追加 / 截断的唯一原语，以[存储基建的原子写工具](storage.md#原子写工具全仓唯一实现)为准。检查：全仓搜索写目标文件的路径，普通文件覆写必须调用上述 API，原地写必须属于该清单并复用对应原语；`atomic` 单测覆盖成功、占用耗尽、目录目标。
2. **看门狗预算表**：

| 阶段                                                       | 预算                 | 超时动作                                                                 |
| ---------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------ |
| LLM 请求开始至首个护栏安全字符被接纳（含头阶段）           | 30s                  | 取消；按可重试网络错误处理                                               |
| LLM 流式空闲                                               | 90s                  | 取消；不可重试（部分输出已被消费）                                       |
| SQLite `busy_timeout`：open、pragma、begin、commit、backup | 5s                   | 报 `store.locked`                                                        |
| 迁移 SQL 体内的 `SQLITE_BUSY`                              | 同一连接上的 5s      | 报 `store.migration`（事务回滚，`user_version` 不推进；原因放 `detail`） |
| rename 目标占用                                            | 5 次 / 25ms 指数退避 | 报 `store.locked`，清理本次 tmp                                          |

3. **重试单层化**：传输层只对尚未交付任何字节的请求重试网络错误、429 和 5xx，不重试 `llm.tls`。次数是 1 次初始请求加 2 次重试，间隔 500ms 指数、±25% 抖动。第一个已交付字节（或第一条已持久化的 delta）之后，5xx、429 和断线都不再重试。业务循环不得对同一请求再包一层重试。005 的设计启用正常空输出温度重试，触发条件见 llm.md；它与传输重试互斥，每回合共享最多 3 次物理 HTTP 请求，不能各自计数后叠加。首个安全字符被共享输出写入方接纳（与窗口有无监听者无关）或增量持久化，两者任一发生即禁止重发；keep-alive / reasoning 不计交付。模式选择在提交前完成，不在预算之外试探降级。底层 reqwest 重试和 SSE 自动重连必须禁用。检查：评审计数同一请求的最大重试层数；测试断言请求次数上限，并断言首字节之后的 5xx 不再发起下一次请求。
4. **密钥隔离**：优先 OS 凭据库。凭据库不可用时，写入仅当前用户可读的文件：Unix 模式 `0600`；Windows 用只含当前用户的 ACL，不把 `0600` 当成 Windows 权限。IPC 只暴露 `{ set, hint }`，`hint` 是密钥末尾 4 个字符；短于 4 个字符时 `hint` 为空，只报告已设置。明文不进前端状态、日志、错误 `detail`。写入优先 OS 库；credentials.json v2 原子记录每个条目的权威后端：OS 指针（无明文）、降级文件值或删除墓碑。读取只按确认后端取值，OS 写失败后新文件值不会被 OS 的旧值覆盖或清除；OS 读取失败返回 store.*，不伪装未设置。删除先发布墓碑再清 OS，清理失败仍报错但旧值不会在重启后复活。旧版无版本字典按文件值读取、下一次写入升级；不猜测另一后端的先后。私有写入必须先对空临时文件设置 0600 / DACL，再写正文、fsync 与发布，权限失败不发布新值。OS 与文件发布之间不具备分布式原子性：错误可表示提交不确定，应主动查状态而不是重复写猜测。检查：序列化载荷与日志的测试不含明文；文件模式或 ACL 由写入降级文件的存储测试断言。
5. **后台任务门槛**：默认关闭；显式开启后须同时满足「闲置时长 + 距上次尝试冷却 + 素材量」门槛；用户返回时在任务边界让位；进度走事件。检查：每个后台任务在文档中列出门槛参数表。

## 与设计任务的对接位

已在本文冻结的预算、错误形状、信封、密钥规则和取消分界，下游任务只引用，不另写一套。

- **005**：[task 005 — LLM 接入与护栏](../task/005-llm-design.md)：规则与结构见 [LLM 已评审设计](llm.md)；本文集中维护每个 llm.* 的触发条件和 IPC 载荷，模式 / 护栏与重试例外见该文档。
- **006**：[task 006 — 对局记录与上下文](../task/006-record-design.md)：定义记录追加事件、记录 page / body 与有界 view 恢复，以及 engine.interrupted 的持久记录触发。它们不是阶段快照。存储迁移流的 progress / done / failed、流标识、阶段、失败上下文和 store_get_migration 每事件 seq 基线已定稿；实际发送与运行期状态由 [022](../task/022-record-engine-impl.md) 实现。
- **012**：[task 012 — 回合与阶段状态机](../task/012-turn-state-machine-design.md)：冻结 `engine.no-scene`、`engine.invalid-phase` 的触发条件，阶段事件的 data，以及 `engine_get_phase` 的快照载荷。单回合拒绝码用本文的 `app.busy`。

### 记录命令的完整参数校验

记录 page / view / body 与 UI 偏好写入通过完整 camelCase DTO 解码请求对象；字段类型错误、额外字段、非 JSON 请求返回 `app.bad-request`，不透传框架字符串错误。命令只适配解码、ready 门禁、业务调用及 CmdError；磁盘工作由共享 blocking 边界处理。迁移诊断在业务存储不可用时仍可读取，不触发新迁移。

自动请求因 Token 估算需要重新标定而暂停时返回 `engine.invalid-phase`；其语义属于自动请求门禁，不使用费用 `budget.exceeded`。实时主动取消仍不使用 `engine.interrupted`，后者只用于重开恢复的中断正文记录。
