# 015 — 实现：CmdError 与命令错误形状对齐通信契约

- 状态：done
- 依赖：010、013
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-07

## 目标与背景

[通信契约](../architecture/ipc-contract.md)已定稿（010 done）：所有命令返回 `Result<T, CmdError>`，`CmdError` 序列化为 `{ code, message, detail? }`，`store.*` 码由 `StoreError::code()` 加前缀得到，中文 `message` 由命令层映射器编写。本任务把这一形状落地为 `src-tauri` 的命令层基座，让后续 005 / 006 的命令直接复用。

## 必读

[通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md) · [Rust 约定](../standards/rust.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

交付（`src-tauri/src/ipc.rs`）：

1. `CmdError`：`Serialize` 为 `{ code, message, detail? }`（`detail` 为 `None` 时省略字段），字段 camelCase。
2. `From<StoreError> for CmdError`：code 加 `store.` 前缀；`message` 按 `StoreError::code()` 映射中文文案；按变体携带 path、lockPath 或迁移 version / reason；无上下文时省略 detail。
3. `greet` 示例对齐契约形状：`Result<String, CmdError>`（首个真实命令落地时替换，本任务不改其行为）。

非目标：事件信封与 `seq`（由 016 提供）；`llm.*` / `engine.*` 错误码（005 / 012 冻结）；业务命令与前端 UI。

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

初次完成记录：

| 日期       | 命令                     | 预期        | 实际结果                                        |
| ---------- | ------------------------ | ----------- | ----------------------------------------------- |
| 2026-10-05 | `cargo test --workspace` | 全过        | 62 passed（src-tauri 3 含 ipc.rs 2 + store 59） |
| 2026-10-05 | `bun run coverage:rust`  | 行覆盖 100% | ipc.rs 81/81；全仓 1416/1416                    |
| 2026-10-05 | `bun run verify`         | 十项 exit 0 | exit 0（首跑格式失守，format 后复跑通过）       |

issue #7 修复及 015–017 整体复核（本地 macOS）：

| 日期       | 命令                    | 实际结果                                         |
| ---------- | ----------------------- | ------------------------------------------------ |
| 2026-10-05 | `bun run verify`        | 十项 exit 0；Rust 85 tests，Web 7 tests          |
| 2026-10-05 | `bun run coverage:rust` | 1838/1838 行（100%），含 commands / events / ipc |

## 风险与回退

前端仅按 code 分支，不把中文文案或诊断 reason 解析成稳定协议。真实平台投递尚无发送方；错误映射测试不等同于实际事件送达测试。回退必须同步消费者签名和对应契约。

## 决策与工作记录

- 2026-10-05：创建并完成 CmdError、store 九码前缀与中文映射，greet 保留示例行为。
- 2026-10-05：第一次 review 扩展 detail：AlreadyRunning 带 lockPath，Corrupt 带 reason，Migration 带 version / reason；缺省字段用 Io 变体验证。
- 2026-10-05：issue #6 同步状态与已落地映射描述。事件纯逻辑由 016 负责，真实发送适配仍未接入。
- 2026-10-05：issue #7 整体复核，将平台事件错误上下文从 lib.rs 移入可测映射器，补测序列化形状和原始非法路径保留；TS CmdError 同型声明。错误码目录保持不变，完整验证见本次复核记录。
- 2026-10-06：第六轮消融发现 `store_message` 九条中文映射无任何断言（变异为回传裸码后套件全绿），补 `store_messages_are_display_chinese_not_codes` 直测全部映射与 `io` 兜底。同轮 commands / ipc 消融记录列出四类子不变量：备份 50 项上限、not-ready 专用码、`store.` 前缀（触发 2 个测试失败）、迁移原因进 detail；这些变异均被测试捕捉。
- 2026-10-07：issue #62 同步更新日期，消融记录按已列四类机制描述，前缀变异的 2 指失败测试数；删除无第五项清单支撑的 5/5，不补造历史实验。

## 完成摘要

CmdError 和 store 错误映射已落地，detail 缺省省略；平台事件失败的诊断映射有直测。当前两端类型、注释与契约对齐，验证结果按阶段记录，不将后续累计测试数混写为初次完成结果。
