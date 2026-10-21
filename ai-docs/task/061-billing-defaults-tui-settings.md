# 061 — 内置计价默认值与 TUI 账单设置

- 状态：done
- 依赖：035
- 优先级：P1
- 创建 / 更新：2026-10-21 / 2026-10-21

## 目标与背景

035 已交付不可变价格账本、GUI 价格登记与单局预算编辑，但没有随应用版本提供可确认的常用模型价格目录；新周目因此可能没有已选价格，也没有可用额度。058 的 TUI 只有账单摘要，导致用户必须离开 TUI 才能完成一次请求所需的价格选择与预算设置。补齐内置默认价目和共享账单服务后，玩家可自行选择 DeepSeek 账户使用 CNY 或 USD 价格，并在 GUI / TUI 确认内置价目、编辑本局额度。

## 必读

- [计价架构](../architecture/billing.md)
- [Rust / Web / 测试依赖方向](../standards/layering.md)
- [TS / Rust 边界](../architecture/ts-rust-boundary.md)
- [Rust 规范](../standards/rust.md)
- [前端规范](../standards/frontend.md)
- [测试规范](../standards/testing.md)
- [注释规范](../standards/comments.md)
- [CLI / TUI 共用运行层](058-cli-tui-shared-game-runtime.md)
- DeepSeek 官方中英文价格页：[CNY](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)、[USD](https://api-docs.deepseek.com/quick_start/pricing/)；价格和节假日表在实现时复核并记下查阅日期。

## 范围与非目标

- 在无状态依赖的 `aoidos-billing` 提供版本化内置价格模板；首版覆盖 DeepSeek `deepseek-flash` 的官方 CNY 与 USD 价格、峰谷时段和来源。模板不直接成为已登记价格，玩家选择币种并确认后才写入不可变账本并绑定 profile。
- 用户在配置 DeepSeek profile / 输入 API Key 的流程中选择 CNY 或 USD 账户计价币种，初始预选 CNY，玩家可改为 USD；该非敏感选择与 profile 的已选价格版本一致持久化，不写入 Keychain 或密钥文件。GUI 与 TUI 使用同一选择结果，之后不要求每局重复选择。
- GUI 与 TUI 均须在确认前展示当前币种的完整内置价目（缓存命中 / 未命中 / 写入、输出、峰时费率、核验日期及来源），并可确认该价目、保存本局 CNY / USD 上限；手工价格编辑保留在 GUI。额度建议继续由 profile 已选价格和既定参考 Token 数计算，缺失的另一币种价格不伪造建议值。
- 将价格登记、profile 计价币种选择、额度建议与预算写入封装成共享 `aoidos-app::BillingService` typed API；Tauri 命令与 CLI/TUI 仅负责输入、确认、呈现和错误适配，共用操作不直接访问账本、不复制账本规则。
- 已有 GUI 设置保持可用；手工登记和内置模板使用同一 PriceVersion 校验、不可变版本、来源与审计路径。
- 不按界面语言猜测计价币种；DeepSeek 用户在设置 profile / API Key 时默认预选 CNY，也可按账户扣款口径改选 USD。币种偏好按 profile 保存，不和密钥同存或从余额响应推断；币种不自动互换，预算分桶不互借。
- 不接入自动价格拉取、自动换汇、供应商账单同步或新的 Provider；不在 CI 调用真实 API。

## 设计起点与边界

- 本任务启动时，035 已交付 `BillingLedger` 和 GUI billing API，但内置价格目录为空，app billing service 仅提供摘要和预检读取。
- 本任务启动时，058 的 CLI/TUI 运行链路已通过仓库内 fake-provider 验证；其真实 DeepSeek API 验收等待价格 / 预算设置完成。061 已补齐该能力，真实请求由 058 单独验收。
- crate 依赖遵守[分层规范](../standards/layering.md)：`aoidos-app` 可新增对纯 `aoidos-billing` 的单向依赖；CLI/TUI 与 Tauri 不互相依赖，`aoidos-billing`、`aoidos-engine` 不依赖 app 或入口；更新 Cargo 依赖允许表与 `lint:layers` 后再改 manifest。
- DeepSeek 官方同时公布 CNY 与 USD 报价；设置默认预选 CNY，但需由用户确认账户实际币种，不把中文界面等同人民币，也不把任一报价当作实际扣款证明。

## 实施步骤

1. 核对 035 的计价、峰谷日历和现有 GUI 表单；建立带来源、核验日、时段版本的 DeepSeek CNY / USD 内置模板。
2. 在共享 app billing service 增加 catalog、价格登记 / 选择、建议额度和 budget 更新 typed API；应用事务和冲突语义复用 engine 的唯一账本，不新建 SQL 状态机。
3. 让现有 Tauri 命令调用共享服务；在 GUI 与 TUI 的 profile / API Key 配置流程加入 CNY / USD 明确选择、完整价目预览（含峰时、日期与来源）及内置模板确认；GUI 保留手工修改价格；费用设置面板继续允许编辑本局双币额度，保留发送前展示并再次读取账单状态的确认门。
4. 对齐 GUI / TUI 展示和输入校验，处理不可用模板、过期价格、价格版本冲突、预算 revision 过期及缺少缓存写入费率的模型；保持密钥不进入配置 API、TUI、日志或账本。
5. 增加无网络行为测试，验证两币种模板、用户选择、建议额度、保存与 stale revision、价格版本不可变、预算门禁和 GUI / TUI 使用同一 app API。
6. 更新计价架构、分层图、TS/Rust 边界、CLI 说明及任务索引；独立评审完整差异并运行仓库验收。

## 涉及改动

- `src-rust/aoidos-billing`：内置价目模板及纯数据 / 校验测试。
- `src-rust/aoidos-app`：共享计费设置 typed API；依赖 `aoidos-billing` 与现有 engine。
- `src-tauri`：计费命令变为 app 服务薄 adapter。
- `src-cli`：费用设置交互和 TUI 布局 / 行为测试。
- `src-web`：在 GUI 费用表单消费统一模板 / 额度建议；已有手工编辑保留。
- `ai-docs/architecture/billing.md`、`ai-docs/standards/layering.md`、`ai-docs/architecture/README.md`、`ai-docs/architecture/ts-rust-boundary.md`、058 task 与本 task。

## 验收标准

- [x] DeepSeek `deepseek-flash` 内置模板同时提供经官方来源核验的 CNY / USD 两套费率、峰谷 schedule、有效日期和价格来源；内置模板可随版本更新但不覆盖已登记历史。
- [x] GUI 与 TUI 的 DeepSeek profile / API Key 配置均允许用户明确选择 CNY 或 USD；确认前展示对应价目的缓存命中 / 未命中 / 写入、输出、峰时费率、核验日期和来源，再由用户确认并绑定到当前 profile；GUI 保留手工价格编辑；偏好作为非敏感计价元数据持久化，不根据界面语言或余额响应猜测币种。
- [x] GUI 与 TUI 均可编辑当前周目的 CNY / USD 上限；默认建议使用被选币种和既定参考 Token 预算计算，用户可覆盖，未选择的币种不填伪造价格或转换值。
- [x] DeepSeek 内置价格确认、profile 价格选择、价格读取、额度建议和单局额度读写均经同一个 `aoidos-app::BillingService`；Tauri 其他报表、结算恢复和历史补录命令仍可经 engine 账本接口执行，不复制账本规则。两个入口不直接执行账本 SQL，不引入 crate 环或 adapter 间依赖。
- [x] 价格确认生成不可变 PriceVersion；修改价格生成新版本，已冻结请求及历史 usage 不被改写；stale revision、失效 / 缺失模板及无可验证币种时失败关闭。
- [x] 所有真实请求继续经过逐物理请求的账本预留和用户确认；额度缺失 / 不足时 HTTP 不派发；默认模板不会让新周目静默发送。
- [x] TUI 操作覆盖窄终端与键盘路径；GUI / TUI 共享同一服务用例测试；测试不访问网络或密钥库。
- [x] 分层规范、架构图、TypeScript / Rust DTO、注释与 task 一致；独立评审完成且发现已处理；`bun run verify` 通过。

## 验证计划与结果

| 日期       | 环境 / 命令      | 预期                                                  | 实际结果                                                                                                                                                                                                                                            |
| ---------- | ---------------- | ----------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 2026-10-21 | `bun run verify` | 格式、lint、分层、Web / Rust 覆盖、视觉与构建检查通过 | 已通过；Web 52 个文件 / 461 个测试，Rust 覆盖 96.83%（39,114 / 40,395 行）；视觉回归及最终全量验收通过。此前一次复跑的回合终态竞争测试单独及随后全量复跑均通过；Tauri billing IPC 测试也已改为显式初始化 profile 服务，避免依赖其他测试的执行顺序。 |

## 风险与回退

内置价格可能因供应商更新而过时；每个模板必须保留核验时间和来源，显示提示并允许手工登记新版本，不后台抓价。GUI 与 TUI 若自行维护输入校验会产生差异，因此只允许 UI 做可访问性 / 编辑状态管理，业务校验、建议额度、币种约束与版本冲突统一交给 app / engine。

## 决策与工作记录

- 2026-10-21：TUI 真实调用预检确认默认 profile 为 `deepseek / deepseek-flash`，但价格和本局额度缺失；请求在凭据读取 / HTTP 之前被拒绝。用户确认 DeepSeek 应由玩家选择 CNY 或 USD，并要求 GUI / TUI 均可设置；新增本任务补齐计价默认值与共享入口服务。
- 2026-10-21：核对 DeepSeek 官方中英文价页，二者分别列出 USD 与 CNY 费率；DeepSeek 计价默认预选 CNY，用户可改选 USD，不按界面语言推断。
- 2026-10-21：内置 DeepSeek 价目只放在 `aoidos-billing/providers/deepseek.rs`；`aoidos-llm/providers` 继续只维护模型协议和能力，不复制价格数据。默认预选 CNY，用户可改选 USD。
- 2026-10-21：GUI / TUI 共用 `aoidos-app::BillingService` 处理内置价目确认、profile 价格选择、价格查询与单局预算；TUI 支持内置价目确认及预算编辑，手工价格编辑继续留在 GUI。TUI 的预算草稿绑定周目和 revision，切页 / 关闭 / 换局时取消；模型介绍绘制和鼠标行计算共用换行结果。
- 2026-10-21：整体复核发现确认前没有显示用户将登记的价格细节。现由 app 单一目录提供只读价格模板，GUI / TUI 展示费率、峰谷、核验日期和来源后再确认；TUI profile 鼠标命中按实际换行高度计算。完整验收已通过。

## 完成摘要

实现已覆盖唯一内置价目、GUI / TUI 价目预览与币种确认、单局双币上限、共享 app 计费操作和窄终端输入；独立复审发现的展示及鼠标行映射问题已修复，`bun run verify` 全项通过。真实供应商请求属于 058 后续验收，不由本任务重复发起。
