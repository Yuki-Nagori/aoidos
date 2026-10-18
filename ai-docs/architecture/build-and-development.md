# 构建与开发

更新日期：2026-10-18。

[架构总览](README.md)

## 环境

预装 [Bun](https://bun.sh) 与 [rustup](https://rustup.rs) 后，`bun install` 一次完成其余一切：npm 依赖与 husky 钩子，加上 postinstall 钩子里的 Rust 侧配置（scripts/setup.mts）——按 `rust-toolchain.toml` 安装缺失的工具链组件（含 clippy / rustfmt / llvm-tools-preview）、修复损坏安装、补装覆盖率门禁用的 cargo-llvm-cov；组件已就绪时只做本地查询。仓库 Cargo 命令与 Tauri CLI 通过 `scripts/cargo.mts` / `scripts/toolchain.mts` 按该文件调用 `rustup run`，不受系统 `PATH` 中其他 cargo / rustc 发行版影响。CI 会先通过预编译 action 安装覆盖率工具，再运行 `bun install`，避免冷 runner 从源码构建。环境异常时 `bun run setup` 手动重跑同一逻辑。平台 WebView 依赖属系统层：Windows 需 WebView2（Win10/11 一般自带），macOS 需 Xcode Command Line Tools，Linux 需 webkit2gtk 等开发包（[Tauri 前置要求](https://tauri.app/start/prerequisites/)）。

## 常用命令

全部命令经根 `package.json` 的 bun scripts 进出，不绕开 bun。

| 命令                                   | 作用                                                                                                          |
| -------------------------------------- | ------------------------------------------------------------------------------------------------------------- |
| `bun install`                          | 安装依赖（同时启用 husky pre-commit）                                                                         |
| `bun run tauri:dev`                    | 桌面窗口开发（Vite HMR + cargo 增量编译）                                                                     |
| `bun run dev`                          | 浏览器纯前端预览（无 IPC，命令走前端回退）                                                                    |
| `bun run build`                        | typecheck + Vite 生产构建 → `dist/`（`build:web` 含 i18n 预检）                                               |
| `bun run verify`                       | 任务收尾时运行的完整本地门禁，构成见下                                                                        |
| `bun run test:native`                  | 聚合原生 UI / OS 凭据与主 Webview IPC 集成测试，需桌面会话                                                    |
| `bun run test` / `test:coverage`       | Vitest / Vitest + 覆盖率门槛                                                                                  |
| `bun run test:rust` / `coverage:rust`  | 普通 workspace 测试 / cargo-llvm-cov 聚合行覆盖率（最低 90%）                                                 |
| `bun run test:affected`                | 按统一 Git 变更计划运行受影响 Rust packages 与 Vitest 镜像测试；未知范围回退全量                              |
| `bun run format:affected`              | 按同一变更计划检查 rustfmt、Biome 与 dprint 的受影响文件                                                      |
| `bun run lint:affected`                | 按同一变更计划运行 Biome、i18n、Rust 分层与受影响 crate Clippy                                                |
| `bun run lint` / `lint:rust`           | Biome + i18n 检查 / clippy（`lint:fix` 自动修 TS 侧，`lint:rust:fix` 自动修 Rust 侧：clippy --fix + rustfmt） |
| `bun run format` / `format:rust`       | Biome + dprint / rustfmt（`:check` 为只读检查）                                                               |
| `bun run typecheck` / `typecheck:rust` | vue-tsc + 仓库脚本 tsc / cargo check                                                                          |
| `bun run bench`                        | tinybench 基准示例                                                                                            |
| `bun run clean:rust`                   | 清理 Rust 生成缓存；可追加 `-- -p <crate>` 限定包                                                             |
| `bun run doc:rust`                     | rustdoc 构建并拒绝警告（文档内链契约）                                                                        |
| `bun run knip`                         | 死依赖 / 死导出检查                                                                                           |
| `bun run i18n:check`                   | 校验语言资源键、占位参数、静态引用及保留命名空间                                                              |
| `bun run tauri:build`                  | 发布打包                                                                                                      |

`build:web` 先运行 `i18n:check`；`build:web:assets` 只生成产物，供三平台 CI 与 `test:native` 复用。i18n 静态检查由 Ubuntu lint 负责。

前端与夹具源代码按根 `tsconfig.json` 检查；仓库 `.mts` 脚本使用 `scripts/tsconfig.json`，继承严格检查并单独启用 Bun / Node 类型及 `allowImportingTsExtensions`，保持 `noEmit`。`typecheck` 聚合两项，前端配置不增加脚本运行环境类型。`@types/bun` / `@types/node` 作为直接开发依赖，版本由 `bun.lock` 固定。

目录移动后若 Tauri 构建缓存仍引用旧绝对路径，先运行 `bun run clean:rust -- -p tauri`；仍存在其他旧缓存时用 `bun run clean:rust` 完整清理，再重跑门禁。

## verify 构成

`typecheck` · `typecheck:rust` · `lint` · `lint:rust` · `lint:layers` · `format:check` · `format:rust:check` · `test:coverage` · `coverage:rust` · `knip` · `bench` · `build:web` · `doc:rust`，共十三项，任一失败即失败。Rust 库单测随覆盖率任务运行一次；`test:rust` 保留为本地普通 workspace 全量测试入口。PR CI 先由 `changes` job 生成共享变更计划，再并行运行 Ubuntu 质量门禁与三平台构建 / 受影响测试。类型检查、格式、lint 和 Clippy 在 Ubuntu 运行；Web 产物构建及受影响 Rust / Vitest 测试在三平台运行，原生验收在相关 Web 产物、原生测试输入或 Rust 消费闭包受影响时运行。Nightly 在三平台构建并运行完整 Vitest、workspace Rust 与原生测试；Ubuntu nightly 运行完整 `bun run verify`，覆盖率、knip、bench、rustdoc 也只在 Ubuntu 运行，并支持手动触发。共享配置、空 diff、未知路径或不可用 metadata / 基线回退全量。CI 的复用步骤位于 `.github/actions/`，workflow 负责可见的任务关系与平台条件。Biome / dprint 按文件类型选择，Biome 检查 TS / JS / CSS / JSON lint，i18n 语义检查由 `i18n:check` 承担，Vue、HTML、Markdown 和 YAML 由 dprint 排版。bench 只验证基准链路可用与产物非空，不做性能阈值判定；build:web 验证 Vite 产物链（资产解析 / 打包）；doc:rust 以 `RUSTDOCFLAGS=-D warnings` 拒绝断链等文档告警。每个本地提交前运行 `test:affected`、`format:affected` 与 `lint:affected`；`bun run verify` 用于任务收尾完整验收，不要求对每个中间 commit 重复执行。失败时先复现受影响检查；格式挂了跑对应 format，lint 能自动修的走 `lint:fix` / `lint:rust:fix`。

PR CI 省略单独的 `cargo check`（Clippy 已含类型检查）；格式、lint 和 Clippy 在 Ubuntu 运行，Web 构建、受影响 Tauri 应用构建及受影响测试在三平台运行，相关原生验收才启动全平台会话。Nightly 在三平台构建并运行完整 Vitest、Rust workspace 与原生测试，完整 `verify` 和覆盖率仅在 Ubuntu 执行。测试、格式和 lint 选择器读取同一份计划：Cargo metadata 生成 workspace 反向依赖闭包；Vue / TS 源码映射到 `tests/web/` 镜像；格式按 Biome、dprint 与 rustfmt 的输入范围区分。本地比较 `origin/main...HEAD` 并合并暂存、未暂存和未跟踪文件，CI 使用事件 base / head SHA。差异缺失、共享配置或未知路径触发全量。`cargo-llvm-cov` 仍通过预编译 action 安装，避免 postinstall 回退到源码编译。依赖方向见[分层规范](../standards/layering.md)，触发范围见下文。

CI 把变更规划、Ubuntu 质量门禁、三平台受影响测试和按需原生验收拆成独立 job；nightly 另在三平台运行完整 Rust / 原生测试。Linux GTK / WebKit / D-Bus 安装通过 composite action 复用。PR job 最多运行 30 分钟，nightly job 最多运行 90 分钟；工作流 token 只需读取仓库内容。

验收分别记录 PR 与合并提交的 main push CI。PR 成功后仍须核验默认分支运行；main 失败时补记失败、继续修复并复验，不能只引用 PR 成功宣称交付通过。纯文档提交若按路径过滤不触发 CI，保留最近一次对应代码提交的默认分支证据。

触发范围：main push 与 pull request 仅修改 `ai-docs/**` 或 Markdown 文件时跳过全量 CI；代码、依赖、脚本、工作流及其他配置改动仍跑三平台。PR 按相对 base 的累计 diff 判断，代码 PR 后续补文档仍可能触发；新建的纯文档 PR 与合并后的纯文档 push 会跳过。文档提交保留本地 / husky 格式检查。

覆盖率口径：前端逻辑层（`utils/`、`stores/`、`composables/`、组件与视图旁 `use*.ts`）行 / 分支 / 函数 / 语句各至少 90%；Rust `coverage:rust` 对纳入的 workspace lib 源行设置 90% 聚合底线。`lib.rs` 与 `platform/windows.rs` 等装配 / 原生适配在配置中排除，并由风险行为清单与平台验收保护；覆盖率用于发现未审查逻辑，不替代行为测试。详情见[测试规范](../standards/testing.md)与[048](../task/048-test-quality-ci.md)。

首次 `cargo check` 或改动 `[profile.*]` 后的全量重编译是一次性成本，属正常现象。

## 原生能力集成验证

`bun run test:native` 是独立于本地 `verify` 的真实会话测试，显式开启 `desktop-session`。Windows 使用 CredUI / Credential Manager，macOS 使用 AppKit / Keychain，Linux 使用 GTK / Secret Service。测试在进程主线程执行 UI，以合成值自动确认 / 取消并验证凭据读写清，不输出密钥；环境不满足时失败，不用模拟后端替代验收。

入口聚合 `aoidos-native-tests` 全部桌面会话目标，新增目标只登记 Cargo；定向排查用 `bun run test:native --test ipc-platform`。根脚本先构建手写 `webview/main.ts` 到 `gen/webview/`，测试配置加载生成目录；`webview/` 与 `tauri.conf.json` 入库，整个 `gen/` 从 Git / Biome 排除。

`ipc-platform` 使用真实主 Webview、产品命令及实际前端消费者，覆盖回合、记录、迁移、阶段 / 骰判、回退 / 重启和恢复 / 监听释放；`game-fixture.rs` 提供可信场景，不调用收费 API。具体断言与验证结果见 019–023 对应 task。

CI 各平台运行聚合测试，PR 单步超时 5 分钟、nightly 为 10 分钟；Linux 使用 Xvfb 与独立 D-Bus / gnome-keyring 会话。单平台结果不代替三平台 CI，原生实测不代替安装包验证；覆盖率边界见[测试规范](../standards/testing.md#rust-覆盖豁免台账)。

## 打包与版本

```console
$ bun run tauri:build
```

内部顺序：`bun run build` → cargo release 编译（LTO / strip）→ bundle。产物在 `target/release/bundle/`：Windows 为 `msi/*.msi` 与 `nsis/*-setup.exe`；macOS、Linux 走同一命令出 dmg / deb / rpm / AppImage（本仓库的验证口径是 Windows，首次跨平台发布请在目标平台实测）。当前桌面版本号只在根 `Cargo.toml` 的 `[workspace.package] version` 定义，成员 Cargo manifest 继承；`package.json` 和 `tauri.conf.json` 未设置 version，Tauri 按 Cargo 版本打包。升级时修改该定义，运行 `bun run typecheck:rust` 刷新根 `Cargo.lock`，提交相应变化；不为不存在的 package version 维护第二份版本号。

## 图标与示例命令

1. 换图标：替换 `src-tauri/icons/icon.png`（1024×1024 方形）后执行 `bun run tauri icon src-tauri/icons/icon.png`，全套生成到 `src-tauri/icons/`（含 android / ios 子目录，纯桌面项目可删）；浏览器 favicon 用 `public/icon.png`，随手同步一份。
2. 新增命令：`commands.rs` 定义 → `generate_handler![]` 注册 → 需要插件能力时在 `capabilities/default.json` 加权限 → TS 声明同型并 `invoke` → 照 `tests/web/App.test.ts` mock 测试。产品入口不提供浏览器本地生成回退。
3. 工作方式登记：非平凡改动从[任务索引](../task-index.md)建 task 开始。

Windows / MSVC 的 Tauri 构建按 Cargo 目标平台判断，共用 `scripts/rust/tauri-build.rs`，将 Common Controls v6 manifest 嵌入应用、lib 单测和原生测试入口。Tauri 默认资源链接仅覆盖 bins，新增 mock IPC 单测会使无 manifest 的测试程序在启动时返回 `STATUS_ENTRYPOINT_NOT_FOUND`；共享适配替换默认 manifest 注入，避免重复资源，同时保留应用默认 v6 能力。依据 [Tauri 上游 issue #13419](https://github.com/tauri-apps/tauri/issues/13419)，上游覆盖全部测试目标后可撤除此适配；实际 Windows 验证以 CI 为准。

剧本原文 `resources/scripts/**` 是编译资源，修改正文或许可触发三平台 CI；普通 Markdown 与 `ai-docs/**` 仍跳过纯文档流水线。

Rust lint / lint:rust:fix 启用测试 crate 的 `desktop-session`，以编译检查原生集成 target；两入口先运行 `build:web:assets`，再调用 `build:native-fixture` 生成 Tauri 宏所需的 Webview 资源，不能依赖本地遗留 `gen/`。Clippy 不执行原生窗口，实际会话验证由 `test:native` 执行；它复用同一夹具构建入口。`build:web` 保留 i18n 预检，跨平台 CI / 原生测试调用 assets-only 构建，i18n 静态检查由 Ubuntu lint 负责。
