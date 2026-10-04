# 013 — 实现：存储与文件基建 crate（mythos-store）

- 状态：ready
- 依赖：011
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

[存储基建设计](../architecture/storage.md)已定稿（011 done）。本任务按设计落地首个业务 crate `src-rust/mythos-store`（不依赖 tauri），提供全仓复用的持久化基座：原子写、路径消毒、迁移 runner、多开锁与错误码。设计中标定的参数在此落定初值。

## 必读

[存储基建](../architecture/storage.md) · [职责边界](../architecture/ts-rust-boundary.md) · [Rust 约定](../standards/rust.md) · [测试规范](../standards/testing.md)。

## 范围与非目标

交付（`src-rust/mythos-store`）：

1. `paths`：组件消毒（非法字符 / Windows 保留名 / 尾点尾空格）、`script_id_from_name`（`[a-z0-9-]{1,64}`，截断 + FNV-1a 短 hash，CJK 回退）、`join_under_root`（拒 `..` / 分隔符 / 盘符）。
2. `atomic`：`write_atomic` / `write_text_atomic`（唯一 tmp + fsync + rename，Windows 锁定退避重试，失败上抛并清理 tmp）、`clean_temp_files`（`.tmp` 自愈）、`truncate_incomplete_jsonl`（尾行截断恢复）。
3. `lock`：`InstanceLock`（fs2 独占锁，进程退出自动释放，写 PID），二次获取报 `already-running`。
4. `db`：`open(path, migrations)`——rusqlite bundled、WAL、`PRAGMA user_version` 顺序迁移（事务包裹，失败回滚拒启）、迁移前文件备份（保留 3 份）、库新于二进制时报 `corrupt`。
5. `error`：`StoreError` 与 `code()` 命名空间 `store`（invalid-path / already-running / locked / migration / disk-full / permission / not-found / corrupt / io）。

标定初值（设计留白在此落定）：WAL 日志模式；备份保留 3 份；rename 重试 5 次、25ms 指数退避；多开用 OS 级独占锁（进程死亡自动释放，无需 PID 存活探测）。

非目标：业务数据 schema 与消费命令（随 006 / 007）；UI 与设置接线；shell opener（Tauri 侧能力，随首个使用它的功能走）。

## 实施步骤

1. 根 `Cargo.toml`：`members` 增 `src-rust/mythos-store`，workspace 依赖增 rusqlite（bundled）、fs2。
2. 按 `error → paths → atomic → lock → db` 顺序实现，每模块带单测（含错误路径）。
3. `cargo fmt / clippy / test / coverage:rust`（新 crate 纳入 workspace 100% 行覆盖），`bun run verify` 十项。

## 预计改动

新建 `src-rust/mythos-store/`（Cargo.toml、lib.rs、五个模块）；修改根 `Cargo.toml`、`Cargo.lock`、`repository-layout.md`（目录树补 src-rust 实体）。

## 验收标准

- [ ] `bun run verify` 十项 exit 0；`coverage:rust` 对新 crate 行覆盖 100%。
- [ ] 原子写：崩溃模拟（tmp 残留）可自愈；目标被占用时重试后报 `locked` 且 tmp 已清理。
- [ ] 迁移：顺序执行幂等，失败回滚且 `user_version` 不变，备份文件生成且保留 3 份；库新于二进制报 `corrupt`。
- [ ] 多开：二次获取报 `already-running`，释放后可重取。
- [ ] 明文密钥 / 业务 schema 不在本 crate（范围纪律）。

## 验证计划与结果

| 日期 | 命令         | 预期 | 实际结果 |
| ---- | ------------ | ---- | -------- |
| —    | 待开始后填写 | —    | 未执行   |

## 风险与回退

rusqlite bundled 需要本机 C 编译器（MSVC 已具备，tauri 构建依赖同款）；rename 重试在非 Windows 上几乎不触发，路径经 cfg 隔离不影响覆盖口径。回退：crate 独立，摘除 members 即还原。

## 决策与工作记录

- 2026-10-05：创建任务（ready）。设计标定初值：WAL、备份 3 份、重试 5 次 25ms 指数退避、OS 级独占锁。

## 完成摘要

未完成。
