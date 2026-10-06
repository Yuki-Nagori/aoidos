# 002 — Mythos 产品化：模板改名、产品定位与构想类别落档

- 状态：done
- 依赖：001
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-05

## 目标与背景

仓库此前是通用模板（bun-vue-tauri-template），现在正式成为产品 **Mythos**：AI 驱动的剧情跑团桌面应用——用户与 AI 共同编排一段有因果、有推进、有结局的故事（创建世界 → 登台角色 → 开演 → 推进 → 掷骰判定 → 记忆成长 → 落幕）。本任务完成全局改名，把产品定位写进 README；并按用户要求新增 `ai-docs/ideas/` 类别，收纳未排期的构想（只 mark，不承诺实现）。核心的状态与记忆构想已建档 [ideas/001](../ideas/001-context-state-decoupling.md)：对话缓存随对局线性变贵变慢 → 本地状态库 + 对局文档，LLM 按索引读取「现状与下一步」，上下文占用与对局时长解耦。

## 必读

[架构总览](../architecture/README.md) · [目录规划](../architecture/repository-layout.md) · [构建与开发](../architecture/build-and-development.md) · [提交规范](../standards/commits.md)。

## 范围与非目标

交付：

1. 全局改名：`package.json` name、`src-tauri/Cargo.toml` package 与 lib 名、`main.rs` 引用、`tauri.conf.json`（productName / identifier / 窗口 title）、`index.html`、`App.vue`、bench 输入串，锁文件同步刷新。
2. README 重写为 Mythos 产品定位（核心流程、体验关键词、当前状态）。
3. AGENTS 与 ai-docs 中「模板 / 启用模板」表述改为产品语境；「启用模板改名清单」一章随改名完成而移除，保留换图标与替换 greet 示例的指引。
4. 新增 `ai-docs/ideas/` 构想类别（README 索引 + 001 条目），状态记忆构想移入其中，README 留摘要并链接过去；路由与文档职责表同步。

非目标：产品功能实现（世界 / 角色 / 场景等均未开始）；LLM 接入；换图标（沿用 001 的通用立方体）；删除 `greet` 示例（等第一个真实命令落地时替换）。

## 前置条件与待决策

- 前置：001 模板基线已 done，verify 十项全绿。
- 决策：产品显示名 `Mythos`；npm / crate 包名 `mythos`；Rust lib 名 `mythos_lib`；identifier `com.yuki.mythos`（沿用作者反 DNS 前缀）。
- 待决策（留给后续 task，构想细节见 [ideas/001](../ideas/001-context-state-decoupling.md)）：本地存储引擎选型（SQLite 等）、对局文档格式、索引与检索策略、LLM 接入方式。

## 实施步骤

1. 建本 task 并登记索引。
2. 改代码与配置品牌串；`cargo check` 刷新 `Cargo.lock`。
3. 重写 README；更新 AGENTS 与 ai-docs 表述；移除已失效的「启用模板」清单。
4. `bun run verify` 十项全过。
5. task 与索引标 done，重新暂存。

## 预计改动

现存文件：`package.json`、`bun.lock`（如涉及）、根 `Cargo.lock`、`src-tauri/Cargo.toml`、`src-tauri/tauri.conf.json`、`src-tauri/src/main.rs`、`index.html`、`src-web/App.vue`、`src-web/bench/greet.ts`、`src-web/app.css`、`README.md`、`AGENTS.md`、`ai-docs/`（architecture 三篇、standards 两篇 + 规范索引、task-index）。新建文件：本 task、`ai-docs/ideas/`（README + 001）。无接口与持久化兼容负担（尚无真实数据）。

## 验收标准

- [x] 代码、配置与锁文件无 `bun-vue-tauri` 残留（task 历史记录中的旧名指称除外）。
- [x] `tauri.conf.json` 的 productName / identifier / 窗口 title 为 Mythos / com.yuki.mythos / Mythos。
- [x] README 呈现产品定位与当前状态，不把未实现功能写成已有能力。
- [x] ai-docs 无断链；「模板」仅在与任务模板（`_template.md`）、Vue `<template>` 或 task 历史记录相关处保留。
- [x] `bun run verify` 十项 exit 0。

## 验证计划与结果

| 日期       | 环境 / 命令                                                      | 预期   | 实际结果                                          |
| ---------- | ---------------------------------------------------------------- | ------ | ------------------------------------------------- |
| 2026-10-05 | `cargo check --workspace`（先 `cargo clean` 清除 kairos 旧缓存） | 编译过 | exit 0，`mythos v0.1.0` 编译通过，Cargo.lock 同步 |
| 2026-10-05 | `git grep "bun-vue-tauri"`（排除 task 记录）                     | 无命中 | 无命中                                            |
| 2026-10-05 | `bun run verify`                                                 | exit 0 | 十项 exit 0；commands.rs 行覆盖 100%              |
| 2026-10-05 | `bun run format:check`（task 收尾编辑后复跑）                    | exit 0 | exit 0                                            |

## 风险与回退

identifier 变更影响安装包升级路径（Windows 按 product 判别）——尚无发布版本，无迁移负担。回退：git 还原本次暂存前的模板状态。

## 决策与工作记录

- 2026-10-05：创建任务。产品定位与状态设计方向由产品构想落档；命名采用 mythos / mythos_lib / com.yuki.mythos。
- 2026-10-05：完成全局改名与文档改写。`cargo check` 首跑失败：`target/` 内有从 kairos 复制来的过期构建缓存（权限文件路径指向旧仓库），`cargo clean` 后重编通过——与改名无关，属迁移残留。
- 2026-10-05：应用户要求增设 `ai-docs/ideas/` 构想类别，状态记忆构想移入 ideas/001；AGENTS 路由、task-index 目录树、architecture 文档边界、documentation / 规范索引职责表同步，README 摘要改指 ideas。
- 2026-10-06：issue #37 明确桌面版本唯一来源为 workspace.package，package / Tauri 配置未设 version；Bun 锁文件刷新未更新根名，校正旧 excel 元数据为 mythos 并检查 frozen-lockfile，依赖解析版本不变。

## 完成摘要

Mythos 改名、产品定位与构想类别落档完成：代码、配置、锁文件全部改用 mythos / mythos_lib / com.yuki.mythos / Mythos；README 呈现产品核心流程，状态记忆构想摘要指向 [ideas/001](../ideas/001-context-state-decoupling.md)；AGENTS 与 ai-docs 改为产品语境，「启用模板」清单移除；`ai-docs/ideas/` 收纳未排期构想并接入文档路由。verify 十项 exit 0。限制：图标沿用 001 的通用立方体；`greet` 示例保留待第一个真实命令替换；状态存储选型、对局文档格式与索引策略为待决策，见「前置条件与待决策」。
