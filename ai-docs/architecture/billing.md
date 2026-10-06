# LLM 计价与费用预算

更新 / 官方资料核验日期：2026-10-06。[027](../task/027-llm-cost-control-design.md) 设计定稿，实施由 [035](../task/035-llm-cost-control-impl.md) 承接，当前无计费运行时。调用优先级、单次硬限与回合归 [LLM 架构](llm.md)，精确展示归[国际化](i18n.md)，凭据归 019；本文是价格、账本与费用周期的精确协议。

## 控制单位与默认额度

周目与全应用自然月都使用可修改的固定金额上限；同一物理请求必须同时满足两者。Token 是原始用量与单次硬限，不建立累计 Token 额度。参考 Token 仅生成默认金额，不能将混合模型消费折成另一个参考 Token 预算，也不以记忆模块另设额度。

初始参考模型由用户指定，未指定取首个登记成功的模型；缺必要价格先登记，不生成零默认。首版开发默认为每周目输入 2,000,000 / 输出 500,000 Token，自然月输入 10,000,000 / 输出 2,500,000 Token；按缓存未命中及最高时段价格、明确币种与汇率计算，向上取到分。它们是可编辑的初始预算策略，不是普通对局实测消耗或模型能力保证，发布前 035 验证提示与合理性。

每个作用域保存默认模板与来源快照；首次生成后金额固定。模型切换、价格更新和界面语言切换不重算上限。用户可显式重新生成 / 编辑上限，展示参考输入 / 输出与价格依据；当前周期已经消耗不清零。合法上限为非负金额、最多 9 位小数且可表示；零额度仅允许零价且其他门禁合法的请求，零价必须显式登记。

## 价格身份、缓存与更新

ProviderIdentity 由已保存的供应商登记 ID 与 API 部署身份组成，模型保留精确 API ID；兼容协议不等于同一计价主体。凭据只保留 019 的引用，不混入价格缓存。配置持久化在 SQLite，内存缓存以 providerId / modelId / configRevision 为键，登记或修改成功后替换缓存，重启从已校验配置恢复；不涉及登录或云同步。

不可变 PriceVersion 保存 versionId、providerId / modelId、currency、unitTokens、inputUncached / inputCached / output 单价、usageMappingVersion、sourceKind / sourceUrl / checkedAt、effectiveFrom / validUntil、时段 / 日历版本与配置摘要。单价为十进制字符串，非负、最多 9 位小数；unitTokens 为正安全整数。缓存不区分时显式指定同价，缺字段不能填零。每次请求保存冻结版本，不只保存可变配置引用。

内置价格目录随应用版本维护，优先登记已核验主流供应商 / 模型，每条有官方来源 / 日期 / 生效范围；不宣称自动覆盖任意自建端点。用户首次确认内置条目也形成已登记配置；未登记 / 缺价 / 已过 validUntil 先补齐或显式重新确认。内置更新只提供新版本供后续确认，不覆盖手动设置；撤销后禁止新请求，历史仍可查。手动版本可无到期日但显示登记日期与过期风险，确认不是官方价格准确性保证。

价格、汇率、节假日数据均不可变；修改产生新版本，未知版本 / 损坏配置失败关闭，不自动套其他模型。来源撤回不删除历史证据。原始 Token 用量另存，不因价格变化改写。

## 用量归一化与时段

保存原始、受界限约束的 usage 与归一化字段：inputTotal、inputCached、inputUncached、outputTotal、reasoningIncluded、totalTokens；未知项省略。inputUncached = inputTotal - inputCached，缓存明确不支持时 inputCached = 0。reasoning 作为输出子集展示，不再重复计费；供应商若分开返回推理 Token，适配器先按其冻结映射并入 outputTotal。缺失计价必需项或负值 / 不一致 / 溢出不得虚构 usage，进入 unconfirmed。总 Token = inputTotal + outputTotal，不重复加缓存或 reasoning。

PriceVersion 可附 IANA 时区、星期、半开时间窗、时段单价与版本化节假日表；跨午夜拆窗，重叠、空价和非法时区拒绝。优先级为显式日期覆盖 → 星期 / 时间窗 → 基准价。日历声明覆盖年份及调整工作日策略；未覆盖 / 无法确定采用可适用最高价并标 estimated，不默认为假日优惠。内置日历随版本更新，手动覆盖明确来源；不自动推断节假日。

2026-10-06 核验的 [DeepSeek 官方价格说明](https://api-docs.deepseek.com/quick_start/pricing/)有峰谷价，周一至周五 UTC 01:00–04:00 / 06:00–10:00（北京时间 09:00–12:00 / 14:00–18:00），中国公众假日除外，谷价为峰价一半。该实例只说明时段结构，不作为永久价格常量。当前页面未承诺长请求的时段归属；本地按 dispatchAt 选价是估算假设，详情显示该依据。预留按该版本所有可适用时段最高价，不因为预计落在谷时就降低预留。

## 金额、换汇与币种变更

内部 NanoMoney 是 i64 的币种 × 10^-9；单项、聚合、乘法全部检查溢出，拒绝不能表示的配置或预留。Rust 使用 rust_decimal 与 checked i128 中间计算，禁止 f64；IPC 金额为十进制字符串 + 明确 currency，前端不计算额度。费用先按有理数合计，再统一换汇 / 舍入，不逐 Token 舍入累积误差。预留向上到 nano，结算 HALF_UP 到 nano，记录舍入政策版本；主视图舍入到分不改变账本。

请求原币费用 = (inputUncached × inputUncachedPrice + inputCached × inputCachedPrice + outputTotal × outputPrice) / unitTokens。按原币 → 对应预算周期币种的冻结 FxVersion 换算；同币无需汇率，跨币必须由用户填写正十进制汇率、方向、生效时间与到期日（首版默认 30 天，可修改）。首版无隐式网络汇率源，无有效汇率先补齐，明确标本地估算；不自动用倒数 / 当前汇率重算历史请求。

首次预算币种按 i18n 冻结，之后显式选择 CNY / USD。币种修改用于新周目与下一自然月，设置显示两者生效范围；已有周期保留原币种 / 上限 / 历史金额。过渡期一个请求可能分配到不同币种的周目 / 月度周期，分别冻结换汇并校验上限，不直接相加；请求本体只保存一次原币费用，作用域分配不是两次消费。统计限定明确周期币种，跨币历史按币种分组，不提供无汇率依据的混合总数。此规则细化 LLM 的统一币种原则为每个预算周期单一币种，防止修改设置重计旧账。

## 持久账本与发送门禁

物理 requestId 每次网络尝试唯一，关联 turnId / roundId / runId、taskKind、profileRevision、priceVersion、usageMappingVersion、FxVersion、dispatchAt 与周目 / 月份 periodId。重试是新的物理身份并重新预算，重放响应仍是同一身份。所有剧情、整理与已授权记忆请求共用端口，主流程优先；可选后台不足跳过，必要请求不足暂停推进。本地计算、快照恢复继续，恢复不自动重发收费请求。

SQLite 表职责如下，具体迁移编号由 store 服务统一分配，业务 crate 不另持锁：

| 表                                               | 主键 / 约束                       | 职责                                    |
| ------------------------------------------------ | --------------------------------- | --------------------------------------- |
| price_versions / fx_versions / calendar_versions | 不可变 versionId                  | 历史计价依据                            |
| budget_periods                                   | periodId；月序号唯一              | 币种、边界、限额 revision、统计         |
| physical_requests                                | requestId 唯一                    | 状态、冻结请求身份、原始 usage / 费用   |
| budget_allocations                               | requestId + scope 唯一            | 恰好周目 / 自然月两笔分配，不重复总消费 |
| budget_warnings                                  | scope + periodId + threshold 唯一 | 80% 提示事实与已读状态                  |
| budget_audit                                     | operationId 唯一                  | 设置、补价和人工未确认处置证据          |

发送前持单在飞租约，在 BEGIN IMMEDIATE 事务中校验价格 / 汇率与两周期、冻结配置，判断 settled + reserved + unconfirmed + 本次预留 <= 上限，两分配和 reserved 请求同时提交。比较与修改使用整数，不能先查后写；失败只返回拒绝，不发 HTTP。dispatchAt 为持久发送意图时刻，紧接发送；长时间排队在登记前结束；调试请求必须显式关联合法测试周目，不创建绕过双作用域的虚拟额度。网络不可与 SQLite 原子提交，崩溃在登记与发送之间仍可能消耗未知，恢复标 unconfirmed，不以“没看到响应”推断没发送。时段 / 月归属按该持久边界，是本地可审计约定，不能声称精确等于供应商收件时刻。

状态机为 reserved → settled / unconfirmed；unconfirmed 只有获得可核验完整 usage 或显式人工审计处置才能 → settled。未发送可证实的本地失败可以 settled=0、标 notDispatched；仅超时 / 取消 / 断流 / 无 usage 不能退款。零 usage 只有供应商完整合法报告或上述发送前证据才接受。人工处置必须明确实际费用或保留未知，记录理由与审计，不自动把未知抹成零。

结算在同一 BEGIN IMMEDIATE 中按 requestId + 旧状态 CAS，保存 usage / 原币费用、两分配及统计，释放剩余预留；重复同摘要是 no-op，冲突摘要保留首次结果并记异常，不覆写。原始响应摘要可核验，不保存 reasoning / 密钥或无限正文。完整 usage 费用仍是按本地版本计算，显示 calculated；日历 / 归属等假设另标 estimated，与未知 unconfirmed 区分。

账本结算状态与正文终态独立：输出完整合法而 usage 缺失 / 非法时，正文仍按 LLM 协议完成，账本保持 unconfirmed，budget.invalid-usage 仅用于费用诊断；不撤回已提交正文、不伪造 finishReason、不以重生成获取 usage。

实际费用超过预留仍完整记账，显示超额并拒绝后续请求，不截断、隐式充值或丢掉 usage。额度修改即时生效、与预留同事务顺序；已经发送的请求按冻结计价完成，新的限额不能回滚已发生费用。汇率 / 价格修改不改在飞请求。迟到结算更新原周期，不扣新月；无法确认事务提交结果先按 requestId 查询，不再次结算或重发。

## 周期、时区与提醒

周目周期用稳定 runId；新周目刷新该作用域，恢复 / 回退 / 段切不刷新费用。全应用月周期覆盖全部运行与任务，不能按模型、剧本或记忆分月。首次预算时区取系统有效 IANA 值并持久化，无法确定回退 UTC 并告知；设备后来变化不覆盖。jiff 负责自然月边界、DST 和时区库版本，存储 UTC 起止瞬间与规则版本。

月切换由请求前 / 主动查询推进，无需后台计时或持续轮询。periodId 由数据库分配，calendarLabel 与边界是字段；每个新 period 唯一，正常为该时区每月一日零点至下月一日零点。DST 的不存在午夜采用当日最早合法瞬间，重复午夜取最早瞬间；实测奇异时区用例，边界必须严格递增。长期离线按年月运算定位当前周期，只创建实际需要的周期，不逐月生成无消费空账本；待生效的时区 / 币种配置仍按原定边界应用，再定位当前期。墙钟回拨不复用已关闭周期，保留当前最高周期并提示时间异常；错时钟 / 恶意改钟不承诺供应商计费防作弊。

手动时区从旧时区下一月边界 T 生效，当前月不动。同一边界仅建立一个新周期。跨区切换建立 transition 周期：calendarLabel 取旧月标签的下一年月，start=T，end 为该标签的下一月一日在新时区的零点；然后恢复新时区正常自然月。新时区若仍在前一月，不额外开一个短周期刷新额度；若已进入新月也不补发额度。transition 明确显示真实起止和时区，不冒充标准自然月。多次修改以边界前最后确认值生效，当前周期依然唯一。

80% 条件由已结算金额达到当前上限 × 0.8 判断，零上限不弹无意义提醒。结算 / 补价 / 修改上限在事务内建立唯一 warning；可先持久后 UI 主动读取，丢事件也不丢事实。读过 / 调高 / 调低 / 换模型不删去重键，同周期只提示一次；重新打开界面可查看状态，不反复弹窗。预留与未知单列，占用可用额度；80% 不替代请求前拒绝。限额以下仍可能被供应商实际余额拒绝。

## 查询、历史与错误

查询按 periodId / runId、providerId + modelId 分组，金额 / input / output / total / cached / reasoning / 请求次数均可见。汇总与同范围账本一致，不把双作用域分配加倍；未知用量数量、未确认占用、未计价小计显式列出，不展示零。列表复用 IPC 50 默认 / 200 最大分页，游标冻结查询 revision / 截止序号，后续页旧 revision 返回 app.bad-request（detail.reason=staleRevision）；不把账本全集塞进回合事件。

历史导入的未计价请求保留身份 / usage / 原周期；补价命令提供范围、版本与预览摘要，按 audit operationId 幂等提交，仅针对完整 usage 且尚未计价项，已结算项不重算。补价后核对原周期预警；不能以补价创建新消费或编造 usage。未知金额使历史完整性为 incomplete，新请求前不能借历史缺价绕过当前周期门禁：先处理影响本周期余额的未知记录。

设计命令族 `budget_get_settings / budget_set_settings / budget_get_period / budget_list_requests / budget_list_model_usage / budget_register_price / budget_backfill_price / budget_get_balance` 归 035 定型，同 commit 登记 Rust / TS 类型。设置需 expectedRevision；精确查询用稳定周期 ID；补价使用明确 auditId / 预览 revision。当前不注册这些 API 或扩大 llm:turn:* 载荷。

错误 code 独立登记为 budget.exceeded（scope / periodId / currency / needed / available）、budget.price-missing（providerId / modelId / missingFields）、budget.fx-missing（from / to）、budget.invalid-usage（requestId），均为受约束结构化 detail，不含密钥 / 原始响应。参数错为 app.bad-request，旧设置 / 查询为 app.bad-request（detail.reason=staleRevision），存储沿用 store.*。供应商实际额度拒绝是 llm.quota，不与本地超限混用；已有公共 OutcomeError 只有 code / message 的地方保持同型，不为计费单独塞 detail。前端按码本地化并保留 default。

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

只有最新有效响应明确报告额度不可用时可返回 llm.quota；HTTP 401 / 403 / 网络失败 / 未支持接口保持原错误 / unknown，不当余额不足。并排显示本地与供应商金额。偏差提醒仅比较同币种同账户、相同凭据版本、两次有效快照间已结算消费，要求无未确认且语义可比；阈值默认 max(0.01, 本地期间费用的 10%)，可设置，单窗口去重。充值、赠送 / 过期、外部用 key、税费与汇率差异均可能产生偏差，只提示核查，不补写账本或自动变更预算。

## 实施与验证依据

035 交付价格 / 汇率 / 账本、统一调用端口、IPC、费用设置 / 明细与可选余额；接 018–020 / 025 / 034，030 / 033 等待 035。不购买额度、不读取供应商逐请求账单，不隐藏额外收费调用。此前“参考 Token 额度”措辞由金额额度规则取代。

[足精度 Decimal](https://docs.rs/rust_decimal/latest/rust_decimal/)与 [jiff 时区](https://docs.rs/jiff/latest/jiff/)在实现时核验并集中 workspace 版本，不在设计提交中安装。字节输入估计必须包含完整序列化输入、协议固定开销与输出 / reasoning 上限；供应商适配器声明可证明的上界 / 保守系数，不能对任意 tokenizer 宣称 1 byte = 1 Token 永远成立。不能给出可靠上界时按模型输入硬限预留；超出估计完整结算且停新请求。未来 tokenizer 接入另行授权 / 验证，不暗加网络计数调用。

验证重点是同请求重复 / 冲突回调、两作用域并发不足、登记与发送崩溃、结算提交不确定、无 usage / 取消、跨月 / 时区 / DST、币种与价格修改、溢出舍入、补价 / 预警、分页 revision 和余额权限 / 缓存。估算不等于供应商账单；参考额度不是性能实测。三平台设置和精确展示由实施留证，最终 bun run verify 十项通过。
