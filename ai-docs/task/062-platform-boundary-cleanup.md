# 062 — 平台能力边界收敛

- 状态：done
- 依赖：013、019、058
- 优先级：P0
- 创建 / 更新：2026-10-21 / 2026-10-22

## 目标与背景

审查 Rust workspace 的平台差异后，发现两项实现与分层规范不一致：降级凭据文件的 POSIX 权限 / Windows DACL 由 `aoidos-llm` 实现，但属于 `aoidos-store` 的私有文件原语；GTK、AppKit、CredUI 原生输入由 `aoidos-llm` 实现，但属于 Tauri 桌面入口，并使 CLI 依赖桌面 GUI 库。统一对外语义，平台实现保留在正确的低层或入口边界。

## 必读

- [Rust / Web 依赖分层](../standards/layering.md)
- [Repository layout](../architecture/repository-layout.md)
- [LLM 架构](../architecture/llm.md)
- [存储架构](../architecture/storage.md)
- [国际化架构](../architecture/i18n.md)
- [凭据与代理实现](019-llm-profile-credentials-impl.md)
- [Rust 规范](../standards/rust.md)
- [测试规范](../standards/testing.md)

## 范围与非目标

- 把仅当前用户可读的文件权限实现移入 `aoidos-store` 平台私有模块；POSIX 使用 owner-only mode，Windows 使用仅当前用户的受保护 DACL，不互相模拟。
- 将 `write_atomic_private` 收敛为默认保证私有权限的存储接口；保留内部测试故障注入，不让领域调用方提供平台权限回调。
- 将 GTK / AppKit / CredUI 原生密码输入、统一窄接口及桌面真实烟测移入 `src-tauri` 平台 adapter；CLI 继续使用终端输入，凭据业务、keyring 与 Vault 留在 `aoidos-llm`。
- 移除 `aoidos-llm` 对 GUI 平台库和原生 UI 的依赖；原生会话测试改为调用 Tauri adapter。
- 同步分层、目录、LLM、存储、i18n 文档与依赖图。
- 不重写 CLI / Tauri 数据目录解析；它们属于入口宿主责任。
- 不重构主题 / legacy scan 的根内文件读取协议；相关竞态和 store 原语扩展需独立设计评估。

## 前置条件与待决策

- layering 规范已明确桌面原生对话框留在入口、凭据后端留在 LLM；本任务按该边界落地，并保持命令通过同型函数接口调用，不在命令层写 OS 分支。
- `aoidos-store` 已有 Windows `windows-sys` 依赖；迁入 ACL 后按 crate 声明最小特性，桌面 UI 特性不随 workspace 共享到 CLI。
- 三平台桌面输入依赖只放在 `aoidos` target-specific dependency；native smoke feature 由原生测试 crate 转发至 Tauri crate。
- DACL、mode、原子发布失败顺序与明文不落盘等语义不变。

## 实施步骤

1. [x] 把 owner-only 权限实现和平台测试迁入 `aoidos-store`；收紧私有原子写 API，验证空临时文件先收紧权限、失败不发布正文。
2. [x] 把统一原生输入类型和各平台 UI adapter 迁至 `src-tauri`；拆出 Windows DACL 代码，不把存储权限带入桌面层。
3. [x] 更新 Cargo feature / target dependencies、原生集成测试调用点与三平台依赖计划。
4. [x] 更新架构文档和 task-index，检查 CLI 依赖闭包不再携带 GTK / AppKit / CredUI。
5. [x] 运行本机完整验证；由 gpt-6.1-sol medium subagent 独立 review，修复发现并复验。
6. [x] PR 三平台 CI 与原生会话验证通过；核实 Windows DACL 行为后完成验收。

## 预计改动

- `src-rust/aoidos-store/src/atomic.rs` 及新增私有权限平台模块。
- `src-rust/aoidos-llm/src/platform/`、`credentials.rs` 与 Cargo manifest。
- `src-tauri/src/native_prompt/`、Tauri 命令 / 本地化适配与 Cargo manifest。
- `tests/rust/native-platform/`、根 Cargo feature 转发与 `Cargo.lock`。
- `layering.md`、`repository-layout.md`、`llm.md`、`storage.md`、`i18n.md`。

## 验收标准

- [x] `write_atomic_private(path, bytes)` 在所有受支持平台都于写入正文前设置 owner-only 权限；注入失败时不发布明文、不损坏旧目标且清理临时文件。
- [x] POSIX mode 与 Windows DACL 均由 `aoidos-store` 实际验证；Windows DACL 只含当前用户授权且受保护，不把 Unix 0600 语义套用到 Windows。
- [x] `aoidos-llm` 不实现文件权限或 GUI 对话框，也不再依赖 GTK、AppKit、CredUI UI 功能；CLI 凭据交互仅走终端接口。
- [x] Tauri 命令只调用统一的原生输入函数接口；取消为 `None`、失败脱敏、密码缓冲清理及 UI 主线程约束保持原语义，命令层不含 OS 条件分支。
- [x] Windows、macOS、Linux 确认 / 取消烟测从 Tauri 入口通过专用 native-session runner 验证；未实际验证的平台不宣称支持。
- [x] workspace 依赖图无环，CLI 的生产依赖闭包不包含 GUI 原生输入库；分层 lint、受影响格式 / lint / 测试通过。
- [x] `repository-layout.md`、`llm.md`、`storage.md`、`i18n.md` 与 layering 规范描述一致；旧归属与失效引用已清理。
- [x] 代码、中文注释、类型、测试、文档和 task 一致；subagent 独立 review 的问题已处理，PR 三平台 CI 通过。

## 验证计划与结果

| 日期       | 环境 / 命令                                                                                                                                        | 预期                                        | 实际结果                                                                                                                                |
| ---------- | -------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-21 | `cargo test -p aoidos-store --lib`                                                                                                                 | 私有权限和原子写测试通过                    | macOS 89 项通过；Windows DACL 结构测试已实现，待 Windows CI 编译和运行                                                                  |
| 2026-10-21 | `cargo test -p aoidos-llm --lib`; `cargo test -p aoidos --lib`                                                                                     | 凭据业务与 Tauri 命令测试通过               | LLM 241 项、Tauri 58 项通过                                                                                                             |
| 2026-10-21 | `cargo check -p aoidos --features native-smoke`; native-platform test target check                                                                 | 原生 adapter、烟测 feature 与 runner 能编译 | macOS 编译通过；尚未运行真实对话框烟测                                                                                                  |
| 2026-10-21 | `cargo clippy -p aoidos-store -p aoidos-llm -p aoidos -p aoidos-cli --all-targets -- -D warnings`; `bun run lint:layers`                           | 受影响 crate 无警告、依赖方向合法           | 通过；Cargo metadata 显示 Windows UI features 仅由 Tauri 声明，CLI 的 macOS 依赖树未包含 GTK / AppKit / UI 库                           |
| 2026-10-21 | `cargo metadata --no-deps`; `bun run format:check`; `bun run format:rust:check`                                                                    | 目标依赖与格式符合边界                      | 通过；Windows-sys DACL 功能只由 store 声明，GUI 依赖只由 Tauri 按目标声明                                                               |
| 2026-10-21 | `bun run verify`                                                                                                                                   | 完整本机质量门禁通过                        | 通过；473 项 Vitest、Rust 覆盖率 94.71%、视觉回归、全 workspace Clippy、构建与 rustdoc 均成功                                           |
| 2026-10-21 | gpt-6.1-sol medium 独立 final review                                                                                                               | 平台边界与安全关键改动无阻塞问题            | 通过；修正 Windows `GetAce` 导入及 LLM 文档路径，复核后无阻塞发现                                                                       |
| 2026-10-22 | `gh run view 39629694896`; `cargo test -p aoidos-store --lib quarantine_tree_cleanup_is_idempotent_for_missing_roots_and_rejects_identity_changes` | Linux 文件身份更换测试可重复通过            | Ubuntu CI 揭示 inode 可在删除后立即复用；测试改为旧、新文件并存时先确认身份不同，再执行替换；本机目标测试通过，修复后 PR 三平台 CI 通过 |
| 2026-10-22 | PR #192 CI `39633749029`                                                                                                                           | 三平台构建、受影响测试及原生平台会话均通过  | Ubuntu、macOS、Windows 与共享质量门禁全部通过；PR #192 已合并                                                                           |

## 风险与回退

权限收紧或原生对话框迁移失败时，不降级为无保护文件或 Webview 明文输入。保留失败即不写入 / 不发布的原子顺序；可分别回退 adapter 迁移和私有写接口，但不得让权限实现重新回到 LLM 层。

## 决策与工作记录

- 2026-10-21：全仓平台审计确认 POSIX / Windows 权限实现应归 `aoidos-store`，GTK / AppKit / CredUI 输入应归 Tauri 入口；CLI 数据根与终端适配、LLM keyring、store 文件身份和原子写属于各自层次，不迁移。
- 2026-10-21：其余生产平台分支复核后保留：`aoidos-store` 的 Windows rename 错误分类、本机路径编解码与权限实现属于 L1 文件语义；CLI 的数据根解析属于入口宿主职责；engine 中 Linux RSS 读取及 Unix 符号链接夹具只在测试目标编译。没有发现其他 L2 / L3 生产 crate 自行分支平台实现。主题与 legacy scan 的根内有界读取不属于平台差异，暂不抽象成通用文件 API。
- 2026-10-22：PR #192 三平台 CI 与原生平台会话检查全部通过并已合并。Ubuntu 曾发现文件身份测试假设删除后 inode 不会复用；测试现于旧文件仍存在时创建替代文件并比较身份，目标用例通过。

## 完成摘要

已完成平台边界迁移、其余 OS 分支审计、测试与文档同步。本机完整验证与独立评审通过，PR #192 的 Ubuntu、macOS、Windows CI 和原生平台会话检查全绿并已合并。
