# 构建与开发

[架构总览](README.md)

## 环境

预装 [Bun](https://bun.sh) 与 [rustup](https://rustup.rs) 后，`bun install` 一次完成其余一切：npm 依赖与 husky 钩子，加上 postinstall 钩子里的 Rust 侧配置（scripts/setup.mts）——按 `rust-toolchain.toml` 安装工具链（含 clippy / rustfmt / llvm-tools-preview）、对半截安装卸载重装自愈、补装覆盖率门禁用的 cargo-llvm-cov；全部就位时只是两次秒级探测。环境异常时 `bun run setup` 手动重跑同一逻辑。平台 WebView 依赖属系统层：Windows 需 WebView2（Win10/11 一般自带），macOS 需 Xcode Command Line Tools，Linux 需 webkit2gtk 等开发包（[Tauri 前置要求](https://tauri.app/start/prerequisites/)）。

## 常用命令

全部命令经根 `package.json` 的 bun scripts 进出，不绕开 bun。

| 命令                                   | 作用                                                                                               |
| -------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `bun install`                          | 安装依赖（同时启用 husky pre-commit）                                                              |
| `bun run tauri:dev`                    | 桌面窗口开发（Vite HMR + cargo 增量编译）                                                          |
| `bun run dev`                          | 浏览器纯前端预览（无 IPC，命令走前端回退）                                                         |
| `bun run build`                        | typecheck + Vite 生产构建 → `dist/`（`build:web` 为纯构建段，verify 复用）                         |
| `bun run verify`                       | 提交门禁，构成见下                                                                                 |
| `bun run test:native`                  | 聚合原生 UI / OS 凭据与主 Webview IPC 集成测试，需桌面会话                                         |
| `bun run test` / `test:coverage`       | Vitest / Vitest + 覆盖率门槛                                                                       |
| `bun run test:rust` / `coverage:rust`  | cargo test / cargo-llvm-cov 行覆盖门槛                                                             |
| `bun run lint` / `lint:rust`           | ESLint / clippy（`lint:fix` 自动修 TS 侧，`lint:rust:fix` 自动修 Rust 侧：clippy --fix + rustfmt） |
| `bun run format` / `format:rust`       | Prettier / rustfmt（`:check` 为只读检查）                                                          |
| `bun run typecheck` / `typecheck:rust` | vue-tsc + 仓库脚本 tsc / cargo check                                                               |
| `bun run bench`                        | tinybench 基准示例                                                                                 |
| `bun run doc:rust`                     | rustdoc 构建并拒绝警告（文档内链契约）                                                             |
| `bun run knip`                         | 死依赖 / 死导出检查                                                                                |
| `bun run tauri:build`                  | 发布打包                                                                                           |

前端与夹具源代码按根 `tsconfig.json` 检查；仓库 `.mts` 脚本使用 `scripts/tsconfig.json`，继承严格检查并单独启用 Bun / Node 类型及 `allowImportingTsExtensions`，保持 `noEmit`。`typecheck` 聚合两项，前端配置不增加脚本运行环境类型。`@types/bun` / `@types/node` 作为直接开发依赖，版本由 `bun.lock` 固定。

## verify 构成

`typecheck` · `typecheck:rust` · `lint` · `lint:rust` · `format:check` · `format:rust:check` · `test:coverage` · `test:rust` · `coverage:rust` · `knip` · `bench` · `build:web` · `doc:rust`，共十三项，任一失败即失败。bench 只验证基准链路可用与产物非空，不做性能阈值判定；build:web 验证 Vite 产物链（资产解析 / 打包）；doc:rust 以 `RUSTDOCFLAGS=-D warnings` 拒绝断链等文档告警。husky 在每次 commit 前只跑双端格式检查（秒级反馈，拦住排版噪音）；lint、测试与覆盖率交给推送前自查与 CI——CI 把十三项拆成具名步骤逐步执行，不聚合调用，失败直接定位。失败处理：格式挂了跑对应 format，lint 能自动修的走 `lint:fix` / `lint:rust:fix`（修完必须重跑 verify），测试挂了用 `test:watch` 本地复现。

CI 相对本地 verify 有两处**编译形态合并**（语义不变，省两次全量 Rust 编译）：`cargo check` 不单跑（clippy 已含类型检查）；`cargo test` 不单跑（llvm-cov 先执行同一套测试，单次插桩编译同时验证测试与覆盖率门槛）。触发范围见下文。

CI 先完成格式、前端、knip、基准等快速门禁，再安装 Linux 原生依赖并执行 rustdoc / clippy / Rust 覆盖率，缩短这些错误的反馈时间；Rust 依赖缓存失败时也保存，工作区源码仍重新检查。三平台矩阵与门槛不变，每个 job 最多运行 30 分钟，工作流 token 只需读取仓库内容。

触发范围：main push 与 pull request 仅修改 `ai-docs/**` 或 Markdown 文件时跳过全量 CI；代码、依赖、脚本、工作流及其他配置改动仍跑三平台。PR 按相对 base 的累计 diff 判断，代码 PR 后续补文档仍可能触发；新建的纯文档 PR 与合并后的纯文档 push 会跳过。文档提交保留本地 / husky 格式检查。

覆盖率口径：前端对逻辑层（`utils/`、`stores/`、`composables/`、组件与视图旁 `use*.ts`）要求行 / 分支 / 函数 / 语句 100%；Rust 侧 `coverage:rust` 由根目录 `coverage-rust.config.mts` 声明、`scripts/coverage-rust.mts` 执行——逐文件未覆盖行预算，缺省 0（必须 100%），`lib.rs` 与 `platform/windows.rs` 等确属装配或无法注入的文件在配置登记豁免与理由，详见[测试规范](../standards/testing.md)；src-tauri 的 commands / events / ipc 等非忽略文件与业务逻辑均须足额。改口径属于门禁变更，先登记 task。

首次 `cargo check` 或改动 `[profile.*]` 后的全量重编译是一次性成本，属正常现象。

## 原生能力集成验证

`bun run test:native` 是独立于本地 verify 十三项的真实会话测试，显式开启 `desktop-session`。Windows 使用 CredUI / Credential Manager，macOS 使用 AppKit / Keychain，Linux 使用 GTK / Secret Service。测试在进程主线程执行 UI，以合成值自动确认 / 取消并验证凭据读写清，不输出密钥；环境不满足时失败，不用模拟后端替代验收。

入口一次执行 `mythos-native-tests` 的全部桌面会话目标，新增目标只登记 Cargo，不追加根脚本。定向排查可用 `bun run test:native --test ipc-platform`。入口先用 Bun 构建手写 `webview/main.ts` 到 `gen/webview/`；测试配置加载这一生成目录，因此必须经根脚本运行。`ipc-platform` 使用真实主 Webview 注册产品相同命令并装载实际 `useLlmTurn`，验证订阅、提交、正文 / 终态、快照、重复取消、非法输入、重连、全事件丢失后的主动恢复与卸载清理；只运行本地夹具，不调用收费 API。测试的 `webview/` 与 `tauri.conf.json` 是手写夹具，须入库；整个 `gen/` 包含 Tauri schema 与打包后的 Webview 资产，Git / ESLint 均排除，不能把第三方打包代码当自有源码检查。

CI 每个平台额外执行该聚合测试，单步超时 5 分钟；Linux 使用 Xvfb 与独立 D-Bus / gnome-keyring 会话。覆盖率边界见[测试规范](../standards/testing.md#rust-覆盖豁免台账)，原生凭据结果归 [019](../task/019-llm-profile-credentials-impl.md)，发送端结果归 [020](../task/020-llm-turn-ipc-impl.md)，实际消费者验证归 [021](../task/021-llm-web-recovery-impl.md)。原生能力实测不代替目标平台安装包验证。

## 打包与版本

```console
$ bun run tauri:build
```

内部顺序：`bun run build` → cargo release 编译（LTO / strip）→ bundle。产物在 `target/release/bundle/`：Windows 为 `msi/*.msi` 与 `nsis/*-setup.exe`；macOS、Linux 走同一命令出 dmg / deb / rpm / AppImage（本仓库的验证口径是 Windows，首次跨平台发布请在目标平台实测）。当前桌面版本号只在根 `Cargo.toml` 的 `[workspace.package] version` 定义，成员 Cargo manifest 继承；`package.json` 和 `tauri.conf.json` 未设置 version，Tauri 按 Cargo 版本打包。升级时修改该定义，运行 `bun run typecheck:rust` 刷新根 `Cargo.lock`，提交相应变化；不为不存在的 package version 维护第二份版本号。

## 图标与示例命令

1. 换图标：替换 `src-tauri/icons/icon.png`（1024×1024 方形）后执行 `bun run tauri icon src-tauri/icons/icon.png`，全套生成到 `src-tauri/icons/`（含 android / ios 子目录，纯桌面项目可删）；浏览器 favicon 用 `public/icon.png`，随手同步一份。
2. 替换 greet 示例为第一个真实命令：`commands.rs` 定义 → `generate_handler![]` 注册 → 需要插件能力时在 `capabilities/default.json` 加权限 → TS 声明同型并 `invoke` → 照 `tests/web/App.test.ts` mock 测试。示例 `greet` 被替换后删除。
3. 工作方式登记：非平凡改动从[任务索引](../task-index.md)建 task 开始。
