# 050 — CI 分层与影响范围选择器

- 状态：in-progress
- 依赖：048、049
- 优先级：P1
- 创建 / 更新：2026-10-16 / 2026-10-16

## 目标与背景

将 Aoidos 当前分层职责写成可评审、可检查的规则，并将 CI 拆成变更规划、共享质量门禁和三平台验收。选择器根据同一份变更计划分别确定 Rust / Vitest 测试、Biome / dprint / rustfmt 格式检查，以及 Biome / Clippy / 分层 lint 范围。

PR 在三平台构建 Web 产物及受影响 Tauri 应用，并运行受影响测试；格式、lint、Clippy 等质量检查仅在 Ubuntu 执行，原生验收在相关 WebView 夹具输入、原生测试输入或 Rust 消费闭包受影响时启动三平台会话。Nightly 在三平台构建并运行完整 Vitest、workspace Rust / 原生测试，覆盖率与全量静态检查由 Ubuntu nightly 承担。共享配置、未知路径、空 diff、不可用元数据或无法可信解析的基线一律回退全量。

## 必读

- [测试规范](../standards/testing.md)
- [分层规范](../standards/layering.md)
- [CI 与构建](../architecture/build-and-development.md)
- [提交规范](../standards/commits.md)
- [048：测试质量与 CI 用时优化](048-test-quality-ci.md)
- [049：Windows 测试 I/O 与原生 HMR 超时](049-windows-test-io-hmr.md)

## 范围与非目标

- 增加 Rust crate 依赖分层规范和 Cargo metadata 边检查；Web 与测试层的职责 / 方向写入规范。
- 本地和 CI 共用路径解析、Cargo 反向依赖闭包与测试 / 格式 / lint 计划，不维护第二份运行时依赖图。
- 三个平台构建 Web 产物，并运行受影响 Rust crate、其 workspace 消费者和镜像 Vitest 测试；完整 workspace 测试与覆盖率由 nightly 运行。
- Biome、dprint 与 rustfmt 按变更文件类型选择；Biome lint 与 Clippy 按变更范围选择。共享配置和未知输入扩至完整检查。
- CI 拆分变更规划、Ubuntu 共享质量门禁与三平台构建 / 受影响测试；nightly 在三平台运行完整构建与测试，并在 Ubuntu 执行完整 `verify`。
- 不按单个 Rust 测试函数筛选，不对 Vitest coverage 做选择性采样，不调整覆盖率下限或原生验收范围。

## 实施步骤

1. 核对 Rust workspace 依赖图，记录允许边，并添加 Cargo metadata 架构 lint。
2. 实现共用 CI 计划：Rust 消费闭包、Vitest 镜像测试、Biome / dprint / rustfmt 文件与 crate 范围。
3. 对共享配置、缺少镜像测试、未知输入、空 diff、基线或元数据失败采用明确的全量回退。
4. 将 PR CI 拆为规划、Ubuntu 质量检查和三平台构建 / 测试；受影响测试与 lint 按差异选择，完整 workspace 测试、coverage 和完整 `verify` 移到 nightly，原生验收仅在相关变更时运行。
5. 更新架构 / 规范 / task 文档；验证规划单测、局部运行、全量本地门禁并独立 review。

## 预计改动

- 新增 `ai-docs/standards/layering.md`、Rust layer checker 与跨工具 CI selector。
- 修改 `package.json`、TS 配置、Vitest / Knip 配置、`.husky/pre-commit` 与 `.github/workflows/ci.yml`。
- 更新 `AGENTS.md`、提交规范、架构总览、测试规范、构建文档、本 task 与 task index。

## 验收标准

- [x] Cargo layer lint 覆盖全部 workspace crates 与 normal / build / dev 依赖，拒绝未登记 crate 和反向边。
- [x] 单 crate 改动选择该 crate 与传递消费者；无关 Rust package 不进入 lint / test 范围。
- [x] TS 源文件映射其镜像测试；缺少专属测试时运行完整 Vitest，避免静默跳过。
- [x] format 与 lint selector 按 Biome、dprint、rustfmt、Clippy 的实际文件 / crate 输入选择。
- [x] Cargo / Bun / CI / 工具配置、空 diff、未知路径、元数据失败及不可解析基线回退全量。
- [x] 本地和 CI 共用同一规划器；无 shell 拼接路径，不受平台路径分隔符影响。
- [x] PR 三平台构建 Web 产物及受影响 Tauri 应用并按影响范围运行测试，质量检查留在 Ubuntu；nightly 配置三平台完整 Vitest / Rust / 原生测试和 Ubuntu `bun run verify`，PR 原生桌面验收按相关变更触发。
- [x] selector 行为测试覆盖依赖闭包、无关范围、资源与镜像映射、格式 / lint 选择及安全回退。
- [x] 本地 `bun run verify` 全项通过。
- [x] PR 三平台构建与受影响测试通过，Windows / macOS 选择范围有日志可审查。
- [ ] Nightly 三平台全量构建与测试通过。
- [x] subagent 独立评审完成，发现已处理并复验。

## 验证记录

- **2026-10-16 · selector / layer Vitest**：4 个文件、26 项测试通过；包含 Rust 闭包触发原生测试、WebView 夹具输入映射和 workflow-only 不扩大 Rust 测试。
- **2026-10-16 · `lint:layers`**：11 个 workspace crate 边通过。
- **2026-10-16 · `bun run verify`**：完整通过；Vitest 38 个文件 / 375 项，覆盖率 100%；Rust workspace 测试通过，覆盖率 31,693 / 31,806 行（99.64%，门槛 90%）；typecheck、lint、Clippy、分层检查、Biome / dprint / rustfmt、Knip、bench、Web build 与 rustdoc 均通过。
- **2026-10-16 · 独立评审**：`/root/review_050_selector` 使用 `gpt-6.1-sol` / medium 检查完整实现及最终小幅修正；未发现可执行缺陷或回归。
- **2026-10-16 · PR #151**：Actions run [38913067825](https://github.com/Yuki-Nagori/aoidos/actions/runs/38913067825) 全部通过；Ubuntu、macOS、Windows 三平台构建与受影响测试通过，Ubuntu 共享质量门禁通过。Windows/macOS 的受影响测试计划由 `detect affected checks` 生成并供各平台 job 消费。
- **Nightly**：三平台全量构建与测试尚未执行，保留为唯一未验收项。

## 风险与回退

错误缩小测试范围可能漏掉消费者回归。反向依赖闭包和未知路径全量回退作为主要保护；选择器行为测试固定这些规则。发现漏选时可立即将 Windows / macOS 恢复为 `bun run test:rust`，选择器保留用于诊断，待修复后再接回 CI。

## 决策与工作记录

- 2026-10-16：参考 panta 分层与 CI 选择实践，但只移植适合 Aoidos 的 Cargo crate 边检查、共享变更计划与保守全量回退；不引入 C++ / QML 专用工具链。
- 2026-10-16：共享计划将测试、格式、lint 分类；Ubuntu coverage 不受选择器缩小。Rust 选择使用 workspace 反向依赖闭包，Vitest 使用源码到镜像测试的显式路径关系。
- 2026-10-16：Git 差异采集关闭 rename 检测，移动的源 / 目的路径分别参加映射；Cargo metadata 或可信基线不可用时运行全量。
- 2026-10-16：独立 review 发现并修复删除 / 排除路径误传、共享 Rust 构建输入漏选、普通 Web 源码漏跑 i18n，以及 CI 冷 runner 的覆盖率工具源码安装风险；修复后 review 通过。
- 2026-10-16：本地提交前改用受影响范围测试、格式与 lint；Husky 同样改用 `format:affected`，完整 `verify` 留给任务收尾。
- 2026-10-16：PR 三平台构建 Web 产物、受影响 Tauri 应用并只跑受影响测试，格式 / lint / Clippy 等质量门禁留在 Ubuntu；原生验收按相关变更触发。Nightly 在三平台构建并运行完整 Vitest / Rust / 原生测试，Ubuntu 执行完整 `bun run verify`，可手动触发。

## 完成摘要

分层规范、选择器、PR / nightly CI 拆分与本机完整验证已完成；PR 三平台受影响构建 / 测试已通过。Nightly 三平台全量运行尚未验证，完成后再将任务标为 done。
