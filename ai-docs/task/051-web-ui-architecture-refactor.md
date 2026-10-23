# 051 — Web UI 架构拆分

- 状态：done
- 依赖：025、034、046、047
- 优先级：P1
- 创建 / 更新：2026-10-19 / 2026-10-19

## 目标与背景

当前 `src-web/` 已有 API、composable、utility 和若干界面组件，但根 `App.vue` 仍组合多个产品流程、状态和页面模板，新增设置、记忆与计费界面时会继续扩大根组件职责。先按清晰的应用、功能和共享边界拆分，保持现有产品行为与视觉不变，为 052 的设计移植提供稳定结构。

## 必读

[前端规范](../standards/frontend.md) · [UI 风格](../standards/ui.md) · [UI 架构](../architecture/interface/ui-shell.md) · [目录规划](../architecture/core/repository-layout.md) · [分层规范](../standards/layering.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md) · [提交规范](../standards/commits.md)。

## 范围与非目标

- 将前端结构整理为应用组合层、按功能归档的页面 / 组件 / composable、跨功能共享展示与纯逻辑，以及既有 `api/`、`i18n/`、`styles/` 独立层。
- 从 `App.vue` 拆出具有独立状态生命周期或页面职责的组合；保留现有 API、用户流程、locale、主题、面板状态与视觉，不改业务协议。
- 更新架构图、目录规划、前端 / 分层规范与测试镜像规则，使代码、测试和文档中的目录一致。
- 不在本 task 移植 `aoidos-art` 页面样式、不新增产品功能、不引入路由、全局 store 或组件库；若实测需要新依赖，先记录理由并独立审查。
- 不以文件行数、组件数量或目录深度作为目标；仅在职责、状态归属或复用边界更清楚时拆分。

## 前置条件与待决策

025 的现有界面能力、034 的本地化和 046 的角色选择已实现；可接受其已交付实现状态，不将未完成的原生平台验收误报为本 task 已验证。实施开始前依据当前 `src-web/` 与 `tests/web/` 实际引用确认最终 feature 边界；保持依赖单向，不让共享层反向导入 feature。

## 实施步骤

1. 标注 `App.vue` 当前状态、事件与模板职责，确定 feature 与 shared 的真实边界。
2. 以小步移动并收紧模块导出，维持功能组件的 props / emits 和 API 类型契约；同步镜像测试及选择器。
3. 补充针对组合根、功能页面与跨层依赖的行为验证；不复制实现细节或为覆盖率造测试。
4. 更新架构 README 链路图、目录规划、规范和本 task；执行受影响检查与完整 `bun run verify`。
5. 由独立 subagent 评审完整 diff，处理发现后复验。

## 预计改动

将前端组合拆为 `src-web/app/`、`src-web/features/<feature>/` 与 `src-web/shared/`，按职责迁移原组件、composable、utility 和镜像测试。共享层仅保留跨 feature 能力；应用层组合设置与记忆面板，对局工作区持有游戏界面生命周期。同步 `ai-docs/standards/frontend.md`、`ai-docs/standards/layering.md`、`ai-docs/architecture/README.md`、`ai-docs/architecture/core/repository-layout.md`、`ai-docs/architecture/interface/ui-shell.md`、`ai-docs/architecture/interface/i18n.md`、CI selector、coverage 口径与入口测试文档路径。

## 验收标准

- [x] `App.vue` 仅负责应用入口级组合，不承载多项 feature 的状态机、业务流程及大段页面模板。
- [x] 页面 / 组件 / composable / 纯逻辑归属能从目录直接判断；共享层没有 feature 依赖，feature 间不存在循环依赖。
- [x] 现有对局、输入、记录、恢复、主题、语言、费用、记忆与角色选择行为保持一致；没有复制 API 或引入第二套状态真源。
- [x] 测试跟随新目录并通过影响范围选择器，Vitest 行为验证覆盖主要用户路径。
- [x] 架构链路图、目录规划、规范与 task 同步；独立评审完成，评审发现已处理。
- [x] 最终 `bun run verify` 全项通过。

## 验证计划与结果

| 日期       | 环境 / 命令                        | 预期                 | 实际结果                                                                                                                                                                                        |
| ---------- | ---------------------------------- | -------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-19 | `bun run verify`                   | 全项通过             | 通过：前端 46 个文件 / 409 个测试，前端覆盖率语句 99.63%、分支 99.19%、函数 100%、行 99.73%；Rust 覆盖率 96.82%，Web 构建、格式、lint、i18n、依赖分层、knip、benchmark 与 Rust 文档生成均通过。 |
| 2026-10-19 | 独立架构评审（gpt-6.1-sol medium） | 无未处理的高风险发现 | 通过；修复组件偏好注入、DOM refs / 历史修订连接、错误映射重复和 native selector，并补齐组件装配测试与文档。                                                                                     |

## 风险与回退

移动模块可能改变 Vite、Vitest、knip、i18n 和覆盖率选择器的发现范围。每次移动后检查构建入口、静态扫描和测试镜像；若 feature 边界迫使共享层形成反向依赖，先调整接口 / 组合位置，不用动态导入掩盖循环。

## 决策与工作记录

- 2026-10-19：与 052 分开登记。051 只处理结构和职责，要求现有视觉 / 行为不变，避免架构重构与大规模样式变化混在一个评审中。
- 2026-10-19：工作区 composable 只依赖 `focus()` 端口，不依赖具体输入组件；共享错误转换支持调用方提供 fallback code，保留偏好错误的 `store.io` 语义。共享层可以引用 `api/` 的纯类型，不依赖 IPC 运行时。

## 完成摘要

已完成 feature-first 迁移、应用组合根、跨 feature shared 边界和测试镜像；修复 review 发现并补齐工作区 / 账单组件的动态行为测试。架构链路、目录、依赖规则、coverage 与 CI selector 均与实现同步。`bun run verify` 和独立评审通过。
