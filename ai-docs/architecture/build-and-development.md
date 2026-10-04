# 构建与开发

[架构总览](README.md)

## 环境

预装 [Bun](https://bun.sh) 与 [rustup](https://rustup.rs) 后，`bun install` 一次完成其余一切：npm 依赖与 husky 钩子，加上 postinstall 钩子里的 Rust 侧配置（scripts/setup.mts）——按 `rust-toolchain.toml` 安装工具链（含 clippy / rustfmt / llvm-tools-preview）、对半截安装卸载重装自愈、补装覆盖率门禁用的 cargo-llvm-cov；全部就位时只是两次秒级探测。环境异常时 `bun run setup` 手动重跑同一逻辑。平台 WebView 依赖属系统层：Windows 需 WebView2（Win10/11 一般自带），macOS 需 Xcode Command Line Tools，Linux 需 webkit2gtk 等开发包（[Tauri 前置要求](https://tauri.app/start/prerequisites/)）。

## 常用命令

全部命令经根 `package.json` 的 bun scripts 进出，不绕开 bun。

| 命令                                   | 作用                                       |
| -------------------------------------- | ------------------------------------------ |
| `bun install`                          | 安装依赖（同时启用 husky pre-commit）      |
| `bun run tauri:dev`                    | 桌面窗口开发（Vite HMR + cargo 增量编译）  |
| `bun run dev`                          | 浏览器纯前端预览（无 IPC，命令走前端回退） |
| `bun run build`                        | typecheck + Vite 生产构建 → `dist/`        |
| `bun run verify`                       | 提交门禁，构成见下                         |
| `bun run test` / `test:coverage`       | Vitest / Vitest + 覆盖率门槛               |
| `bun run test:rust` / `coverage:rust`  | cargo test / cargo-llvm-cov 行覆盖门槛     |
| `bun run lint` / `lint:rust`           | ESLint / clippy（`lint:fix` 自动修 TS 侧） |
| `bun run format` / `format:rust`       | Prettier / rustfmt（`:check` 为只读检查）  |
| `bun run typecheck` / `typecheck:rust` | vue-tsc / cargo check                      |
| `bun run bench`                        | tinybench 基准示例                         |
| `bun run knip`                         | 死依赖 / 死导出检查                        |
| `bun run tauri:build`                  | 发布打包                                   |

## verify 构成

`typecheck` · `typecheck:rust` · `lint` · `lint:rust` · `format:check` · `format:rust:check` · `test:coverage` · `test:rust` · `coverage:rust` · `knip`，共十项，任一失败即失败。husky 在每次 commit 前只跑双端格式检查（秒级反馈，拦住排版噪音）；lint、测试与覆盖率交给推送前自查与 CI——CI 把十项拆成具名步骤逐步执行，不聚合调用，失败直接定位。失败处理：格式挂了跑对应 format，lint 能自动修的走 `lint:fix`，测试挂了用 `test:watch` 本地复现。

CI 相对本地 verify 有两处**编译形态合并**（语义不变，省两次全量 Rust 编译）：`cargo check` 不单跑（clippy 已含类型检查）；`cargo test` 不单跑（llvm-cov 先执行同一套测试，单次插桩编译同时验证测试与覆盖率门槛）。见 ci.yml 顶部注释。

覆盖率口径：前端对逻辑层（`utils/`、`stores/`、`composables/`、组件旁 `use*.ts`）要求行 / 分支 / 函数 / 语句 100%；Rust 侧 `cargo llvm-cov --workspace --lib` 要求行 100%，`lib.rs` 是装配（事件循环不可测）经 `--ignore-filename-regex` 不计，命令与业务文件必须足额。改口径属于门禁变更，先登记 task。

首次 `cargo check` 或改动 `[profile.*]` 后的全量重编译是一次性成本，属正常现象。

## 打包与版本

```console
$ bun run tauri:build
```

内部顺序：`bun run build` → cargo release 编译（LTO / strip）→ bundle。产物在 `target/release/bundle/`：Windows 为 `msi/*.msi` 与 `nsis/*-setup.exe`；macOS、Linux 走同一命令出 dmg / deb / rpm / AppImage（本仓库的验证口径是 Windows，首次跨平台发布请在目标平台实测）。升级版本改两处定义——`package.json` 与根 `Cargo.toml` 的 `[workspace.package] version`——再由 `bun install` / `cargo check` 刷新锁文件一并提交。

## 图标与示例命令

1. 换图标：替换 `src-tauri/icons/icon.png`（1024×1024 方形）后执行 `bun run tauri icon src-tauri/icons/icon.png`，全套生成到 `src-tauri/icons/`（含 android / ios 子目录，纯桌面项目可删）；浏览器 favicon 用 `public/icon.png`，随手同步一份。
2. 替换 greet 示例为第一个真实命令：`commands.rs` 定义 → `generate_handler![]` 注册 → 需要插件能力时在 `capabilities/default.json` 加权限 → TS 声明同型并 `invoke` → 照 `tests/web/App.test.ts` mock 测试。示例 `greet` 被替换后删除。
3. 工作方式登记：非平凡改动从[任务索引](../task-index.md)建 task 开始。
