# 042 — OpenCode Zen / Go 模型目录与多协议 Provider

- 状态：done
- 依赖：018、019、034
- 优先级：P1
- 创建 / 更新：2026-10-11 / 2026-10-11

## 目标

让 OpenCode Zen 与 OpenCode Go 成为独立、可选的 LLM Provider，按官方已核验目录提供模型选择，并按每个模型要求的协议完成真实流式调用。默认选择 DeepSeek V4.1 Flash。Zen / Go 的凭据与计价身份不共享；OpenCode Go 的会话路由 header、订阅与 fallback 语义不被隐藏。

## 范围

- Zen 官方 endpoint 表中支持自由文本生成的 Chat Completions、Responses、Anthropic Messages 与 Gemini 模型；Go 官方模型目录列出的 Chat Completions、Responses 与 Anthropic Messages 模型。
- 将三种协议的 SSE 文本、终态、usage、缓存读取 / 创建用量映射到 Provider 类型；会话 ID 稳定传递至 Go 的 `x-opencode-session`，请求使用 Aoidos User-Agent。
- profile / key 设置、provider 型号选择、默认模型、能力约束、重试与错误脱敏，及对应英 / 中文文档。
- OpenCode 价格与缓存写入定价、Go 峰谷时段由 035 账本接入，不在本任务重复实现计费存储；订阅费用不进入 Aoidos 计费范围。

## 明确边界

- 目录以 [OpenCode Zen endpoint 表](https://docs.opencode.ai/docs/zen/) 和 [OpenCode Go endpoint 表](https://opencode.ai/v2/docs/console/go) 为依据，并记录核验日期；模型 `/models` 只返回 ID 时不能据此推测 API 协议、上下文或定价。
- Zen 的 Jev / Jev Free 使用 SystemOne 结构化问答端点，不符合当前自由文本生成契约，不纳入文本 Provider。未列入官方 endpoint 表的实验 / 兼容 ID 也不自动启用。
- Go 是固定月订阅额度，模型 token 单价用于估算每模型配额使用；可选 Zen 余额回退可能形成独立 Zen 按量费用。产品不追踪订阅费用、不读取或推断私有控制台接口。
- 图片、工具调用、音频、JSON schema、模型 reasoning 控件等不在当前 ChatInput / ProviderDelta 契约内的能力不宣称支持。

## 验收标准

- [x] Zen 与 Go 的所有范围内模型都有唯一协议映射；目录、UI 选项与 adapter 能力声明一致，DeepSeek V4.1 Flash 是两个 OpenCode Provider 的默认模型。
- [x] Chat Completions、Responses、Anthropic Messages 与 Zen Gemini 流请求覆盖 body、鉴权、端点、session/User-Agent、文本、finishReason、usage 与错误路径；SSE 分片、EOF、取消和恶意 / 超限响应保持既有 Provider 边界。
- [x] Anthropic cache-read 与 cache-creation、OpenAI / Gemini cached input 和 reasoning usage 不被错误合并或丢失；`Usage` 独立保留 `cache_creation_tokens`，035 分别登记读写缓存单价并将缓存创建用量纳入预算与结算。
- [x] Go 的 session ID 稳定使用同一对局身份；不记录 prompt 或凭据。Zen / Go profile、key、价格和账单统计身份互相隔离。
- [x] 模型目录核验日期与官方来源同步到架构文档；不支持的结构化 Jev API 和模型目录未知项不会出现在可选列表。
- [x] Zen / Go 每个模型单独登记上下文窗口与最大输出，按 provider-specific 模型目录取值；同名模型可有不同限制，未知不能默认 1M。
- [x] Rust / TypeScript / 文档 / task 同步，`bun run verify` 全项通过；task 交付前由 subagent 独立 review 并处理发现。

## 实施记录

- 2026-10-11：对照 OpenCode 官方 endpoint 文档确认 Zen 与 Go 使用多种协议；Go 文档表列出 32 个模型，Zen 当前 endpoint 表映射 86 个自由文本模型。已接入目录、多协议 SSE adapter、Go 稳定 session ID 和 Gemini Zen route；cache-read / cache-creation 分别保留，035 已接入写入费率、预算与结算。完整 `bun run verify` 通过，GPT-6.1 Sol medium 独立 review 无 findings。实现与仓库内验收完成；未对动态上游路由做实机能力保证。Go 峰谷结算由 035 承接；固定订阅费用不进入 Aoidos 计费范围。
- 2026-10-11：确认 OpenCode 目录包含不同模型窗口，不能统一按 DeepSeek 的 1M 处理；按 Zen / Go provider-specific 数据逐型号登记，不共用单一默认输入或输出上限。
- 2026-10-11：将 86 个 Zen 与 32 个 Go 型号的协议、上下文窗口、最大输出集中登记在 `providers/models/mod.rs`。取 OpenCode provider-specific Models.dev 元数据，Qwen3.7 Max / Plus 以 Alibaba 官方规格补齐；投影侧另有 32K 游戏输入硬限，且不宣称 OpenCode 网关的动态后端已实机验证。
