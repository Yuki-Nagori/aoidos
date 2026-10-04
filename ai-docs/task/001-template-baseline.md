# 001 — 模板基线：bun + vue + ts + tauri 最小环境

- 状态：done
- 依赖：无
- 优先级：P0
- 创建 / 更新：2026-10-04 / 2026-10-05

## 目标与背景

自注塑成型 CAE 仓库（kairos）裁剪出一个 bun + vue + ts + tauri 的最小桌面模板：保留其验证过的工具链与质量门禁，移除全部领域代码，使新项目可以此为起点直接开工。本任务同时是本仓库 task 工作方式的示范记录。

## 必读

[架构总览](../architecture/README.md) · [构建与开发](../architecture/build-and-development.md) · [提交规范](../standards/commits.md) · [测试规范](../standards/testing.md)。

## 范围与非目标

交付：最小可运行骨架（`greet` IPC 示例 + 浏览器回退）、bun 聚合命令、verify 十项门禁、husky 格式钩子、三平台 CI、双端 100% 覆盖率门槛、knip、tinybench 基准示例、panta 式分层文档、图标与打包链路。

非目标：业务功能；状态管理 / 路由 / UI 组件库；移动端目标；发布自动化（release workflow）。这些等真实需求出现后各自立 task。

## 前置条件与待决策

- 前置：仓库以全新历史重新初始化，原 kairos 提交不再被引用；本任务记录即新历史的起点。
- 决策：依赖升级到 eslint ^10 / vite ^8 / vitest ^5.0.3 / knip ^6；TypeScript 停在 ^5.9（TS 7 尚无 typescript-eslint 支持）；Rust 工具链升 1.99.0。

## 实施步骤

1. `git rm` 全部领域代码与文档：src-crates、旧 ai-docs、scripts、.github、.husky、knip 配置、tests、src-web 与 src-tauri 的领域模块。
2. 重写最小骨架：`src-web/`（main.ts、App.vue、app.css、utils/greet.ts）与 `src-tauri/`（lib.rs、commands.rs、tauri.conf.json、capabilities）。
3. 重建门禁：vitest 覆盖率门槛、Rust 行覆盖门槛（装配文件不计）、knip.json、husky 格式钩子、拆分步骤的 ci.yml。
4. 文档体系重建为 panta 分层（architecture / standards / task-index / task）。
5. 全链验证并登记证据。

## 预计改动

本任务为仓库初建，改动即整个工作区；无既有接口或持久化兼容负担。

## 清理与兼容例外

删除项：kairos 全部领域代码与文档、旧 comment-style / docs-check 脚本、旧 coverage 脚本、release workflow、bench 旧链、coverage 旧口径。无兼容例外。

## 验收标准

- [x] `bun install` 后 `tauri:dev` 可开发、`dev` 可浏览器预览（浏览器路径实测；桌面路径经 1.98.1 打包验证）。
- [x] verify 十项在本地全绿（见验证记录；Rust 1.98.1 时期全过）。
- [x] Rust 门禁在 1.99.0 上复验通过（cargo check / clippy / test / coverage:rust）。
- [x] `tauri:build` 出包（1.99.0 上 msi + nsis 实测）。
- [x] 文档体系与仓库现状一致，无断链、无过期描述。

## 验证计划与结果

| 日期       | 环境                                | 检查                                                                                      | 结果                                 |
| ---------- | ----------------------------------- | ----------------------------------------------------------------------------------------- | ------------------------------------ |
| 2026-10-04 | Windows / Rust 1.98.1               | `bun run verify`                                                                          | 十项全过；前端覆盖率 100%            |
| 2026-10-04 | Windows / Rust 1.98.1               | `bun run tauri:build`                                                                     | msi + nsis 出包                      |
| 2026-10-05 | Windows / 升级后                    | 前端检查与 `bench`                                                                        | 全过                                 |
| 2026-10-05 | Windows / Rust 1.99.0               | `bun run verify`                                                                          | 十项 exit 0；commands.rs 行覆盖 100% |
| 2026-10-05 | Windows / Rust 1.99.0、Tauri 2.12.1 | `bun run tauri:build`                                                                     | msi 1.56 MiB + nsis 1.08 MiB         |
| 2026-10-05 | macOS / issue #3 与 CI 优化         | `bun run verify`                                                                          | 十项 exit 0                          |
| 2026-10-05 | Ubuntu / macOS / Windows            | [PR #4 / run 37237430870](https://github.com/Yuki-Nagori/mythos/actions/runs/37237430870) | 提交 `a443f12` 三平台全部通过        |

issue #3 复核已核对三平台矩阵、Rust 工具链、CI 调用的 bun scripts 与本地门槛。testing.md 仅引用脚本入口，实际参数统一在 package.json 维护。CI 的编译形态合并与步骤顺序见[构建与开发](../architecture/build-and-development.md)。

## 风险与回退

Rust 工具链升级可能引入新的 clippy 门禁。需要回退时同步修改 `rust-toolchain.toml` 与 CI 工具链配置。CI 顺序优化只改变失败反馈时机，三平台及全部门槛保持原值；可独立撤销工作流配置改动。

## 决策与工作记录

- 2026-10-04：创建任务。自 kairos 裁剪骨架，替换通用图标，重建分层文档；根 Cargo.toml 改为虚拟工作区，移除无使用方的领域代码与依赖。
- 2026-10-05：升级 ESLint 10、Vite 8、Knip 6 等依赖，TypeScript 保留 5.9；Rust 工具链升级 1.99.0。`greet` 移入 commands.rs，装配 lib.rs 不计覆盖。husky 只查格式，完整门禁由推送前验证与 CI 执行。
- 2026-10-05：环境配置聚合到 `bun install` 的 postinstall，负责工具链自检、自愈半截安装和补装 cargo-llvm-cov；`bun run setup` 为手动入口。
- 2026-10-05：对齐 npm 与 Rust 侧 Tauri 版本，解决 bundler 的版本不匹配错误；完整验证与 Windows 打包通过，任务标为 done。
- 2026-10-05：[issue #3](https://github.com/Yuki-Nagori/mythos/issues/3) 复核发现 testing.md 保留了过期的 `--summary-only` 命令副本。改为引用 `bun run coverage:rust`，保留统计范围与门槛说明，避免参数漂移。
- 2026-10-05：CI 将格式 / 前端 / knip 快速门禁前置，Linux 系统依赖安装和 Rust 编译后置；失败也保存 Rust 依赖缓存，token 仅需 `contents: read`，每 job 30 分钟超时。三平台验证通过，证据补入本任务，不新增 task。

## 完成摘要

最小骨架、bun 命令、十项门禁、husky / CI、分层文档与打包链路已落地。issue #3 的过期文档已修正，CI 优化通过本地验证与三平台检查。浏览器预览使用无 IPC 回退；发布打包仅在 Windows 实测，其他平台的 CI 验证不等同于打包验证。
