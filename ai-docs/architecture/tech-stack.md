# 技术栈

[架构总览](README.md)

精确小版本以 `bun.lock` 与根 `Cargo.lock` 为准，本文只维护主版本口径；升级后同步本表。

| 层                | 选型                                                       | 版本口径                             | 职责                                                                                              | 配置位置                                                            |
| ----------------- | ---------------------------------------------------------- | ------------------------------------ | ------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------- |
| 包管理 / 脚本聚合 | Bun                                                        | ^1                                   | 依赖管理与全部命令入口                                                                            | `package.json`                                                      |
| UI 框架           | Vue                                                        | ^3.5                                 | 视图与 UI 编排（`<script setup>`）                                                                | `src-web/`                                                          |
| 语言              | TypeScript                                                 | ^5.9（strict 全开）                  | 前端类型安全                                                                                      | `tsconfig.json`                                                     |
| 构建              | Vite                                                       | ^8                                   | dev server（端口 1420 固定）与生产构建                                                            | `vite.config.ts`                                                    |
| 桌面壳            | Tauri                                                      | ^2                                   | 窗口、系统集成、跨平台打包                                                                        | `src-tauri/tauri.conf.json`                                         |
| 系统语言          | Rust                                                       | 1.99.0（`rust-toolchain.toml` 锁定） | 命令层（src-tauri）与业务 crate（src-rust：mythos-store、mythos-llm、mythos-engine、mythos-json） | 根 `Cargo.toml`、`rust-toolchain.toml`、`src-tauri/` 与 `src-rust/` |
| JSON 编解码       | serde / serde_json                                         | ^1 / ^1                              | `mythos-json` 共用有界解码、重复键拒绝、脱敏分类及显式规范化；公共交换类型绑定 serde_json         | 根 `Cargo.toml`、`src-rust/mythos-json/`                            |
| LLM 传输          | reqwest（rustls-no-provider + ring）                       | ^0.13                                | 流式响应；显式 `retry(never)` 禁用隐式重试                                                        | `src-rust/mythos-llm/`                                              |
| OS 凭据库         | keyring（默认特性三平台各自实现）                          | ^4                                   | API 密钥存储；权威后端指针 / 墓碑与私有文件降级见通信契约                                         | `src-rust/mythos-llm/src/credentials.rs`                            |
| 异步运行时        | tokio / tokio-util / futures                               | ^1 / ^0.7 / ^0.3                     | mythos-llm 请求调度 / 流，mythos-engine 回合协调 / 取消 / 提交确认                                | 根 `Cargo.toml`                                                     |
| 脚本类型          | @types/bun / @types/node                                   | ^1 / ^26                             | 仓库 .mts 的 Bun / Node 类型检查，非产品运行时依赖                                                | `scripts/tsconfig.json`、`package.json`                             |
| 运行身份          | uuid                                                       | ^1（v4）                             | Rust 分配回合及物理请求 UUID，不由前端生成                                                        | 根 `Cargo.toml`、`mythos-engine` / `mythos-llm`                     |
| 确定性骰判        | rand_chacha / rand_core / getrandom                        | ^0.9 / ^0.9 / ^0.4                   | ChaCha20 原始字流与 OS seed；业务无偏映射由 engine 冻结                                           | 根 `Cargo.toml`、`mythos-engine::game::dice`                        |
| 记录摘要与游标    | sha2 / hmac / time                                         | ^0.10 / ^0.12 / ^0.3                 | 原行 SHA-256、内容引用 HMAC、UTC RFC3339 时间核验                                                 | 根 `Cargo.toml`、`mythos-engine::record`                            |
| 前端测试          | Vitest + @vitest/coverage-v8 + happy-dom + @vue/test-utils | ^5 / ^5 / ^20 / ^2                   | 单测与覆盖率门槛                                                                                  | `vitest.config.ts`                                                  |
| Rust 测试         | cargo test / cargo-llvm-cov                                | 工具链 + 独立子命令                  | 单测与覆盖率门槛                                                                                  | `package.json`                                                      |
| 样式              | Tailwind CSS v4（@tailwindcss/vite）                       | ^4                                   | 工具类消费层；token 真源是 ui.md 的 custom properties，@theme inline 映射并清默认色板             | `src-web/app.css`                                                   |
| 格式化            | Prettier / rustfmt                                         | ^3 / 工具链内置                      | 排版唯一权威                                                                                      | `.prettierrc.json`                                                  |
| Lint              | ESLint / clippy                                            | ^10 / 工具链内置                     | 质量规则（不管排版）                                                                              | `eslint.config.ts`                                                  |
| 死代码检查        | knip                                                       | ^6                                   | 未用依赖、导出与文件                                                                              | `knip.json`                                                         |
| 基准              | tinybench                                                  | ^6                                   | 前端微基准示例                                                                                    | `src-web/bench/`                                                    |

TypeScript 停在 5.x：TypeScript 7 尚无 typescript-eslint 支持，等工具链跟上再升。npm 镜像可能缺失部分包的 `latest` dist-tag，`bun update --latest` 报 tag not found 时按 `bun outdated` 的具体版本号手动写入 `package.json`。

原生密钥输入使用 Windows CredUI、macOS AppKit（objc2）和 Linux GTK 3（gtk-rs）；依赖按目标平台声明，使用点经同型接口调用。平台验证记录归 [019 配置与凭据](../task/019-llm-profile-credentials-impl.md)，不把原生输入可用等同于安装包已验证。

## IPC 通道

前端 `@tauri-apps/api/core` 的 `invoke(name, args)` ↔ Rust `#[tauri::command]`，命令注册在 `src-tauri/src/lib.rs` 的 `generate_handler![]`；插件与系统能力经 `src-tauri/capabilities/default.json` 声明权限，当前只有 `core:default`。可运行示例：`commands::greet` ↔ `src-web/App.vue`。命令名、事件、错误形状与看门狗预算见[通信契约](ipc-contract.md)。

## CI

`.github/workflows/ci.yml` 在 push main 与 PR 时于三平台（macOS / Windows / Ubuntu）执行与本地 verify 等价的具名门禁；cargo check 并入 clippy，cargo test 并入 llvm-cov，编译形态合并与触发范围见[构建与开发](build-and-development.md#verify-构成)；Linux 在 Rust 门禁前安装 Tauri / GTK 系统依赖，三平台均安装 cargo-llvm-cov；另执行真实原生输入、OS 凭据与主 Webview IPC 集成，环境及覆盖边界见[构建与开发](build-and-development.md#原生能力集成验证)。

JSON 采用 [Serde](https://serde.rs/) 与 [serde_json](https://docs.rs/serde_json/) 的现有实现；项目层统一校验策略，消费方仍直接依赖 serde_json 的类型 / 宏，职责及兼容性见[架构总览](README.md#json-共用能力)。
