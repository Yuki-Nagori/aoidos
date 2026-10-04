# 016 — 实现：IPC 事件基建（信封、每流 seq、emit 适配）

- 状态：done
- 依赖：010、015
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

[通信契约](../architecture/ipc-contract.md)冻结了事件形状：`<域>:<对象>:<阶段>` 命名、`{ seq, data }` 信封、seq 按「事件名 + 流标识」单调递增、监听者以快照对齐且不重放。本任务把事件发送侧做成基座，让 005（`llm:turn:*`）与 017（`store:migration:*`）只声明事件名与 data，不重复实现序号与信封。

## 必读

[通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md)（业务 crate 不得依赖 Tauri，进度经无 Tauri 类型的出口交出）· [注释规范](../standards/comments.md)。

## 范围与非目标

交付（`src-tauri/src/events.rs`）：

1. 每流 seq 分配器：`Mutex<HashMap<String, u64>>`，键为 `"{event}:{stream_id}"`；同流单调递增，异流互不占号。
2. 信封构造：`envelope(event, stream_id, data) -> serde_json::Value`，形状 `{ seq, data }`（无版本号，契约总则）。
3. emit 装配胶水：lib.rs `emit_event`（契约信封 + `emit_to("main")`，错误映射 `app.event-failed`）。

非目标：具体事件与快照命令（005 / 006 / 012 / 017）；业务 crate 的回调 trait 形态（随第一个真实发送方在 005 定，本任务只保证命令层一侧就绪）。

**范围修订（实现期）**：tauri `test` feature 在 Windows 让测试二进制加载即崩（STATUS_ENTRYPOINT_NOT_FOUND，wry DLL 依赖链）——mock 方案不可用。改判：seq / 信封纯逻辑留 events.rs 直测；真实 `emit_to` 收敛为 lib.rs 装配层一行胶水 `emit_event`（该文件在覆盖率口径外，与 Builder 同类），带 `TODO(task 017)` 的 `allow(dead_code)`——017 的命令层将成为首个调用方。dev-dep 的 test feature 移除。

## 实施步骤

1. `events.rs`：seq 分配器 + 信封；单测覆盖「同流递增 / 异流独立 / 信封形状」。
2. lib.rs 装配胶水 `emit_event`（契约错误码 `app.event-failed`）。
3. 契约错误码目录补 `app.event-failed` 行。

## 预计改动

新建 `src-tauri/src/events.rs`；修改 `src-tauri/src/lib.rs`（挂模块 + 胶水）、`ai-docs/architecture/ipc-contract.md`（错误码目录 +1 行）。

## 验收标准

- [x] 同流 seq 单调、异流互不占号（单测断言）。
- [x] 信封形状 `{ seq, data }` 且无版本号（单测断言）。
- [x] `app.event-failed` 登记进契约错误码目录。
- [x] `bun run verify` 十项 exit 0（行覆盖含 events.rs 100%）。

## 验证计划与结果

| 日期       | 命令                                             | 预期        | 实际结果                                        |
| ---------- | ------------------------------------------------ | ----------- | ----------------------------------------------- |
| 2026-10-05 | `cargo test --workspace`                         | 全过        | 65 passed（src-tauri 6 含 events 3 + store 59） |
| 2026-10-05 | `bun run coverage:rust`                          | 行覆盖 100% | 1481/1481 行；events.rs 纯逻辑全覆盖            |
| 2026-10-05 | `cargo fmt --all --check` / clippy `-D warnings` | exit 0      | 均通过                                          |

## 风险与回退

真实投递路径（`emit_to`）落在覆盖率口径外的装配层，无自动化覆盖——首次真实使用（017 / 005）时人工验证一次投递；事件名与信封形状的纯逻辑已全覆盖。回退：摘除 events.rs 与 lib.rs 胶水即还原。

## 决策与工作记录

- 2026-10-05：创建任务（ready）。emit 适配的测试走 tauri mock 而非闭包注入——真实 `emit_to` 路径被覆盖比注入纯函数更有价值；seq / 信封保持纯函数。
- 2026-10-05：实现完成，**范围修订**：tauri `test` feature 在 Windows 让测试二进制加载即崩（STATUS_ENTRYPOINT_NOT_FOUND，wry DLL 依赖链，社区已知问题）——mock 方案放弃；纯逻辑留 events.rs 直测，真实 `emit_to` 收敛为 lib.rs 装配层一行胶水（覆盖率口径外，带 `TODO(task 017)` 的 allow 与移除条件）。契约错误码目录补 `app.event-failed`。

## 完成摘要

事件基建落地：`events.rs`（每流 seq + 信封，纯逻辑直测）、lib.rs `emit_event` 装配胶水（覆盖率口径外，017 起使用）、契约 `app.event-failed` 登记。验证：65 测试全过、行覆盖 100%、fmt / clippy 过。限制：真实投递路径无自动化覆盖（Windows DLL 约束），首次真实使用（017 / 005）时人工验证一次投递。
