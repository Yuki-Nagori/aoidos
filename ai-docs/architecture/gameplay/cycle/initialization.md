# 周目初始化与冻结

更新日期：2026-10-23。实施由 [056](../../../task/056-ending-flow-impl.md) 跟进；角色字段的定义见[角色档案架构](../characters/profiles.md)。

## 目标

每个新周目开局时，LLM 根据剧本、已登记的可信输入 schema 和角色档案生成一个可执行的最终目标、有限标签目录、受限规则和初始任务。首个引导 NPC 负责把最终目标自然地交给玩家。开局结果必须保存在 run 中并保持不可变；运行期间不能因为重试、改剧本或模型变化而重写主线目标和人物真值。

## 生命周期

```text
validated script + frozen profile / budget identity
  → create a non-playable record/header and durable creation journal
  → atomically persist recovery-required catalog + confirmed budget + frozen input + pending request identity
  → request a structured initialization snapshot
  → validate the complete response against registered schemas
  → atomically persist snapshot + hash and mark run playable
```

初始化未完成的 run 不可提交行动。失败或 unknown 不自动重发、不换身份、不重新创建预算；用户显式重试使用原 run / budget 和新的物理 requestId。用户可以放弃未完成 run，保留审计费用。旧 run 不补造初始化快照。

### 与现有周目目录、owner 和账本的接线

初始化状态与账本状态是两个独立维度：首阶段数据库行使用 `initializing`、`failed`、`unknown`、`playable`、`abandoned`；物理 attempt 行使用 `pending`、`failed`、`unknown`、`accepted`。UI 可将这些状态投影为等待、生成中等文案，但不能把 UI 状态名当作持久协议。账本仍由 035 的 physical request / settlement 状态表示，不能把“产物已保存”误当费用已结算，也不能把 `unknown` 按零退款。只有显式登记在初始化表中的新周目受新门禁约束；没有初始化行的历史周目继续按 legacy 恢复，不补造目标或档案。

`session_catalog.state` 继续表示 profile、预算、账务对账和修复状态，不与初始化状态做笛卡尔组合。目录摘要可以单独返回初始化状态。普通 `open_target` / session owner 激活与只读恢复 profile 的路径分离：初始化未完成时，普通 owner、行动、普通模型请求及回合恢复都必须拒绝；初始化专用读取只返回已冻结的 profile、剧本输入和明确的初始化 attempt，不提供普通回合能力。服务端的门禁必须覆盖 `Product::open`、`open_selected`、CLI/TUI、已注册 coordinator 和 `ProfileFactory` 的普通冻结入口，不能只靠前端按钮隐藏。

新局创建前先完成 profile、模型能力、双币预算和脚本输入预检；预算尚未配置时，应在创建 run / 产生任何物理请求之前要求用户设置并确认，不创建一个无法继续的半成品 run。`Product::prepare_initialization` 现可接收调用方明确确认的双币上限和 locale，构造冻结 profile / script / entity / cycle-input 快照及规范请求，再通过创建 journal 保存 non-playable run。保存前使用 `aoidos_llm::providers::capabilities_for_profile` 按静态模型目录核验 provider、模型、调用形态、thinking、temperature 与输出上限；该预检不读取凭据、不创建 HTTP client，也不联系 Provider。创建 journal 冻结 script body、entity manifest、cycle-input 原始字节及 hash 所绑定的 script revision；只接受当前 locale (`zh-Hans` / `en`)、prompt version `init-v1` 与响应 schema version `2`。engine 的 canonical prompt builder 根据这些冻结资源和 profile 生成 Completion / Chat 请求，并在写入前要求持久化的 provider request 与重算结果完全相同；请求受统一输入字节上限约束。固定角色的 `truth` 与 `initialKnowledge` 从 prompt 投影中移除，系统约束要求公开目标、标签和任务不泄露秘密。准备 API 不关闭 / 替换现有 owner、不激活周目、不领取 dispatch 权、不预留费用，也不访问 Provider；之后须通过 `run_dispatch_initialization` 对已保存的周目身份触发单次派发。该命令只在严格校验并原子冻结输出后返回 playable 初始化状态，已领取的 requestId 不能重放。先写 header-only record，再在单个 SQLite `IMMEDIATE` 事务中登记 `recovery-required` catalog、canonical billing runId、已确认的初始双币预算及审计、`cycle_initializations` 输入和未 dispatch attempt，并关闭创建 journal。旧 journal 没有准备快照时继续保留原恢复语义。`run_prepare_initialization` / `run_dispatch_initialization` IPC 与 TypeScript API 类型现已暴露准备和派发；尚无 Tauri / Web 新局流程调用它们，现有产品界面不会触发准备或初始化请求。初始化专用 `aoidos_llm::schedule::run_generation_once_with_output` 接收已持久化的规范 UUID `requestId`，并强制 `transport_retries=0`、温度 ladder 置空、`max_requests=1`；预算预留、Provider 调用和费用结算沿用同一 requestId。该接口只提供“每次调用最多一个物理请求”的调度原语，不保证相同身份的重复 / 并发调用幂等；调用方须先持久化并原子领取初始化 attempt，防止同一 attempt 被重复派发，重启时将已开始或 unknown 的请求转入恢复而非再次调用。调用方也负责失败 / unknown 状态恢复。玩家显式重试沿用原 run / profile / budget，分配新 requestId，旧 attempt 与费用记录保留。

dispatch 围栏由初始化专用预算端口实现：它在单个 SQLite `IMMEDIATE` 事务内 CAS 领取当前 attempt 的 `dispatch_started` 权，并写入同一 `requestId` 的物理费用预留；价格、额度或结算队列校验失败时两者一起回滚。重复或并发提交同一身份无法再次领取，即使物理账本对相同预留幂等，也不能重新访问 Provider。只有事务提交确认后才可调用 one-shot scheduler。进程在权已领取后退出时，恢复按可能已发送处理，不自动重放；费用预留继续承担原有对账义务。显式 retry 创建新 requestId 和未领取 attempt。

恢复 IPC 只返回 `sessionId`、`revision`、当前 `requestId`、初始化状态和 `dispatchAvailable`，不包含冻结输入或输出。`dispatchAvailable` 只在当前 attempt 仍为 pending、尚未领取派发权、run 仍为 `recovery-required` 且没有 memory owner 时为 true；前端必须以此门禁显示单次派发按钮。显式重试要求调用方传入当前 revision、前一 requestId 与稳定的新 requestId；同一个重试请求可在 IPC 响应丢失时幂等重放。存储事务同时要求 run 仍处于 `recovery-required`、未创建 memory owner 且保留冻结 profile 与预算身份，避免删除或激活开始后继续创建初始化 attempt。周目管理 UI 先显式检查状态；failed / unknown 只显示创建新 attempt 的重试操作，重试不会自动派发。只有显式点击派发按钮才调用模型，并明确提示可能产生费用；已领取的初始化显示等待 / 刷新说明而不允许重复派发。playable 后刷新目录，仍由用户显式继续周目。

records 文件和 SQLite 不是同一事务。先创建不可玩的 record/header，再持久登记 canonical run、profile / 输入快照、预算身份和初始化状态；崩溃留下的未激活 record 必须可识别并恢复或显式放弃，不能被 legacy scanner 当作可玩旧局。物理请求身份和冻结输入必须在调用 Provider 前持久化。响应整体校验后，以同一 SQLite 事务 CAS 当前 run revision / requestId，保存规范化快照与 hash 并标记 `playable`；失败不得部分写入，也不得先注册普通 owner。成功冻结与 owner 注册之间若发生崩溃，恢复从冻结快照建立 owner，不再调用 Provider。

首个创建阶段以 `session_creation_ops` 持久保存 operationId、session/header、公开 profile 快照及 hash；可选 `session_creation_preparations` 以有界、版本化内容保存初始化 requestId、完整 provider input、双币上限及校验 hash。写入顺序为 intent → header-only transcript → 单个 SQLite 事务登记目录、预算 / 审计、初始化快照 / pending attempt 并结束创建 journal。每一步可按完全相同快照重放；应用启动会恢复未结束的 intent。创建 journal 持有的 sessionId 对 legacy scanner 是排除项，因此扫描即使先于目录登记，也不能将 header 导入成历史可玩周目。该准备事务不注册 `Records` owner、不更改 `active_session`、不创建记忆 run、不预留费用、不领取 dispatch 权或访问 Provider；新迁移给旧 journal 保留“无初始化快照”的旧行为。只有后续初始化 coordinator 接好费用 dispatch、响应校验 / 冻结、恢复门禁和 owner 激活后，才能开放产品新局入口。

持久化接口应由 engine 内的初始化 repository 提供 begin / load / prepare-attempt / finish-attempt / finalize 等窄操作；状态跃迁以 runId、requestId 和 revision 作 CAS，重复同结果调用幂等，冲突或旧身份拒绝。跨多表的预算与 attempt 原子写入需抽取 `BillingLedger` 的事务内 helper；不能把会自行开启 SQLite 事务的公开 API 嵌套进外层事务。首个存储契约提交可先只对测试显式构造的初始化 run 启用门禁；在所有入口具备恢复与预算设置能力前，不得切换现有新局入口。

放弃未完成初始化仅停止该 run，保留请求与费用审计；玩家再次勾选并确认完整删除时，复用周目删除 journal 清理本地该局数据，包括未结物理请求和结算 outbox。删除仍校验账务身份范围，不触及共享价目或无法确认归属的记录。

## 输入和生成约束

版本化 `cycle-input-vN.json` 提供维护者明确登记的状态字段、剧情角色名册和可选固定档案，不从 Markdown 自然语言推断执行字段或角色名册。v2 的 `storyCharacterIds` 必须逐一引用完整实体清单中的 Character；模型只能为名册中没有固定档案的 ID 补档，不能创造或替换现有角色身份。模型可引用已登记状态 ID，但不能创造状态 source、世界事实或任意规则。固定的真值档案不由 LLM 重新编写；缺少作者固定档案的主角或配角由开局阶段生成一次，并与目标等初始化产物共同冻结。主角和配角都必须在 run 快照中有档案，不能因为主角由玩家控制而省略其 in-world identity。历史 v1 输入只用于恢复原修订，不为它补造名册或初始化产物。

最终目标是唯一主线顶层项，开场由固定或生成的首个引导者交代；后续普通任务和连续任务按已提交里程碑追加。任务状态用于叙事导向，不是游戏世界中的事实断言。详见[任务投影](tasks.md)。

## 私有档案保护

初始化快照的角色真值不能被序列化进通用 UI DTO、公开记录、日志或所有模型请求。模型请求按用途最小化投影：生成开场引导语时可获得引导者需要知道的开局资料，但不应因此获得其他角色的秘密或玩家未获知的主角真值。需要同时运用秘密和输出公开文本时，不能仅靠提示词要求模型“不要泄漏”；应采用 engine 侧显式字段筛选、分开的工作输入 / 输出职责，或未来建立独立的秘密推理步骤和公开叙事步骤。

每个快照记录 schema、prompt、locale、script revision、角色档案来源 / revision、profile revision、物理 requestId 和规范化内容 hash。预算与 run 身份在发请求前保存；成功结果只在一个 SQLite 事务中从 initializing 进入 playable。locale 只控制本地化文案，不得改变角色的 canonical ID、真值或初始化结果。
