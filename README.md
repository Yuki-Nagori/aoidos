# Mythos

AI 驱动的剧情跑团桌面应用。你不是在和 AI 聊天，而是在和它共同编排一段有因果、有推进、有结局的故事。

## 核心流程

1. **创建世界（Mythos）**：定义时代、地点、基调与规则——中世纪奇幻、赛博朋克、克苏鲁怪谈，或自己捏的设定，AI 据此构建自洽的世界观。
2. **登台角色（Persona）**：创建角色——身份、性格、能力、目标；NPC 由 AI 分别扮演，各有独立的性格、动机与记忆，不是千篇一律的工具人。
3. **开演（Scene）**：故事从场景开始，你用自然语言描述行动、对话与心理活动；没有固定选项，想做什么就做什么。
4. **推进（Plot）**：世界自行运转——NPC 各有目的，事件自行发酵，每个选择都产生后果；故事有张力、有转折，而非无限循环的日常对话。
5. **掷骰与判定（Dice / Check）**：不确定的结果交给骰子，能力、处境与积累的优劣势影响成败；失败不是终点，而是剧情走向另一条分支。
6. **记忆与成长（Memory / Growth）**：关键事件、人物关系与选择被记住；角色成长，NPC 变化，世界因你的行动而改变。
7. **落幕（Epilogue）**：故事有始有终，结局由你和 AI 共同造就，而不是预设好的选项。

核心体验：**自由**（没有固定剧本）、**因果**（每个选择都有后果，世界会记住）、**共演**（AI 是和你一起演戏的搭档）、**有终**（故事会结束，而不是无限拖延）。

一句话：Mythos 让你和 AI 一起，从一个设定出发，演出一段有开头、有中段、有结尾的故事。

## 状态与记忆的构想

对话历史全量进 LLM 上下文，会随剧情推进线性变贵、变慢。Mythos 的构想是在本地维护世界状态库（属性、关系、事件）与一组对局文档，LLM 每轮按索引读取「现状与下一步」，而不是回放全部对话缓存，让上下文占用与对局时长解耦。当前 v1 采用固定前缀、recap 与近期窗口的[记录投影设计](ai-docs/architecture/record-engine.md)，尚未实现；检索式方案保留接口位置，后续按记忆任务推进，完整构想见[构想索引](ai-docs/ideas/README.md)。

## 当前状态

可运行的工程骨架：`greet` 示例演示 IPC 往返（桌面 + 浏览器回退），一条 `bun run verify` 覆盖双端类型检查、ESLint / Clippy、Prettier / rustfmt、双端测试与 100% 覆盖率门槛、knip 死代码检查。产品功能尚未实现，路线图见 [ai-docs/task-index.md](ai-docs/task-index.md)。

## 快速开始

环境：[Bun](https://bun.sh) 与 [rustup](https://rustup.rs)。`bun install` 一次完成全部配置：npm 依赖、husky 钩子，以及 postinstall 里的 Rust 侧自检（按 `rust-toolchain.toml` 装工具链、补装 cargo-llvm-cov）。Windows 一般自带 WebView2，Linux 见 [Tauri 前置要求](https://tauri.app/start/prerequisites/)。

```console
$ bun install            # npm 依赖 + husky + Rust 工具链 + cargo-llvm-cov，一条到位
$ bun run tauri:dev      # 桌面窗口（Vite HMR + cargo 增量编译）
$ bun run dev            # 或：浏览器纯前端预览（无 IPC，示例命令走前端回退）
```

## 常用命令

| 命令                                | 作用                                                 |
| ----------------------------------- | ---------------------------------------------------- |
| `bun run verify`                    | 提交门禁（CI 逐步执行同一组检查）                    |
| `bun run tauri:dev` / `tauri:build` | 桌面开发 / 打包安装包（`target/release/bundle/`）    |
| `bun run build`                     | 类型检查 + 前端构建                                  |
| `bun run test` / `test:coverage`    | 测试 / 测试 + 覆盖率门槛                             |
| `bun run lint` / `format`           | ESLint / Prettier（`:rust` 后缀为 Clippy / rustfmt） |
| `bun run bench` / `knip`            | 基准示例 / 死代码检查                                |

## 文档

完整文档入口是 [AGENTS.md](AGENTS.md)，体系在 [ai-docs/](ai-docs/)：`architecture/` 说明结构与技术栈，`standards/` 是编码与提交规范，`task/` 承载工作记录。换图标、替换 greet 示例等见[构建与开发](ai-docs/architecture/build-and-development.md)。

## License

Apache-2.0
