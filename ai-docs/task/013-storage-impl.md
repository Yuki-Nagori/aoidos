# 013 — 实现：存储与文件基建 crate（mythos-store）

- 状态：done
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
3. `lock`：`InstanceLock`（fs4 独占锁，进程退出自动释放，写 PID），二次获取报 `already-running`。
4. `db`：`open(path, migrations)`——rusqlite bundled、WAL、`PRAGMA user_version` 顺序迁移（事务包裹，失败回滚拒启）、迁移前文件备份（保留 3 份）、库新于二进制时报 `corrupt`。
5. `error`：`StoreError` 与 `code()` 命名空间 `store`（invalid-path / already-running / locked / migration / disk-full / permission / not-found / corrupt / io）。

标定初值（设计留白在此落定）：WAL 日志模式；备份保留 3 份；rename 重试 5 次、25ms 指数退避；多开用 OS 级独占锁（进程死亡自动释放，无需 PID 存活探测）。

非目标：业务数据 schema 与消费命令（随 006 / 007）；UI 与设置接线；shell opener（Tauri 侧能力，随首个使用它的功能走）。

## 实施步骤

1. 根 `Cargo.toml`：`members` 增 `src-rust/mythos-store`，workspace 依赖增 rusqlite（bundled、backup）、fs4。
2. 按 `error → paths → atomic → lock → db` 顺序实现，每模块带单测（含错误路径）。
3. `cargo fmt / clippy / test / coverage:rust`（新 crate 纳入 workspace 100% 行覆盖），`bun run verify` 十项。

## 预计改动

新建 `src-rust/mythos-store/`（Cargo.toml、lib.rs、五个模块）；修改根 `Cargo.toml`、`Cargo.lock`、`repository-layout.md`（目录树补 src-rust 实体）。

## 验收标准

- [x] `bun run verify` 十项 exit 0；`coverage:rust` 对新 crate 行覆盖 100%。
- [x] 原子写：崩溃模拟（tmp 残留）可自愈；目标被占用时重试后报 `locked` 且 tmp 已清理。
- [x] 迁移：顺序执行幂等，失败回滚且 `user_version` 不变，备份文件生成且保留 3 份；库新于二进制报 `corrupt`。
- [x] 多开：二次获取报 `already-running`，释放后可重取。
- [x] 明文密钥 / 业务 schema 不在本 crate（范围纪律）。

## 验证计划与结果

2026-10-05 补充整体复核：macOS `bun run verify` 十项 exit 0，mythos-store 60 + src-tauri 1 个测试通过，Rust 行覆盖 1312/1312（100%）；新增测试覆盖临时文件冲突 / 写失败清理、原始路径编码、部分 SQL 与延迟外键 COMMIT 失败回滚。

| 日期       | 命令                                     | 预期        | 实际结果                                                                     |
| ---------- | ---------------------------------------- | ----------- | ---------------------------------------------------------------------------- |
| 2026-10-05 | `cargo test --workspace`                 | 全过        | Windows 55 passed（mythos-store 54 + src-tauri 1）；Unix 少 1 个占用文件测试 |
| 2026-10-05 | `bun run coverage:rust`                  | 行覆盖 100% | Windows 1222/1222 行；mythos-store 五模块全 100%                             |
| 2026-10-05 | `cargo fmt --all` / clippy `-D warnings` | exit 0      | 均通过                                                                       |
| 2026-10-05 | `bun run verify`                         | 十项 exit 0 | Windows exit 0                                                               |
| 2026-10-05 | `bun run coverage:rust`（issue #1 修复） | 行覆盖 100% | macOS 1237/1237 行，mythos-store 55 + src-tauri 1 个测试通过                 |

## 风险与回退

issue #1 修复后，macOS 本地 `bun run verify` 十项 exit 0（2026-10-05）。

rusqlite bundled 需要本机 C 编译器（MSVC 已具备，tauri 构建依赖同款）。目录目标在三端都立刻报 `io`；占用重试只吃三端通用的 busy，外加 Windows 的 5/32。Unix 上打不开「文件被占用所以 rename 失败」的集成场景，该测试 `cfg(windows)`。回退：crate 独立，摘除 members 即还原。

## 决策与工作记录

- 2026-10-05：issue #1 修复 CI [37236789035](https://github.com/Yuki-Nagori/mythos/actions/runs/37236789035) 三平台通过：Ubuntu / macOS 1237/1237 行（55 + 1 测试），Windows 1260/1260 行（56 + 1 测试），行覆盖均 100%。已通过 gh 评论并关闭 issue。
- 2026-10-05：按用户要求整体复核五模块并优化，不新增 task。临时文件改排他创建，仅清理本次成功创建的文件；临时名和路径归一化保留 OS 原始编码；迁移改用 RAII IMMEDIATE 事务，新增已执行 DDL / DML 与 COMMIT 失败的回滚、版本不推进及连接复用验证。修正幂等测试检查错误备份目录的断言。公开注释明确单写者锁、可信迁移 SQL、数据根所有权，以及 rename 后同步失败时目标可能已更新。`error` 无需改动；`lock` 保留 OS 锁及中断重试，缩短失效注释。

- 2026-10-05：创建任务（ready）。设计标定初值：WAL、备份 3 份、重试 5 次 25ms 指数退避、OS 级独占锁。
- 2026-10-05：[issue #1](https://github.com/Yuki-Nagori/mythos/issues/1) 复核：原验收只在 Windows 成立；Ubuntu / macOS CI 均为 1197/1199 行（99.83%）。macOS 本地复现。`finish_rename_err` 已有三端映射测试，缺的是完整重试失败路径；泛型 `replace_with_retry` 的不同闭包实例还会各自缺少成功 / 失败路径，汇总 HTML 行号不能完整反映实例缺口。改用 `&mut dyn FnMut` 共享同一实现，补充 busy 耗尽与非 busy 立即失败测试，断言调用次数、错误与目标路径、原文件保留和 tmp 清理。门槛与忽略口径保持原值。修复后的三平台 CI 验证证据同步在 issue 评论中。
- 2026-10-05：实现完成。实测沉淀三条：(1) 拼接路径的混合分隔符（`a/b\c`）会让 SQLite 报 PATH_NOT_FOUND——归一化收敛到 `paths::normalize`（路径层职责，不在 db 消费点修补）；(2) 错误映射一行体用具名函数（`err_open` 等）而非闭包，闭包错误分支不可触达会拖垮 llvm-cov 行覆盖，约定沉淀至[注释规范](../standards/comments.md)；(3) 三端错误码按语义对齐，不按原始 errno 数字对齐。目录目标一律 `io` 且不重试。`WouldBlock` / `ResourceBusy` / `ExecutableFileBusy` 三端都是 `locked`。Windows 另把 rename 的 5（ACCESS_DENIED）和 32（SHARING_VIOLATION）当占用；这两个数字在 Unix 上是 EIO / EPIPE，保持 `io`。备份走 rusqlite `backup` feature 的在线备份 API（热 WAL 上 `fs::copy` 主文件会丢掉未 checkpoint 的提交），保留份数按文件名里的版本号和时间戳整数排序，忽略非 `storage-v*` 文件。实例锁只解锁不删文件。`script_id` 上限 64，FNV-1a 取低 32 位，Windows 保留名追加 `-0`，与 `join_under_root` 之后的目录名相同。比二进制新的库在改成 WAL 之前拒绝。`busy_timeout` 5 秒。父目录缺失时 Windows rename 的 ACCESS_DENIED 与 Unix 的 ENOENT 都报 `not-found`。`FlushFileBuffers` 需要写句柄，同步用写打开。多开锁从 fs2 换成 fs4 1.1：`TryLockError::WouldBlock` 直接表示占用，不再比对原始错误码。Rust 1.89 起 `File::try_lock` 是固有方法，调用必须写 `FileExt::try_lock`。

## 完成摘要

补充复核边界：`join_under_root` 提供文本组件校验，未实现不可信目录的符号链接沙箱；同目录只放一个业务库。非 UTF-8 文件名在 Unix 上验证临时名构造保真，落盘仍受文件系统支持限制。

`src-rust/mythos-store` 落地：`paths`（组件消毒 / script-id / join_under_root / normalize）、`atomic`（唯一原子写 + tmp 自愈 + JSONL 尾行截断）、`lock`（OS 级独占实例锁）、`db`（rusqlite WAL + user_version 顺序迁移 + 迁移前在线备份保留 3 份）、`error`（`store` 错误码命名空间）。Windows 上 `cargo test --workspace` 55 通过，行覆盖 1222/1222，`bun run verify` 十项 exit 0。crate 不依赖 tauri；消费命令随 006 / 007 接入。
