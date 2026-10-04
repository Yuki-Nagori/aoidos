# 017 — 实现：store 域命令与迁移事件

- 状态：done
- 依赖：016、013
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

[通信契约](../architecture/ipc-contract.md)的 store 域目前只有错误码（013 / 015 已映射），还没有真实命令与事件。本任务交付契约的第一个真实命令 `store_list_backups`（正例）、快照命令 `store_get_migration`，以及 `db::open` 的迁移进度回调（契约已注明：当前迁移在返回前同步跑完，发不出事件）。

## 必读

[通信契约](../architecture/ipc-contract.md)（`store_list_backups` 正例、快照命令表、`store.*` 错误码）· [存储基建](../architecture/storage.md) · [016 事件基建](016-ipc-events-infra.md) · [013](013-storage-impl.md)。

## 范围与非目标

交付：

1. `mythos-store`：`db::open` 保留为便捷封装，新增 `open_with_progress(path, migrations, on_progress)`（`MigrationProgress { from, to }`，回调为纯数据、无 Tauri 类型——职责边界）。
2. `mythos-store`：`list_backups(db_path) -> Vec<BackupEntry>`（路径、版本、时间戳、大小；新到旧；复用 `backup_rank` 解析，目录不存在视为空）。
3. `mythos-store`：`current_version(path)`（只读 user_version，供快照命令）。
4. `src-tauri` 命令：`store_list_backups {} -> { items }`（契约正例）、`store_get_migration {} -> { from, to }`（当前 user_version）；lib.rs setup 注入业务库路径（OnceLock）。

非目标：**greet 退役与前端占位调整随 008 / 首个界面任务**（本任务只新增命令，不删 greet、不改 App.vue）；备份恢复 / 删除命令；剧本包导入导出。

**范围修订（实现期）**：`store:migration:*` 事件的命令层适配**随 006 推迟**——迁移序列为空（006 前无真实迁移）时事件永不发出，适配代码无消费方即无测试路径；`db::open` 的进度回调参数已就绪，006 落地时传入「回调 → 016 emit」适配即可。

## 实施步骤

1. `mythos-store`：`open_with_progress` + 进度单测（按序断言）。
2. `mythos-store`：`list_backups` + 单测（排序 / 非法名过滤 / 目录缺失为空）。
3. `mythos-store`：`current_version` + 单测。
4. `src-tauri`：setup 注入 DB_PATH（OnceLock）+ 两个命令（薄壳委托可测的 payload 函数）+ `not_ready` 直测。
5. `bun run verify` 十项。

## 预计改动

修改 `src-rust/mythos-store/src/db.rs`（回调参数、list_backups、current_version）、`src-tauri/src/commands.rs`（命令 + OnceLock）、`src-tauri/src/lib.rs`（setup + 注册）。

## 验收标准

- [x] `store_list_backups` 返回 `{ items }`，内容与 `backups/` 目录一致（含排序语义与非备份名过滤）。
- [x] `store_get_migration` 返回当前 `user_version`（from == to == 当前版本）。
- [x] 迁移进度回调按序触发（直测断言）；命令层事件适配随 006（已注明）。
- [x] 回调为纯数据、无 Tauri 类型（`mythos-store` 依赖树核验）；`bun run verify` 十项 exit 0。

## 验证计划与结果

| 日期       | 命令                                             | 预期        | 实际结果                                                      |
| ---------- | ------------------------------------------------ | ----------- | ------------------------------------------------------------- |
| 2026-10-05 | `cargo test --workspace`                         | 全过        | 70 passed（src-tauri 8 含 round-trip / not-ready + store 62） |
| 2026-10-05 | `bun run coverage:rust`                          | 行覆盖 100% | 1629/1629 行                                                  |
| 2026-10-05 | `cargo fmt --all --check` / clippy `-D warnings` | exit 0      | 均通过                                                        |

## 风险与回退

`db::open` 签名变化影响既有调用点（006 前只有测试）——一次改齐。回退：回调参数默认化或摘除，存储行为不变。

## 决策与工作记录

- 2026-10-05：创建任务（planned）。greet 退役不在本任务——前端占位调整随 008 / 首个界面任务，避免界面改动跑在交互设计前面。
- 2026-10-05：实现完成，**范围修订**：`store:migration:*` 事件的命令层适配随 006 推迟（迁移序列为空时事件永不发出，适配代码无测试路径）；`db::open` 进度回调参数按设计落地（`open_with_progress`，`open` 保留为 noop 封装）；`list_backups` 目录缺失视为空（fresh app 不报错）；命令层用 `OnceLock<PathBuf>` 注入业务库路径（lib.rs setup 注入，命令无参、可直测）。

## 完成摘要

store 域基座落地：`mythos-store` 新增 `open_with_progress`（进度回调）、`list_backups`（新到旧 + 过滤 + 目录缺失为空）、`current_version`；`src-tauri` 命令 `store_list_backups` / `store_get_migration` 走契约形状（`{ items }` / `{ from, to }`），setup 经 OnceLock 注入业务库路径。70 个 Rust 测试全过，行覆盖 100%，verify 十项 exit 0。限制：`store:migration:*` 事件适配随 006；`greet` 退役随 008。
