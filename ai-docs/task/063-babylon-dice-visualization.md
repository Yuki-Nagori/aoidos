# 063 — Babylon.js 骰子可视化移植

- 状态：in progress
- 依赖：023、026、052
- 优先级：P1
- 创建 / 更新：2026-10-22 / 2026-10-23

## 目标与背景

将 `aoidos-art` 的 3D 骰子展示移植为 Aoidos 的 Vue 组件，并以 Babylon.js 实现渲染。当前设计基线按提交 `c74e82f7ff4ad2eb0a73c878c8a9fad7e5a18210`（Babylon.js 9.32.0 骰子渲染与 WebGPU 优先）和 `8a8a374484afaa8939c344b0f0e4a6370a5c8e30`（峰谷价目、官方价目确认等补充）核对；前序 `399f67f4d7e50aefff0847c9bd77181311b022ff` 记录骰子工坊初版。当前需直接参考最新 `assets/dice/dice.js`、`assets/dice/check-tray.js`、`assets/dice/skins.json`、`screens/dice.html` 与对应设置 / 判定页面，不按旧 Three.js 初版推测行为。

设计原型的核心边界是骰值由引擎提供、组件只呈现指定结果。Aoidos 当前 Rust 引擎注册的是 `2d6-pbta-v1`；骰判草案中的 `d20-dc-v1`、`d8-tiers-v1` 尚未成为 Aoidos 引擎能力。规则 ID 统一采用 `<dice>-<mechanic>-v1`，骰式在规则名之前。两个草案 ID 只是目标命名，不代表规则已由 Aoidos 定稿或启用；后续规则设计任务须同步修订相应规范。3D 组件不得采样随机数来决定骰值、保留哪颗骰子或判定档位，也不得让动画结果覆盖 Rust 已提交的骰判事实。

基线包含 d20 / d8 / d6 几何、7 套皮肤、骰面 / 皮肤选择器、判定卡托盘、charge / roll / settle / result 状态、优势劣势的舍弃骰视觉、由结果事实触发的特殊效果，以及 reduced-motion / 静态截图模式。判定托盘和设置预览使用 3D 骰子；记录流保持简洁，只显示骰式、骰值与合计，不渲染 3D 骰子。设计原型的骰子工坊会用模拟数据演示规则，不能把工坊模拟器、示例概率或示例结果直接带入产品。

## 必读

- [依赖分层规范](../standards/layering.md)、[前端规范](../standards/frontend.md)、[UI 规范](../standards/ui.md)、[测试规范](../standards/testing.md)。
- [界面结构与交互](../architecture/interface/ui-shell.md)、[TS / Rust 边界](../architecture/core/ts-rust-boundary.md)、[回合状态机](../architecture/gameplay/turn-state-machine.md)、[记录引擎](../architecture/data/record-engine.md)、[主题架构](../architecture/interface/theming.md)、[通信契约](../architecture/core/ipc-contract.md)。
- 实施前核对 Babylon.js 官方文档与选定版本的 WebGL / WebGPU 支持、包体、许可证和 Tauri WebView 支持；将版本、日期与核对结论记入本 task，不引入未核验的运行时依赖。

## 范围与非目标

- 用 Vue / TypeScript 封装独立骰子展示，直接移植上述当前 Babylon.js 设计基线的骰面、材质、投掷动画、设置预览、托盘交互和特效；不搬入 Three.js 代码或 `three.min.js`。
- 将已有 Rust 事实映射为组件输入：骰式与骰面来自已确认的 `dice` 记录，结果与暴击标记来自对应的 `check` 记录。组件通过 props 接收不可变结果，不直接调用骰判 IPC；提交入口仍只传现有 session / round / plan 身份。
- 判定托盘可以作为现有手动 `engine_submit_check` 的可访问触发器；由 Vue 父级调用现有 API 并传入身份字段，结果动画只在收到权威骰判事实后播放。不得将指针速度、蓄力、拖动或随机轨迹编码成骰值。
- 检视并映射 7 套皮肤、自动跟随主题 / 剧本的现有规则和 d6 点数 / 数字样式；经用户明确要求完整移植后，将皮肤选择加入现有 UI 偏好结构，旧版偏好文件缺省为 `auto`，不另建存储或 IPC。
- 视觉骰子不产生游戏随机数、不分类结果、不提交记录、不改写恢复状态。手动 / 自动骰判模式继续由 Rust 冻结并执行；重开、恢复、重生成和 history fork 继续复用 / 读取原有骰值。
- 本 task 不新增骰判规则执行器、骰式解析能力、记录字段或 IPC。若将来要启用 d20 / d8 规则，必须先有独立引擎设计与实现任务，并通过已登记的结果类型接入展示；不以设计草案视为当前产品契约。
- 声音不在本 task 默认范围内；设计稿仅定义宿主回调事件且没有音频素材。若实现需要声音，必须先明确现有声音偏好 / 资源边界，组件不得自行播放、下载或捆绑音频。

## 前置条件与待决策

- 052 完成其当前 UI 视觉差异验收；本组件后续作为独立移植范围接入 `RecordFeed` / 判定卡，不把 052 中未完成的整页视觉验收默认为已通过。
- Babylon.js 优先使用 WebGPU 图形 API；WebGPU 不可用时回退到 WebGL。两种后端均不可用或初始化失败时，组件提供静态骰面回退；不得因渲染能力缺失而阻止对局或记录查看。
- Babylon.js 引擎与场景按需动态加载；可见组件才创建引擎，离开视口 / 卸载时释放场景、材质和引擎。需通过生产构建检查按需拆包，并验证实际同屏实例数量。
- 骰面资产、原型程序化几何和 7 套皮肤的完整视觉细节需在实现前逐项盘点并记录可复用 / 重做项；不得假定原型资源已具备独立再分发许可。
- 减少动态效果、后台 / 隐藏页面、设备失去 WebGPU / WebGL context、两种图形 API 被禁用及低性能设备时的具体静态呈现细节在实施前确认；至少始终显示正式数值和结果文本。
- 当前只有 `2d6-pbta-v1` 引擎能力；d8 / d20 几何仅是可视组件能力，不代表规则可用于对局。是否将来扩展规则、制作声音均不属于本 task 的隐含决策。

## 实施步骤

1. 复核设计仓库目标页与骰子资产，保存来源提交、屏幕 / 状态清单、授权和差异记录；核对 Aoidos 当前 `dice` / `check` 载荷以及回合、主题、偏好契约。
2. 评估并登记 Babylon.js 固定版本、许可证、包体、目标 WebView 的 WebGPU 优先 / WebGL 回退能力和按需加载方案；确定渲染场景生命周期、同屏实例预算及静态回退协议。
3. 在 `src-web/features/game/` 实现骰子展示组件与适配逻辑。遵循 Web 层依赖方向：feature 组件消费 `api/` 的只读类型和 game feature 数据，不从 `api/` 反向导入 Vue，也不将 Babylon 细节暴露到 app / Rust；只有明确跨 feature 复用时才放 `shared/`。
4. 由 `RecordFeed` / 判定卡把对应事实组装为只读 props。按骰面值确定最终朝向；动画结束只切换到已经给定的面。若轨迹需要可重放 key，只能从已提交记录身份派生，并与点数、骰面采样相互独立。keep / 暴击 / 失败效果只使用引擎返回的事实，不从表达式或渲染状态再推断规则。
5. 覆盖 reduced motion、页面隐藏、渲染器 / context 不可用、组件卸载与快速切换结果等生命周期，提供数值与文本静态回退，并保证失败不影响回合控制。
6. 完成行为 / 视觉 / 可访问性 / 性能验证和设计差异记录，按规范运行受影响检查；候选完成后进行一次独立评审，修复后复验并更新架构 / task 记录。

## 预计改动

- `src-web/features/game/components/`：Vue 骰子可视化及记录 / 判定卡接线；必要时新增 feature 内 composable / utils。
- `src-web/features/game/assets/`：确需保留且许可明确的静态纹理 / 材质资源；若无需外部位图则不新增资源。
- `package.json`、`bun.lock`：仅在评估通过后登记固定版本 Babylon.js 依赖。
- `tests/web/features/game/` 与 `tests/visual/`：载荷映射、生命周期、静态回退、reduced-motion 和布局验收。
- 按实际职责同步 `ai-docs/architecture/interface/ui-shell.md`、`ai-docs/architecture/core/ts-rust-boundary.md`、`ai-docs/architecture/core/repository-layout.md` 与本文；若跨层规则需要变化，先更新对应架构 / 分层文档。
- 不修改 Rust 引擎、Tauri IPC、骰判规则、`aoidos-art` 设计仓库或 Three.js 运行时依赖；发现必须修改时先拆分 / 更新设计任务与依赖关系。

## 验收标准

- [x] 设计基线、来源提交、目标屏幕 / 状态、资产复用与授权结论及视觉差异有可复核记录。
- [ ] 生产代码使用 Babylon.js；依赖版本、许可证、Tauri WebView 渲染支持和包体增量有证据，WebGPU 优先且 WebGL 是其不可用时的回退，Three.js 代码 / 依赖未进入产品。
- [ ] 视觉骰值只能来自对应 Rust `dice` 事实，最终结果和关键效果只能来自 Rust `check` 事实；组件和动画没有随机采样、判定分类或提交路径。未知骰式 / 结果安全显示原事实或静态兼容内容，不猜测档位。
- [ ] 判定托盘的鼠标、键盘与触控行为都只触发既有计划身份提交；动画方向、蓄力值与触控轨迹无法改变 Rust 采样。加载期间防止重复提交并遵守现有 busy / check.status 门禁。
- [ ] 相同冻结骰值无论动画正常结束、被缩减动态效果跳过、组件重新挂载或渲染失败，最终显示值均与事实相同；取消 / 恢复 / 重生成 / 历史回退不会生成新骰值。
- [ ] `prefers-reduced-motion` 或产品减少动态效果偏好启用时不播放持续翻滚、待机浮动、粒子或强光效果，直接呈现最终数字与结果；用户偏好在产品已有 API 中不存在时，不另建持久偏好。
- [ ] WebGPU 不可用时自动使用 WebGL；两种后端均不可用、canvas 创建失败或 context 丢失时退回可读静态骰面 / 数值 / 结果；游戏提交、恢复和历史记录仍可使用。
- [ ] 数字不只靠颜色、动画、朝向或视觉特效表达；提供可访问名称和包含骰式、每颗骰值、保留 / 舍弃及最终规则结果的静态文本。动画阶段不重复朗读逐帧状态，结果更新只播报一次且尊重现有 live region 策略。
- [ ] 对应 640×480 窄窗、常规窗和宽窗有视觉验收；不发生横向溢出、遮挡关键判定文本或阻塞记录阅读。截图 / 页面基线记录来源提交且测试不读取私有设计仓库。
- [ ] 行为验证覆盖事实到显示映射、不同确定骰值、复用 / 重放、被舍弃骰淡化但仍可读、暴击 / 大失败仅由 check 事实触发、reduced motion、渲染不可用与 context / 卸载清理；视觉测试覆盖尺寸、皮肤和静态回退。
- [ ] 确认首帧不等待 Babylon 初始化；场景 / observer / engine 在卸载、隐藏与失败路径释放；限定同时活动实例，滚动历史中的不可见骰子不持续渲染；记录首屏延迟、资源占用与渲染器错误的验收口径。
- [ ] 文档和依赖分层同步；受影响测试 / 类型 / lint / 格式 / 构建检查通过，独立评审完成并处理发现。未执行的真实三平台 WebView / 辅助技术验收按状态列明，不能用 Chromium 代替。

## 验证计划与结果

| 日期       | 环境 / 命令                                                                                                                                                                                                                                    | 预期                                                        | 实际结果                                                              |
| ---------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------- | --------------------------------------------------------------------- |
| 2026-10-22 | `bun run typecheck`                                                                                                                                                                                                                            | 前端与脚本类型检查通过                                      | 通过                                                                  |
| 2026-10-22 | `bun run test -- tests/web/features/game/utils/dice-visual.test.ts tests/web/features/game/components/RecordFeed.test.ts tests/web/features/game/composables/useUiPreferences.test.ts tests/web/app/App.test.ts tests/web/api/records.test.ts` | 覆盖骰式事实映射、皮肤选择、偏好持久化和现有记录视图        | 5 个测试文件、26 个测试通过                                           |
| 2026-10-23 | `bun run verify`                                                                                                                                                                                                                               | 全量类型、lint、格式、覆盖率、视觉、构建和 Rust 文档门禁    | 通过；Vitest 545 项，Rust workspace 覆盖率 94.72%                     |
| 2026-10-23 | Playwright WebGL2 设置页烟测                                                                                                                                                                                                                   | 检查视口内九个骰子预览均生成 Babylon 画面                   | 通过；无头 Chromium 未提供 WebGPU adapter，尚待桌面 WebView 实机确认  |
| 2026-10-23 | `bun run verify`（跟进复核）                                                                                                                                                                                                                   | 验证仓库全量检查                                            | 通过；Vitest 546 项，Rust workspace 覆盖率 94.59%                     |
| 2026-10-23 | `bun run test:native`（macOS arm64）                                                                                                                                                                                                           | 验证真实 Tauri WebView、IPC 与本机平台 fixture              | 通过；fixture 使用隔离临时存储与假数据，不调用外部 Provider           |
| 2026-10-23 | `bun run tauri -- build --debug --bundles app --no-sign`                                                                                                                                                                                       | 验证 macOS bundle 与 third-party license resources          | 通过；75.87 MiB debug bundle 内含 Babylon LICENSE、NOTICE、Draco 许可 |
| 2026-10-23 | `bun run typecheck` 与骰子渲染器 / 组件定向测试                                                                                                                                                                                                | 验证 context-loss 回退配置、共享 RAF 异常隔离及现有组件行为 | 通过；15 项 Vitest、类型检查、格式与受影响 lint 通过                  |
| 2026-10-23 | `bun run verify`                                                                                                                                                                                                                               | 对竞态修复执行完整仓库门禁                                  | 通过；549 项 Vitest、Rust 覆盖率、视觉回归、构建与文档门禁通过        |

## 风险与回退

WebGL 能力、驱动和可用内存因系统与设备而异；任何初始化失败都回退为静态结果，不阻塞记录 / 回合。Babylon.js 的增量包体或冷启动超过实施时登记的预算时，先缩小按需加载和实例范围；仍不满足时保留纯静态骰面方案，不转回 Three.js。视觉库不得改变记录真相，回退时原始 dice / check 文本仍可读。

## 决策与工作记录

- 2026-10-22：只读核对 Aoidos 架构与 task 目录，并检查 `aoidos-art` 的 `399f67f4d7e50aefff0847c9bd77181311b022ff` 基线。原型骰子依赖本地 Three.js r158；任务指定采用 Babylon.js。规则 ID 统一骰式在前：Aoidos 当前引擎登记 `2d6-pbta-v1`，设计仓库 `d20-dc-v1` / `d8-tiers-v1` 仍只是未启用的规则草案。task 只规划可视化消费已提交骰值，不扩大引擎规则。渲染后端采用 WebGPU 优先、WebGL 回退、静态结果兜底。
- 2026-10-22：采用 Babylon.js 9.31.0，按需加载并优先尝试 WebGPU，初始化失败回退 WebGL；7 套皮肤与自动剧本绑定规则已迁入前端 feature，设置文字在 `locales/`，组件样式在 `src-web/styles/components/dice.css`。
- 2026-10-22：应用户要求将骰子托盘替换等待判定时的输入区；托盘仍仅触发现有判定提交，手势参数不参与骰值。骰子皮肤加入既有 UI 偏好，并通过 serde 默认值兼容旧文件；未更改骰判协议和规则。
- 2026-10-23：修正 WebGPU 帧提交顺序。Babylon 的 `endFrame()` 会提交 GPU 命令，必须在从共享 GPU 画布执行 `drawImage()` 前完成；原型 `aoidos-art/assets/dice/dice.js` 也按此顺序实现。现将 begin/render/readiness/endFrame 收拢为可测的帧提交 helper，并添加调用顺序测试。真实 WebGPU adapter 在无头 Chromium 不可用，当前仅完成 WebGL2 九个预览实例的浏览器烟测，待桌面 WebView 实机验收。
- 2026-10-23：将运行时依赖升级到固定版本 Babylon.js 9.32.0；其 Apache-2.0 LICENSE、NOTICE 和 Draco 许可文件已随 Tauri bundle 提供。Chromium WebGL2 骰子烟测通过，macOS native fixture 与 debug `.app` 打包通过；打包验证确认许可资源位于 app bundle。native fixture 验证的是隔离的 Tauri WebView / IPC，不代表骰子 GPU adapter 已在原生 WebView 验收。
- 2026-10-23：将骰面 atlas / 静态绘制提取到 `game/utils/dice-canvas.ts`，共享引擎与 RAF / 取消逻辑放在 `game/utils/dice-renderer.ts`；Vue SFC 保留 Babylon 场景编排和交互状态，避免把高耦合场景局部机械拆分。视觉回归脚本同步新增的设置分区顺序。
- 2026-10-23：复核动态加载器时发现 `@babylonjs/core/Maths/math.vector` 被重复 `import()` 并重复传入 `Promise.all`；移除重复项，继续由同一个模块提供 `Vector3` 与 `Quaternion`。将锁定版本 9.32.0 随包提供的 Apache-2.0 LICENSE、NOTICE 和 Draco 许可文件复制到 `resources/third-party/babylonjs/` 并登记为 Tauri bundle resources，避免发布包只带渲染代码、不带第三方许可文本。`bun run typecheck`、骰子组件 / renderer 13 项定向测试通过；`bun run verify` 全量通过（546 个 Vitest、Rust 覆盖率 94.59%），macOS `bun run test:native` 的真实 WebView / 本地 fixture 烟测通过。该 native smoke 不覆盖骰子在原生 WebView 上的 WebGPU adapter 与辅助技术；Windows / Linux 实机检查按用户要求跳过。
- 2026-10-23：独立代码复核发现 Babylon 9.32.0 的 WebGL / WebGPU 引擎在 `doNotHandleContextLost: true` 时不会通知当前订阅器，导致静态回退和判定落定路径不可达。已启用 Babylon context/device-loss 处理，并使共享 RAF 将单实例 render 异常交由该实例静态回退，不再中断其他骰子；页面隐藏 / 移出视口时销毁场景、取消待就绪初始化并释放共享引擎引用；运行中监听 reduced-motion 偏好变化。定向测试新增引擎选项和共享 RAF 异常隔离覆盖，类型、格式、14 项骰子测试及受影响 lint 通过。独立复核指出的未知骰式 settling、权威 keep 映射、轨迹 seed、辅助技术和原生 WebView GPU 验收仍需按设计补证；063 继续保持 `in-progress`。
- 2026-10-23：第二轮独立复核进一步发现错误处理器在场景唤醒时被默认值覆盖，以及初始化竞态可能在隐藏后泄漏旧引擎、或让旧初始化改写新场景的共享画布。已让无处理器的唤醒保留实例错误处理器；Babylon 引擎创建不再写共享画布，场景只在确认自身代次仍有效后从当前引擎取得并发布画布；动态模块加载后、创建共享引擎前增加取消检查。新增 RAF 再唤醒回归测试；15 项定向测试、类型检查、格式、受影响 lint 与 `bun run verify`（549 项 Vitest、Rust 覆盖率、视觉回归、构建和文档门禁）通过。独立静态复核确认这三项问题已修复；原生 WebView GPU/context-loss 与辅助技术验收、Windows / Linux 实机检查仍待完成，063 保持 `in-progress`。

## 完成摘要

Babylon.js 骰子实现、许可资源与仓库内验证已完成；仍需补实际骰子在目标桌面 WebView 的 WebGPU / WebGL 与辅助技术验收、性能基线及独立评审。Windows / Linux 实机检查本轮按用户要求跳过，不能以 Chromium 或 macOS 结果代替。
