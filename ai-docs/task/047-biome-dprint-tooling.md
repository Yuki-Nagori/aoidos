# 047 — Biome 与 dprint 工具链迁移

- 状态：in-progress
- 依赖：无
- 优先级：P1
- 创建 / 更新：2026-10-13 / 2026-10-13

## 目标与背景

以 Biome 替换 ESLint 的前端 lint 与 Prettier 的格式化职责，并用 dprint 覆盖 Biome 暂未成熟支持的 Markdown、Vue 等格式。迁移必须保留现有 TypeScript、Vue、i18n 与格式门禁的可观察检查；工具不能覆盖的项目约定由仓库脚本显式承接。

## 必读

- [前端规范](../standards/frontend.md)
- [测试规范](../standards/testing.md)
- [工具链架构](../architecture/tech-stack.md)
- [开发与验证命令](../architecture/build-and-development.md)
- [Biome formatter](https://biomejs.dev/formatter/)（查阅：2026-10-13）
- [Biome configuration](https://biomejs.dev/reference/configuration/)（查阅：2026-10-13）
- [Biome VS Code](https://biomejs.dev/reference/vscode)（查阅：2026-10-13）
- [Biome suppressions](https://biomejs.dev/analyzer/suppressions/)（查阅：2026-10-13）
- [Biome `useImportType`](https://biomejs.dev/linter/rules/use-import-type/javascript/)（查阅：2026-10-13）
- [Biome Vue 支持](https://biomejs.dev/internals/changelog/version/2-3-0/)（实验性支持，查阅：2026-10-13）
- [dprint 插件](https://dprint.dev/plugins/)（查阅：2026-10-13）

## 范围与非目标

- Biome 负责 JavaScript / TypeScript / Vue、CSS、JSON 的 lint 与其支持范围内的格式化。
- dprint 负责 Markdown、Vue / HTML、YAML 等 Biome 不支持或尚不稳定的格式；发布剧本原文继续按字节冻结。
- 删除 ESLint、Prettier 及其专用配置和依赖，修正主题 CSS 生成器对 Prettier API 的直接依赖。
- 同步 VS Code 默认 formatter、保存修复与扩展推荐，避免编辑器仍调用旧工具。
- 保留并加强 i18n 检查：语言键对齐、占位符一致、静态引用存在、未使用键诊断及预留命名空间约束。
- Rust 格式化 / lint、TypeScript 类型检查、测试及现有 `verify` 次序不在范围内。
- 不迁移其他项目或剧本仓库。

## 前置条件与待决策

- 项目选择 Biome + dprint 的激进迁移；Vue 语法 lint / format 的实验性差异须通过现有代码、类型检查、测试和独立审查验证。
- dprint 的远程 Wasm 插件需固定版本 URL 并由锁定文件及 CI 验证；不使用浮动版本。

## 实施步骤

1. 盘点 ESLint / Prettier 实际规则、忽略路径、生成脚本依赖和 CI / hook 入口。
2. 增加 Biome、dprint 配置和锁定版本，迁移 lint / format 命令；替换生成器格式化接口。
3. 补齐 i18n 检查，删除 ESLint / Prettier 配置、依赖与陈旧文档引用，更新 CI 和前端规范。
4. 运行格式化并检查差异，运行与本任务相关的门禁及 `bun run verify`。
5. 由独立 subagent 审查工具规则覆盖与变更，处理发现并复验。

## 预计改动

`package.json`、`bun.lock`、Biome / dprint 配置、VS Code 设置、`scripts/i18n-check.mts`、`scripts/theme-defaults.mts`、CI、Husky hook，以及前端与工具链文档和本 task。

## 验收标准

- [x] Biome / dprint 覆盖原 ESLint / Prettier 当前检查的目标文件，剧本原文冻结规则保留。
- [x] i18n 专用检查覆盖缺失 / 未使用键、跨语言键和占位符一致性及保留命名空间。
- [x] 当前工具链配置、VS Code、CI、hook 与活动文档不再调用 ESLint / Prettier；历史 task 记录保留当时的工具状态，Rust 门禁不变。
- [x] 生成主题 CSS 的检查和构建可重复，产物不含无关格式漂移。
- [ ] `bun run verify` 十三项全通过；CI 复验通过。
- [x] subagent 独立评审完成，发现的问题已修复并复验。

## 验证计划与结果

| 日期       | 环境 / 命令                    | 预期                          | 实际结果                                                                 |
| ---------- | ------------------------------ | ----------------------------- | ------------------------------------------------------------------------ |
| 2026-10-13 | Biome / dprint / i18n 局部门禁 | 格式、规则及静态 key 诊断正确 | 通过；覆盖 SFC 模板反引号 key、注释伪引用、动态对象 / 函数参数和缺失 key |
| 2026-10-13 | `bun run verify`               | 十三项全部通过                | 全部通过；覆盖报告仅含既有 2 行预算，生产构建与 rustdoc 通过             |

## 风险与回退

Biome 的 Vue lint / formatter 尚属实验支持；dprint 的 Vue 格式插件也需要检查空白敏感模板的输出。先审阅完整格式 diff 和测试结果，再接受格式化结果；如插件错误修改渲染敏感空白，收窄其匹配范围并保留明确的受支持文件覆盖，不能静默跳过门禁。

## 决策与工作记录

- 2026-10-13：登记激进迁移方案，采用 Biome + dprint 全面替换 ESLint / Prettier；明确保留 i18n 语义检查并替换主题生成器的 Prettier API 调用。
- 2026-10-13：按 Biome 官方配置收窄扫描排除，补 `vcs.defaultBranch` 与本地 schema；Vue lint 保留、格式交给 dprint。编辑器同步切换 Biome / dprint；i18n 检查改用 TypeScript 与 Vue AST，避免正则误判模板字面量和注释。生成主题 CSS 改为直接输出 Biome 认可的确定格式。
- 2026-10-13：6.1 Sol medium 独立评审发现 i18n AST 检查器对普通 `key` 属性及条件分支中的动态函数参数误报；已限制收集范围并用 fixture 验证动态对象 / 参数不会误识别、真实静态 key 仍能诊断。最终复审无遗留问题；本地 `bun run verify` 十三项通过，等待 CI 复验。

## 完成摘要

本地工具链迁移、i18n 检查和独立整体评审已完成，`bun run verify` 十三项全部通过；等待 CI 复验后关闭 task。
