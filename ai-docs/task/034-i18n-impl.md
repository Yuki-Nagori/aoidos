# 034 — 实现：界面国际化与本地化

- 状态：done
- 依赖：028、013、026
- 优先级：P1
- 创建 / 更新：2026-10-06 / 2026-10-10

## 目标与背景

按 [i18n 架构](../architecture/i18n.md)交付 zh-Hans / en 底座与原生界面。实现、十三项本地 verify、独立评审及 PR #107 的 macOS / Ubuntu / Windows CI 均已通过。025 接完整界面文案，031 接记忆设置，035 消费精确格式化。

## 必读

[国际化架构](../architecture/i18n.md) · [主题架构](../architecture/theming.md) · [通信契约](../architecture/ipc-contract.md) · [前端规范](../standards/frontend.md) · [Rust 规范](../standards/rust.md) · [注释规范](../standards/comments.md) · [测试规范](../standards/testing.md) · [提交规范](../standards/commits.md)。库官方依据和查阅日期见架构，实现时核验锁定版本。

## 范围与非目标

交付资源预编译 / 类型与质量检查、语言偏好单项持久化、首窗 bootstrap、原生文案、错误映射和精确 Intl 包装，接入当前示例。非目标：完整 UI、剧本包格式、自动翻译、日志翻译、计价或云同步。

## 前置条件与待决策

013 / 026 完成，复用同一个 store 服务及 main 构建入口；锁定插件与 ESLint 10 / Vite / Bun 兼容性。若插件不兼容，需等价门禁替代，不降级现有检查。

## 实施步骤

1. 已添加统一依赖与根 locales 单源资源，启用 vue-i18n Composition、预编译与生产编译器移除。
2. 已新增 aoidos-locale 偏好 API，接入 engine 共享 SQLite 迁移与 Tauri 单项命令，Rust / TS 载荷同型。
3. 已修改 index.html 静态根为 lang=en，并将 locale bootstrap 合并进 026 创建的同一初始化脚本；每次页面加载在 Vue 挂载前回读最新偏好，避免 reload 沿用旧语言；实现启动回退、串行最新意图、失败回读与原生 pending 提示。
4. 已从相同 JSON 构建嵌入 native 子集；接入原生凭据提示、应用菜单、托盘和窗口标题，保留系统管理文案边界。
5. 已新增精确十进制金额、BigInt 数字及显式时区日期格式化；当前示例完成界面文案接入。
6. 本地全量验证、覆盖率修复、独立评审、三平台原生集成及 PR CI 均已完成。

## 预计改动

改动覆盖 `locales/`、`src-web/{api,composables,i18n,utils}`、Vite / ESLint 与资源校验脚本、`src-rust/aoidos-locale`、engine 共享迁移、Tauri locale 命令及单独的 Wry 菜单 / 托盘适配、原生凭据文案和精确格式化；共享数据库与 026 首窗入口保持单一所有者。目录、命令、依赖图与验证证据同步在相关架构文档中。

## 验收标准

- [x] 双语言同键 / 参数 / native 资源互校；静态 key 强类型，缺键 / 裸文案门禁有效。
- [x] 静态 html lang=en 与 en 资源同 commit 接入；无 bootstrap 首帧为 en，有 bootstrap 挂载前覆盖；生产 CSP 无 unsafe-eval，首帧 / html lang / 读屏一致。
- [x] 保存失败、旧响应、未知持久版本、显式重载和原生 pending 可恢复，原文未改。
- [x] 大额 / 极小 / 负额金额无 Number 损失，日期时区显式，三平台原生 UI 证据齐全。
- [x] 注释、类型、代码、文档同步，最终 bun run verify 全项通过。

## 验证计划与结果

| 日期       | 检查                                                                | 预期     | 实际结果                                                                                        |
| ---------- | ------------------------------------------------------------------- | -------- | ----------------------------------------------------------------------------------------------- |
| 2026-10-10 | `bun run verify`（十三项）、25 个前端测试文件 / 203 项测试          | 全项通过 | 通过；前端行 / 分支 / 函数 / 语句覆盖率均 100%，Rust 覆盖门禁通过（94 文件）                    |
| 2026-10-10 | 独立评审                                                            | 无阻塞   | 通过；两项原生测试缺口已修复，托盘 / 凭据对话框的双语读取断言仍未覆盖                           |
| 2026-10-10 | [PR #107](https://github.com/Yuki-Nagori/aoidos/pull/107) 三平台 CI | 全项通过 | 通过：macOS（3:34）、Ubuntu（3:50）、Windows（7:02）；含三平台原生 UI / 凭据与 WebView IPC 验证 |

## 风险与回退

WebView Intl 与原生系统 UI 可控范围需实测；不通过时保留精确字符串、显式降级，不改账本或放宽 CSP。撤回运行时须同步清理依赖 / 接线，保留已存偏好。

## 决策与工作记录

- 2026-10-06：issue #43 设计评审后登记；028 / 013 / 026 为直接前置，不反向依赖 025，避免初始化循环。
- 2026-10-06：issue #47 补齐现状与目标的区别、index.html 改动归属、静态 en / head / Vue 一致验收；实现尚未开始。
- 2026-10-10：013 与 028 已完成，026 已经 PR #105 三平台 CI 验收并合并；前置条件满足，任务转为 ready。
- 2026-10-10：开始实施 034；先核对 i18n 设计与已合并的主题 bootstrap、共享 store、Tauri 原生 UI 边界，再逐步落地资源、偏好和格式化链路。
- 2026-10-10：初轮落地资源、命令、共享 SQLite 偏好、bootstrap、原生文案与格式化；前端 186 项及 locale crate 4 项测试通过。随后修复静态检查发现的两处类型 / derive 错误，并由最终 verify 复验通过。
- 2026-10-10：复核修正 vue-i18n Composition locale ref 与文档重载首帧；Wry 原生表面拆出独立适配并补双语菜单 / 标题断言。补足 Rust 偏好及 Tauri IPC 边界测试、前端启动注入覆盖；同步 knip bootstrap 入口与测试规范。
- 2026-10-10：`bun run verify` 十三项全通过：前端 203 项测试、四指标 100%；Rust workspace 测试、94 文件覆盖门禁、knip、bench、Web 构建和 rustdoc 均通过。独立评审与三平台 CI 尚待完成。
- 2026-10-10：独立评审发现原生标题断言原值不变、语言 bootstrap 未随产品 HTML 进入夹具；现已加入双语切换前标题哨兵、构建并复制语言 bootstrap、注入中文首帧并在 Vue 挂载前断言。评审通过；三平台 CI 仍待执行。
- 2026-10-10：[PR #107](https://github.com/Yuki-Nagori/aoidos/pull/107) 的 macOS / Ubuntu / Windows `verify` 全部通过，三平台原生会话与 WebView IPC 检查成功；全部验收条件有证据，任务标记 done。

## 完成摘要

本地十三项 verify、独立评审与 PR #107 三平台 CI 均通过；任务验收完成。菜单 / 标题及 WebView 首帧已在原生夹具验证；托盘 tooltip 与凭据对话框的双语读取断言未纳入本次验收。
