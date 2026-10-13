# AGENTS.md

Aoidos 是基于 Bun、Vue 3、TypeScript 和 Tauri 2 的 AI 剧情跑团桌面应用。本文件为仓库级协作规则与文档索引；开始改动前按工作范围阅读对应文档。

## 文档导航

### 架构与工程

| 主题                                     | 文档                                                                                           |
| ---------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 架构总览、模块关系与设计实施映射         | [ai-docs/architecture/README.md](ai-docs/architecture/README.md)                               |
| 技术选型、版本、IPC 通道与 CI            | [ai-docs/architecture/tech-stack.md](ai-docs/architecture/tech-stack.md)                       |
| 目录结构、模块归属与命名                 | [ai-docs/architecture/repository-layout.md](ai-docs/architecture/repository-layout.md)         |
| TypeScript / Rust 职责边界与工程纪律     | [ai-docs/architecture/ts-rust-boundary.md](ai-docs/architecture/ts-rust-boundary.md)           |
| 命令、事件与错误码契约                   | [ai-docs/architecture/ipc-contract.md](ai-docs/architecture/ipc-contract.md)                   |
| 数据目录、SQLite、实例锁、原子写入与迁移 | [ai-docs/architecture/storage.md](ai-docs/architecture/storage.md)                             |
| Provider、请求策略、护栏与凭据           | [ai-docs/architecture/llm.md](ai-docs/architecture/llm.md)                                     |
| 模型计价、额度、账本与预算周期           | [ai-docs/architecture/billing.md](ai-docs/architecture/billing.md)                             |
| 记录块、投影预算、持久化与恢复           | [ai-docs/architecture/record-engine.md](ai-docs/architecture/record-engine.md)                 |
| 记忆三域、门控、回写与待决策项           | [ai-docs/architecture/memory.md](ai-docs/architecture/memory.md)                               |
| 记忆算法、正确性约束与标定实验           | [ai-docs/architecture/memory-algorithms.md](ai-docs/architecture/memory-algorithms.md)         |
| 回合阶段、骰判、场景推进与因果回退       | [ai-docs/architecture/turn-state-machine.md](ai-docs/architecture/turn-state-machine.md)       |
| 舞台、面板状态、输入与骰判交互           | [ai-docs/architecture/ui-shell.md](ai-docs/architecture/ui-shell.md)                           |
| 界面语言、原生文案、偏好与格式化         | [ai-docs/architecture/i18n.md](ai-docs/architecture/i18n.md)                                   |
| 主题 token、偏好防闪、皮肤校验与回退     | [ai-docs/architecture/theming.md](ai-docs/architecture/theming.md)                             |
| 命令、verify、覆盖率、打包与图标         | [ai-docs/architecture/build-and-development.md](ai-docs/architecture/build-and-development.md) |

### 开发规范

| 主题                                    | 文档                                                                     |
| --------------------------------------- | ------------------------------------------------------------------------ |
| 规范目录与官方依据                      | [ai-docs/standards/README.md](ai-docs/standards/README.md)               |
| Vue / TypeScript 约定、SFC 与 mock      | [ai-docs/standards/frontend.md](ai-docs/standards/frontend.md)           |
| UI 风格、设计 token 与动效              | [ai-docs/standards/ui.md](ai-docs/standards/ui.md)                       |
| Rust 约定、命令层、workspace 与 profile | [ai-docs/standards/rust.md](ai-docs/standards/rust.md)                   |
| 测试策略、覆盖率、knip 与基准           | [ai-docs/standards/testing.md](ai-docs/standards/testing.md)             |
| 文档结构与图表                          | [ai-docs/standards/documentation.md](ai-docs/standards/documentation.md) |
| Rust / TypeScript / Vue 注释与 TODO     | [ai-docs/standards/comments.md](ai-docs/standards/comments.md)           |
| 剧本格式、来源、解析与可信登记          | [ai-docs/standards/scenarios.md](ai-docs/standards/scenarios.md)         |
| 提交内容与消息格式                      | [ai-docs/standards/commits.md](ai-docs/standards/commits.md)             |

### 项目规划

- 当前任务和状态：[ai-docs/task-index.md](ai-docs/task-index.md)
- 新任务模板：[ai-docs/task/_template.md](ai-docs/task/_template.md)
- 未排期的产品构想：[ai-docs/ideas/](ai-docs/ideas/)
- 外部项目调查与参考：[ai-docs/research/](ai-docs/research/)

## GitHub、分支与提交

- GitHub issue、PR、Actions 和仓库状态的查询与操作统一使用已认证的 `gh` CLI，例如 `gh issue`、`gh pr`、`gh run`；需要低层接口时使用 `gh api`。不要改用网页手工操作或自写 HTTP 请求。执行写操作前确认仓库、目标编号、当前分支及用户授权范围。
- 代码、测试、依赖、构建 / CI 配置，以及实现与配套文档混合的改动，都在专用工作分支完成；task 实现和 issue 修复默认通过 PR 合并到 `main`。
- 仅文档 issue 可在 `main` 直接提交，且须用户明确要求或已在当前会话授权。提交前确认改动确实只有文档，工作区没有夹带其他内容。实现任务中的文档同步不属于此例外。
- 不确定是否为纯文档改动时，按需要分支处理；测试、脚本、配置或生成逻辑都不算纯文档。
- 不直接推送未经审阅的实现、不合并 PR、不改写共享分支历史，除非用户在当前任务中明确授权。分支和 PR 生命周期遵循当前指示。

## Subagent 使用规则

### 何时调用

- 每个非平凡 task 在交付前安排一次独立评审。评审应基于需求、风险和完整 diff 检查，而不是复述实现者自检。拼写、格式或单行小修复不强制评审；用户明确要求时除外。
- 覆盖率缺口按[测试规范](ai-docs/standards/testing.md)拆分调查或修复；主代理整合结果并运行完整门禁。不得为达到覆盖率数字而机械补断言或豁免代码。
- 复杂问题可按独立产出并行委派，例如跨平台 CI 根因、模块边界或文档一致性。委派前明确任务边界，避免多个代理同时修改同一文件或逻辑。
- 常规小改动、主代理可直接完成且没有独立评审要求时，不为形式启动 subagent。

### 默认模型

- 代码与架构的独立评审默认使用 `gpt-6.1-sol`、`medium` reasoning effort。用户对当前工作指定其他模型或强度时，遵循用户指示。
- 调查和实现子任务默认沿用当前代理可用模型；只有用户或任务要求时才指定覆盖。工具支持时，在委派参数中明确写出所需模型与 reasoning effort。

### 委派与评审边界

- 委派说明列出目标、范围、相关路径、验收条件、所需检查，以及任务是只读评审还是允许修改。独立评审默认只读，不顺手改写被评代码。
- 评审发现应指出文件 / 行号、触发条件和影响；无发现时说明检查范围与限制。主代理判断发现是否成立，负责修复、复验和同步文档，不把评审结论当作测试通过证明。
- 等待评审时继续处理不依赖其结论的工作。修复评审发现后，重跑相关检查和完整交付门禁；必要时让评审者复核修复。
- subagent 不自行提交、推送、创建或合并 PR，也不代表用户对外沟通，除非用户明确授权。

## 日常工程约定

- 命令通过根 `package.json` 中的 Bun scripts 执行；`bun run verify` 当前十二项全部通过才算本地验证完成。Husky 检查格式，CI 执行完整平台门禁。
- `bun.lock` 与根目录唯一的 `Cargo.lock` 随依赖改动同步提交。
- Rust 定型命令参数和返回值后，TypeScript 侧立即声明同型；`invoke` 不负责运行时校验。
- 前端纯逻辑放在 `utils/` 并配行为测试；Rust 命令层保持薄，业务增长后拆为 workspace crate。
- 非平凡改动先按[任务模板](ai-docs/task/_template.md)登记 task；提交消息包含 `Task: NNN`。CI 修复等平凡改动可不建 task，例外遵循[任务索引](ai-docs/task-index.md)和[提交规范](ai-docs/standards/commits.md)。
- 技术行为变化同步更新受影响文档。目录结构使用文本代码块；Mermaid 图入库前渲染校验。
