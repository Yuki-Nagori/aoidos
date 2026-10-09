# 034 — 实现：界面国际化与本地化

- 状态：planned
- 依赖：028、013、026
- 优先级：P1
- 创建 / 更新：2026-10-06 / 2026-10-06

## 目标与背景

按 [i18n 架构](../architecture/i18n.md)交付 zh-Hans / en 底座与原生界面；028 完成仅代表设计通过，当前没有相关运行时。025 接完整界面文案，031 接记忆设置，费用模块消费精确格式化。

## 必读

[国际化架构](../architecture/i18n.md) · [主题架构](../architecture/theming.md) · [通信契约](../architecture/ipc-contract.md) · [前端规范](../standards/frontend.md) · [Rust 规范](../standards/rust.md) · [注释规范](../standards/comments.md) · [测试规范](../standards/testing.md) · [提交规范](../standards/commits.md)。库官方依据和查阅日期见架构，实现时核验锁定版本。

## 范围与非目标

交付资源预编译 / 类型与质量检查、语言偏好单项持久化、首窗 bootstrap、原生文案、错误映射和精确 Intl 包装，接入当前示例。非目标：完整 UI、剧本包格式、自动翻译、日志翻译、计价或云同步。

## 前置条件与待决策

013 / 026 完成，复用同一个 store 服务及 main 构建入口；锁定插件与 ESLint 10 / Vite / Bun 兼容性。若插件不兼容，需等价门禁替代，不降级现有检查。

## 实施步骤

1. 添加工作区统一依赖与根 locales 单源资源，配置 vue-i18n Composition / 预编译 / 生产 compiler 移除。
2. 增补独立 LocalePreference 迁移、单项 get / set、Rust / TS 同型及错误映射。
3. 修改 index.html 静态根为 lang=en，与 en 资源同 commit 接入；合并 026 可信 head / 注入入口，无 bootstrap 为 en、有 bootstrap 挂载前覆盖，应用 lang 和初始 locale；实现串行写入、代次与 pending 主动恢复。
4. build.rs 嵌入 native 子集，接原生菜单 / 托盘 / 对话框 / 标题，覆盖系统文案不可控制的边界。
5. 薄 utils 实现精确金额、数字与显式时区日期格式化；当前示例接入，025 后续消费。
6. 验证产物、首帧和三平台行为，更新现状及任务证据。

## 预计改动

拟新增 locales、src-web 国际化模块 / utils / API 与测试；修改现存 `index.html` 的静态 lang / 可信 head bootstrap、Vite / ESLint / 根 scripts 与依赖锁、src-tauri build.rs / 装配，复用 aoidos-store。当前新路径及接口尚不存在，实现时同步契约与目录文档。

## 验收标准

- [ ] 双语言同键 / 参数 / native 资源互校；静态 key 强类型，缺键 / 裸文案门禁有效。
- [ ] 静态 html lang=en 与 en 资源同 commit 接入；无 bootstrap 首帧为 en，有 bootstrap 挂载前覆盖；生产 CSP 无 unsafe-eval，首帧 / html lang / 读屏一致。
- [ ] 保存失败、旧响应、未知持久版本、显式重载和原生 pending 可恢复，原文未改。
- [ ] 大额 / 极小 / 负额金额无 Number 损失，日期时区显式，三平台原生 UI 证据齐全。
- [ ] 注释、类型、代码、文档同步，最终 bun run verify 全项通过。

## 验证计划与结果

| 日期 | 检查                                            | 预期         | 实际结果   |
| ---- | ----------------------------------------------- | ------------ | ---------- |
| —    | 资源 / 纯逻辑 / 生命周期 / 产物测试与三平台验收 | 满足设计矩阵 | 实现未开始 |
| —    | bun run verify                                  | 全项通过     | 实现未开始 |

## 风险与回退

WebView Intl 与原生系统 UI 可控范围需实测；不通过时保留精确字符串、显式降级，不改账本或放宽 CSP。撤回运行时须同步清理依赖 / 接线，保留已存偏好。

## 决策与工作记录

- 2026-10-06：issue #43 设计评审后登记；028 / 013 / 026 为直接前置，不反向依赖 025，避免初始化循环。
- 2026-10-06：issue #47 补齐现状与目标的区别、index.html 改动归属、静态 en / head / Vue 一致验收；实现尚未开始。

## 完成摘要

未完成。设计与任务编排不作为实现验收。
