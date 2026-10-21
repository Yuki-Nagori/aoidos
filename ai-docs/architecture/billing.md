# LLM 计价与费用预算

更新日期：2026-10-21；官方资料核验日期：2026-10-21。[027](../task/027-llm-cost-control-design.md) 定义协议，[035](../task/035-llm-cost-control-impl.md) 接入运行时。当前实现包括 `aoidos-billing` 精确金额与读写缓存单价、engine SQLite 账本、LLM 物理尝试预算端口、Tauri 命令及双语费用面板。CLI/TUI 通过共享 `aoidos-app::BillingService` 查看所选 profile 的有效价格、确认内置 DeepSeek 价目并编辑当前周目双币上限；提交玩家行动、骰判后继续及恢复操作均须先预览并显式确认，确认时重新读取 profile、价格和额度。应用内部的传输重试及后续生成阶段仍由既定回合调度器管理，每个物理尝试分别经账本预留，不把一次玩家确认等同一次 HTTP 请求。缓存写入 Token 和费率已独立持久化；旧价格的写入费率保留为未知，并阻止新请求直到用户补齐。OpenCode Go 与 DeepSeek API 的峰谷规则已按发送时刻计价，峰时单价随价格版本冻结；界面把官方 UTC 窗口转换到已选报表时区显示。OpenCode Go / Go Plus 订阅费不纳入 Aoidos 费用面板或账本；Go 的每模型 Token 费率只用于估算其订阅额度使用。完整历史用量仅在原请求冻结价格仍可验证时允许预览并恢复；DeepSeek 余额仅在玩家主动查询时调用官方余额 API，其他供应商当前没有余额适配器。以上均不是实际账单对账。调用优先级、单次硬限与回合归 [LLM 架构](llm.md)，精确展示归[国际化](i18n.md)，凭据归 019；本文区分稳定协议与当前实现范围。

结局规则生成（仅采用 LLM 时）和可选 AI 结局叙事都是独立的物理请求，必须复用 027 / 035 的预留、结算和 unknown 恢复；固定事实摘要与周目封存不依赖叙事请求成功。叙事的显式重试是新的物理尝试，unknown 不自动重发；订阅费用不进入 Aoidos 账本。完整流程见[结局架构](ending-flow.md)，由 056 接入。

061 已接入 DeepSeek Flash CNY / USD 内置价目及 GUI / TUI 共用的登记服务。当前价目快照唯一位于 `aoidos-billing/providers/deepseek`；`aoidos-llm/providers` 只维护协议、能力与模型上限。app 只读发布模板供 GUI / TUI 预览，不另存价格目录。DeepSeek 新 profile 的币种默认预选为 CNY，玩家可在 API 配置时改为 USD；默认预选不会登记价格或自动放行请求，必须先查看缓存命中 / 未命中 / 写入、输出与峰谷费率、核验日期和来源，再明确确认。模板更新不改写账本中已冻结的历史版本。

## 控制单位与默认额度

每局游戏分别设置可修改的 CNY 与 USD 原币上限；应用不设全局月度消费上限。每笔请求按服务商该计价身份的原币费用占用本局同币种额度，不换算、不互借。全应用自然月只提供按服务商和原币种分组、模型作为下钻维度的费用 / Token 统计，不跨币相加。Token 是原始用量与单次硬限，不建立累计 Token 额度，也不以记忆模块另设额度。

每种原币额度的默认值分别由对应参考模型、默认输入 / 输出 Token 数及该模型原币价格计算；未指定参考模型时取首个以该币种计价且登记成功的模型。缺必要价格先登记，不生成零默认。首版开发参考用量为每局输入 2,000,000 / 输出 500,000 Token，按缓存未命中及最高时段价格计算，向上取到该币种最小单位。该初始金额可编辑，不是普通对局实测消耗或模型能力保证，发布前 035 验证提示与合理性。模型切换和价格更新不改变当前局限额。

每局保存 CNY 与 USD 两个独立上限、默认模板与来源快照。模型切换、价格更新和界面语言切换不重算上限；玩家可在开局时分别编辑两种币种额度，也可调整当前局上限，设置变更提交后立即生效。在飞请求保留已经提交的预留与冻结价格；修改额度不返还已发生费用。上限为非负金额、最多 9 位小数且可表示；零额度仅允许对应币种零价且其他门禁合法的请求，零价必须显式登记。

## 价格身份、来源与更新

计价主体由实际请求经过的服务商接入方式、API 部署身份、精确模型 ID 和路由策略共同确定；兼容协议不等于同一计价主体。同一模型直连服务商与经 OpenRouter 调用是不同价格身份，不能互相套价。凭据只保留 019 的引用，不混入价格缓存。配置持久化在 SQLite，内存缓存以 providerId / modelId / configRevision 为键，登记或修改成功后替换缓存，重启从已校验配置恢复；不涉及登录或云同步。

不可变 PriceVersion 保存 versionId、providerId / modelId / routePolicy、priceCurrency、unitTokens、inputUncached / inputCachedRead / inputCachedWrite / output 单价、usageMappingVersion、sourceKind / sourceUrl / checkedAt、effectiveFrom / validUntil、时段 / 日历版本与配置摘要。`priceCurrency` 表示此计价身份实际采用的价格表币种，不由界面语言推导；服务商账户存在多币种或官方同时发布多币种价格时，只有账户实际计费币种可验证，或用户明确登记后才可启用。单价为十进制字符串，非负、最多 9 位小数；unitTokens 为正安全整数。缓存写入 / 读取价格相同须显式指定，缺字段不能填零。每次请求保存冻结版本，不只保存可变配置引用。

checkedAt、effectiveFrom、validUntil 和 dispatchAt 必须是可解析的 RFC 3339 时间戳；有效期按实际 UTC 时刻比较，采用 `[effectiveFrom, validUntil)` 半开区间，不能按原始字符串或界面时区比较。价格查询先按服务商、模型、路由和币种筛选，再由 Rust 精确比较完整时刻，避免 SQLite 日期函数在亚毫秒边界选错版本。每组计价身份最多保留 1,024 个不可变价格版本；新版本超限时拒绝登记，已存在版本的幂等重试仍成功。账本按绝对时间归属月报，月界的小数秒仍属于其实际月份。

价格来源按实际接入路径选择：服务商直连使用该服务商核验过的官方价格；OpenRouter 接入使用 OpenRouter 对应模型 / 路由策略的价格记录；OpenCode Zen / Go 使用各自官方模型目录中的价目。OpenRouter 与 OpenCode Zen / Go 是不同计价身份，不能互相套价，也不能替代同模型的直连价格。OpenRouter [按上游价格路由](https://openrouter.ai/blog/insights/model-routing/)，模型目录报价只适用于其所声明的接入身份；允许的上游价格不同且本次实际上游无法确定时，预留必须覆盖允许路由的保守上界，否则固定路由或要求用户登记明确费率，不能用单一低价放行。OpenRouter 的[计费说明](https://openrouter.ai/support)表明其额度以 USD 计、推理价格透传上游。价格与路由身份无法可靠匹配时，阻止收费请求。

OpenCode Go / Go Plus 是固定月订阅；官方文档列出套餐及各模型额度规则。Aoidos 不登记或展示固定订阅费用，只按已核验的 Go 每模型 Token 费率估算额度使用；这不是额外按量扣费或供应商的实际账单。若启用 Go 的 Zen balance fallback，只有服务商提供可验证 usage / 路由证据时才单列为 Zen 费用，不能仅从 Go usage 猜出实际余额扣款。模型档位与可用额度会变化，费率须有来源及核验日期。[OpenCode Go pricing and fallback](https://opencode.ai/v2/docs/console/go)。

OpenCode Go 的公开 [`/zen/go/v1/models`](https://opencode.ai/zen/go/v1/models) 返回模型 ID 列表；Console 的 [`GET /api/v2/config`](https://opencode.ai/v2/docs/console/api/) 返回工作区可用 provider / model 配置。两者都不是计费或发票接口：模型列表不提供费率、协议、上下文与实际订阅用量，配置接口也不提供账单金额。Aoidos 可用接口目录辅助核对模型可用性，但新增模型仍须核验协议与能力后方可调用；费率仍需本地版本化保存，才能按请求 Token 和时段执行预算估算。

官方价格有多种币种报价时，价格币种必须和用户实际账户的扣款口径对应。DeepSeek 官方中英文价格页分别以 USD 与 CNY 发布报价，[余额 API](https://api-docs.deepseek.com/api/get-user-balance/)也可能返回 CNY / USD 余额；因此不能用界面语言选择其价格表。若接口无法证明具体账户的计价口径，首次登记时须由用户确认，保存来源和核验日期；确认的是费用估算口径，不保证与实际账单完全相同。OpenRouter 采用 USD 额度，价格透传上游；如果未来读取其 generation 费用记录，应作为 OpenRouter 接入的实际费用证据单独建模，不混同于目录价或服务商直连账单，本期仍按冻结价格版本和 usage 估算，不额外查询账单接口。

内置价格目录随应用版本维护，优先登记已核验主流供应商 / 模型，每条有官方来源 / 日期 / 生效范围；不宣称自动覆盖任意自建端点。用户首次确认内置条目也形成已登记配置；未登记 / 缺价 / 已过 validUntil 先补齐或显式重新确认。内置更新只提供新版本供后续确认，不覆盖手动设置；撤销后禁止新请求，历史仍可查。手动版本可无到期日但显示登记日期与过期风险，确认不是官方价格准确性保证。

价格、节假日数据均不可变；修改产生新版本，未知版本 / 损坏配置失败关闭，不自动套其他模型。原始 Token 用量另存，不因价格变化改写。

## 用量归一化与时段

保存原始、受界限约束的 usage 与归一化字段：inputTotal、inputCachedRead、inputCachedWrite、inputUncached、outputTotal、reasoningIncluded、totalTokens；未知项省略。inputUncached = inputTotal - inputCachedRead - inputCachedWrite，缓存明确不支持时两项为 0。reasoning 作为输出子集展示，不再重复计费；供应商若分开返回推理 Token，适配器先按其冻结映射并入 outputTotal。完整 usage 缺失或计数无法安全转换、持久化时不虚构 usage，保持 unconfirmed；完整数值已进入恢复队列但归一化或计价校验失败时保持 reserved 并显示诊断。总 Token = inputTotal + outputTotal，不重复加缓存或 reasoning。

PriceVersion 可附 IANA 时区、星期、半开时间窗、时段单价与版本化节假日表；跨午夜拆窗，重叠、空价和非法时区拒绝。优先级为显式日期覆盖 → 星期 / 时间窗 → 基准价。日历声明覆盖年份及调整工作日策略；未覆盖 / 无法确定采用可适用最高价并标 estimated，不默认为假日优惠。内置日历随版本更新，手动覆盖明确来源；不自动推断节假日。

DeepSeek API 的 [USD 价格](https://api-docs.deepseek.com/quick_start/pricing/)与[人民币价格](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)均有峰谷价：周一至周五 UTC 01:00–04:00 / 06:00–10:00（北京时间 09:00–12:00 / 14:00–18:00），中国公众假日全天非峰时，谷价为峰价一半。2026 年公众假日表依据[国务院办公厅通知](https://www.gov.cn/zhengce/zhengceku/202511/content_7047091.htm)，价格日历版本为 `deepseek-api-utc-weekday-holiday-2026-v1`；未覆盖年份不推断假日折扣，峰时按较高价格保守预留，官方新日历发布后新增版本。OpenCode Go 对 DeepSeek V4.1 Flash / V4 Pro / V4 Flash / Vision Exp 列出独立峰谷费率；其官方时段相同，但只明确周末全天非峰时，不排除中国公众假日。两种接入分别登记独立费率和日历版本。每笔按 `dispatchAt` 选择单价并冻结版本；预留覆盖可适用最高费率。设置界面以本周为基准，将官方 UTC 窗口转换到已选报表时区；这只改变说明文字，不改变费率判定时区。

## 金额、原币与单局分币种额度

内部 NanoMoney 是 i64 的币种 × 10^-9；单项、同币种聚合、乘法全部检查溢出，拒绝不能表示的配置或预留。不同 currency 的 Money 禁止比较、相加或汇总。Rust 用精确整数分子和 checked i128 中间计算，禁止 f64；IPC 金额为十进制字符串 + 明确 currency，前端不计算额度。费用按有理数合计，不逐 Token 舍入累积误差。预留向上到 nano，结算 HALF_UP 到 nano，记录舍入政策版本。预算与费用界面统一显示到 0.01；显示时以整数分精确舍入，不回写账本或用于预算比较。DeepSeek 余额界面同样以精确格式显示原币金额，并按显式报表时区格式化查询时间；计价单价可在诊断明细显示更高精度。

请求原币费用估算 = (inputUncached × inputUncachedPrice + inputCachedRead × inputCachedReadPrice + inputCachedWrite × inputCachedWritePrice + outputTotal × outputPrice) / unitTokens。费用金额、币种和 PriceVersion 一经结算永久按服务商原币保存。每局 CNY / USD 上限独立配置与校验；请求只占用其原币对应的额度，没有汇率换算、跨币总额或额度互借。原币额度与原币费用估算都来自服务商计价版本，不代表供应商实际账单的保证。服务商支持多种实际扣款币种时，模型登记选择与账户扣款口径一致的 PriceVersion；DeepSeek 的币种由实际账户选择，OpenRouter / OpenCode 使用 USD。界面语言只控制数字 / 货币格式，不决定模型计价币种或预算额度。

## 持久账本与发送门禁

物理 requestId 每次网络尝试唯一，关联 turnId / roundId / runId、taskKind、profileRevision、priceVersion、usageMappingVersion、priceCurrency、dispatchAt 与 runBudgetId。重试是新的物理身份并重新预算，重放响应仍是同一身份。所有剧情、整理与已授权记忆请求共用端口，主流程优先；可选后台不足跳过，必要请求不足暂停推进。本地计算、快照恢复继续，恢复不自动重发收费请求。

SQLite 表职责如下，具体迁移编号由 store 服务统一分配，业务 crate 不另持锁：

| 已实现表                        | 主键 / 约束                     | 职责                                           |
| ------------------------------- | ------------------------------- | ---------------------------------------------- |
| `billing_price_versions`        | 不可变 `version_id`             | 用户登记的原币价格版本                         |
| `billing_active_profile_prices` | `profile_id` 唯一               | profile 明确选择的 provider / route / 币种版本 |
| `billing_run_budgets`           | `run_id` 唯一                   | CNY / USD 独立上限与设置 revision              |
| `billing_physical_requests`     | `request_id` 唯一               | 冻结请求身份、用量、原币费用与预留状态         |
| `billing_budget_warnings`       | run + currency + threshold 唯一 | 80% 提醒去重事实                               |
| `billing_audit`                 | `operation_id` 唯一             | 价格、币种选择及上限修改审计记录               |
| `billing_meta`                  | 单行主键                        | 账本 revision 与持久化月报时区                 |

当前以 `billing_physical_requests` 的状态及 `reserved_nanos` 表达预算占用；缓存写入用量和 `billing_price_versions.schedule_json` 由追加迁移保存，旧写入费率 / 时段保持 NULL。`billing_pending_settlements` 是有界恢复 outbox，只保存最多 256 条待恢复请求的数值用量、摘要和稳定诊断码，不保存正文、密钥或供应商原始响应。尚无独立预算分配表或历史补价来源；不能把设计表名误认为已创建的 schema。engine 在共享 SQLite 连接上按顺序应用 `aoidos-billing::schema::SCHEMA` 及其追加迁移，store 不依赖计费领域。

发送前持单在飞租约，在 BEGIN IMMEDIATE 事务中校验 PriceVersion 与当前 runBudget，按请求原币选择 CNY 或 USD 限额，判断该币种 settled + reserved + unconfirmed + 本次预留 <= 单局上限，并原子提交原币请求与对应分配。输入预留逐次按冻结 ProviderInput 的完整 JSON 字节长度加 1 KiB 请求封套余量计算，并受模型上下文窗口上限约束；`PreparedGeneration` 在派发前同时限制原文和序列化输入不超过 256 KiB，避免为短请求预留整个模型窗口。此值是本地请求的保守预留，不替代供应商账单用量；结算仍以实际 usage 为准。为保证已发送请求的完整用量有界可恢复，未决物理请求与待结算 outbox 按 requestId 去重后共用 256 个容量名额，达到上限时阻止新请求。任一币种未配置上限或额度不足则拒绝且不发 HTTP；不能借用另一币种额度。比较与修改使用整数，不能先查后写。dispatchAt 为持久发送意图时刻，紧接发送；长时间排队在登记前结束；调试请求必须显式关联合法测试周目，不创建绕过单局额度的虚拟预算。网络不可与 SQLite 原子提交，崩溃在登记与发送之间仍可能消耗未知，恢复标 unconfirmed，不以“没看到响应”推断没发送。时段归属按该持久边界，是本地可审计约定，不能声称精确等于供应商收件时刻。

状态机为 reserved → settled / unconfirmed；unconfirmed 只有获得可核验完整 usage 或显式人工审计处置才能 → settled。预算已预留、但取消明确发生在 `provider.start` 调用前时，以 `notDispatched` 和零 usage 结算并释放预留；进入 provider 调用后，超时 / 取消 / 断流 / 缺 usage 都不能推断为未发送或退款。零 usage 只有供应商完整合法报告或上述发送前证据才接受。人工处置必须明确实际费用或保留未知，记录理由与审计，不自动把未知抹成零。

结算在同一 BEGIN IMMEDIATE 中按 requestId + 旧状态 CAS，保存 usage / 原币费用、对应币种分配及统计，释放剩余预留；相同用量重放是 no-op，冲突用量保留首次结果并留下可见诊断，不覆写。完整 usage 先以数值字段写入有界 `billing_pending_settlements`，再从持久化用量与请求冻结的价格版本结算；结算失败时保留预留与 outbox，明细显示稳定诊断码并允许玩家显式重试。重试只访问本地 SQLite，不请求供应商 API、不重新发送生成请求；恢复成功后才清除 outbox。未决请求与 outbox 按 requestId 去重，共用 256 个容量名额。若数据库本身不可写，系统无法保证持久化新收到的用量，账本保留未知占用并拒绝把它显示成已结算。原始响应摘要可核验，不保存 reasoning / 密钥或无限正文。完整 usage 费用仍是按本地版本计算，显示 estimated，与未知 unconfirmed 区分。

账本结算状态与正文终态独立：输出完整合法但 usage 缺失、无法安全持久化或无法核验时，正文仍按 LLM 协议完成，账本保留未知占用；只有完整数值用量已写入恢复队列时才保持 reserved 并显示结算待恢复，否则标为 unconfirmed。此类账务问题不撤回已提交正文、不伪造 finishReason，也不通过重生成获取 usage。

原币费用按实际用量完整记账；若该币种费用超过对应单局上限，显示该币种超额并拒绝后续该币种请求，不截断、隐式充值或丢掉 usage。额度修改即时生效、与预留同事务顺序；已经发送的请求按冻结价格版本完成，新的限额不能回滚已发生费用。价格修改不改在飞请求。迟到结算更新原币费用及原 runBudget；无法确认事务提交结果先按 requestId 查询，不再次结算或重发。

## 周期、时区与提醒

单局上限绑定稳定 runId；新局创建新预算，恢复 / 回退 / 段切不刷新或退还费用。全应用月报覆盖全部运行与任务，按服务商和原币分组、模型作为下钻明细，不设月额度。月报时区取系统有效 IANA 值并持久化，无法确定回退 UTC 并告知；设备后来变化不覆盖。jiff 负责自然月报表边界、DST 和时区库版本。

月报按请求 dispatchAt 与已保存报表时区归类，不创建月度额度记录、不因跨月刷新或释放预算。SQLite 只筛选覆盖月份边界的有界候选区间，最终成员关系由 Rust 按 RFC 3339 瞬间精确判断，避免 `julianday()` 的毫秒舍入改变月界归属。月度统计可按需查询，不需要后台计时或持续轮询。DST 的不存在午夜采用当日最早合法瞬间，重复午夜取最早瞬间；墙钟异常按本地时间标注统计边界，但不影响单局冻结预算。

玩家修改报表时区只影响新查询如何归类请求；已经展示的历史数据按其记录的时区版本可复现。时区变化不重设单局上限，也不改变消费 / 币种。

80% 提醒按本局 CNY、USD 两个上限分别计算；只有对应币种已配置非零上限时才提醒。结算 / 补价 / 修改上限在事务内建立唯一 warning；可先持久后 UI 主动读取，丢事件也不丢事实。读过 / 调高 / 调低 / 换模型不删去重键，每局每币种同阈值只提示一次；重新打开界面可查看状态，不反复弹窗。预留与未知单列，占用对应币种本局可用额度；80% 不替代请求前拒绝。估算低于本局限额仍可能超出供应商实际扣款。

## 查询、历史与错误

查询按 runId 或自然月，先按 providerId 与 priceCurrency 分组，再支持 modelId 下钻；原币费用按币种展示，不存在跨币估算或合计。input / output / total / cached / reasoning / 请求次数均可见。月报同一服务商、同一币种内合计；未知用量数量、未确认占用、未计价小计显式列出，不展示零。列表复用 IPC 50 默认 / 200 最大分页，游标冻结查询 revision / 截止序号，后续页旧 revision 返回 app.bad-request（detail.reason=staleRevision）；不把账本全集塞进回合事件。

历史恢复只接受 `unconfirmed`、完整保存数值 usage、尚未进入结算队列且仍能读取原 `priceVersionId` 的单条请求。预览使用原冻结费率和原 `dispatchAt`，提交前核对 ledger revision，再以 audit `operationId` 幂等写入本地恢复队列；成功后复用结算恢复路径。不能导入外部账单、选择较新的费率重算已结算记录或补造缺失 usage。缺原价格或任一必要 usage 字段时不提供候选项，历史仍显示未确认。

已注册 `budget_get_settings / budget_set_settings / budget_get_period / budget_get_monthly / budget_list_requests / budget_get_suggested_limits / budget_get_price / budget_register_price / budget_get_selected_price / budget_select_price / budget_retry_settlement / budget_preview_backfill / budget_commit_backfill / budget_get_provider_balance`，同型 Rust / TS 载荷见 `src-tauri/src/billing_commands/` 与 `src-web/api/billing.ts`。设置写入支持 expectedRevision；请求页限制 200 条并绑定 ledger revision，账本变更后游标失效。`budget_list_model_usage` 由周期摘要返回分组结果。余额命令只允许 DeepSeek profile，不能改变本地预算门禁或账本。

错误 code 独立登记为 budget.exceeded（runBudgetId / currency / estimatedNeeded / available）、budget.run-limit-missing（runId / currency）、budget.price-missing（providerId / modelId / missingFields）、budget.price-history-limit（单一计价身份的价格版本达到 1,024 个上限）、budget.settlement-backlog（恢复队列达到 256 条上限）、budget.conflict（账本请求与已有操作状态冲突）、budget.invalid-usage（requestId），均为受约束结构化 detail，不含密钥 / 原始响应。参数错为 app.bad-request，旧设置 / 查询为 app.bad-request（detail.reason=staleRevision），计费命令的数据库存储失败映射为 `store.database`（由命令层构造，不是 `StoreError` 的通用映射）。供应商实际额度拒绝是 llm.quota，不与本地超限混用；已有公共 OutcomeError 只有 code / message 的地方保持同型，不为计费单独塞 detail。前端按码本地化并保留 default。

## 可选供应商余额

余额是参考信息，不替代本地账本或让预算拒绝放行。当前只对 DeepSeek profile 开放显式查询，返回 `{ providerId, queriedAt, stale, status, accounts }`；status 为 available / unavailable / unknown，accounts 保留 DeepSeek CNY / USD 总额、赠送与充值十进制字符串。缓存键由 profile 配置和凭据摘要构成，但摘要不会离开进程；profile 或 API key 变化不会复用旧缓存。查询串行单飞，内存缓存最多 50 项。OpenCode Zen / Go 没有本期支持的公开余额适配器；不能从未知供应商 URL 猜接口或抓网页。

其他供应商的公开接口仅作未来评估，不代表产品已接入：

| 供应商                | 接口与来源                                                                                           | 身份 / 金额边界                                                                |
| --------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| DeepSeek（已接入）    | [GET /user/balance](https://api-docs.deepseek.com/api/get-user-balance/)                             | is_available 与 CNY / USD 字符串分别显示，不把不同币种相加；不需要额外管理密钥 |
| Kimi（未接入）        | [GET /v1/users/me/balance](https://platform.kimi.com/docs/api/balance)                               | 启用前须确认所支持主机、业务 status / code 与账户币种                          |
| SiliconFlow（未接入） | [GET /v1/user/info 官方 OpenAPI](https://github.com/siliconflow/siliconcloud/blob/main/openapi.yaml) | 余额币种语义须先核实；不将昵称 / 邮箱送前端                                    |
| OpenRouter（未接入）  | [GET /api/v1/key](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-api-key)         | 需验证密钥权限与余额语义；null 上限不代表账户无限余额                          |

DeepSeek 以外的供应商余额尚未接入；这不表示供应商永久不存在其他管理接口。供应商数值 JSON 在 Rust 保留数字词法并用 Decimal 解析，不能先经 f64；适配器必须有核验过的币种 / 可用性语义。未知币种、负值语义或解析异常是 unknown，不改账本。

查询仅由玩家点击发起，没有后台刷新、定期对账或 token 事件请求。DeepSeek 余额响应和 unknown 结果在内存缓存；每个配置最多每 5 分钟查询一次，失败指数退避至 1 小时，最多一次余额 HTTP 请求在飞。过期缓存带 `stale` 标记；profile / key 变化使用不同的缓存身份，卸载 UI 不会取消已发送的单次查询，但迟到结果由前端 generation guard 丢弃。响应体 / 超时沿用有界 HTTP 纪律。

余额查询不返回 `llm.quota`，也不阻断生成；不可用 / 未知状态只作为供应商余额信息显示。当前没有账本偏差自动对账或充值 / 赠送推断，不补写账本或自动变更预算。

## 实施与验证依据

035 已交付价格登记与原币账本、单局预算门禁、统一物理调用端口、费用设置 / 明细及月报、缓存创建 Token、峰谷计价、有界结算恢复、受限历史恢复和 DeepSeek 显式余额查询；三平台实机 UI 展示证据仍待补，不代表真实账单对账。接入 018–020 / 025 / 034，030 / 033 可消费已实现的计费端口。产品不购买额度、不读取供应商逐请求账单、不追踪订阅费用、不使用实时外汇、不设全局月度上限、不隐藏额外收费调用。此前“参考 Token 额度”措辞由费用额度规则取代。

[足精度 Decimal](https://docs.rs/rust_decimal/latest/rust_decimal/)与 [jiff 时区](https://docs.rs/jiff/latest/jiff/)在实现时核验并集中 workspace 版本，不在设计提交中安装。字节输入估计必须包含完整序列化输入、协议固定开销与输出 / reasoning 上限；供应商适配器声明可证明的上界 / 保守系数，不能对任意 tokenizer 宣称 1 byte = 1 Token 永远成立。不能给出可靠上界时按模型输入硬限预留；超出估计完整结算且停新请求。未来 tokenizer 接入另行授权 / 验证，不暗加网络计数调用。

验证重点是同请求重复 / 冲突回调、单局 CNY / USD 上限并发不足、登记与发送崩溃、结算提交不确定、无 usage / 取消、服务商币种归属、跨月报表 / 时区 / DST、价格与单局上限修改、溢出舍入、补价 / 分币种预警、分页 revision 和余额权限 / 缓存。单局原币上限估算不等于供应商账单；参考额度不是性能实测。三平台设置和精确展示由实施留证，最终 bun run verify 十三项通过。

## 周目删除与账务义务

[059 周目管理设计](../task/059-run-management-design.md)确认，canonical `billingRunId` 为 `sessionId`，不对应独立生成的 `memoryRunId`。现有剧情请求使用 `sessionId`，030 私有记忆请求曾使用 `memoryRunId`；060 将后者修正，并通过 `memory_run_sessions` 的精确关联迁移旧 scope。迁移会保留 requestId / pending settlement，合并 warnings；预算冲突将停止收费，直到玩家明确保存 canonical 上限。目录显式保存并核验 canonical `billingRunId`；清理还须覆盖迁移前的已验证 legacy scope，不依赖外键级联或猜测 UUID 映射。没有预算和请求的周目属于尚未配置，不是账本损坏。

本局存在 `reserved` / `unconfirmed` 物理请求或 `billing_pending_settlements` outbox 时拒绝删除，须先解决费用义务。已有完整持久 usage 的请求可走本地结算恢复；当前没有任意费用核销入口，`unconfirmed` 若缺完整 usage 且无供应商回执等可核验证据，只能保留，不能手动改为 settled 或删除。删除不能把未知用量当零或从月报隐去未确认请求。所有请求已结算后，玩家显式确认才一并清除本局预算、预警、请求明细与可精确归属的本地账单审计：仅清理 target 明确关联到已核验周目 scope 的预算 / warning 操作，以及其已核验物理请求的结算审计；价格登记、模型选择和归属不明记录保留。共享价格版本、profile 价格选择及供应商侧账单不受影响。该清除不保证从全库 SQLite 备份物理擦除字节。
