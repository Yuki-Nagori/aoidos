# 016 — 实现：IPC 事件基建（信封、每流 seq、emit 适配）

- 状态：ready
- 依赖：010、015
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

[通信契约](../architecture/ipc-contract.md)冻结了事件形状：`<域>:<对象>:<阶段>` 命名、`{ seq, data }` 信封、seq 按「事件名 + 流标识」单调递增、监听者以快照对齐且不重放。本任务把事件发送侧做成基座，让 005（`llm:turn:*`）与 017（`store:migration:*`）只声明事件名与 data，不重复实现序号与信封。

## 必读

[通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md)（业务 crate 不得依赖 Tauri，进度经无 Tauri 类型的出口交出）· [注释规范](../standards/comments.md)。

## 范围与非目标

交付（`src-tauri/src/events.rs`）：

1. 每流 seq 分配器：`StreamSeqs`（`Mutex<HashMap<String, u64>>`），键为 `"{event}:{stream_id}"`；同流单调递增，异流互不占号。
2. 信封构造：`envelope(seq, data) -> serde_json::Value`，形状 `{ seq, data }`（无版本号，契约总则）。
3. emit 适配：`emit_to_main(app, event, seq, data)`——拼信封后 `emit_to("main", …)`；错误映射为 `CmdError`（码 `app.*`，如 `app.event-failed`，登记进契约错误码目录）。
4. 可测试性：seq 与信封为纯函数直测；emit 适配用 `tauri::test` 的 mock app 执行真实 `emit_to` 路径（src-tauri dev-deps 增加 tauri `test` feature），并断言 mock 窗口收到载荷。

非目标：具体事件与快照命令（005 / 006 / 012 / 017）；业务 crate 的回调 trait 形态（随第一个真实发送方在 005 定，本任务只保证命令层一侧就绪）。

## 实施步骤

1. `events.rs`：StreamSeqs + envelope + emit_to_main；单测覆盖「同流递增 / 异流独立 / 缺口语义由监听者处理（本层只保证 seq 单调）」。
2. src-tauri dev-deps 加 tauri `test` feature；mock app + `main` 窗口监听断言。
3. 契约错误码目录补 `app.event-failed` 行。

## 预计改动

修改 `src-tauri/src/ipc.rs` 或新建 `src-tauri/src/events.rs`（实现时定，倾向独立文件）、`src-tauri/Cargo.toml`（dev feature）、`ai-docs/architecture/ipc-contract.md`（错误码目录 +1 行）。

## 验收标准

- [ ] 同流 seq 单调、异流互不占号（单测断言）。
- [ ] mock 窗口收到 `{ seq, data }` 信封（单测断言）。
- [ ] `app.event-failed` 登记进契约错误码目录。
- [ ] `bun run verify` 十项 exit 0（行覆盖含新模块 100%）。

## 验证计划与结果

| 日期 | 命令         | 预期 | 实际结果 |
| ---- | ------------ | ---- | -------- |
| —    | 待开始后填写 | —    | 未执行   |

## 风险与回退

tauri `test` feature 的 mock app 在 Windows 上的行为差异——只在 dev-deps 与测试中使用，不影响生产路径；若 mock 不可用，退化为把 emit 适配的参数注入化（闭包）保持覆盖。回退：摘除 events.rs 即还原。

## 决策与工作记录

- 2026-10-05：创建任务（ready）。emit 适配的测试走 tauri mock 而非闭包注入——真实 `emit_to` 路径被覆盖比注入纯函数更有价值；seq / 信封保持纯函数。

## 完成摘要

未完成。
