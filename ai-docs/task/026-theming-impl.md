# 026 — 实现：主题偏好与剧本皮肤

- 状态：in-progress
- 依赖：004、009、013
- 优先级：P1
- 创建 / 更新：2026-10-06 / 2026-10-10

## 目标与背景

009 冻结主题 / 皮肤设计并于 2026-10-10 明确所有主题使用平级 ThemeId；004 提供默认语义 token / Tailwind 映射。本任务交付 Rust 校验、应用偏好、受控首窗 bootstrap 与前端样式生命周期，025 后续消费完整设置界面，不重复底层实现。

## 必读

[主题架构](../architecture/theming.md) · [UI 风格](../standards/ui.md) · [界面结构](../architecture/ui-shell.md) · [通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md) · [存储基建](../architecture/storage.md) · [前端规范](../standards/frontend.md) · [Rust 规范](../standards/rust.md) · [注释规范](../standards/comments.md) · [测试规范](../standards/testing.md) · [提交规范](../standards/commits.md)。依赖官方资料及日期见主题架构；实际版本在接入时锁定。

## 范围与非目标

- 新增不依赖 tauri 的 aoidos-theme crate，目录元数据、CSS 子集 / 引用图校验、预算与规范化常量输出。
- store 中的独立 ThemePreference 项及迁移，thin theme 命令和同型 TS 类型；不完整替换 UiPreferences。
- 已登记受控剧本目录的有界 theme.css 读取，缺失 / 非法 / IO 回退，内置与第三方同一验证。
- 首窗初始化顺序、可信 head bootstrap、受控 main 构建；与 025 窗口状态插件共用入口。
- 前端主题确认、加载代次、Constructable Stylesheet 清退、可关闭当前皮肤的最小入口；生产 / dev CSP 与三平台验证。

非目标：完整游戏 UI、主题制作工具、约百个占位 token、通用剧本导入、包内 / 外部资产皮肤、网络字体、DTCG / Style Dictionary、VueUse 主题库或自动付费调用。T / R / E 扩展须另行登记，不借通用 CSS 放行。

## 前置条件与待决策

004、009、013 完成；004 已提供目录 / CSS / alias 的互校夹具，026 落地 Rust 目录后由其导出同源夹具补齐三方验证，不反向阻塞 004。现有 setup 只注入路径，不能当作 store / 偏好已初始化；实施时统一实例锁与 store 服务：022 已落地则复用，尚无服务则建立共享基础供 022 扩展，不另开写者或争用迁移编号；运行期迁移事件仍归 022。

注册目录来自可信映射；本任务以最小登记 / 只读夹具验证，不依赖尚未实现的任意 zip 导入。cssparser 是首选 tokenizer 候选，lightningcss / CSS.registerProperty 不必需；落地时核验锁定版本、三平台 API、真实 CSP 与窗口重建 / 状态恢复。

## 实施步骤

1. 将 `assets/themes/<id>.css` 作为内置主题默认值真源；建立 Rust 动态目录并自动嵌入主题文件，生成并提交 `src-web/styles/generated/theme-defaults.css`，互校 token / aliases 与生成物新鲜度；crate 加入根 workspace。
2. 实现有界解析和引用解析，所有名称 / 值经 tokenizer 解码与子集检查；结构错整份失败，单条错误有界告警，输出不含原 CSS / 路径，仅保留经类型校验且由基础主题提供的合法 `var(--token)` 引用。
3. 通过 store 增补主题配置及受控读取接口；注册主题目录、偏好与皮肤命令并同步 TS 薄调用 / 类型、错误映射与配置迁移测试。
4. 改为单一受控 main 创建，读取确认偏好再注入 bootstrap，可信 head 在 CSS / Vue 前应用；复用原窗口尺寸 / 标签 / capability，不新增第二个主窗口。
5. 在 composable 实现纯显示转换与异步生命周期：串行确认主题、保留最后成功偏好、隔离主题与皮肤诊断、按剧本 ID 单请求在飞并合并最新目标加载通用皮肤、代次 / 卸载清理和最小设置入口；不在前端补 CSS 解析。
6. 接入生产 / dev CSP，完成构造样式表、首帧和三平台烟测，清理临时接线并同步 004 / 025、架构现状和模块目录。

## 预计改动

待创建 `src-rust/aoidos-theme/` 及 Rust 目录导出 / 校验测试；修改 workspace / Cargo.lock、aoidos-store 配置与受控文件读取、src-tauri 命令 / 错误映射 / main 装配和窗口 / CSP 配置。前端修改 index.html、src-web/app.css、主题 API / utils / composable 与最小入口；测试镜像目录，并更新 004 过渡目录夹具。确需脚本通过根 package.json 注册，不用运行时联网生成目录。

## 验收标准

- [ ] Rust 目录、生成默认 CSS 与 Tailwind inline aliases 互校；默认色板不可误用，未登记 / 状态色 / Z 不可覆盖。
- [ ] `dark`、`light`、`light-purple` 与模板登记主题作为平级 ID；缺套、空文件 / 全部单条丢弃按所选主题默认值正确回退；结构 / 硬预算错无部分输出；警告有界且无原文 / 路径。
- [ ] 变量循环 / 依赖 / 类型与非法重复有黄金例，规范输出是常量；转义资源函数、坏字符串、选择器 / at 规则、深度与计数攻击均拒绝。
- [ ] theme.css 读取有界且遵守稳定受控目录前提；未登记 id、缺文件、符号链接、IO 和损坏路径分别有明确结果，不伪装成缺皮肤。
- [ ] 主题写入独立且串行，快速写入中的较早成功成为新的确认值；较晚失败恢复该确认值，主题诊断不被皮肤响应清除；不确定提交主动读取核验。
- [ ] `colorScheme` 只作为每个主题的绘制提示；非法持久 ID 报损坏，合法但当前不可用的 ID 保留已确认偏好并仅临时降级。
- [ ] main 唯一、标签 / 事件 / capability / 几何配置保留，025 可复用构建入口，不重复持实例锁 / store 写者。
- [ ] 快速切换 script / theme、旧响应、缺套、关闭皮肤和卸载正确清退；皮肤最多一个加载请求在飞且只保留最新目标，有界主题缓存，没有轮询或任意 CSS 注入。
- [ ] Windows / macOS / Linux 的 CSSOM / CSP、首帧、IPC / favicon 与 dev HMR 留证，失败平台明确降级，不用 happy-dom 代替实测。
- [ ] 代码、注释、类型、文档与 task 同步；最终状态 `bun run verify` 十三项通过。

## 验证计划与结果

Rust 解析 / 图解析 / IO / 配置失败注入；Web 纯逻辑与订阅 / 样式表生命周期测试；目录 / aliases 及编译后 CSS 互校。真实窗口录制首帧内置及模板登记主题与错误降级，三平台验证实际 CSP、窗口重建 / 几何恢复和非法皮肤无资源请求。构造样式表不支持时验证默认主题可用与明确降级；所有自动命令走根 bun scripts。

| 日期       | 环境 / 命令                                                                                 | 预期                                                                                | 实际结果                                                                                 |
| ---------- | ------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| 2026-10-10 | `cargo test -p aoidos-theme`                                                                | Rust 目录、生成 CSS 与 Tailwind aliases 同源                                        | 37 项通过；新增逐主题值和 aliases 交叉校验                                               |
| 2026-10-10 | `bun run test:native`（macOS）                                                              | Vue 挂载前主题 bootstrap / 产品 CSS、三内置主题 CSS、favicon、生产 CSP、IPC / CSSOM | 通过；原生 WebView 加载实际生产 CSS / icon，首模块同步采样，三内置主题选择器均有计算样式 |
| 2026-10-10 | `cargo test -p aoidos-native-tests --features desktop-session --test hmr-platform`（macOS） | 精确 dev CSP 下 CSS HMR 生效且文档不重载                                            | 通过；Vite 更新 `--hmr-marker`，文档身份保持不变                                         |
| —          | 三平台 CI 原生验收与 `bun run verify`                                                       | Windows / macOS / Linux 及十三项本地门禁通过                                        | 待 PR CI；本地完整 verify 待最终复跑                                                     |

## 风险与回退

CSS 解析容错、CSP / Webview 差异和早期窗口创建可能导致皮肤越界或闪色。保持 Rust 子集封闭，平台不可用回默认，不放宽 CSP。回退插件 / 窗口装配时同步撤销接口与失效说明，保留确认偏好和剧本文件；不能覆盖旧配置或删除存档。

## 决策与工作记录

- 2026-10-10：开始实施；004 / 009 / 013 已完成。先复核共享 store、首窗创建与默认 token 真源，保持 theme 偏好独立于现有 UiPreferences。
- 2026-10-10：根据用户意见简化主题模型：内置 dark / light / light-purple 与模板显式登记主题均是平级 ThemeId；偏好、bootstrap、CSS 选择器、皮肤覆盖和 TS / IPC 映射不得再区分模式与预设。
- 2026-10-10：独立验收发现 Rust 目录与 Web / Tailwind 默认 CSS 缺少直接交叉校验，原生夹具也未加载真实产品样式 / favicon，dev CSP 缺 Vite 脚本来源并阻止内联 HMR 样式。补目录值 / aliases 互校、复用 dist CSS 与图标的 Vue 挂载前首模块 / CSP 烟测，以及从生产 `devCsp` 读取策略的真实 WebView HMR 测试；原生夹具同步验证三个内置主题的生产 CSS，CSP 探针等待精确违规事件，HMR 专项测试每次从源 fixture 恢复初值。本机 macOS 通过，等待三平台 CI。
- 2026-10-10：内置主题 CSS 是唯一默认值来源；`src-web/styles/generated/theme-defaults.css` 固定提交，源主题变更时重新生成并提交，CI 通过 check 防止生成物漂移。生成文件不再排除，旧根目录副本移除。
- 2026-10-06：按 issue #12 设计定稿新增独立实施任务；默认样式归 004、主题运行时归 026、完整界面归 025。当时处于规划状态，平台 / 解析器证据由实施记录补充。
- 2026-10-06：issue #28 补充实施夹具：默认 app-bg 必须通过 GradientList 子集，无单位 0 位置仍拒绝；M01 的 --ease-* 名称不代表 Tailwind 别名权限，仍按精确目录校验。

## 完成摘要

实现与本机验收已完成；原生记录证明可信 head bootstrap 后、Vue 挂载前的主题与生产 CSS 状态，以及生产 CSP / favicon / CSSOM / IPC 和开发 CSS HMR。它不声称录制了物理显示帧或完成模板主题 / 错误降级的全场景录像。三平台 CI 和完整 verify 证据待补；未通过前保持 in-progress。
