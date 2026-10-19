# 056 — 实现：标签驱动结局与周目落幕

- 状态：planned
- 依赖：018、019、020、023、027、032、035、039、040、041、052、054
- 优先级：P1
- 创建 / 更新：2026-10-19 / 2026-10-19

## 目标与背景

实现 [054](054-ending-flow-design.md) 定稿的周目规则冻结、隐藏数值求值、语义标签、规范结局摘要、AI 叙事与可恢复落幕流程。引擎决定正史并原子封存；AI 文本不能写世界事实，也不能授权玩家从 `ending-choice.html` 固定列表选择结局。

## 对照设计

`aoidos-art` 基线 `1f19504ebc7390b7d9954b1c597fc3d98b6b7fde` 中的 `ending-approach.html`、`cycle-init.html`、`cycle-init-failed.html`、`ending-pending.html`、`ending.html`、`ending-narrating.html` 与 `ending-narration-failed.html` 展示冻结规则、终局恢复及可选叙事的视觉状态。页面状态不得先于权威 engine / storage 协议实现；`ending-choice.html` 的静态选项不构成玩家选结局的授权，剧本必须先声明规则目录，缺少 sidecar 的 run 继续兼容原终态。

## 必读

[054 结局生成与周目落幕设计](054-ending-flow-design.md) · [结局架构](../architecture/ending-flow.md) · [回合与阶段状态机](../architecture/turn-state-machine.md) · [记录引擎](../architecture/record-engine.md) · [剧本格式](../standards/scenarios.md) · [计费架构](../architecture/billing.md) · [记忆架构](../architecture/memory.md) · [通信契约](../architecture/ipc-contract.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：严格剧本结局规则 / 标签目录、作者规则与新周目预生成规则快照、规则及标签版本与 provenance、终局规则求值、冻结事实与确定性摘要、AI 结局叙事、计费 / 恢复 / 回退、UI 状态 / IPC、032 刻痕衔接和跨层验收。

非目标：不增加玩家选择固定结局、不允许模型生成或改写权威世界事实、不把 AI 叙事用作记忆素材或后续规则事实；不在本任务制作剧本编辑器或开放任意脚本表达式。

## 前置条件与待决策

054 明确的规则 schema、用途区分校验、enabled / disabled / legacy-disabled 生命周期、开局锁定、标签组合、规范摘要、AI 文本边界、失败恢复和事务图为冻结输入。实现前核对 023 世界状态写入与恢复、032 刻痕周期、040 解析器、041 内置剧本及 035 账本接口；新增 crate / module 依赖须保持架构无环。规则生成若使用 LLM，必须在新周目开始前显式执行、经 027 / 035 计费，并只处理剧本声明的字段 / 规则目录；生成中的运行不能进入可玩状态。生成失败或结果不确定时不得暗中重发或换用另一份规则。没有结局 sidecar 的剧本沿用旧流程；旧存档不得补造规则或进入新的 endingResolved 补交流程。

## 实施步骤

1. 为 `resources/scripts/<scriptId>/` 登记严格、有版本的结局规则与标签目录；预设规则和生成规则使用同一类型、校验器、规范化与哈希。
2. 在开始周目时创建或恢复初始化状态；锁定当前剧本 hash / 规则版本与完整规则快照到 SQLite，再允许进入可玩状态。已有周目和历史分支始终复用原快照。
3. 在可信终局触发点基于已提交世界状态求值标签；冻结规则来源、tag ID、rule ID / version、历史修订与结果摘要。未知缺失字段不得按零值处理，冲突按 054 决策拒绝或走明示优先规则。
4. 用确定性模板从已公开标签生成规范摘要并原子提交结束事实；AI 只消费摘要生成可选非权威叙事，失败不会阻止规范周目结束，显式重试复用同一冻结结果。
5. 定义供应商超时 / unknown、已扣费、已回执未保存、数据库失败、重开、回退、重复命令和新周目入口的状态恢复；结局叙事从普通 memory narration / candidate 投影中排除。
6. 接入 Tauri 命令、TS 类型、界面结束状态及 AI 叙事标识；设计页的固定结局选项只复用视觉，不建立选项选择命令。
7. 用故障注入和跨层行为测试覆盖主路径、并发、幂等、隐私和无环依赖；更新状态机、记录、剧本、记忆、IPC、architecture README、本 task，并由 subagent 独立评审。

## 预计改动

`resources/scripts/`、`src-rust/aoidos-engine/src/game/`、必要的 `src-rust/aoidos-script/`、Tauri commands、TypeScript API / Vue game feature、locales、SQL migration 与 Rust / native / Vitest 测试；新增 `ai-docs/architecture/ending-flow.md`，同步 `turn-state-machine.md`、`record-engine.md`、`memory.md`、`billing.md`、`standards/scenarios.md`、`ipc-contract.md`、`architecture/README.md`。

## 验收标准

- [ ] 作者规则与开局生成规则都经过同一严格白名单 / schema / 值域校验；静态生成声明可为空规则，生成后冻结快照必须含合法规则且声明目录不变；完整规则快照在首回合前持久化，后续恢复 / 分支 / 剧本更新不重算。
- [ ] 终局只由权威游戏条件触发；标签由冻结规则从已提交数值确定，带来源版本、组合 / 冲突语义和 revision；缺失状态不伪装为零。
- [ ] 标签确定性地产生权威事实摘要；隐藏原始数值不进入玩家选项或叙事请求，未公开标签不泄露。
- [ ] AI 叙事明确标记为非权威，不更新世界状态、普通 narration、记忆素材 / evidence 或后续规则上下文；事实摘要不依赖 AI 成功。
- [ ] 计费按真实物理请求；未知结果不自动重发，失败恢复不重算规则 / 标签、不重复提交终态，调用不存在跨模块循环依赖。
- [ ] 结束 / 初始化 / retry / restart / rollback / replay / new-cycle 的 Tauri / Rust / TS 类型同型且有行为与故障注入测试。
- [ ] 未配置结局规则的新 run 及缺少 mode 字段的旧存档保持兼容，不生成规则、不补造历史 snapshot 或 endingResolved。
- [ ] UI 不提供固定结局选择；运行期展示以引擎终态为准，AI 文本与规范摘要可辨权威性。
- [ ] 代码、中文注释、类型、文档及 task 同步；subagent 独立评审处理完成；最终 `bun run verify` 全项通过。

## 验证计划与结果

待实施后填写：规则 schema / 哈希、SQL migration、WorldPort / 周目恢复、027/035 计费与 unknown、032 单向对接、跨平台 IPC、Vitest、故障注入及完整 `bun run verify`。

## 风险与回退

结局规则属于周目不可变输入，旧周目不得因 parser、剧本或提示版本变化而漂移。生成文本可能误述但不得覆盖正史；若关键事实输出无法校验，使用确定性规范摘要完成落幕，并保留显式恢复 / 再生成状态。历史 branch 只恢复该 branch 的结束快照，禁止重新求值覆盖。

## 决策与工作记录

- 2026-10-19：Aoidos 内嵌 Mistbell 作者稿已移除固定结局选项及代价阈值，并声明标签 / 规范摘要 / 非权威 AI 叙事边界；内容仓库作者稿已在本地分支同步。严格 sidecar、属性规则与 Python 镜像验收仍由本任务实现，不因文案更新提前启用结局模式。

- 2026-10-19：根据 054 定稿登记实现。Engine 独占规则、标签与终局权威；memory、billing 和 UI 仅消费显式端口 / 结果，依赖方向不反转。

## 完成摘要

未开始。
