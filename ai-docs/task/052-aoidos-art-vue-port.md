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

外部设计来源：用户维护的 `aoidos-art` 仓库；2026-10-19 复核并采用提交 `1f19504ebc7390b7d9954b1c597fc3d98b6b7fde`，共 43 个 `screens/*.html`。该版本补充快捷键速查 / 设置设计和 055 / 056 对照页，并更新结局流程与在场选择稿。实施时复核其 `README.md`、`index.html`、`components.html`、`screens/` 和 `assets/app.css` 的最新版本及界面清单，并记录实际采用的提交。设计仓库不纳入 Aoidos 的构建或运行时依赖。

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

## 设计页到产品状态映射

按当前登记的 43 个 `screens/*.html` 快照逐项核对，静态预览文案和演示数据不能当作产品服务已经实现的证据。

| 设计页                                                                                                                                                                                   | Vue / 产品映射                                                   | 处理边界                                                                                                                                                                                                                                                                                                                                                          |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `welcome.html`、`welcome-key-missing.html`                                                                                                                                               | `WelcomeScreen.vue`、`useProduct`                                | 剧本、模型与密钥状态来自真实 API；开局需用户显式触发。                                                                                                                                                                                                                                                                                                            |
| `main-pinned.html`、`main-collapsed.html`、`panel-hover.html`、`panel-expanded.html`                                                                                                     | `GameStage.vue`、`usePanelState`、`RecordFeed.vue`               | 使用现有四态面板和有界记录投影。                                                                                                                                                                                                                                                                                                                                  |
| `min-window.html`                                                                                                                                                                        | `app.css`、`WelcomeScreen.vue`、`GameStage.vue`                  | 由正式窗口尺寸、响应式规则和窄窗布局承载；需外部 WebView 视觉验收。                                                                                                                                                                                                                                                                                               |
| `slash-menu.html`、`recipient-select.html`、`check-awaiting.html`、`check-rolling.html`                                                                                                  | `ActionComposer.vue`、`useGameWorkspace`                         | 斜杠菜单、046 已接入的在场角色选择、手动确认和滚动状态均由当前输入 / 阶段快照驱动。                                                                                                                                                                                                                                                                               |
| `rewind-confirm.html`、`regenerate-confirm.html`、`resume-required.html`                                                                                                                 | `GameWorkspace.vue`、`RecordFeed.vue`                            | 使用既有确认对话框、回退入口和显式继续操作。                                                                                                                                                                                                                                                                                                                      |
| `stall-hint.html`                                                                                                                                                                        | `useSceneHint`、`GameWorkspace.vue`                              | 仅显示引擎实际领取的场景提示；无提示时显示真实不可用状态。                                                                                                                                                                                                                                                                                                        |
| `settings.html`、`settings-model.html`、`settings-gameplay.html`、`billing.html`、`credits.html`、`#api`、`#gameplay`、`#billing`、`#price`、`#balance`、`#scenario`                     | `SetupPanel.vue`、`BillingPanel.vue`、locale / theme composables | 所有选项、费用、余额、价格和署名来自当前正式 API，不照搬静态预览结果。                                                                                                                                                                                                                                                                                            |
| `memory-settings.html`、`memory-list.html`、`memory-pending.html`、`#memory`、`#memory-list`、`#memory-pending`                                                                          | `MemorySettingsPanel.vue`、`MemoryQueryPanel.vue`                | 设置 / 列表 / 待确认页复用正式配置和有界查询状态；记忆列表页同时提供 033 已实现的显式素材收集 / 批次操作；详情按需读取，枚举本地化。设计稿搜索框暂不接入：当前列表 API 只支持有界快照分页，客户端搜索已加载页会造成不完整结果；完整检索语义及可复用搜索输入组件已由 [053](053-memory-query-search-design.md) 定稿，实现由 [055](055-memory-search-impl.md) 承接。 |
| `shortcuts.html`、`shortcuts-settings.html`、`#shortcuts`                                                                                                                                | 当前尚无全局快捷键注册表或改键持久化契约                         | 只沿用已实现的面板 / Escape / 输入交互；速查浮层、快捷键设置、改键和平台菜单接线由 [057](057-keyboard-shortcuts-impl.md) 承接，不以静态文案冒充已支持按键。                                                                                                                                                                                                       |
| `price-register.html`、`provider-balance.html`、`backfill-preview.html`                                                                                                                  | `BillingPanel.vue`、`useBilling`                                 | 复用账本已有的预览 / 确认历史补价流程。`backfill-preview.html` 的整页遮罩批量摘要与产品单请求补价契约不符；Vue 保留当前条目内联预览，不模拟批量操作或虚构汇总。                                                                                                                                                                                                   |
| `loading.html`                                                                                                                                                                           | `WelcomeScreen.vue` 与各功能加载态                               | 只在对应数据读取期间显示局部加载状态，不把一个快照伪装成独立路由。                                                                                                                                                                                                                                                                                                |
| `disconnected.html`                                                                                                                                                                      | `GameWorkspace.vue` 连接错误告警与显式重新连接                   | 使用现有 transport 连接错误驱动提示和重连；API 不提供最后同步时间、连续重试次数，因此不伪造这些值，也不加入后台轮询。                                                                                                                                                                                                                                             |
| `needs-recovery.html`                                                                                                                                                                    | `GameWorkspace.vue`、记录 / 阶段恢复 composables                 | 由快照中的恢复标记决定，重连后先恢复再允许继续。                                                                                                                                                                                                                                                                                                                  |
| `budget-blocked.html`、`price-missing.html`                                                                                                                                              | `App.vue` 错误映射、`GameWorkspace.vue` 即时告警、费用设置       | 显示 Rust 返回的脱敏错误；即时错误不追加为正式对局记录。                                                                                                                                                                                                                                                                                                          |
| `empty-records.html`、`no-scene.html`                                                                                                                                                    | `RecordFeed.vue`、`GameStage.vue`                                | 使用真实空记录与无活动场景状态。                                                                                                                                                                                                                                                                                                                                  |
| `ending-choice.html`、`ending-approach.html`、`ending-pending.html`、`ending.html`、`ending-narrating.html`、`ending-narration-failed.html`、`cycle-init.html`、`cycle-init-failed.html` | 当前没有对应的产品状态或 IPC 契约                                | 不呈现静态假结局；玩家以行动回应剧本，不提供固定结局选择。规则冻结、事实摘要、可选 AI 叙事与恢复由 [054](054-ending-flow-design.md) 定稿，实现由 [056](056-ending-flow-impl.md) 承接。                                                                                                                                                                            |

跨界面共有的搜索、连接、终局等设计操作，只有在存在权威数据和明确状态转换时才可接线；缺少协议的页面继续以真实已实现状态为界，不通过本 task 偷加后端语义。完整记忆检索与结局流程的设计已定稿，分别由 055、056 实现；052 不承担其后端协议或产品交互。

## 预计改动

`src-web/features/`（按 051 的最终结构）、共享组件与正式主题样式、`locales/`、对应 `tests/web/` 及必要的 UI 验收脚本；同步 `ai-docs/architecture/ui-shell.md`、`ai-docs/architecture/README.md`、`ai-docs/architecture/repository-layout.md`、`ai-docs/standards/frontend.md`、`ai-docs/standards/ui.md` 和 025 的外部验收引用（仅在实际影响其范围时）。不改 `aoidos-art` 仓库。

## 验收标准

- [x] 设计快照和屏幕清单有可复核版本；43 个目标设计页 / 组件状态均有对应 Vue 页面状态或有记录的范围排除，不遗留“样式像但状态缺失”的界面。
- [x] 设计稿交互映射到正确的现有 Rust / IPC / composable 行为；无假数据、虚构成功反馈或第二套持久状态。
- [ ] `dark`、`light`、`light-purple`、Mistbell 皮肤、zh-Hans / en，以及设计指定窗口尺寸 / 关键状态的表现通过检查；Vue 结构及几何布局与对应 HTML 可逐项核对，差异有具体原因和记录。
- [x] 欢迎页主题 / 语言 / Mistbell 皮肤组合、欢迎页三个窗口尺寸与真实会话下设置抽屉三个视口的几何 / 十个分区内容，以及密钥缺失 / 剧本读取失败状态由 Chromium 自动回归覆盖；基线只保留已核对的数值锚点，不访问或依赖私有 `aoidos-art` 仓库。
- [x] 所有界面使用 Aoidos 正式 token 和本地化资源；预览工具代码、静态 HTML、生成字典及未登记 `--x-*` 不进入生产构建。
- [x] 关键路径和失败 / 禁用态有行为测试；键盘、焦点、IME、屏幕阅读语义、窄窗口和 reduced-motion 沿用对应产品契约。
- [x] 架构 / 规范 / task 与实现同步，独立评审完成并处理发现；最终 `bun run verify` 通过。
- [ ] 需要外部 WebView / DPI / 辅助技术证据的项目按平台实际验证并分别记录；本轮按用户决定暂缓实机验收，当前状态如实记为未验证，不据此宣称跨平台表现通过。

## 验证计划与结果

- 2026-10-19，当前分支完整 `bun run verify` 通过；包括 47 个 Vitest 文件 / 425 项、Rust workspace 覆盖率 96.82%、布局回归、lint、构建和 rustdoc。独立 subagent review 发现并促成修复内容容器横向溢出断言，修复后重新运行完整 verify。
- 2026-10-19，`bun run test:visual`：Chromium 自动回归通过；960×640 覆盖 dark / light / light-purple、zh-Hans / en、Mistbell 皮肤开 / 关组合；640×480 和 1440×900 另测 dark / en / 皮肤开启，密钥缺失与剧本读取失败也在 960×640 下检查。比较只依赖 `tests/visual/welcome-layout.json` 中的数值布局锚点；不读取、克隆或请求私有设计仓库，也不需要仓库凭据。
- 2026-10-19，设置抽屉三种视口改用正式开局入口和固定 IPC fixture 建立对局；布局检查覆盖费用汇总、价格表单、余额快照、应用 / 剧本记忆参数、记忆条目和待确认问题。该测试发现 640px 价格登记表单横向溢出 11px；计价表单在 720px 窄抽屉断点改为单列，修复后 960 / 700 / 640px 的十个实际分区均通过无溢出检查。fixture 不连接供应商、不访问用户数据或私有设计仓库。
- 设计快照清单核对：按已登记基线完成 43 个页面状态的 Vue / API 映射；欢迎页已加入 Chromium 几何布局和关键状态回归，其他页面逐项视觉对照尚无可复核结果，平台验收按用户决定暂缓。
- 2026-10-19，Vitest（App + useProduct + BillingPanel）：欢迎 / 抽屉 / 密钥门禁和费用面板关键行为通过，20 项；另通过 `bun run typecheck`、`bun run lint`、`bun run format:check`、`bun run lint:layers`、`bun run build:web`。
- 2026-10-19，Vitest 全量覆盖：46 个文件、411 项通过；语句 / 分支 / 函数 / 行覆盖率为 99.63% / 99.19% / 100% / 99.73%。剩余 5 行位于既有 `useSceneHint` 与 `useScenePresence`，不在本批改动中。
- 2026-10-19，`bun run test:affected`：通过，受影响 Rust workspace 测试与 Vitest 46 个文件、414 项全部通过。
- 2026-10-19，`bun run format:affected`、`bun run lint:affected`、`bun run typecheck`：通过；lint 同时完成 i18n 检查、Vite 生产构建和 native fixture 构建。
- 2026-10-19，对局工作台继续按设计稿改为全窗顶栏、居中 16:9 舞台和右侧悬浮面板；Mistbell 使用剧本专属舞台图，舞台外 HUD 只展示真实阶段、场景与提示。费用摘要从当前局账本按原币汇总，不做跨币换算；为此补充金额汇总行为测试。
- 2026-10-19，文档层固定视口并关闭整页滚动 / overscroll，避免 WKWebView 橡皮筋回弹；`tests/web/appcss.test.ts` 增加 CSS 契约检查。顶栏账本摘要随已提交记录更新，账本并发响应按请求代次丢弃旧结果，避免旧快照覆盖新金额。`bun run test:affected` 通过，受影响 Rust workspace 测试及 Vitest 47 个文件、421 项通过；`bun run format:affected`、`bun run lint:affected`、`bun run typecheck` 均通过。lint 包含 i18n、Vite 生产构建与 native fixture 构建。
- 2026-10-19，加入与设计稿对应的折叠式“工作过程”区域，展开呈现权威快照中的阶段、场景路径、判定状态、操作终态和失败诊断码；不显示无法绑定到当前操作的 token 用量。`ui-shell.md` 已记录当前公开数据边界，Vitest 覆盖默认折叠、场景与判定状态。
- 2026-10-19，设置抽屉的外观与语言区改用剧本皮肤开关和原生语言单选分段；主题选项仍来自 Rust 动态目录，不硬编码选项。新增状态 / 持久化行为断言；针对性 Vitest 3 项、类型检查与格式检查通过。
- 2026-10-19，模型 / API 设置改为并排 Provider 与模型选择，并以状态卡显示真实凭据状态，缺失 / 未知时使用警示样式；密钥仍只经 Rust 原生窗口设置。设计稿中的协议路由选择暂不呈现：当前 profile 只保存 `completion` / `chat` 形态，Provider 适配器决定协议，没有可持久化的用户路由选择能力。App 行为测试覆盖已设置与缺失密钥，针对性 Vitest、类型、lint、构建与格式检查通过。
- 2026-10-19，游戏设置新增手动 / 自动掷骰的单选分段，复用既有 `UiPreferences` 持久化；写入失败时由 composable 回滚到上次确认值。App 行为测试验证设置抽屉通过 IPC 保存偏好；针对性 Vitest 3 项、类型、lint、构建与格式检查通过。
- 2026-10-19，费用面板新增按 CNY / USD 分开的本局用量与上限卡片，汇总复用顶栏账本函数，不换算币种；限制输入改为两列响应式表单，并统一费用面板按钮状态。BillingPanel 组件测试覆盖模型用量与已登记原币上限的显示；针对性 Vitest 2 项、类型检查、`bun run lint:affected`、`bun run format:affected` 通过。
- 2026-10-19，剧本署名页改为设计稿中的信息卡结构，展示所选剧本的真实本地化名称、ID 与署名正文；不虚构当前 API 未提供的作者、许可证或资源清单。App 行为测试验证名称和署名来自 `engine_list_scripts`；针对性 Vitest 3 项、类型检查、lint 与格式检查通过。
- 2026-10-19，价格登记页增加当前 Provider 展示名、模型 ID、币种和价格可用状态上下文；输入项按登记信息、基础价格、峰时价格分组并适配窄窗口。Provider 名称复用产品目录，价格版本仍不可变且沿用原有校验。BillingPanel 测试覆盖当前模型身份及价格状态；相关 Vitest 5 项、类型检查、`bun run lint:affected`（含 i18n / Vite / native fixture）和格式检查通过。
- 2026-10-19，设置抽屉标题与关闭操作改为跟随当前内容区，导航栏仅承担页签切换；各页面共用 `SettingsSectionHeading`，删除不再使用的侧栏关闭样式。App 行为测试覆盖 API 页关闭抽屉；Vitest 3 项、类型检查、lint 与格式检查通过。
- 2026-10-19，设置导航为外观、API、游戏、计费、记忆及其子页签补齐设计稿对应的内嵌 SVG 图标；移除已有选中态圆点伪元素，避免图标和额外标记重复表达状态。图标为装饰内容，页签名称继续作为唯一可访问名称；App 行为测试验证所有十个页签均有图标，Vitest 3 项、类型检查、lint 与格式检查通过。
- 2026-10-19，服务商余额页按设计稿整理为真实快照卡，展示当前供应商、可用状态、查询时间、过期标志和原币账号；不把余额与账单费用混比，非 DeepSeek 仍明确提示暂不支持。测试覆盖缓存状态、金额和手动刷新；针对性 Vitest 3 项、类型检查、`bun run lint:affected`（含 i18n / Vite / native fixture）与格式检查通过。
- 2026-10-19，记忆设置移除抽屉标题下重复的总折叠层，直接呈现简要说明及应用默认值 / 剧本覆盖两个独立分组；记忆列表和待确认页移除没有可见标题的原生折叠外壳，详情改为在选中条目内展开并可收起。条目状态、主体类型、记忆维度、态度和变更类型显示本地化标签，数据仍由现有 Rust API 驱动，不批量发起详情请求；组件测试验证结构、标签、真实详情、来源 / 证据及展开状态。加入标签映射后，两个记忆组件的 20 项测试、`bun run typecheck`、`bun run lint:affected`（含 i18n / Vite / native fixture）和 `bun run format:affected` 通过。
- 2026-10-19，对局命令的 IPC / Rust 错误此前只传给设置抽屉显示，玩家留在对局时看不到预算或配置失败原因。`GameWorkspace` 现在在记录流上方显示脱敏的即时告警；有当前 transport 连接错误时提供显式重连，不把错误追加为正式记录，也不补造设计稿里的同步时间 / 重试次数。GameWorkspace、useGameWorkspace 与 App 针对性 Vitest 5 项通过。
- 2026-10-19，按 43 个设计快照补齐 Vue / 产品状态映射，标出真实 API 边界；记忆搜索与结局 / 落幕协议由 053、054 定稿，落地范围分别登记在 055、056，避免把新服务塞进 031、032 或 052。
- 2026-10-19，用户决定暂缓 052 的外部 WebView / DPI / 键盘 / 辅助技术人工验收；相关平台状态继续标记为未验证，不作为本轮自动化检查已通过的证据。
- 2026-10-19，独立 review 发现记忆批次操作仅在组件的未使用 `all` 视图可见；已将显式素材收集、批次准备 / 发送 / 恢复 / 完成入口放回应用实际使用的记忆列表视图，并通过行为测试确认列表页可见、待确认页不误显。
- 2026-10-19，独立 review 还发现设置焦点循环会命中隐藏页、IME Escape 会关闭抽屉、对局内设置关闭不恢复触发焦点，以及密钥状态旧读取可能覆盖刚保存结果。已修复焦点可见性 / IME 优先级 / 触发元素恢复和密钥请求代次，并补充相应行为测试；后续复核未发现遗留代码问题。
- 2026-10-19，`bun run verify` 全项通过；Vitest 47 个文件、425 项通过。覆盖率：语句 99.63%、分支 99.15%、函数 100%、行 99.73%；Rust workspace 测试、格式、lint、类型、构建、依赖与文档检查通过。
- 2026-10-19，复核关键交互行为证据：`ActionComposer.test.ts` 覆盖输入 / 命令建议、禁用提交与无障碍角色，`GameStage.test.ts` 覆盖快捷键、Escape、焦点返回和折叠态 inert，`useGameInput.test.ts` 覆盖 IME 输入，`App.test.ts` 覆盖设置抽屉焦点循环 / 恢复与 tab 语义，`GameWorkspace.test.ts` 覆盖连接失败提示和显式重连；`appcss.test.ts` 验证 reduced-motion，Chromium 视觉回归验证 640×480 欢迎页 / 设置抽屉无横向溢出。以上测试支撑本标准，不替代真实屏幕阅读器或原生 IME 验收。
- WebView / DPI / 键盘与辅助技术检查：按用户决定暂缓；未执行的平台保持未验证状态。自动化覆盖设置抽屉隐藏页焦点、IME Escape 和触发元素焦点恢复，不替代后续实机检查。
- 2026-10-19，完成实现与自动化验证记录整理。欢迎页的主题 / 语言 / 皮肤组合、三种窗口尺寸和两类失败状态已有 Chromium 几何回归；设置抽屉在 960、700、640 像素窗口下验证几何、十个分区对应关系与溢出，并修复 720 像素断点处英文子页签溢出。052 仍等待对其余设计页面和状态逐项对照 `aoidos-art` 基线，记录需要保留的差异。当前无完整页面截图或逐项对照记录，因此不宣称全部视觉验收通过；WebView、DPI、键盘及辅助技术平台验收仍按此前决定暂缓。

## 风险与回退

设计仓库可能变化，产品与稿件出现分歧时以本 task 固定的设计快照和 Aoidos 已批准的产品 / IPC 契约为界；需更新稿件基线时先记录差异。视觉调整不得破坏已交付的回合状态、费用金额、记忆授权、正文恢复和主题持久化。分阶段保留可回退的 Vue 组件与样式提交，避免单次替换导致 UI 不可用。

## 决策与工作记录

- 2026-10-19：将架构整理与视觉移植分为 051、052。052 依赖 051，确保界面迁移基于稳定职责边界；设计仓库仅作为视觉来源，产品能力仍以 Aoidos 后端契约为准。
- 2026-10-19：051 已合并；初始核对设计基线 `f068466692a2256a4814d6f8bbf06dabcda83abe`；随后复核 `aoidos-art` 更新到 `1f19504ebc7390b7d9954b1c597fc3d98b6b7fde`，将目标清单扩为 43 页，并把新增结局 / 快捷键状态分别交由 056 与新登记的快捷键实现任务承接。052 进入实施，先完成 UI / API / locale / token 映射，再分批移植。
- 2026-10-19：第一轮欢迎页错误地把设计稿的左右双栏重做成单张居中卡片，设置分组也有合并，未达到本 task 的视觉移植目标。用户明确要求用 Vue 重实现设计 HTML，而不是直接把静态 HTML 放进产品；后续按 HTML 的 DOM 层级和 CSS 几何复现，再替换真实数据与 Aoidos 操作。当前欢迎页已恢复双栏主从布局、原稿标志、介绍 / 四项特点、剧本和模型摘要、密钥状态以及“重开 / 新建周目”两个入口；顶栏与设置抽屉开始按原稿调整。样例剧情与账单数字不作为产品数据。完整页面映射、逐视口视觉对照、未接通页面状态和平台验收仍待完成。

## 完成摘要

Vue 移植、43 个设计状态映射、行为验证、文档同步与 `bun run verify` 已完成。052 当前仍待人工逐视口视觉对照；外部 WebView、DPI、键盘及辅助技术验收另按用户决定暂缓。取得可复核的视觉对照结果并处理差异前，任务保持 `in-progress`。
