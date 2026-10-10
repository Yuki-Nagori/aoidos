# LLM 计价与费用预算

更新日期：2026-10-11；官方资料核验日期：2026-10-10。[027](../task/027-llm-cost-control-design.md) 设计定稿，实施由 [035](../task/035-llm-cost-control-impl.md) 承接，当前无计费运行时。调用优先级、单次硬限与回合归 [LLM 架构](llm.md)，精确展示归[国际化](i18n.md)，凭据归 019；本文是价格、账本与费用周期的精确协议。

## 控制单位与默认额度

每局游戏分别设置可修改的 CNY 与 USD 原币上限；应用不设全局月度消费上限。每笔请求按服务商该计价身份的原币费用占用本局同币种额度，不换算、不互借。全应用自然月只提供按服务商和原币种分组、模型作为下钻维度的费用 / Token 统计，不跨币相加。Token 是原始用量与单次硬限，不建立累计 Token 额度，也不以记忆模块另设额度。

每种原币额度的默认值分别由对应参考模型、默认输入 / 输出 Token 数及该模型原币价格计算；未指定参考模型时取首个以该币种计价且登记成功的模型。缺必要价格先登记，不生成零默认。首版开发参考用量为每局输入 2,000,000 / 输出 500,000 Token，按缓存未命中及最高时段价格计算，向上取到该币种最小单位。该初始金额可编辑，不是普通对局实测消耗或模型能力保证，发布前 035 验证提示与合理性。模型切换和价格更新不改变当前局限额。

每局保存 CNY 与 USD 两个独立上限、默认模板与来源快照。模型切换、价格更新和界面语言切换不重算上限；玩家可在开局时分别编辑两种币种额度，也可调整当前局上限，设置变更提交后立即生效。在飞请求保留已经提交的预留与冻结价格；修改额度不返还已发生费用。上限为非负金额、最多 9 位小数且可表示；零额度仅允许对应币种零价且其他门禁合法的请求，零价必须显式登记。

## 价格身份、来源与更新

计价主体由实际请求经过的服务商接入方式、API 部署身份、精确模型 ID 和路由策略共同确定；兼容协议不等于同一计价主体。同一模型直连服务商与经 OpenRouter 调用是不同价格身份，不能互相套价。凭据只保留 019 的引用，不混入价格缓存。配置持久化在 SQLite，内存缓存以 providerId / modelId / configRevision 为键，登记或修改成功后替换缓存，重启从已校验配置恢复；不涉及登录或云同步。

不可变 PriceVersion 保存 versionId、providerId / modelId / routePolicy、priceCurrency、unitTokens、inputUncached / inputCached / output 单价、usageMappingVersion、sourceKind / sourceUrl / checkedAt、effectiveFrom / validUntil、时段 / 日历版本与配置摘要。`priceCurrency` 表示此计价身份实际采用的价格表币种，不由界面语言推导；服务商账户存在多币种或官方同时发布多币种价格时，只有账户实际计费币种可验证，或用户明确登记后才可启用。单价为十进制字符串，非负、最多 9 位小数；unitTokens 为正安全整数。缓存不区分时显式指定同价，缺字段不能填零。每次请求保存冻结版本，不只保存可变配置引用。

价格来源按实际接入路径选择：服务商直连使用该服务商核验过的官方价格；OpenRouter 接入使用 OpenRouter 对应模型 / 路由策略的价格记录。OpenRouter 价格不作为全项目模型基准，也不能替代同模型的直连价格。OpenRouter [按上游价格路由](https://openrouter.ai/blog/insights/model-routing/)，模型目录报价只适用于其所声明的接入身份；允许的上游价格不同且本次实际上游无法确定时，预留必须覆盖允许路由的保守上界，否则固定路由或要求用户登记明确费率，不能用单一低价放行。OpenRouter 的[计费说明](https://openrouter.ai/support)表明其额度以 USD 计、推理价格透传上游。价格与路由身份无法可靠匹配时，阻止收费请求。

官方价格有多种币种报价时，价格币种必须和用户实际账户的扣款口径对应。DeepSeek 官方中英文价格页分别以 USD 与 CNY 发布报价，[余额 API](https://api-docs.deepseek.com/api/get-user-balance/)也可能返回 CNY / USD 余额；因此不能用界面语言选择其价格表。若接口无法证明具体账户的计价口径，首次登记时须由用户确认，保存来源和核验日期；确认的是费用估算口径，不保证与实际账单完全相同。OpenRouter 采用 USD 额度，价格透传上游；如果未来读取其 generation 费用记录，应作为 OpenRouter 接入的实际费用证据单独建模，不混同于目录价或服务商直连账单，本期仍按冻结价格版本和 usage 估算，不额外查询账单接口。

内置价格目录随应用版本维护，优先登记已核验主流供应商 / 模型，每条有官方来源 / 日期 / 生效范围；不宣称自动覆盖任意自建端点。用户首次确认内置条目也形成已登记配置；未登记 / 缺价 / 已过 validUntil 先补齐或显式重新确认。内置更新只提供新版本供后续确认，不覆盖手动设置；撤销后禁止新请求，历史仍可查。手动版本可无到期日但显示登记日期与过期风险，确认不是官方价格准确性保证。

价格、节假日数据均不可变；修改产生新版本，未知版本 / 损坏配置失败关闭，不自动套其他模型。原始 Token 用量另存，不因价格变化改写。

## 用量归一化与时段

保存原始、受界限约束的 usage 与归一化字段：inputTotal、inputCached、inputUncached、outputTotal、reasoningIncluded、totalTokens；未知项省略。inputUncached = inputTotal - inputCached，缓存明确不支持时 inputCached = 0。reasoning 作为输出子集展示，不再重复计费；供应商若分开返回推理 Token，适配器先按其冻结映射并入 outputTotal。缺失计价必需项或负值 / 不一致 / 溢出不得虚构 usage，进入 unconfirmed。总 Token = inputTotal + outputTotal，不重复加缓存或 reasoning。

PriceVersion 可附 IANA 时区、星期、半开时间窗、时段单价与版本化节假日表；跨午夜拆窗，重叠、空价和非法时区拒绝。优先级为显式日期覆盖 → 星期 / 时间窗 → 基准价。日历声明覆盖年份及调整工作日策略；未覆盖 / 无法确定采用可适用最高价并标 estimated，不默认为假日优惠。内置日历随版本更新，手动覆盖明确来源；不自动推断节假日。

2026-10-10 核验的 [DeepSeek USD 价格](https://api-docs.deepseek.com/quick_start/pricing/)与[人民币价格](https://api-docs.deepseek.com/zh-cn/quick_start/pricing/)均有峰谷价，周一至周五 UTC 01:00–04:00 / 06:00–10:00（北京时间 09:00–12:00 / 14:00–18:00），中国公众假日除外，谷价为峰价一半。该实例只说明价格来源和时段结构，不作为永久价格常量。当前页面未承诺长请求的时段归属；本地按 dispatchAt 选价是估算假设，详情显示该依据。预留按该版本所有可适用时段最高价，不因为预计落在谷时就降低预留。

## 金额、原币与单局分币种额度

内部 NanoMoney 是 i64 的币种 × 10^-9；单项、同币种聚合、乘法全部检查溢出，拒绝不能表示的配置或预留。不同 currency 的 Money 禁止比较、相加或汇总。Rust 用精确整数分子和 checked i128 中间计算，禁止 f64；IPC 金额为十进制字符串 + 明确 currency，前端不计算额度。费用按有理数合计，不逐 Token 舍入累积误差。预留向上到 nano，结算 HALF_UP 到 nano，记录舍入政策版本。预算、费用和余额界面统一显示到 0.01；显示时以整数分精确舍入，不回写账本或用于预算比较。计价单价可在诊断明细显示更高精度。

请求原币费用估算 = (inputUncached × inputUncachedPrice + inputCached × inputCachedPrice + outputTotal × outputPrice) / unitTokens。费用金额、币种和 PriceVersion 一经结算永久按服务商原币保存。每局 CNY / USD 上限独立配置与校验；请求只占用其原币对应的额度，没有汇率换算、跨币总额或额度互借。原币额度与原币费用估算都来自服务商计价版本，不代表供应商实际账单的保证。服务商支持多种实际扣款币种时，模型登记选择与账户扣款口径一致的 PriceVersion；DeepSeek 的币种由实际账户选择，OpenRouter 使用 USD。界面语言只控制数字 / 货币格式，不决定模型计价币种或预算额度。

## 持久账本与发送门禁

物理 requestId 每次网络尝试唯一，关联 turnId / roundId / runId、taskKind、profileRevision、priceVersion、usageMappingVersion、priceCurrency、dispatchAt 与 runBudgetId。重试是新的物理身份并重新预算，重放响应仍是同一身份。所有剧情、整理与已授权记忆请求共用端口，主流程优先；可选后台不足跳过，必要请求不足暂停推进。本地计算、快照恢复继续，恢复不自动重发收费请求。

SQLite 表职责如下，具体迁移编号由 store 服务统一分配，业务 crate 不另持锁：

| 表                                 | 主键 / 约束                             | 职责                                  |
| ---------------------------------- | --------------------------------------- | ------------------------------------- |
| price_versions / calendar_versions | 不可变 versionId                        | 历史计价依据                          |
| run_budgets                        | runBudgetId / runId 唯一                | CNY / USD 独立限额及 revision         |
| physical_requests                  | requestId 唯一                          | 冻结身份、原始 usage、原币费用 / 币种 |
| run_budget_allocations             | requestId 唯一                          | 原币对应额度的预留 / 结算             |
| budget_warnings                    | runBudgetId + currency + threshold 唯一 | 80% 提示事实与已读状态                |
| budget_audit                       | operationId 唯一                        | 设置、补价和人工未确认处置证据        |

发送前持单在飞租约，在 BEGIN IMMEDIATE 事务中校验 PriceVersion 与当前 runBudget，按请求原币选择 CNY 或 USD 限额，判断该币种 settled + reserved + unconfirmed + 本次预留 <= 单局上限，并原子提交原币请求与对应分配。任一币种未配置上限或额度不足则拒绝且不发 HTTP；不能借用另一币种额度。比较与修改使用整数，不能先查后写。dispatchAt 为持久发送意图时刻，紧接发送；长时间排队在登记前结束；调试请求必须显式关联合法测试周目，不创建绕过单局额度的虚拟预算。网络不可与 SQLite 原子提交，崩溃在登记与发送之间仍可能消耗未知，恢复标 unconfirmed，不以“没看到响应”推断没发送。时段归属按该持久边界，是本地可审计约定，不能声称精确等于供应商收件时刻。

状态机为 reserved → settled / unconfirmed；unconfirmed 只有获得可核验完整 usage 或显式人工审计处置才能 → settled。未发送可证实的本地失败可以 settled=0、标 notDispatched；仅超时 / 取消 / 断流 / 无 usage 不能退款。零 usage 只有供应商完整合法报告或上述发送前证据才接受。人工处置必须明确实际费用或保留未知，记录理由与审计，不自动把未知抹成零。

结算在同一 BEGIN IMMEDIATE 中按 requestId + 旧状态 CAS，保存 usage / 原币费用、对应币种分配及统计，释放剩余预留；重复同摘要是 no-op，冲突摘要保留首次结果并记异常，不覆写。原始响应摘要可核验，不保存 reasoning / 密钥或无限正文。完整 usage 费用仍是按本地版本计算，显示 estimated，与未知 unconfirmed 区分。

账本结算状态与正文终态独立：输出完整合法而 usage 缺失 / 非法时，正文仍按 LLM 协议完成，账本保持 unconfirmed，budget.invalid-usage 仅用于费用诊断；不撤回已提交正文、不伪造 finishReason、不以重生成获取 usage。

原币费用按实际用量完整记账；若该币种费用超过对应单局上限，显示该币种超额并拒绝后续该币种请求，不截断、隐式充值或丢掉 usage。额度修改即时生效、与预留同事务顺序；已经发送的请求按冻结价格版本完成，新的限额不能回滚已发生费用。价格修改不改在飞请求。迟到结算更新原币费用及原 runBudget；无法确认事务提交结果先按 requestId 查询，不再次结算或重发。

## 周期、时区与提醒

单局上限绑定稳定 runId；新局创建新预算，恢复 / 回退 / 段切不刷新或退还费用。全应用月报覆盖全部运行与任务，按服务商和原币分组、模型作为下钻明细，不设月额度。月报时区取系统有效 IANA 值并持久化，无法确定回退 UTC 并告知；设备后来变化不覆盖。jiff 负责自然月报表边界、DST 和时区库版本。

月报按请求 dispatchAt 与已保存报表时区归类，不创建月度额度记录、不因跨月刷新或释放预算。月度统计可按需查询，不需要后台计时或持续轮询。DST 的不存在午夜采用当日最早合法瞬间，重复午夜取最早瞬间；墙钟异常按本地时间标注统计边界，但不影响单局冻结预算。

玩家修改报表时区只影响新查询如何归类请求；已经展示的历史数据按其记录的时区版本可复现。时区变化不重设单局上限，也不改变消费 / 币种。

80% 提醒按本局 CNY、USD 两个上限分别计算；只有对应币种已配置非零上限时才提醒。结算 / 补价 / 修改上限在事务内建立唯一 warning；可先持久后 UI 主动读取，丢事件也不丢事实。读过 / 调高 / 调低 / 换模型不删去重键，每局每币种同阈值只提示一次；重新打开界面可查看状态，不反复弹窗。预留与未知单列，占用对应币种本局可用额度；80% 不替代请求前拒绝。估算低于本局限额仍可能超出供应商实际扣款。

## 查询、历史与错误

查询按 runId 或自然月，先按 providerId 与 priceCurrency 分组，再支持 modelId 下钻；原币费用按币种展示，不存在跨币估算或合计。input / output / total / cached / reasoning / 请求次数均可见。月报同一服务商、同一币种内合计；未知用量数量、未确认占用、未计价小计显式列出，不展示零。列表复用 IPC 50 默认 / 200 最大分页，游标冻结查询 revision / 截止序号，后续页旧 revision 返回 app.bad-request（detail.reason=staleRevision）；不把账本全集塞进回合事件。

历史导入的未计价请求保留身份 / usage / 原周期；补价命令提供范围、版本与预览摘要，按 audit operationId 幂等提交，仅针对完整 usage 且尚未计价项，已结算项不重算。补价后核对原周期预警；不能以补价创建新消费或编造 usage。未知金额使历史完整性为 incomplete，新请求前不能借历史缺价绕过当前周期门禁：先处理影响本周期余额的未知记录。

设计命令族 `budget_get_settings / budget_set_settings / budget_get_period / budget_list_requests / budget_list_model_usage / budget_register_price / budget_backfill_price / budget_get_balance` 归 035 定型，同 commit 登记 Rust / TS 类型。设置需 expectedRevision；精确查询用稳定周期 ID；补价使用明确 auditId / 预览 revision。当前不注册这些 API 或扩大 llm:turn:* 载荷。

错误 code 独立登记为 budget.exceeded（runBudgetId / currency / estimatedNeeded / available）、budget.run-limit-missing（runId / currency）、budget.price-missing（providerId / modelId / missingFields）、budget.invalid-usage（requestId），均为受约束结构化 detail，不含密钥 / 原始响应。参数错为 app.bad-request，旧设置 / 查询为 app.bad-request（detail.reason=staleRevision），存储沿用 store.*。供应商实际额度拒绝是 llm.quota，不与本地超限混用；已有公共 OutcomeError 只有 code / message 的地方保持同型，不为计费单独塞 detail。前端按码本地化并保留 default。

## 可选供应商余额

余额是参考信息，不替代本地账本或让预算拒绝放行。Provider 能力表新增可选 balance（由 035 对齐 018），适配器返回 `{ providerId, credentialRevision, queriedAt, status, accounts }`；status 为 available / unavailable / unknown，accounts 项含 account / key 的 scope、currency、decimal amount 与原始可用性。普通密钥无此能力则隐藏查询入口，不能从未知供应商 URL 猜接口或抓网页。

核验的接口与限制如下；只是协议依据，不表示凭据实测通过：

| 供应商          | 接口与来源                                                                                           | 身份 / 金额边界                                                   |
| --------------- | ---------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| DeepSeek        | [GET /user/balance](https://api-docs.deepseek.com/api/get-user-balance/)                             | is_available 与 CNY / USD 字符串，分别显示，不把不同币种相加      |
| Kimi            | [GET /v1/users/me/balance](https://platform.kimi.com/docs/api/balance)                               | 可用余额 CNY，含代金券；业务 status / code 须校验                 |
| SiliconFlow     | [GET /v1/user/info 官方 OpenAPI](https://github.com/siliconflow/siliconcloud/blob/main/openapi.yaml) | v1 为 base URL 前缀，提取余额 / 状态；不将昵称 / 邮箱送前端       |
| OpenRouter      | [GET /api/v1/key](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-api-key)         | key 的剩余额度；null 上限不代表账户无限余额                       |
| OpenRouter 账户 | [GET /api/v1/credits](https://openrouter.ai/docs/api/api-reference/credits/get-remaining-credits)    | 要管理密钥，total_credits - total_usage；额外凭据自愿登记并走 019 |

首版不登记 OpenCode Zen / OpenAI / Anthropic 普通推理凭据的余额适配器；不以此宣称供应商永久不存在其他管理接口。供应商数值 JSON 在 Rust 保留数字词法并用 Decimal 解析，不能先经 f64；适配器必须有核验过的币种 / 可用性语义，SiliconFlow 仅凭 totalBalance 名称不能推定单位；证据不全暂不开放该适配器。未知币种、负值语义或解析异常是 unknown，不改账本。

查询仅显式刷新，或用户单独开启有界对账；默认无定期网络请求。启用后最短 5 分钟间隔、每次操作最多一次在飞，失败指数退避至 1 小时，卸载 / 撤销即停；不使用 LLM、不随 token 事件发请求，身份变化丢旧结果。响应体 / 超时沿用有界 HTTP 纪律。新鲜度上限 5 分钟，过期显示查询时间与 stale，不用旧 unavailable 永久阻止请求。

只有最新有效响应明确报告额度不可用时可返回 llm.quota；HTTP 401 / 403 / 网络失败 / 未支持接口保持原错误 / unknown，不当余额不足。并排显示本地与供应商金额。偏差提醒仅比较同币种同账户、相同凭据版本、两次有效快照间已结算消费，要求无未确认且语义可比；阈值默认 max(0.01, 本地期间费用的 10%)，可设置，单窗口去重。充值、赠送 / 过期、外部用 key 和税费可能产生偏差，只提示核查，不补写账本或自动变更预算。

## 实施与验证依据

035 交付价格 / 原币账本、单局预算估算、统一调用端口、IPC、费用设置 / 明细与可选余额；接 018–020 / 025 / 034，030 / 033 等待 035。不购买额度、不读取供应商逐请求账单、不使用实时外汇、不设全局月度上限、不隐藏额外收费调用。此前“参考 Token 额度”措辞由费用额度规则取代。

[足精度 Decimal](https://docs.rs/rust_decimal/latest/rust_decimal/)与 [jiff 时区](https://docs.rs/jiff/latest/jiff/)在实现时核验并集中 workspace 版本，不在设计提交中安装。字节输入估计必须包含完整序列化输入、协议固定开销与输出 / reasoning 上限；供应商适配器声明可证明的上界 / 保守系数，不能对任意 tokenizer 宣称 1 byte = 1 Token 永远成立。不能给出可靠上界时按模型输入硬限预留；超出估计完整结算且停新请求。未来 tokenizer 接入另行授权 / 验证，不暗加网络计数调用。

验证重点是同请求重复 / 冲突回调、单局 CNY / USD 上限并发不足、登记与发送崩溃、结算提交不确定、无 usage / 取消、服务商币种归属、跨月报表 / 时区 / DST、价格与单局上限修改、溢出舍入、补价 / 分币种预警、分页 revision 和余额权限 / 缓存。单局原币上限估算不等于供应商账单；参考额度不是性能实测。三平台设置和精确展示由实施留证，最终 bun run verify 十三项通过。
