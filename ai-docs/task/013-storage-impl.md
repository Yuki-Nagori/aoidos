# 013 — 实现：存储与文件基建 crate（mythos-store）

- 状态：done
- 依赖：011
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-07

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

以下记录区分初次实现、issue #1 修复与整体复核，历史数字不代表当前测试数量。测试数写为「mythos-store + src-tauri」；Rust 行覆盖门槛始终为 100%。

| 阶段          | 日期       | 环境              | 检查                                                                                      | 结果                                   |
| ------------- | ---------- | ----------------- | ----------------------------------------------------------------------------------------- | -------------------------------------- |
| 初次实现      | 2026-10-05 | Windows           | `bun run verify`                                                                          | 十项 exit 0；54 + 1 测试；1222/1222 行 |
| issue #1 修复 | 2026-10-05 | macOS 本地        | `bun run verify`                                                                          | 十项 exit 0；55 + 1 测试；1237/1237 行 |
| issue #1 修复 | 2026-10-05 | Ubuntu / macOS CI | [run 37236789035](https://github.com/Yuki-Nagori/mythos/actions/runs/37236789035)         | 全过；55 + 1 测试；1237/1237 行        |
| issue #1 修复 | 2026-10-05 | Windows CI        | 同上                                                                                      | 全过；56 + 1 测试；1260/1260 行        |
| 整体复核      | 2026-10-05 | macOS 本地        | `bun run verify`                                                                          | 十项 exit 0；60 + 1 测试；1312/1312 行 |
| 整体复核      | 2026-10-05 | Ubuntu / macOS CI | [PR #4 / run 37237430870](https://github.com/Yuki-Nagori/mythos/actions/runs/37237430870) | 全过；60 + 1 测试；1312/1312 行        |
| 整体复核      | 2026-10-05 | Windows CI        | 同上                                                                                      | 全过；59 + 1 测试；1335/1335 行        |

整体复核 CI 对应提交 `a443f12`。新增回归测试验证临时文件冲突不破坏既有文件、写失败清理、路径编码保真，以及部分 SQL / COMMIT 失败后的回滚、版本不推进与连接复用。Unix 有原始字节路径测试，Windows 有宽字符保真和文件占用测试，因此测试数量不同。

## 风险与回退

- rusqlite bundled 需要本机 C 编译器，与 Tauri 构建前提一致。
- `join_under_root` 只校验组件文本，不解析符号链接；数据根及子目录须由应用控制。同目录只放一个业务库，备份共用 `backups/`。
- 迁移 SQL 为可信的内嵌语句，不得自行控制事务。调用方须先持有实例锁。
- rename 后父目录同步失败时，目标可能已经更新；调用方不能将所有错误都视为未写入。原始路径编码保真不代表文件系统支持所有文件名。
- 回退整体复核可撤销对应提交；回退整个基建须摘除 workspace member 及依赖，并同步文档与锁文件。

## 决策与工作记录

- 2026-10-05：创建任务（ready），标定 WAL、备份保留 3 份、rename 重试 5 次 / 25ms 指数退避和 OS 独占锁。
- 2026-10-05：初次实现完成，在 Windows 验证十项门禁。路径归一化集中在 `paths`；SQLite 备份使用 backup API，避免遗漏未 checkpoint 的 WAL 提交；备份按时间戳整数排序。多开使用 fs4，锁文件只解锁不删除。
- 2026-10-05：[issue #1](https://github.com/Yuki-Nagori/mythos/issues/1) 复核发现 Ubuntu / macOS 仅覆盖 1197/1199 行。`finish_rename_err` 已有错误映射测试，但泛型重试函数的闭包实例各自缺少成功 / 失败路径，汇总行号不能完整显示实例缺口。
- 2026-10-05：提交 `9c75d9e` 共享非泛型重试实现，补测 busy 耗尽与非 busy 立即失败，验证次数、错误路径、原文件保留和 tmp 清理。三平台 CI 通过，已用 gh 评论并关闭 issue #1；门槛和忽略口径未改。
- 2026-10-05：按用户要求整体 review 五模块。临时文件改排他创建，仅清理本次成功创建的文件；临时名和归一化路径保留 OS 原始编码；迁移改为 RAII IMMEDIATE 事务；修正幂等测试检查错误备份目录的断言。
- 2026-10-05：公开注释明确实例锁、可信 SQL、数据根所有权与 rename 后错误语义，清理重复及失效说明。`error` 无需修改，`lock` 保留 OS 锁和中断重试，并改用确定更长的 PID 测试数据。内容补入本任务，不新增 task。
- 2026-10-06：第二轮整体 review（含注释逐条核对）：atomic / paths / db / lock 四模块无需代码或注释改动。消融验证 10 项安全机制（单点摘除 → `cargo test -p mythos-store` → 还原，零残留）：Windows 保留名消毒、路径穿越 / 分隔符拒绝、rename 占用退避重试（4 失败）、失败后 tmp 清理、JSONL 半行截断自愈、迁移 SQL 失败回滚上报（2 失败）、备份保留份数（2 失败）、WAL 模式强制、新库版本拒绝、实例锁独占检测——**10/10 全部被既有测试捕捉**，无一漏网。
- 2026-10-06：第五轮子不变量消融前排查发现 `PRAGMA foreign_keys` 无断言（打开路径必须带外键约束），补入 `migrations_apply_in_order_and_are_idempotent`。消融 6 项：foreign_keys 开启（1 失败）、首装不做迁移备份（3 失败，既有 backs-up-once 断言已钉）、备份按解析整数排序（1 失败）、保留名检查前先去尾点空格（3 失败）、script_id CJK 哈希回退（2 失败）——5/5 捕捉；锁 Drop 的显式 `unlock` 变异后套件仍绿，属**非承重防御**：guard drop 时 File 句柄关闭，内核即释放锁，「Drop 释放锁」行为本身由 `second_acquire_fails_release_allows_retry` 钉住，显式调用保留作表达明确性。
- 2026-10-06：第八轮补角消融补记 paths 项：script_id 超 64 字符的截断+哈希后缀（`script_id_truncates_long_names_with_hash` 钉住，变异 keep 边界即失败）。

平台处理保持以下决策：目录目标立即报 `io`；通用 busy 错误报 `locked`；Windows rename 的 5 / 32 作为共享冲突，而 Unix 同号 EIO / EPIPE 保持 `io`。Windows 目录 flush 使用带写权限及 BACKUP_SEMANTICS 的句柄；权限拒绝或卷不支持 flush 时保留已发布的替换结果。

- 2026-10-07：issue #61 同步更新日期与既有消融工作记录；本次为文档元数据修正，未重做历史消融或改动存储实现。
- 2026-10-07：019 评审补全仓共用 write_atomic_private：空 tmp 先收紧权限，再写正文 / fsync / 发布；权限失败保留旧文件并清理空 tmp，有顺序与失败回归测试。不复写第二套原子 IO。

## 完成摘要

`mythos-store` 已提供路径消毒、原子写与文件自愈、独占实例锁、SQLite WAL 迁移和在线备份，以及统一存储错误码。整体复核后的本地十项验证和三平台 CI 全部通过，Rust 行覆盖均为 100%。crate 不依赖 Tauri；业务 schema 与消费命令随 006 / 007 接入，使用边界见「风险与回退」。
