# 004 — 引入 Tailwind CSS v4 并接线设计 token

- 状态：done
- 依赖：002、009
- 优先级：P1
- 创建 / 更新：2026-10-05 / 2026-10-07

## 目标与背景

用户决定前端样式引入 Tailwind，替代「纯手写全局 CSS」路线在真实界面开发中的效率问题（Herta 的 8300 行单文件 CSS 是全职工艺，不适合快速堆界面）。选型 **Tailwind v4**：CSS-first（`@theme inline` 无 JS 配置）、原生消费 CSS custom properties——[UI 规范](../standards/ui.md)的 token 体系保持唯一真源不变，工具类只做消费层。本 task 落地安装、接线与验证；配套的规范修订（色板微调、默认深色主题、剧本皮肤契约、禁则改写）已于创建当日完成在 ui.md。

## 必读

[UI 风格规范](../standards/ui.md) · [主题架构](../architecture/theming.md) · [前端约定](../standards/frontend.md) · [测试规范](../standards/testing.md) · [构建与开发](../architecture/build-and-development.md)。

## 范围与非目标

交付：

1. 安装 `tailwindcss@^4` 与 `@tailwindcss/vite`，`vite.config.ts` 接插件。
2. `src-web/app.css` 改造为「`@import "tailwindcss"` + 语义真源层 + `@theme inline` 映射层 + 清默认色板（按 009 目录）+ `:root[data-theme]` 双主题值 + base 层」，默认深色。
3. 示例页 `App.vue` 换成工具类 + 少量组件类写法，验证链路完整（`dev` / `build` / `tauri:dev`）。
4. 确认 vitest / happy-dom、knip、coverage 口径不受影响（类名是纯字符串，预期无影响，跑通为准）。

非目标：真实业务界面（025）；剧本皮肤运行时 / 持久主题与首窗防闪（026）；UI 组件库（明确不引）；浅色主题精调（只保证 token 结构成立）。

## 前置条件与待决策

- 前置：002 / 009 done；ui.md 规范修订完成（本次 task 创建前已做）。
- 决策：Tailwind v4（非 v3——v4 无 tailwind.config.js，`@theme` 直接映射 token）；工具类为主 + `@layer components` 小组件类（气泡、机器行）为辅；默认主题深色；不引组件库。
- 命名 / 类型 / alias 以 009 目录为准，不在实现时另定白名单；组件类文件拆分按实际大小决定。

## 实施步骤

1. 安装依赖，接 Vite 插件。
2. app.css 改造：import → 清默认色板 / `@theme inline` 映射 → `:root[data-theme]` 双主题语义真源 → base（星云背景、字体栈）。
3. App.vue 换工具类写法，保留既有 mock 测试通过。
4. 跑 `bun run verify` 十项 + `format:check`。

## 预计改动

`package.json` 与 `bun.lock`（新增 tailwindcss / @tailwindcss/vite）、`vite.config.ts`、`src-web/app.css`、`src-web/App.vue` 及主题目录 / 映射互校测试。目录元数据以 009 定稿为准，与 026 同源夹具对接；其余均现存文件。

## 验收标准

- [x] `bun run build` 与 `tauri:dev` 正常，工具类与 token 均生效。
- [x] 语义 token 与 ui.md / 009 目录一致，`tests/web/appcss.test.ts` 互校目录 18 项（语义真源双主题定义 / inline 映射一一对应且无私增 / 直接消费项不进 @theme / 默认深色 / 清默认色板）；`data-theme="light"` 切换经示例页按钮手动验证生效（深浅两主题截图核对）。Rust 侧互校待 026。
- [x] vitest 套件全过（含新增目录互校，计数随测试增长以 CI 为准）；`bun run verify` 全项 exit 0。
- [x] ui.md 已按本次决策修订：色板按立绘微调（青绿 / 紫 / 橙）、默认深色主题、剧本皮肤 = token-only 覆盖契约、禁则改写为 Tailwind 选型（2026-10-05 完成）。

## 验证计划与结果

| 日期       | 命令                             | 预期                      | 实际结果                                                                                       |
| ---------- | -------------------------------- | ------------------------- | ---------------------------------------------------------------------------------------------- |
| 2026-10-07 | `bun run test` / `test:coverage` | 全过 + 覆盖率口径不受影响 | 4 文件 12 测试全过；coverage 只统计逻辑层，类名字符串不影响口径                                |
| 2026-10-07 | `bun run build` + 产物断言       | 工具类编译为语义 var 消费 | text-ink / bg-accent / hover 变体均编译为 var(--ink) 等；旧调色板类（.text-blue 等）不在产物中 |
| 2026-10-07 | `bun run tauri:dev`              | 窗口正常、token 生效      | 深浅双主题截图核对（发现并修复按钮文字对比度：text-ink → 保留的 white 原语）                   |
| 2026-10-07 | `bun run verify`                 | 全项通过                  | 十三项全部通过；前端套件连续 6 轮 12/12                                                        |

## 风险与回退

Tailwind v4 要求现代浏览器内核——WebView2（Chromium）满足；跨平台到 WKWebView / webkit2gtk 时在 task 内复核。happy-dom 环境只见类名字符串，预期无测试影响；若 knip 误报未用依赖，在 knip.json 显式登记。回退：移除依赖与插件，还原 app.css / vite.config.ts。

## 决策与工作记录

- 2026-10-05：创建任务。决策 v4 CSS-first；token 真源不变。同日完成 ui.md 配套修订：色板按立绘微调（`--accent` 青绿 `#1f7a68/#5cc8b4`、`--accent-violet` 紫、`--accent-warm` 橙、LED 蓝→紫 `#8b7cf6`、星云渐变起始值）、默认深色主题（去掉「跟随系统」）、新增「剧本皮肤」节（token-only 覆盖契约 + 回退 + 不可信默认）、禁则改为 Tailwind 选型。
- 2026-10-06：008 已定稿界面规范；本任务仍只承接 Tailwind / token 与示例基线，不扩大为完整舞台 / 面板实现，不预安装窗口状态或类型生成候选。
- 2026-10-06：按 009 / issue #12 回写方案 B：语义真源 + inline 映射 + 清默认色板，目录三方互校；持久化 / 皮肤运行时仍归 026。设计依赖改为 009，避免先实现再冻结目录。
- 2026-10-06：issue #28 澄清接线：默认渐变使用 0% stop，状态色按双主题值；M01 / M02 直接由组件消费，不进入 @theme 或生成同名缓动别名。
- 2026-10-07：实现完成。用户截图核对发现按钮文字对比度问题（ink 随主题变导致浅色底上不可读），按用户要求改恒白；white / black 以保留原语进 @theme static 并注释原因（on-accent 语义 token 待目录扩展登记，004 不私增目录项）。同轮 verify 间歇失败定位为 byte_split 测试的端口耗尽（与前端无关的存量抖动），改直驱 AdapterStream 消除，连带修正第七轮"21 轮零抖动"结论的适用条件（当时无并发 dev server / 端口压力）。

## 完成摘要

Tailwind v4（4.3.3）+ `@tailwindcss/vite` 落地：app.css 分层改造为「`@import "tailwindcss"` → 清默认色板（保留 white / black 两个原语作强调底恒定文字色，正式 on-accent token 待目录扩展登记）→ `@theme inline` 按目录映射 15 个语义 token → `:root[data-theme]` 双主题语义真源（值与 ui.md 色板 / 星云渐变一致）→ base（星云背景 fixed 预渲染、字体栈、reduced-motion 全局降级）→ components（glass-panel 玻璃面板、led-dot 脉冲指示）」。App.vue 换工具类写法并内置主题切换按钮。`tests/web/appcss.test.ts` 互校 theming.md 首版目录 18 项（Rust 侧待 026）。knip 登记 tailwindcss（CSS @import 不被追踪，任务风险节预案）。过程中发现并修复前端无关的存量抖动：byte_split 重放测试直驱 AdapterStream（不经 HTTP），消除约 200 连接 / 轮的临时端口消耗（Windows TIME_WAIT 累积导致 connect 间歇失败）。浅色主题精调、皮肤运行时与首窗防闪归 026/025。
