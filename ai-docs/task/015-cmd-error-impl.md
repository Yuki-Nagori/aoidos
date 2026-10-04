# 015 — 实现：CmdError 与命令错误形状对齐通信契约

- 状态：in-progress
- 依赖：010、013
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

[通信契约](../architecture/ipc-contract.md)已定稿（010 done）：所有命令返回 `Result<T, CmdError>`，`CmdError` 序列化为 `{ code, message, detail? }`，`store.*` 码由 `StoreError::code()` 加前缀得到，中文 `message` 由命令层映射器编写。本任务把这一形状落地为 `src-tauri` 的命令层基座，让后续 005 / 006 的命令直接复用。

## 必读

[通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md) · [Rust 约定](../standards/rust.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

交付（`src-tauri/src/ipc.rs`）：

1. `CmdError`：`Serialize` 为 `{ code, message, detail? }`（`detail` 为 `None` 时省略字段），字段 camelCase。
2. `From<StoreError> for CmdError`：code 加 `store.` 前缀；`message` 按 `StoreError::code()` 映射中文文案；`InvalidPath` / `LockedTimeout` 的路径放 `detail.path`。
3. `greet` 示例对齐契约形状：`Result<String, CmdError>`（首个真实命令落地时替换，本任务不改其行为）。

非目标：事件信封与 `seq`（随 005 实现落地，届时才有真实发送方）；`llm.*` / `engine.*` 错误码（005 / 012 冻结）；业务命令与前端 UI。

## 实施步骤

1. `src-tauri` 增加 `mythos-store` 依赖（workspace 内路径依赖）。
2. 实现 `ipc.rs`（CmdError + From + 中文映射器）与单测：serde 形状、九个 store 码的前缀与文案、`detail.path`。
3. `greet` 改签名；`commands.rs` 测试同步；`bun run verify` 十项。

## 预计改动

新建 `src-tauri/src/ipc.rs`；修改 `src-tauri/src/lib.rs`（挂模块）、`src-tauri/src/commands.rs`（greet 签名与测试）、`src-tauri/Cargo.toml`（依赖 mythos-store 与 rusqlite dev-dep）、根 `Cargo.toml`（workspace 依赖登记 mythos-store 路径）。

## 验收标准

- [x] `CmdError` 序列化形状与通信契约一致（`detail` 缺省时字段不出现）。
- [x] 九个 store 码经 `From<StoreError>` 得到 `store.*` 前缀与对应中文文案。
- [x] `bun run verify` 十项 exit 0（含新模块行覆盖 100%）。
- [x] App.test.ts（前端 mock IPC）不受影响（vitest 套件原样通过）。

## 验证计划与结果

| 日期       | 命令                                                          | 预期             | 实际结果                                        |
| ---------- | ------------------------------------------------------------- | ---------------- | ----------------------------------------------- |
| 2026-10-05 | `cargo test --workspace`                                      | 全过             | 62 passed（src-tauri 3 含 ipc.rs 2 + store 59） |
| 2026-10-05 | `bun run coverage:rust`                                       | 行覆盖 100%      | ipc.rs 81/81；全仓 1416/1416                    |
| 2026-10-05 | `bun run verify`                                              | 十项 exit 0      | exit 0（首跑格式失守，format 后复跑通过）       |

## 风险与回退

clippy 可能对「恒为 Ok 的 Result」提示 `unnecessary_wraps`——契约要求统一形状，若触发则按注释规范加带理由的 `#[allow]`。回退：摘除 ipc.rs 与 greet 签名即还原。

## 决策与工作记录

- 2026-10-05：创建任务。事件信封 / `seq` 明确不在本任务——没有真实发送方不预建，随 005 实现任务落地（契约已冻结形状）。
- 2026-10-05：实现完成。`unnecessary_wraps` 未触发（greet 的 `Ok` 恒定返回在 `-D warnings` 下干净）。`From<StoreError>` 的九码断言用「按码构造 → 前缀 / 文案」表驱动直测；`detail.path` 仅 `InvalidPath` / `LockedTimeout` 携带。

## 完成摘要

命令层错误基座落地：`src-tauri/src/ipc.rs` 提供 `CmdError`（`{ code, message, detail? }`，camelCase，detail 缺省省略）与 `From<StoreError>`（`store.` 前缀 + 九码中文映射 + 路径 detail）；`greet` 对齐契约形状。60 个 Rust 测试全过，行覆盖 100%，verify 十项 exit 0。限制：`detail` 目前只携带路径类信息；事件信封 / `seq` 随 005 实现任务。
