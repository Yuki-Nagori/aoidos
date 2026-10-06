# 目录与模块规划

[架构总览](README.md)

```text
├── index.html                 # Vite 入口 HTML（挂载点 #app）
├── src-web/                   # 前端：Vue 3 + TypeScript
│   ├── main.ts                # 应用入口：挂载根组件
│   ├── App.vue                # 根组件：greet 示例（IPC 往返 + 浏览器回退 + 主题切换）
│   ├── app.css                # Tailwind v4 接线与设计 token 真源（分层见 ui.md / theming.md）
│   ├── api/                   # IPC 薄调用与 TS 载荷类型
│   ├── utils/                 # 纯逻辑函数，配单测（计入覆盖率门槛）
│   └── bench/                 # tinybench 基准示例
├── src-tauri/                 # Tauri 适配层：Rust（仅装配与命令层，业务进 src-rust）
│   ├── src/lib.rs             # 应用装配（Builder）；事件循环不可测，不入覆盖门槛
│   ├── src/commands.rs        # #[tauri::command] 命令层、类型化载荷与命令单测
│   ├── src/llm_commands.rs    # LLM 配置 / 凭据命令（019）；平台原生输入经 mythos-llm 分发
│   ├── src/ipc.rs             # CmdError 与域 / 平台错误映射
│   ├── src/events.rs          # 每流序号与事件信封，实际发送在 lib.rs
│   ├── tauri.conf.json        # 窗口 / 打包 / 开发服务器
│   ├── capabilities/          # IPC 权限声明
│   ├── icons/                 # 平台图标（tauri icon 生成，勿手改）
│   └── build.rs               # tauri-build
├── src-rust/                  # 业务 crate，不依赖 tauri（mythos-store 存储；mythos-llm 供应商适配 / 护栏 / 调度 / 配置凭据代理，providers/ 按厂商拆 adapter，platform/ 按平台拆原生能力）
├── tests/web/                 # Vitest 单测（目录镜像 src-web）
├── scripts/                   # 仓库脚本（setup.mts 环境配置；coverage-rust.mts 跑覆盖率门禁）
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

新增文件的归属按职责判断，不按语言堆放；访问私有实现的测试贴近源码（如 `commands.rs` 的 `mod tests`），前端公开行为的测试放 `tests/web/` 并镜像目录结构。

## 命名

仓库 / 包名 `mythos`（`bun.lock` 的根 workspace 名同型），Rust package 同名、lib 名为下划线形态的 `mythos_lib`；产品显示名 `Mythos`（`tauri.conf.json` 的 productName 与窗口标题）。
