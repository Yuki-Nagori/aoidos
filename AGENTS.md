# AGENTS.md

Mythos：AI 驱动的剧情跑团桌面应用，技术栈 Bun + Vue 3 + TypeScript + Tauri 2。本文件是全部文档的索引（面向 AI 编码代理，人均可读），改代码前按需读所引文档。

## 文档路由

| 需要了解                                  | 去处                                                                                           |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 选型、版本口径、IPC 通道、CI              | [ai-docs/architecture/tech-stack.md](ai-docs/architecture/tech-stack.md)                       |
| 目录结构、模块归属、命名                  | [ai-docs/architecture/repository-layout.md](ai-docs/architecture/repository-layout.md)         |
| TS / Rust 职责边界、src-rust、工程纪律    | [ai-docs/architecture/ts-rust-boundary.md](ai-docs/architecture/ts-rust-boundary.md)           |
| 通信契约（命令 / 事件 / 错误码）          | [ai-docs/architecture/ipc-contract.md](ai-docs/architecture/ipc-contract.md)                   |
| LLM Provider、护栏、请求策略、凭据边界    | [ai-docs/architecture/llm.md](ai-docs/architecture/llm.md)                                     |
| 记录块、投影预算、持久化与恢复            | [ai-docs/architecture/record-engine.md](ai-docs/architecture/record-engine.md)                 |
| 回合阶段、骰判、场景推进与因果回退        | [ai-docs/architecture/turn-state-machine.md](ai-docs/architecture/turn-state-machine.md)       |
| 命令、verify 构成、覆盖率口径、打包、图标 | [ai-docs/architecture/build-and-development.md](ai-docs/architecture/build-and-development.md) |
| 舞台、面板四态、输入与骰判交互            | [ai-docs/architecture/ui-shell.md](ai-docs/architecture/ui-shell.md)                           |
| 前端约定（逻辑归属 / SFC / mock 模式）    | [ai-docs/standards/frontend.md](ai-docs/standards/frontend.md)                                 |
| UI 风格（设计语言 / token / 动效）        | [ai-docs/standards/ui.md](ai-docs/standards/ui.md)                                             |
| Rust 约定（命令层 / 工作区 / profile）    | [ai-docs/standards/rust.md](ai-docs/standards/rust.md)                                         |
| 测试、覆盖率门槛、knip、基准              | [ai-docs/standards/testing.md](ai-docs/standards/testing.md)                                   |
| 文档怎么写、图表规则                      | [ai-docs/standards/documentation.md](ai-docs/standards/documentation.md)                       |
| 注释怎么写（Rust / TS·Vue / TODO）        | [ai-docs/standards/comments.md](ai-docs/standards/comments.md)                                 |
| 提交一致性与消息格式                      | [ai-docs/standards/commits.md](ai-docs/standards/commits.md)                                   |
| 当前任务与状态                            | [ai-docs/task-index.md](ai-docs/task-index.md)                                                 |
| 未排期的产品构想                          | [ai-docs/ideas/](ai-docs/ideas/)                                                               |
| 开源仓库调查与灵感                        | [ai-docs/research/](ai-docs/research/)                                                         |
| 建新任务                                  | [ai-docs/task/_template.md](ai-docs/task/_template.md)                                         |

## 约定速查

- 一切命令经根 `package.json` 的 bun scripts；`bun run verify` 十项全过才算完成，husky 只查格式，CI 跑全量。
- `bun.lock` 与根 `Cargo.lock` 提交并保持同步（工作区唯一一份 `Cargo.lock`）。
- 命令参数 / 返回类型在 Rust 定型后，TS 侧立即声明同型；`invoke` 无校验透传。
- 前端纯逻辑进 `utils/` 配单测；Rust 命令层薄，业务长大后拆 crate 进 workspace members。
- 非平凡改动先建 task（`ai-docs/task/_template.md`）再实现；提交消息带 `Task: NNN`，CI 修复等平凡改动不立项（例外见[任务索引](ai-docs/task-index.md)与[提交规范](ai-docs/standards/commits.md)）。
- 改技术行为的 commit 同步受影响文档；目录树用文本代码块，mermaid 入库前渲染校验。
