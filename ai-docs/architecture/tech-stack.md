# 技术栈

[架构总览](README.md)

精确小版本以 `bun.lock` 与根 `Cargo.lock` 为准，本文只维护主版本口径；升级后同步本表。

| 层                | 选型                                                       | 版本口径                             | 职责                                                                                  | 配置位置                                                            |
| ----------------- | ---------------------------------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| 包管理 / 脚本聚合 | Bun                                                        | ^1                                   | 依赖管理与全部命令入口                                                                | `package.json`                                                      |
| UI 框架           | Vue                                                        | ^3.5                                 | 视图与 UI 编排（`<script setup>`）                                                    | `src-web/`                                                          |
| 语言              | TypeScript                                                 | ^5.9（strict 全开）                  | 前端类型安全                                                                          | `tsconfig.json`                                                     |
| 构建              | Vite                                                       | ^8                                   | dev server（端口 1420 固定）与生产构建                                                | `vite.config.ts`                                                    |
| 桌面壳            | Tauri                                                      | ^2                                   | 窗口、系统集成、跨平台打包                                                            | `src-tauri/tauri.conf.json`                                         |
| 系统语言          | Rust                                                       | 1.99.0（`rust-toolchain.toml` 锁定） | 命令层（src-tauri）与业务 crate（src-rust：mythos-store、mythos-llm）                 | 根 `Cargo.toml`、`rust-toolchain.toml`、`src-tauri/` 与 `src-rust/` |
| LLM 传输          | reqwest（rustls-no-provider + ring）                       | ^0.13                                | 流式响应；显式 `retry(never)` 禁用隐式重试                                            | `src-rust/mythos-llm/`                                              |
| OS 凭据库         | keyring（默认特性三平台各自实现）                          | ^4                                   | API 密钥存储；不可用降级私有文件（Windows DACL / Unix 0600）                          | `src-rust/mythos-llm/src/credentials.rs`                            |
| 异步运行时        | tokio / tokio-util / futures                               | ^1 / ^0.7 / ^0.3                     | mythos-llm 调度、取消与流                                                             | 根 `Cargo.toml`                                                     |
| 前端测试          | Vitest + @vitest/coverage-v8 + happy-dom + @vue/test-utils | ^5 / ^5 / ^20 / ^2                   | 单测与覆盖率门槛                                                                      | `vitest.config.ts`                                                  |
| Rust 测试         | cargo test / cargo-llvm-cov                                | 工具链 + 独立子命令                  | 单测与覆盖率门槛                                                                      | `package.json`                                                      |
| 样式              | Tailwind CSS v4（@tailwindcss/vite）                       | ^4                                   | 工具类消费层；token 真源是 ui.md 的 custom properties，@theme inline 映射并清默认色板 | `src-web/app.css`                                                   |
| 格式化            | Prettier / rustfmt                                         | ^3 / 工具链内置                      | 排版唯一权威                                                                          | `.prettierrc.json`                                                  |
| Lint              | ESLint / clippy                                            | ^10 / 工具链内置                     | 质量规则（不管排版）                                                                  | `eslint.config.ts`                                                  |
| 死代码检查        | knip                                                       | ^6                                   | 未用依赖、导出与文件                                                                  | `knip.json`                                                         |
| 基准              | tinybench                                                  | ^6                                   | 前端微基准示例                                                                        | `src-web/bench/`                                                    |

TypeScript 停在 5.x：TypeScript 7 尚无 typescript-eslint 支持，等工具链跟上再升。npm 镜像可能缺失部分包的 `latest` dist-tag，`bun update --latest` 报 tag not found 时按 `bun outdated` 的具体版本号手动写入 `package.json`。

## IPC 通道

前端 `@tauri-apps/api/core` 的 `invoke(name, args)` ↔ Rust `#[tauri::command]`，命令注册在 `src-tauri/src/lib.rs` 的 `generate_handler![]`；插件与系统能力经 `src-tauri/capabilities/default.json` 声明权限，当前只有 `core:default`。可运行示例：`commands::greet` ↔ `src-web/App.vue`。命令名、事件、错误形状与看门狗预算见[通信契约](ipc-contract.md)。

## CI

`.github/workflows/ci.yml` 在 push main 与 PR 时于三平台（macOS / Windows / Ubuntu）执行与本地 verify 等价的具名门禁；cargo check 并入 clippy，cargo test 并入 llvm-cov，编译形态合并与触发范围见[构建与开发](build-and-development.md#verify-构成)；Linux 额外安装 Tauri 系统依赖，三平台均安装 cargo-llvm-cov。
