# 052 — 将 aoidos-art 界面移植到 Vue

- 状态：in-progress
- 依赖：051、025、026、031、034、035、046
- 优先级：P1
- 创建 / 更新：2026-10-19 / 2026-10-19

## 目标与背景

`aoidos-art` 已完成静态 HTML 界面、组件状态、布局和多主题视觉稿。将它作为 Web 产品的界面设计基准，移植到 Aoidos 的 Vue 组件中，使欢迎 / 对局 / 设置 / 费用 / 记忆等界面统一采用设计稿，同时继续使用 Aoidos 的 IPC、持久状态、国际化、主题和恢复逻辑。

设计仓库是只读的视觉来源，不是产品运行时代码或第二份业务真源。设计稿表达的页面状态并不代表相应功能已接入；实现时逐项核对当前 Rust / IPC 能力，不能用静态假数据伪装后端结果。

## 必读

[前端规范](../standards/frontend.md) · [UI 风格](../standards/ui.md) · [UI 架构](../architecture/ui-shell.md) · [国际化架构](../architecture/i18n.md) · [主题架构](../architecture/theming.md) · [计费架构](../architecture/billing.md) · [记忆架构](../architecture/memory.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md) · [提交规范](../standards/commits.md)。

外部设计来源：用户维护的 `aoidos-art` 仓库；本任务登记时读取提交 `f068466692a2256a4814d6f8bbf06dabcda83abe`（2026-10-19）。实施时复核其 `README.md`、`index.html`、`components.html`、`screens/` 和 `assets/app.css` 的最新版本及界面清单，并记录实际采用的提交。设计仓库不纳入 Aoidos 的构建或运行时依赖。

## 范围与非目标

- 按 `aoidos-art` 当前完整页面清单移植欢迎页、对局舞台 / 面板、输入 / 判定 / 恢复状态、统一设置抽屉，以及费用和记忆相关视图；静态设计状态是视觉验收用例，不强制变为独立产品路由。
- 使用 Vue SFC 和当前组件结构实现，不通过 iframe、静态 HTML 注入或保留第二套 DOM 模板来呈现产品界面。
- 将 `aoidos-art/screens/*.html` 作为 Vue 结构基线：保留其主要 DOM 层级、元素顺序、布局比例、间距、组件状态与响应式断点；只把预览占位、虚构演示内容和预览交互替换为真实 API 状态及已实现操作。不能因为采用 Vue 就另行设计一套“相似”界面；视觉差异须对应明确的产品能力、可访问性或平台约束并记录。
- 只接入 Aoidos 已存在的 Tauri API / composable 能力；未实现服务的设计状态保持不可触发，并明确标注或等待对应产品 task，不造成功提示、不硬编码假账单 / 记忆 / 玩家数据。
- 使用 Aoidos `locales/`、剧本目录的本地化名称、主题 / 皮肤运行时和正式 CSS token。设计仓库的评审工具栏、预览 JS / localStorage、生成 i18n 字典及 `--x-*` 派生 token 不进入产品。
- 保留 025 的焦点、IME、键盘、恢复、有界正文和窗口规则，以及 reduced-motion 和已有三平台支持边界；不在本 task 扩展后端业务协议、世界规则或费用语义。

## 前置条件与待决策

051 的结构拆分完成后再大规模移植。025 与 035 的实现状态可供消费，但其尚未完成的外部实机验收不由 052 代为宣称完成。实施前核验设计稿中的每个交互对应的现存 API、命令与持久状态，列出缺失能力及所属后续 task；无现成接口的控件不得接入临时存储或 mock 作为产品行为。

设计稿可能领先后端能力；若某视觉页要求未排期的独立功能，先将其记录到任务索引并说明依赖，不把该功能塞进本 task。仅完成现有能力下的布局、文案、状态呈现和交互接线。

## 实施步骤

1. 获取并记录 `aoidos-art` 设计快照，整理所有屏幕 / 组件状态、主题、语言及视口验收矩阵。
2. 建立设计状态到现有 Aoidos 页面状态、API、locale key 和 theme token 的映射；标出不支持的操作及其处理方式。
3. 按页面 / 功能分批移植 Vue 视图、响应式样式和可访问交互；复用 Aoidos 状态机与 API，不复制产品逻辑。
4. 为关键页面状态加入 Vitest 行为验证，更新截图 / 手工视觉检查证据与必要的 WebView 验收记录。
5. 更新 UI 架构、前端 / UI 规范、目录图、任务索引及本 task；独立评审后处理发现并复验。

## 预计改动

`src-web/features/`（按 051 的最终结构）、共享组件与正式主题样式、`locales/`、对应 `tests/web/` 及必要的 UI 验收脚本；同步 `ai-docs/architecture/ui-shell.md`、`ai-docs/architecture/README.md`、`ai-docs/architecture/repository-layout.md`、`ai-docs/standards/frontend.md`、`ai-docs/standards/ui.md` 和 025 的外部验收引用（仅在实际影响其范围时）。不改 `aoidos-art` 仓库。

## 验收标准

- [ ] 设计快照和屏幕清单有可复核版本；所有目标设计页 / 组件状态均有对应 Vue 页面状态或有记录的范围排除，不遗留“样式像但状态缺失”的界面。
- [ ] 设计稿交互映射到正确的现有 Rust / IPC / composable 行为；无假数据、虚构成功反馈或第二套持久状态。
- [ ] `dark`、`light`、`light-purple`、Mistbell 皮肤、zh-Hans / en，以及设计指定窗口尺寸 / 关键状态的表现通过检查；Vue 结构及几何布局与对应 HTML 可逐项核对，差异有具体原因和记录。
- [ ] 所有界面使用 Aoidos 正式 token 和本地化资源；预览工具代码、静态 HTML、生成字典及未登记 `--x-*` 不进入生产构建。
- [ ] 关键路径和失败 / 禁用态有行为测试；键盘、焦点、IME、屏幕阅读语义、窄窗口和 reduced-motion 沿用对应产品契约。
- [ ] 架构 / 规范 / task 与实现同步，独立评审完成并处理发现；最终 `bun run verify` 通过。
- [ ] 需要外部 WebView / DPI / 辅助技术证据的项目按平台实际验证并分别记录，不以静态截图或 Vitest 替代。

## 验证计划与结果

- 设计快照清单核对与逐页状态矩阵：进行中；基线提交已登记，逐项映射尚未完成。
- 2026-10-19，Vitest（App + useProduct + BillingPanel）：欢迎 / 抽屉 / 密钥门禁和费用面板关键行为通过，20 项；另通过 `bun run typecheck`、`bun run lint`、`bun run format:check`、`bun run lint:layers`、`bun run build:web`。
- 2026-10-19，Vitest 全量覆盖：46 个文件、411 项通过；语句 / 分支 / 函数 / 行覆盖率为 99.63% / 99.19% / 100% / 99.73%。剩余 5 行位于既有 `useSceneHint` 与 `useScenePresence`，不在本批改动中。
- 2026-10-19，`bun run test:affected`：通过，受影响 Rust workspace 测试与 Vitest 46 个文件、414 项全部通过。
- 2026-10-19，`bun run format:affected`、`bun run lint:affected`、`bun run typecheck`：通过；lint 同时完成 i18n 检查、Vite 生产构建和 native fixture 构建。
- 2026-10-19，对局工作台继续按设计稿改为全窗顶栏、居中 16:9 舞台和右侧悬浮面板；Mistbell 使用剧本专属舞台图，舞台外 HUD 只展示真实阶段、场景与提示。费用摘要从当前局账本按原币汇总，不做跨币换算；为此补充金额汇总行为测试。
- 2026-10-19，文档层固定视口并关闭整页滚动 / overscroll，避免 WKWebView 橡皮筋回弹；`tests/web/appcss.test.ts` 增加 CSS 契约检查。顶栏账本摘要随已提交记录更新，账本并发响应按请求代次丢弃旧结果，避免旧快照覆盖新金额。`bun run test:affected` 通过，受影响 Rust workspace 测试及 Vitest 47 个文件、421 项通过；`bun run format:affected`、`bun run lint:affected`、`bun run typecheck` 均通过。lint 包含 i18n、Vite 生产构建与 native fixture 构建。
- 2026-10-19，加入与设计稿对应的折叠式“工作过程”区域，展开呈现权威快照中的阶段、场景路径、判定状态、操作终态和失败诊断码；不显示无法绑定到当前操作的 token 用量。`ui-shell.md` 已记录当前公开数据边界，Vitest 覆盖默认折叠、场景与判定状态。
- 2026-10-19，设置抽屉的外观与语言区改用剧本皮肤开关和原生语言单选分段；主题选项仍来自 Rust 动态目录，不硬编码选项。新增状态 / 持久化行为断言；针对性 Vitest 3 项、类型检查与格式检查通过。
- `bun run verify`：未执行。
- WebView / DPI / 键盘与辅助技术检查：未执行，按适用平台留证。

## 风险与回退

设计仓库可能变化，产品与稿件出现分歧时以本 task 固定的设计快照和 Aoidos 已批准的产品 / IPC 契约为界；需更新稿件基线时先记录差异。视觉调整不得破坏已交付的回合状态、费用金额、记忆授权、正文恢复和主题持久化。分阶段保留可回退的 Vue 组件与样式提交，避免单次替换导致 UI 不可用。

## 决策与工作记录

- 2026-10-19：将架构整理与视觉移植分为 051、052。052 依赖 051，确保界面迁移基于稳定职责边界；设计仓库仅作为视觉来源，产品能力仍以 Aoidos 后端契约为准。
- 2026-10-19：051 已合并；核对 `aoidos-art` 当前仍为登记的基线提交 `f068466692a2256a4814d6f8bbf06dabcda83abe`，开始按该快照盘点屏幕状态与现有能力。052 进入实施，先完成 UI / API / locale / token 映射，再分批移植。
- 2026-10-19：第一轮欢迎页错误地把设计稿的左右双栏重做成单张居中卡片，设置分组也有合并，未达到本 task 的视觉移植目标。用户明确要求用 Vue 重实现设计 HTML，而不是直接把静态 HTML 放进产品；后续按 HTML 的 DOM 层级和 CSS 几何复现，再替换真实数据与 Aoidos 操作。当前欢迎页已恢复双栏主从布局、原稿标志、介绍 / 四项特点、剧本和模型摘要、密钥状态以及“重开 / 新建周目”两个入口；顶栏与设置抽屉开始按原稿调整。样例剧情与账单数字不作为产品数据。完整页面映射、逐视口视觉对照、未接通页面状态和平台验收仍待完成。

## 完成摘要

未开始。完成后填写设计快照、移植范围、产品能力缺口、视觉 / 行为验证及平台限制，再与任务索引一起更新状态。
