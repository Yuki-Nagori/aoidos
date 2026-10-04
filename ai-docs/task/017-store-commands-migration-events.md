# 017 — 实现：store 域命令与迁移事件

- 状态：planned
- 依赖：016、013
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

[通信契约](../architecture/ipc-contract.md)的 store 域目前只有错误码（013 / 015 已映射），还没有真实命令与事件。本任务交付契约的第一个真实命令 `store_list_backups`（正例）、快照命令 `store_get_migration`，以及 `store:migration:progress / done` 事件——后者要求 `db::open` 增加迁移进度回调（契约已注明：当前迁移在返回前同步跑完，发不出事件）。

## 必读

[通信契约](../architecture/ipc-contract.md)（`store_list_backups` 正例、快照命令表、`store.*` 错误码）· [存储基建](../architecture/storage.md) · [016 事件基建](016-ipc-events-infra.md) · [013](013-storage-impl.md)。

## 范围与非目标

交付：

1. `mythos-store`：`db::open` 增加迁移进度回调参数（`on_progress: impl FnMut(MigrationProgress)`，`MigrationProgress { from, to }`）；回调为纯数据、无 Tauri 类型（职责边界）。默认不影响现有调用（006 落地时传 noop）。
2. `mythos-store`：`list_backups(db_path) -> Vec<BackupEntry>`（名称、大小、排序；复用 `backup_rank` 的解析）。
3. `src-tauri` 命令：`store_list_backups {} -> { items }`（契约正例，数量小于分页阈值时不带 cursor）、`store_get_migration {} -> { from, to }`（当前 user_version）；迁移事件经 016 基建发送。
4. 回调 → 事件的适配在命令层（业务回调交出普通载荷，命令层 `emit_to`），作为「进度出口模式」的第一个用例。

非目标：**greet 退役与前端占位调整随 008 / 首个界面任务**（本任务只新增命令，不删 greet、不改 App.vue）；备份恢复 / 删除命令；剧本包导入导出。

## 实施步骤

1. `mythos-store`：`db::open` 加回调参数（既有调用点传 noop），单测断言迁移时回调被按序调用。
2. `mythos-store`：`list_backups` + 单测（混合合法 / 非法文件名）。
3. `src-tauri`：两个命令 + 回调 → 事件适配；`store_list_backups` 走契约分页形状（items，无 cursor）。
4. `bun run verify` 十项。

## 预计改动

修改 `src-rust/mythos-store/src/db.rs`（回调参数）、新增或扩展 list 模块；修改 `src-tauri/src/commands.rs`（新命令）、`src-tauri/src/lib.rs`（注册）。

## 验收标准

- [ ] `store_list_backups` 返回 `{ items }`，内容与 `backups/` 目录一致（含排序语义）。
- [ ] `store_get_migration` 返回当前 `user_version`（from == to == 当前版本）。
- [ ] 真实迁移时 `store:migration:progress` / `done` 按 seq 发出（mock 窗口断言）。
- [ ] 回调为纯数据、无 Tauri 类型（`mythos-store` 依赖树核验）；`bun run verify` 十项 exit 0。

## 验证计划与结果

| 日期 | 命令         | 预期 | 实际结果 |
| ---- | ------------ | ---- | -------- |
| —    | 待开始后填写 | —    | 未执行   |

## 风险与回退

`db::open` 签名变化影响既有调用点（006 前只有测试）——一次改齐。回退：回调参数默认化或摘除，存储行为不变。

## 决策与工作记录

- 2026-10-05：创建任务（planned）。greet 退役不在本任务——前端占位调整随 008 / 首个界面任务，避免界面改动跑在交互设计前面。

## 完成摘要

未完成。
