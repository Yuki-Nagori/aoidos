# 目录与模块规划

[架构总览](README.md)

```text
├── index.html                 # Vite 入口 HTML（挂载点 #app）
├── src-web/                   # 前端：Vue 3 + TypeScript
│   ├── main.ts                # 应用入口：挂载根组件
│   ├── App.vue                # 根组件：配置 / 默认剧本选择与最小正式对局入口
│   ├── app.css                # Tailwind v4 接线与设计 token 真源（分层见 ui.md / theming.md）
│   ├── api/                   # IPC 薄调用与 TS 载荷类型
│   ├── utils/                 # 纯消费规则 / 恢复协调及其他纯逻辑，配单测
│   ├── composables/           # Vue 身份、监听与 scope 清理，消费 utils
│   └── bench/                 # tinybench 正文消费基准
├── src-tauri/                 # Tauri 适配层：Rust（装配与命令适配，业务进 src-rust）
│   ├── src/lib.rs             # 应用装配（Builder）；事件循环不可测，不入覆盖门槛
│   ├── src/commands.rs        # #[tauri::command] 命令层、类型化载荷与命令单测
│   ├── src/game_commands/     # 产品阶段 DTO / 命令适配，业务由 engine actor 执行
│   ├── src/turn_commands/     # 回合服务 / 事件适配，mod.rs 与 tests.rs 并列
│   ├── src/llm_commands.rs    # LLM 配置 / 凭据命令（019）；平台原生输入经 aoidos-llm 分发
│   ├── src/ipc.rs             # CmdError 与域 / 平台错误映射
│   ├── src/events.rs          # 每流序号与事件信封，窗口投递由 turn_commands 适配
│   ├── tauri.conf.json        # 窗口 / 打包 / 开发服务器
│   ├── capabilities/          # IPC 权限声明
│   ├── icons/                 # 平台图标（tauri icon 生成，勿手改）
│   ├── src/store_commands/   # 存储参数适配及有界平台事件队列
│   └── build.rs               # tauri-build
├── src-rust/                  # 不依赖 tauri 的业务 crate
│   ├── aoidos-script/         # 剧本原文解析 / 格式校验，不执行内容规则
│   ├── aoidos-json/           # 基于 serde_json 的共用编解码、重复键校验与规范化
│   ├── aoidos-engine/         # 协调 / lease、record/、game/、迁移 / 偏好 / Storage
│   ├── aoidos-memory/         # 029：记忆版本、SQLite manifest、幂等操作、因果核验与恢复
│   ├── aoidos-store/          # 路径、原子发布、有界读取、journal / applied 与 SQLite 基建
│   └── aoidos-llm/            # 供应商适配 / 护栏 / 调度 / 配置 / 凭据 / 代理
│       └── src/platform/     # 三平台原生输入；Unix 权限共用
├── resources/scripts/        # 已发布剧本原文与署名 / 许可，编译期内嵌
├── tests/rust/native-platform/ # 真实 UI / OS 凭据 / 主 Webview IPC 集成测试 crate（显式桌面会话）
├── tests/web/                 # Vitest 单测（目录镜像 src-web）
├── scripts/                   # 仓库脚本（环境 / 覆盖率 / 原生夹具与 Rust 构建适配；TS 独立 tsconfig）
├── coverage-rust.config.mts   # Rust 覆盖率门禁的忽略清单与逐文件预算（scripts/coverage-rust.mts 消费）
├── .github/workflows/ci.yml   # 三平台 CI
├── .husky/pre-commit          # 提交前查双端格式（全量门禁在 CI）
├── ai-docs/                   # 本文档体系
├── public/                    # Vite 静态资产（icon.png 作 favicon）
├── eslint.config.ts / knip.json / vite.config.ts / vitest.config.ts
├── rust-toolchain.toml        # Rust 工具链锁定
└── Cargo.toml                 # 虚拟工作区根（members 与 profile 调优）
```

## 归属规则

组件只做编排与渲染，可复用逻辑进 `utils/` 并配单测；`commands.rs` 只做解参数、调逻辑、回包，业务长大后在 `src-rust/` 下拆独立 crate（加入 `members`，依赖方向与工程纪律见[职责边界](ts-rust-boundary.md)），命令层只做转发。全局状态、路由、UI 组件库在需求真实出现前不引入，也不建占位目录。

标准库类型与调用不按 import 集中封装：各模块可直接使用 `Path`、`Duration`、`io::Error` 等。带有项目统一存储规则的能力（原子写、私有权限发布、有界读取、缺失路径判断、实例锁）归 `aoidos-store`；共用 JSON 编解码 / 重复键校验 / 规范化归 `aoidos-json`，领域 JSON schema / 版本校验归消费 crate，原生输入和权限适配归 `aoidos-llm::platform`，请求超时归 LLM 调度。只提取真实共用的规则，不为统一调用外观增加转发层。

Rust 模块拆分后采用同目录 `mod.rs` 入口，规则见[Rust 规范](../standards/rust.md#rust-模块目录)。

新增文件的归属按职责判断，不按语言堆放；访问私有实现的测试贴近源码（如 `commands.rs` 的 `mod tests`），前端公开行为的测试放 `tests/web/` 并镜像目录结构。

## 命名

仓库 / 包名 `aoidos`（`bun.lock` 的根 workspace 名同型），Rust package 同名、lib 名为下划线形态的 `aoidos_lib`；产品显示名 `Aoidos`（`tauri.conf.json` 的 productName 与窗口标题）。

## 记录模块与夹具

`aoidos-engine/src/record/` 下按格式、注册事实、会话、世界协议、历史、工作集、投影、摘要候选、压缩和视图拆分；带外置单测的模块统一使用 `{mod.rs,tests.rs}`，包括 `facts/`。通用测试 Provider 位于 engine 的 `test_support.rs`，记录夹具位于 `record/test_support.rs`，均仅在测试构建编译。

`aoidos-engine/src/game/` 分为串行驱动 / 执行、场景目录、骰判、转换、恢复、控制和阶段发布；`domain.rs` 定义可信域端口，`input.rs`、`proposal.rs`、`state.rs` 分别保存输入、提议与 IPC 数据。前端阶段链路见[架构总览](README.md#阶段机基础链路)，验收证据见 [023](../task/023-turn-state-machine-impl.md)。

`aoidos-memory/src/repository/` 的 `mod.rs` 提供共享存储能力，materials、operations、reconciliation、recovery 各自使用同目录 `{mod.rs,tests.rs}`；跨模块 SQLite 夹具在 `test_support.rs`。engine 通过 `record/memory/{mod.rs,tests.rs}` 持有记忆端口与共享连接，memory 不反向依赖 engine，避免依赖环。029 的 crate 目前是存储 / 恢复基础，不代表产品记忆管线已接入。

`tests/fixtures/record/token-estimation-v1.json` 保存匿名合成输入与离线 tokenizer 标定数据；不含玩家数据或密钥。原生窗口夹具继续放 `tests/rust/native-platform/`，不把需要私有实现访问的单测迁入此处。
