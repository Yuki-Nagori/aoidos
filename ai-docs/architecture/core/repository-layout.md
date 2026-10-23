# 目录与模块规划

更新日期：2026-10-22。

[架构总览](../README.md)

```text
├── index.html                  # Vite 入口 HTML（挂载点 #app）
├── src-web/                    # Vue 3 + TypeScript 前端
│   ├── main.ts                 # 前端入口
│   ├── app/                    # 根组合、欢迎页与应用级设置抽屉
│   ├── api/                    # IPC 调用封装与 TypeScript 载荷类型
│   ├── features/               # game、product、billing、memory、locale、theme
│   │   ├── game/               # 对局舞台、记录、输入与状态编排
│   │   │   └── assets/         # 游戏视觉资源
│   │   └── <feature>/          # 功能私有组件、composables、utils 与类型
│   ├── i18n/                   # vue-i18n 实例与语言初始化
│   ├── shared/                 # 不依赖 feature 的共享逻辑与展示原语
│   ├── styles/                 # 全局样式扩展；generated/ 保存固定生成物
│   ├── app.css                 # Tailwind 与全局设计 token（见 ../standards/ui.md、theming.md）
│   ├── theme-bootstrap.ts      # 首帧主题引导，构建为 /theme-bootstrap.js
│   ├── locale-bootstrap.ts     # 首帧语言引导，构建为 /locale-bootstrap.js
│   └── bench/                  # tinybench 基准
├── src-tauri/                  # 桌面入口：装配、IPC 与平台适配
│   ├── src/
│   │   ├── lib.rs              # Tauri 应用装配
│   │   ├── commands.rs         # 命令注册、载荷与命令级测试
│   │   ├── billing_commands/   # 预算、计价、账本与供应商余额适配
│   │   ├── game_commands/      # 产品阶段 DTO 与命令适配
│   │   ├── llm_commands.rs     # LLM 配置与凭据命令
│   │   ├── locale_commands/    # 语言偏好 IPC 与原生菜单适配
│   │   ├── memory_commands/    # 记忆配置、查询、确认与批次适配
│   │   ├── native_prompt/      # GTK、AppKit、CredUI 原生输入适配
│   │   ├── run_commands/       # 周目目录、恢复、删除与账务迁移适配
│   │   ├── store_commands/     # 存储参数与平台事件队列适配
│   │   ├── theme_commands/     # 主题目录、偏好与剧本皮肤适配
│   │   ├── turn_commands/      # 回合服务与事件适配
│   │   ├── events.rs           # 事件序号与信封
│   │   └── ipc.rs              # 命令错误及域错误映射
│   ├── capabilities/           # IPC 权限声明
│   ├── icons/                  # Tauri 图标资源
│   ├── tauri.conf.json         # 窗口、打包与开发服务器配置
│   └── build.rs                # tauri-build 入口
├── src-cli/                    # CLI / TUI workspace package
│   ├── Cargo.toml              # aoidos-cli
│   └── src/                    # clap 命令、事件桥与 Ratatui 界面
├── src-rust/                   # 不依赖 Tauri 的领域与基础设施 crate
│   ├── aoidos-app/             # GUI / CLI 共用的应用服务与游戏装配
│   ├── aoidos-billing/         # 模型计价、周期、账本与预算规则
│   ├── aoidos-engine/          # 游戏协调、回合、记录、迁移与存储编排
│   ├── aoidos-json/            # serde_json 共用编解码、重复键校验与规范化
│   ├── aoidos-llm/             # 供应商、护栏、调度、配置、凭据与代理
│   ├── aoidos-locale/          # 受限语言偏好与系统语言归一化
│   ├── aoidos-memory/          # 记忆模型、仓储、批次与轮回协议
│   ├── aoidos-script/          # 剧本原文解析与格式校验
│   ├── aoidos-store/           # 路径、私有原子发布、有界读取与 SQLite 基建
│   └── aoidos-theme/           # 主题目录、偏好与剧本皮肤校验
├── resources/scripts/         # 已发布剧本原文、署名与许可（编译期内嵌）
├── tests/
│   ├── rust/native-platform/  # 真实原生 UI、OS 凭据与 Webview IPC 集成测试
│   └── web/                   # Vitest 行为测试，目录镜像 src-web
│       ├── api/               # 类型化 IPC API 行为测试
│       ├── app/               # 根组合与跨功能界面
│       ├── features/          # 按生产 feature 镜像
│       ├── i18n/              # 启动语言同步测试
│       └── shared/            # 共享逻辑测试
├── scripts/                   # 环境、覆盖率、原生夹具与构建脚本
├── locales/                   # zh-Hans / en 单源 JSON
├── public/                    # Vite 静态资源
├── ai-docs/                   # 架构、规范、研究与实施任务
├── .github/workflows/         # CI 与 Nightly 工作流
├── .husky/pre-commit          # 提交前的双端格式检查
├── coverage-rust.config.mts   # Rust 覆盖率忽略项与逐文件预算
├── biome.json / dprint.json / knip.json / vite.config.ts / vitest.config.ts
├── rust-toolchain.toml        # Rust 工具链锁定
└── Cargo.toml                 # Rust 虚拟工作区与 profile 配置
```

## 归属规则

上图列出主要源码与测试目录；构建输出、生成夹具和被忽略的临时文件不在图中。`app/App.vue` 只装配产品、语言、主题、计费、记忆和对局功能；`WelcomeScreen` 呈现未开局摘要与显式开局，应用级 `SetupPanel` 作为跨功能设置抽屉，通过插槽承载记忆设置 / 查询；`GameWorkspace` 管理游戏内状态与呈现。feature 之间不互相导入实现；共享能力放 `shared/`，测试按生产目录镜像。

组件只做编排与渲染，跨功能共享的纯逻辑进 `shared/utils/` 并配单测，功能私有逻辑留在对应 feature；`commands.rs` 只做解参数、调逻辑、回包，业务长大后在 `src-rust/` 下拆独立 crate（加入 `members`，依赖方向与工程纪律见[职责边界](ts-rust-boundary.md)），命令层只做转发。全局状态、路由、UI 组件库在需求真实出现前不引入，也不建占位目录。

标准库类型与调用不按 import 集中封装：各模块可直接使用 `Path`、`Duration`、`io::Error` 等。带有项目统一存储规则的能力（原子写、私有权限发布、有界读取、缺失路径判断、实例锁）归 `aoidos-store`；共用 JSON 编解码 / 重复键校验 / 规范化归 `aoidos-json`，领域 JSON schema / 版本校验归消费 crate。桌面原生输入适配归 `src-tauri`，CLI 输入留在 CLI；文件权限实现归 `aoidos-store`，凭据业务和 keyring 归 `aoidos-llm`。请求超时归 LLM 调度。只提取真实共用的规则，不为统一调用外观增加转发层。

Rust 模块拆分后采用同目录 `mod.rs` 入口，规则见[Rust 规范](../../standards/rust.md#rust-模块目录)。

新增文件的归属按职责判断，不按语言堆放；访问私有实现的测试贴近源码（如 `commands.rs` 的 `mod tests`），前端公开行为的测试放 `tests/web/` 并镜像目录结构，例如 `tests/web/features/game/utils/turn-consumer.test.ts` 对应 `src-web/features/game/utils/turn-consumer.ts`。

## 命名

仓库 / 包名 `aoidos`（`bun.lock` 的根 workspace 名同型），Rust package 同名、lib 名为下划线形态的 `aoidos_lib`；产品显示名 `Aoidos`（`tauri.conf.json` 的 productName 与窗口标题）。

## 记录模块与夹具

`aoidos-engine/src/record/` 下按格式、注册事实、会话、世界协议、历史、工作集、投影、摘要候选、压缩和视图拆分；带外置单测的模块统一使用 `{mod.rs,tests.rs}`，包括 `facts/`。通用测试 Provider 位于 engine 的 `test_support.rs`，记录夹具位于 `record/test_support.rs`，均仅在测试构建编译。

`aoidos-engine/src/game/` 分为串行驱动 / 执行、场景目录、骰判、转换、恢复、控制和阶段发布；`domain.rs` 定义可信域端口，`input.rs`、`proposal.rs`、`state.rs` 分别保存输入、提议与 IPC 数据。前端阶段链路见[架构总览](../README.md#阶段机基础链路)，验收证据见 [023](../../task/023-turn-state-machine-impl.md)。

`aoidos-memory/src/repository/` 的 `mod.rs` 提供共享存储能力；aliases、config、confirmations、cycle、materials、operations、reconciliation、recovery 各自使用同目录 `{mod.rs,tests.rs}`，cycle 另有 `outbox.rs`；跨模块 SQLite 夹具在 `test_support.rs`。engine 通过 `record/memory/{mod.rs,tests.rs}` 持有记忆端口与共享连接，memory 不反向依赖 engine，避免依赖环。产品链路已接入配置、显式查询 / 确认、素材批次与轮回节点；算法参数及长对局 / 供应商实验的验收状态见 [033](../../task/033-memory-calibration-integration.md)。

`tests/fixtures/record/token-estimation-v1.json` 保存匿名合成输入与离线 tokenizer 标定数据；不含玩家数据或密钥。原生窗口夹具继续放 `tests/rust/native-platform/`，不把需要私有实现访问的单测迁入此处。
