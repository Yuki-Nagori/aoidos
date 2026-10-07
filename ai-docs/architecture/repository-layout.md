# 目录与模块规划

[架构总览](README.md)

```text
├── index.html                 # Vite 入口 HTML（挂载点 #app）
├── src-web/                   # 前端：Vue 3 + TypeScript
│   ├── main.ts                # 应用入口：挂载根组件
│   ├── App.vue                # 根组件：greet 示例（IPC 往返 + 浏览器回退 + 主题切换）
│   ├── app.css                # Tailwind v4 接线与设计 token 真源（分层见 ui.md / theming.md）
│   ├── api/                   # IPC 薄调用与 TS 载荷类型
│   ├── utils/                 # 纯消费规则 / 恢复协调及其他纯逻辑，配单测
│   ├── composables/           # Vue 身份、监听与 scope 清理，消费 utils
│   └── bench/                 # tinybench 基准示例
├── src-tauri/                 # Tauri 适配层：Rust（装配与命令适配，业务进 src-rust）
│   ├── src/lib.rs             # 应用装配（Builder）；事件循环不可测，不入覆盖门槛
│   ├── src/commands.rs        # #[tauri::command] 命令层、类型化载荷与命令单测
│   ├── src/turn_commands/     # 回合服务 / 事件适配，mod.rs 与 tests.rs 并列
│   ├── src/llm_commands.rs    # LLM 配置 / 凭据命令（019）；平台原生输入经 mythos-llm 分发
│   ├── src/ipc.rs             # CmdError 与域 / 平台错误映射
│   ├── src/events.rs          # 每流序号与事件信封，窗口投递由 turn_commands 适配
│   ├── tauri.conf.json        # 窗口 / 打包 / 开发服务器
│   ├── capabilities/          # IPC 权限声明
│   ├── icons/                 # 平台图标（tauri icon 生成，勿手改）
│   └── build.rs               # tauri-build
├── src-rust/                  # 不依赖 tauri 的业务 crate
│   ├── mythos-engine/         # 协调 / lease / 输出端口，src/turn/{mod.rs,tests.rs}
│   ├── mythos-store/          # 路径、原子发布、有界读取、实例锁与 SQLite 基建
│   └── mythos-llm/            # 供应商适配 / 护栏 / 调度 / 配置 / 凭据 / 代理
│       └── src/platform/     # 三平台原生输入；Unix 权限共用
├── tests/rust/native-platform/ # 真实 UI / OS 凭据 / 主 Webview IPC 集成测试 crate（显式桌面会话）
├── tests/web/                 # Vitest 单测（目录镜像 src-web）
├── scripts/                   # 仓库脚本（环境 / 覆盖率 / 原生前端夹具构建；独立 tsconfig）
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

标准库类型与调用不按 import 集中封装：各模块可直接使用 `Path`、`Duration`、`io::Error` 等。带有项目统一存储规则的能力（原子写、私有权限发布、有界读取、缺失路径判断、实例锁）归 `mythos-store`；业务 JSON / 版本校验归消费 crate，原生输入和权限适配归 `mythos-llm::platform`，请求超时归 LLM 调度。只提取真实共用的规则，不为统一调用外观增加转发层。

Rust 模块拆分后采用同目录 `mod.rs` 入口，规则见[Rust 规范](../standards/rust.md#rust-模块目录)。

新增文件的归属按职责判断，不按语言堆放；访问私有实现的测试贴近源码（如 `commands.rs` 的 `mod tests`），前端公开行为的测试放 `tests/web/` 并镜像目录结构。

## 命名

仓库 / 包名 `mythos`（`bun.lock` 的根 workspace 名同型），Rust package 同名、lib 名为下划线形态的 `mythos_lib`；产品显示名 `Mythos`（`tauri.conf.json` 的 productName 与窗口标题）。
