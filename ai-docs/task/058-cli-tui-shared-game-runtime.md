# 058 — CLI / TUI 共用游戏运行层

- 状态：in-progress
- 依赖：013、018、019、020、023、035、040、041
- 优先级：P1
- 创建 / 更新：2026-10-20 / 2026-10-20

## 目标与背景

当前真实 Provider 请求只能从 Tauri 桌面流程触发，界面错误、Rust 装配、密钥读取、计费预留和服务商响应混在一次手工验收中，故障难以定位。按照[分层规范](../standards/layering.md)，在 `aoidos-engine` 之上建立不依赖 Tauri 的 `aoidos-app` Rust 应用层，由桌面 GUI 和 `aoidos-cli` 两个入口在进程内并列调用；先用终端链路验证配置、密钥、提示词、流式回合和费用，再将同一应用层接回 GUI。GUI 不启动 CLI 子进程，也不复制另一套游戏或计费逻辑。

首轮需有剧本提供的建议行动，用户可编辑并显式提交；打开或恢复周目不自动发出收费请求。当前每次请求按冻结 ProviderInput 的 JSON 字节数加 1 KiB 封套余量计算预留，并受模型上下文上限约束；原文与序列化输入都在发送前受 256 KiB 硬限。这个本地上界只用于预算预留，结算仍采用供应商实际 usage；不得把字符估算冒充 token 上界，也不得绕过账本门禁。

## 必读

- [Rust / Web / 测试依赖方向](../standards/layering.md)
- [Repository layout](../architecture/repository-layout.md)
- [LLM 接入与护栏](../architecture/llm.md)
- [计费架构](../architecture/billing.md)
- [UI Shell](../architecture/ui-shell.md)
- [剧本格式规范](../standards/scenarios.md)
- [Rust 规范](../standards/rust.md)
- [测试规范](../standards/testing.md)
- [注释规范](../standards/comments.md)

## 范围与非目标

- 无 Tauri 的 `aoidos-app` 已加入 workspace，并承载共享产品装配、profile / 凭据冻结、对局运行入口及当前周目只读计费摘要；继续检查迁移边界，避免业务逻辑落入 CLI 或 Tauri 命令。
- 在根 `src-cli/` 添加 `aoidos-cli`：复用同一存储、profile、OS 凭据库、Provider、Engine、回合恢复和计费端口，提供可脚本化命令及可交互 TUI。入口直接依赖 engine 的范围限于平台事件端口和受控记录读取，不复制业务规则。
- TUI 可列出剧本与 profile、开始 / 恢复周目、编辑并显式提交首个建议行动、读取流式正文和已提交记录、显示阶段 / 可操作错误提示 / 原币账本，并提供已有的取消 / 显式恢复操作。
- TUI 的可点击控件与键盘焦点映射到同一组动作，至少支持鼠标选择、Tab / 方向键移动、Enter 执行、Esc 返回；终端不支持鼠标时仍可完整键盘操作。交互拆为输入动作、运行时效果和状态更新，渲染组件不直接操作存储、凭据或 Provider。设计取舍见 [Grok Build TUI 调查](../research/003-grok-build-tui.md)。
- 为 Mistbell 提供明确、可本地化的首行动建议；建议是可编辑的玩家输入，不是 system prompt，不自动发送，不改变记录身份。
- 预算预留现按冻结 ProviderInput 的 JSON 序列化字节数加 1 KiB 请求封套余量逐请求计算，并受模型上下文上限约束；发送前拒绝原文或序列化输入超过 256 KiB。该值只控制本地最坏情况预留，结算仍使用供应商实际 usage；输出按冻结请求的 `max_tokens` 预留。保持安全余量、估算误差与校准暂停语义，并用边界测试验证额度不足在发送前拒绝。
- 保留真实 API 的手动验收入口；必须展示将使用的 profile、模型、币种、局限额，并要求用户显式确认。密钥只经 OS 凭据库读取 / 写入，不进入参数、终端回显、日志或历史记录。
- Tauri GUI 后续复用共享应用层；其命令仍是薄 IPC adapter，GUI 不 shell out 到 CLI。
- 接受 035 已交付的应用内接口和仓库验证；035 的三平台实机展示验收仍由 035 自身跟踪。CLI 真实请求 smoke 不等同于 033 费用实验，但遵守同一 035 实机验收门禁；每次产生费用的真实请求仍须有明确用户授权和用户设定的金额上限，不以本 task 的实现授权替代。
- 不在 CI 发送真实供应商请求；不实现新的 Provider、账单余额接口或第二套业务状态机；不扩张 052 的 HTML 视觉移植范围。

## 前置条件与待决策

- 已有的 `aoidos-engine::game::product::Product`、`ProfileFactory`、`runtime::Service`、`LedgerBudgetPort`、Provider 与 OS credential adapter 是复用基础；当前 `SavedProfiles` / profile 冻结装配在 `src-tauri`，按 layering.md 提取到 `aoidos-app`，入口只注入数据路径和平台交互，不允许反向依赖。
- TUI 框架与终端事件依赖需核对跨平台能力、维护状态及仓库 toolchain 兼容后再选；不得为了 UI 引入与业务运行无关的大型依赖。
- Grok Build 的公开 TUI 结构可作为交互与分层参考；首版只评估 Ratatui / Crossterm，不移植其 agent、插件或命令体系。
- 真实 DeepSeek API 测试只在用户本机显式执行，使用已有账户密钥；不得从应用进程私自读取或打印密钥。真实费用额度由用户在验收前明确设定。

## 实施步骤

1. 按 layering.md 登记 `aoidos-app`、`aoidos-cli` 依赖边，复核模块所有权；`aoidos-app` 已加入 workspace 并纳入 `lint:layers`，CLI crate 现放在根 `src-cli/`，路径与分层规范同步。
2. 已抽取 profile / credential 服务与共用 `Product` / runtime 装配；Tauri 命令复用共享 profile 服务，IPC 和既有数据根保持兼容。CLI 正复用同一数据根、OS 锁和共享服务，随后验证跨入口锁与恢复行为。
3. CLI 子命令与 TUI 已接入，复用 Product / Service 的业务操作、持久化、事件和恢复；本轮增加应用层 `BillingService` 供 TUI 读取当前周目按供应商 / 币种 / 模型分组的结算、预留、Token 与缺失用量摘要，不复制账本状态机。
4. 已将场景首行动建议纳入版本化本地化数据及 Rust / TS 契约；GUI 与 TUI 将其作为可编辑草稿，只有显式提交才创建玩家记录及付费请求。
5. 已以序列化输入字节上限和 1 KiB 请求封套余量按物理请求计算预留，受模型窗口约束；保留短请求、上界、额度不足及拒绝前不发送测试，并完善预算拒绝的用户可读诊断。
6. 加入无网络 fake-provider / fixture 测试；CLI 测试走共享 Product、runtime、SQLite 与事件桥，验证新建不发送请求、显式提交剧本建议、流式正文与终态、取消后显式恢复且不重复追加玩家行动、关停重装配后恢复持久记录而不重发，以及认证失败的可读提示。计费测试通过真实 `LedgerBudgetPort` 和 SQLite 验证额度不足时不调用 Provider、成功请求完成预留 / 实际用量结算，并确认费用面板按周目 `memory_run_id` 刷新。真实供应商测试留作手动步骤并记录实际结果。
7. 更新 CLI 使用说明、架构图、LLM / billing / scenario / UI 文档与 task；独立 review 完整 diff，处理发现后运行受影响测试与 `bun run verify`。

## 预计改动

- 新增无 Tauri 依赖的共享应用层或等价模块，以及 `aoidos-cli` workspace crate / binary。
- 修改 `src-tauri` 装配与命令 adapter、Rust workspace / 锁文件、计费预留参数和投影预算边界。
- 为 Mistbell 增加可本地化的首行动建议字段或 sidecar 元数据，并同步剧本格式 schema / IPC 类型；原始剧本正文不混入产品 UI 翻译。
- 更新 CLI 操作文档、架构依赖图、billing / LLM / scenarios / UI 文档和本 task。

## 验收标准

- [x] GUI 与 CLI/TUI 依赖同一无 Tauri `aoidos-app` Rust 应用层；GUI 不启动 CLI，profile / 凭据 / 业务运行逻辑只有一份实现，数据根保持兼容，crate 依赖图与 `lint:layers` 均证明无环。
- [x] CLI/TUI 与 GUI 共用 profile、OS 凭据库、剧本、存档、回合服务、Provider 和 billing ledger；密钥不进入 CLI 参数、输出、日志或记录。
- [x] TUI 能在 fake-provider 测试中新建 / 恢复 Mistbell 周目、编辑首行动建议、显式提交、显示流式正文 / 终态 / 记录 / 计费状态，并取消与显式恢复。
- [x] 新周目不会自动发送建议或收费；关闭后重开不会重复提交建议；草稿不得伪装成已接纳玩家事实。
- [ ] Mistbell 首行动建议有中英文来源；缺失 / 非法本地化 metadata 明确回退或拒绝，不猜测生成玩家输入。
- [x] 每次预留按冻结输入序列化字节上界和请求封套余量计算，且原文 / 序列化请求均受 256 KiB 发送前硬限和模型窗口约束；不得将字符估算冒充供应商 usage。模型窗口、最大输出、不同币种限额及超限前不发送均有测试。
- [x] 额度不足时请求在网络发送前被拒绝，UI / TUI 给出可读原因；可用预算足够时真实请求不会仅因模型未使用的上下文容量而提前失败。
- [ ] 仅在 035 三平台实机验收完成且用户对本次请求单独明确授权后，才用选定的 DeepSeek profile / OS key 完成一次真实请求并显示终态及本地原币估算；记录 profile、模型、币种、冻结价格、上限、实际用量与限制。未执行真实请求的平台不宣称通过。
- [ ] 本地 fake-provider 测试不访问网络；CI 不使用真实密钥、不产生真实费用。
- [ ] GUI 错误码映射与 system record 展示不再显示原始 `budget.exceeded` 或“当前版本无法显示”的通用占位；内容来自稳定码本或已脱敏 system message。
- [ ] 代码、中文注释、类型、文档及 task 同步；独立评审完成并处理发现，`bun run verify` 通过。

## 验证计划与结果

| 日期       | 环境 / 命令                                              | 预期                                   | 实际结果                                                                                                                                                                                       |
| ---------- | -------------------------------------------------------- | -------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-20 | `bun scripts/cargo.mts test -p aoidos-app -p aoidos-cli` | 应用服务摘要及 CLI/TUI 目标测试通过    | 21 项通过；覆盖应用 profile / billing 服务、TUI 无请求开局与恢复、行动提交 / 流式显示 / 终态、取消与显式恢复、认证失败、真实账本预算拒绝不派发、费用预留 / 实际用量结算及面板刷新              |
| 2026-10-20 | `gh run view 39415423031 --log-failed`                   | 定位 main CI 的 macOS 测试失败         | `aoidos-app` 的 profile 配额测试在并行执行时遇到 `store.not-found`；测试临时目录改为进程号 + 原子序号命名并用 `create_dir` 独占创建，防止测试共享目录及互相清理。应用与 CLI 定向测试 21 项通过 |
| 2026-10-20 | `bun run verify`                                         | 全仓检查、测试、覆盖率、构建和文档通过 | 全部通过；454 项 Vitest、Rust workspace 测试通过，Rust 覆盖率 96.83%，视觉回归、基准、Web 构建和 Rust 文档生成通过。                                                                           |

## 风险与回退

共享应用层若直接照搬 Tauri 命令结构，可能把窗口事件、路径状态或插件类型泄露给 CLI；抽取时由应用服务定义与平台无关的事件端口，两个入口各自适配。预算估算不能以“预留较小”牺牲额度约束；输入上界必须由实际投影合同导出，并通过不足额拒绝与精确边界测试。OS 凭据库在无桌面会话环境可能不可用，CLI 应返回可操作错误，不降级到明文文件。

## 决策与工作记录

- 2026-10-20：用户提出先通过 CLI/TUI 真实验证 key 与游戏链路，并考虑 GUI 按钮是否也调用 CLI。决定 GUI 不启动 CLI 子进程；提取共用 Rust 应用层，Tauri GUI 和 `aoidos-cli` 各自作为薄入口。
- 2026-10-20：检查发现正式产品 billing reserve 使用模型完整 `context_limit`，但 prompt projection hard limit 为 32,768；本任务修复两者不一致，CLI 不绕过 ledger。
- 2026-10-20：截图暴露 budget 错误码原样显示、未知 system record 只显示兼容占位；加入稳定本地化与脱敏 message 渲染验收。
- 2026-10-20：新增 `aoidos-app` 并接入 Tauri 装配；共享层提供 profile CRUD、凭据状态 / 写入 / 删除 / 提交冻结，并组合回合协调器、诊断状态、代理快照、engine `Product` 与 runtime。Tauri 仍解析数据根并注入窗口事件；同日后续记录 CLI 接入进展。
- 2026-10-20：确认 CLI / TUI binary 放根 `src-cli/`，业务 crate 保持在 `src-rust/`；采用 Ratatui / Crossterm，CLI 与桌面入口共享应用服务及数据根，通过独立终端事件 adapter 接收事件。
- 2026-10-20：Mistbell 增加中英本地化首行动建议并扩展 ScriptInfo；GUI 可将建议写入草稿，发送仍需玩家显式提交。
- 2026-10-20：issue #179 指出的浅色对白面板底色例外已写入 UI 规范，与 `.dialogue-panel` 的 `--overlay-solid` 实现一致。
- 2026-10-20：CLI/TUI 计费摘要通过 `aoidos-app::BillingService` 从共享 SQLite 读取；周目打开、回合执行期间每秒、回合终态、记录更新和事件溢出恢复时刷新，显示按供应商、原币与模型分组的已结算 / 预留金额、已知 Token、请求数、用量缺失和预算提醒；金额显示到 0.01，不做跨币换算。模型明细可在费用面板内滚动，窄终端按显示宽度截断。
- 2026-10-20：CLI fake Provider 集成测试发现费用摘要误用会话 `session_id` 查询账本；计费账本以 memory `run_id` 为作用域，已统一改用 `OpenedSession.memory_run_id`，并以真实账本适配器覆盖预算拒绝、预留、结算和 TUI 刷新。
- 2026-10-20：main CI 的 profile 配额测试因并行测试临时目录可能碰撞而偶发 `store.not-found`；改用进程号和原子序号生成名称，并以独占目录创建消除共享与误删。

## 完成摘要

未完成。
