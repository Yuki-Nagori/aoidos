# Rust / Tauri 约定

更新日期：2026-10-07。除标注官方依据外均为项目约定。

## 命令层

`#[tauri::command]` 只做解参数、调逻辑、回包，命令按域组织：`src-tauri/src/commands.rs` / `llm_commands.rs` 承接已有入口，回合服务与事件适配位于 `turn_commands/`，存储参数及事件适配位于 `store_commands/`，`lib.rs` 的 `turn_ipc` / `store_ipc` 仅保留宏注册所需的薄 IPC 装配；单个命令超过一屏就把逻辑抽成普通函数或独立 crate。新命令三步：定义 → `generate_handler![]`（`lib.rs`）注册 → 用到插件 / 系统能力时在 `capabilities/default.json` 加权限（当前只有 `core:default`）。

invoke 的 args 对象按 camelCase 匹配 Rust snake_case 形参（[Tauri 命令文档](https://tauri.app/develop/calling-rust/)，2026-10-04 查阅）：单词形参无感，多词形参（`case_dir` ↔ `caseDir`）留意。参数与返回类型在 Rust 定型后，TS 侧在 `src-web/api/` 立即声明同型并提供薄调用（归属见[前端规范](frontend.md#逻辑归属)）——`invoke` 是无校验透传，两端口径漂移是最常见的静默 bug。

错误形状以[通信契约](../architecture/ipc-contract.md)为准：命令返回 `Result<T, CmdError>`，序列化为 `{ code, message, detail? }`。前端按 `code` 分支，不匹配 `message`。

## 平台能力封装

Windows / macOS / Linux 的原生能力由平台模块分别实现，通过 `#[cfg]` 导出同型接口；共享 Unix 权限规则放 unix 模块。平台判断集中于适配层，命令层和业务使用点只调用接口，不以操作系统 `if / else` 或散落的 `#[cfg]` 组织功能。原生输入缺少桌面会话、权威 OS 后端读取失败均按接口错误返回；OS 写入失败的私有文件降级遵循[通信契约](../architecture/ipc-contract.md#工程纪律可检查版)，不能回退到 Webview 明文输入或伪装未设置。

原生交互统一经平台壳调度到 UI 主线程，等待用户期间不持业务锁；保存逻辑与 UI 生命周期分开，便于用注入器验证确认 / 取消 / 失败。平台模块实现存在不等于已验证：每个平台的真实 UI 与 OS 服务结果分别记录，纯逻辑或模拟测试不替代平台验收。

模块封装按业务职责，标准库的通用类型不需要统一转发。共享存储规则归 `aoidos-store`，业务 schema 留在消费 crate；具体归属见[目录规划](../architecture/repository-layout.md#归属规则)。

## Rust 模块目录

小模块保留单文件与内联 `#[cfg(test)] mod tests`。当测试体拆成 `tests.rs` 或实现拆成子模块、已经形成模块目录时，入口统一放该目录的 `mod.rs`，与子模块并列；适用于 `src-tauri` 和业务 crate。例如 `aoidos-engine/src/turn/{mod.rs,tests.rs}`、`src-tauri/src/turn_commands/{mod.rs,tests.rs}`。迁移不改变对外模块名，同时更新目录文档与路径引用，不给单文件模块预建目录。

## Cargo 工作区

根 `Cargo.toml` 是虚拟 manifest：维护工作区配置（resolver / members）、`[workspace.package]`、`[workspace.dependencies]` 与 profile，依赖版本（含 build-dependencies）统一在 workspace 声明、成员以 `xxx.workspace = true` 继承。根 `Cargo.lock` 全工作区唯一，提交并保持同步。业务 crate 加入 `members` 并接入既有门禁。新增 crate 或修改 crate 依赖时，同步检查 Cargo 实际依赖图、更新[架构总览的工作区链路](../architecture/README.md#rust-工作区依赖图)及职责边界；生产依赖必须无环，测试 crate 不得成为生产依赖。task 的先后关系与 crate 依赖分别校验，不能混为一张图。

## 性能 profile（改前先读）

`[profile.dev.package."*"]` 的 `opt-level = 1` + 行表级调试信息：tauri 系依赖在 O0 下运行明显卡顿，此设置同时让 app crate 全量重编译更快；改动本节会使依赖缓存整体失效，下一次构建为一次性全量重编译。`[profile.release]` 用 LTO + `opt-level = "s"` + strip + panic abort，安装包体积优先。profile 只能定义在工作区根 manifest，成员内的会被忽略（[Cargo book](https://doc.rust-lang.org/cargo/reference/profiles.html)，2026-10-04 查阅）。
