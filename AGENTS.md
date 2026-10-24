# AGENTS.md

Aoidos 是基于 Bun、Vue 3、TypeScript 和 Tauri 2 的 AI 剧情跑团桌面应用。本文件为仓库级协作规则与文档索引；开始改动前按工作范围阅读对应文档。

## 文档导航

### 架构与工程

| 主题                               | 文档                                                             |
| ---------------------------------- | ---------------------------------------------------------------- |
| 架构总览与领域导航                 | [ai-docs/architecture/README.md](ai-docs/architecture/README.md) |
| 核心工程、模块边界、IPC 与构建     | [core](ai-docs/architecture/core/README.md)                      |
| 存储、记录与记忆                   | [data](ai-docs/architecture/data/README.md)                      |
| Provider、提示输入与计费           | [AI 与计费](ai-docs/architecture/ai/README.md)                   |
| 周目、任务、回合和角色             | [游戏规则](ai-docs/architecture/gameplay/README.md)              |
| 角色真值、认知、表面身份与引导 NPC | [角色架构](ai-docs/architecture/gameplay/characters/README.md)   |
| 界面结构、主题、快捷键与语言       | [用户界面](ai-docs/architecture/interface/README.md)             |
| Rust / Web / 测试依赖方向          | [分层规范](ai-docs/standards/layering.md)                        |

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
- 不直接推送未经审阅的实现、不改写共享分支历史。用户授权按本流程推进的实现任务：在专用工作分支完成工作并提交，创建 PR；持续等待 CI。CI 报错时，在同一工作分支定位并修复问题、运行相关验证后推送更新，再继续等待 CI；CI 全部通过且仓库要求满足后合并 PR。合并后在本地切回 `main`，运行 `git pull --ff-only origin main` 同步合并结果，再清理已确认废弃的本地分支。删除前确认分支关联的 PR 已合并或远端分支已删除；保留仍有未合并工作的分支和任何工作区改动。

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

- 命令通过根 `package.json` 中的 Bun scripts 执行。常规本地提交前运行 `bun run test:affected`、`bun run format:affected` 和 `bun run lint:affected`，不要每个提交都运行全量 `bun run verify`；选择器遇到共享配置、未知输入或差异不可用时会自动扩展到全量。任务收尾仍运行一次 `bun run verify`；PR CI 在 Ubuntu 执行格式、lint、Clippy 等质量检查，并在三平台构建 Web 产物、受影响 Tauri 应用和运行受影响测试，相关原生验收在三平台执行。Nightly 在三平台构建并运行完整 Vitest、Rust / 原生测试，Ubuntu 运行完整 `verify` 与覆盖率。
- 仓库中的 Rust 命令统一通过 `scripts/cargo.mts`、`scripts/toolchain.mts` 或 `scripts/rust-toolchain.ts` 按 `rust-toolchain.toml` 调用 `rustup run`；不要在项目脚本中直接调用系统 `cargo` 或 Tauri CLI，以免 Homebrew 等 PATH 优先项绕过仓库锁定的工具链。
- `bun.lock` 与根目录唯一的 `Cargo.lock` 随依赖改动同步提交。
- Rust 定型命令参数和返回值后，TypeScript 侧立即声明同型；`invoke` 不负责运行时校验。
- 前端单 feature 纯逻辑放在该 feature 的 `utils/`，跨 feature 逻辑放 `src-web/shared/` 并配行为测试；Rust 命令层保持薄，业务增长后拆为 workspace crate。
- 非平凡改动先按[任务模板](ai-docs/task/_template.md)登记 task；提交消息包含 `Task: NNN`。CI 修复等平凡改动可不建 task，例外遵循[任务索引](ai-docs/task-index.md)和[提交规范](ai-docs/standards/commits.md)。
- 技术行为变化同步更新受影响文档。目录结构使用文本代码块；Mermaid 图入库前渲染校验。
- CI workflow 保留 job 编排与平台条件；重复的工具安装、选择计划执行和桌面会话步骤抽到 `.github/actions/` composite action。测试、格式与 lint 复用 `scripts/ci-scope.mts` 的变更计划；未知路径、共享配置或差异不可用时必须全量回退，不得静默跳过。CI 在 `bun install` 前通过 setup action 安装 `cargo-llvm-cov`，避免 postinstall 在冷 runner 上源码编译。
