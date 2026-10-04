# 003 — UI 风格规范落档（standards/ui.md）

- 状态：done
- 依赖：002
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

用户认可 Herta 的 UI 风格，要求把 [research/001](../research/001-herta.md) 调查所得的设计语言沉淀为 Mythos 的视觉规范。按约定（research 产出 research 外文件时，超出部分须建 task 记录，task 只记额外部分），本 task 只覆盖规范落档与索引接线；调查报告本身不属本 task 范围，作为输入引用。

## 必读

[Herta 调查](../research/001-herta.md) · [文档规范](../standards/documentation.md) · [前端约定](../standards/frontend.md)。

## 范围与非目标

交付：`ai-docs/standards/ui.md`——设计基调（玻璃拟态 + 双 register）、主题与 token（起始色板表，浅 / 深）、字体双栈、布局（880px measure / 共享左缘）、按行类型的视觉语法、动效词汇与降级、禁则、平台风险；同步 standards 索引表、AGENTS 路由、research/001 产出列回链。

非目标：实现样式（`src-web/app.css` 仍为示例，待首个真实界面按规范落地，届时偏差回写本规范）；任何代码改动；换图标 / 品牌色适配（规范中只留方向）。

## 前置条件与待决策

- 前置：research/001 完稿；002 已建立产品语境的文档体系。
- 决策：规范标状态**规划**，不把未实现写成已有；色板以 Herta `reference-ux.css` 数值为基线并标注可按 Mythos 品牌（青绿 / 橙 / 紫）调整；明确只借设计语言、不使用其美术资产。
- 待决策（留给首个界面 task）：品牌色是否替换基线；样式文件组织（单文件分片 vs 按组件拆分）落地时定。

## 实施步骤

1. 从调查报告提炼设计语言，写入 `standards/ui.md`。
2. 接线：standards 索引表、AGENTS 路由、research 索引产出列。
3. 链接检查与格式门禁。

## 预计改动

新建 `ai-docs/standards/ui.md`；修改 `ai-docs/standards/README.md`、`AGENTS.md`、`ai-docs/research/README.md`（均现存）。无代码与配置影响。

## 验收标准

- [x] 规范存在且状态标「规划」，数值可溯源（注明 Herta 基线）。
- [x] 覆盖基调 / token / 字体 / 布局 / 视觉语法 / 动效 / 禁则 / 平台风险，与调查报告一致。
- [x] standards 索引与 AGENTS 路由可达，research/001 产出列回链规范。
- [x] 全仓库 md 相对链接 0 断链；`bun run format:check` 通过。

## 验证计划与结果

| 日期       | 命令                     | 预期   | 实际结果                         |
| ---------- | ------------------------ | ------ | -------------------------------- |
| 2026-10-05 | 链接检查脚本（21 个 md） | 0 断链 | 修复 1 处 `../../` 误用后 0 断链 |
| 2026-10-05 | `bun run format:check`   | exit 0 | exit 0                           |

## 风险与回退

规范为纯文档，无运行时风险。风险在采纳侧：Herta 的 Chromium-only CSS 特性在非 WebView2 平台的兼容性已在规范「平台风险」节声明，首个界面落地时实测并回写。回退：删除规范文件并还原三处索引接线。

## 决策与工作记录

- 2026-10-05：随 research/001 一并产出规范与接线；应用户约定补记本 task（约定：research 外的超产部分记录 task，task 只记额外部分；纯加 research 不用）。

## 完成摘要

UI 风格规范落档完成：standards/ui.md（规划状态）+ 三处索引接线；验证证据见上表。限制：规范未实现到代码，品牌色适配与样式组织方式留给首个真实界面 task。
