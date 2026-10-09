# 014 — 注释规范落档

- 状态：done
- 依赖：002
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

仓库无注释约定，注释风格散落于各文档的零星表述。参考 panta 同名规范（`ai-docs/standards/comments.md`）裁剪为 Aoidos 技术栈（Rust / TypeScript / Vue），落档为独立规范篇。过程中沉淀了一条本仓特有的实现约定：**错误映射等一行体委托用具名函数，不写 `map_err(|e| …)` 闭包**——闭包体的错误分支几乎不可触达，llvm-cov 行覆盖会因此永远差若干百分点，而具名函数可直接单测（动机：aoidos-store 实现中的实测）。

## 必读

[测试规范](../standards/testing.md) · [文档规范](../standards/documentation.md)。

## 范围与非目标

交付：`ai-docs/standards/comments.md`（写什么、Rust / TS·Vue 细则、TODO 格式、评审清单）；接线 standards 索引与 AGENTS 路由；任务队列「文档与基线」组登记。

非目标：既有代码注释的回溯清洗（随触碰逐文件对齐）；文档生成工具配置。

## 验收标准

- [x] comments.md 覆盖写什么 / Rust / TS·Vue / TODO 与评审，适配本仓技术栈。
- [x] standards 索引与 AGENTS 路由可达。
- [x] `bun run format:check` 通过；md 链接 0 断链。

## 验证计划与结果

| 日期       | 检查                      | 预期   | 实际结果 |
| ---------- | ------------------------- | ------ | -------- |
| 2026-10-05 | `bun run format:check`    | exit 0 | exit 0   |
| 2026-10-05 | md 链接检查脚本（全仓库） | 0 断链 | 0 断链   |

## 风险与回退

纯文档，无运行时风险。回退：删除规范文件并还原索引接线。

## 决策与工作记录

- 2026-10-05：创建任务。用户指定参考 panta `comments.md`；裁剪掉 C++ / Qt / CMake / Python 节，Rust 节新增「一行体错误映射用具名函数」约定（llvm-cov 行覆盖实测动机）。

## 完成摘要

注释规范落档完成并接入文档体系；后续代码注释与评审按此执行。
