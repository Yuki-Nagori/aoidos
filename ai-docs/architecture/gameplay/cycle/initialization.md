# 周目初始化与冻结

更新日期：2026-10-23。实施由 [056](../../../task/056-ending-flow-impl.md) 跟进；角色字段的定义见[角色档案架构](../characters/profiles.md)。

## 目标

每个新周目开局时，LLM 根据剧本、已登记的可信输入 schema 和角色档案生成一个可执行的最终目标、有限标签目录、受限规则和初始任务。首个引导 NPC 负责把最终目标自然地交给玩家。开局结果必须保存在 run 中并保持不可变；运行期间不能因为重试、改剧本或模型变化而重写主线目标和人物真值。

## 生命周期

```text
validated script + frozen profile / budget identity
  → persist initializing run and physical request identity
  → request a structured initialization snapshot
  → validate the complete response against registered schemas
  → atomically persist snapshot + hash and mark run playable
```

初始化未完成的 run 不可提交行动。失败或 unknown 不自动重发、不换身份、不重新创建预算；用户显式重试使用原 run / budget 和新的物理 requestId。用户可以放弃未完成 run，保留审计费用。旧 run 不补造初始化快照。

### 与现有周目目录、owner 和账本的接线

初始化状态与账本状态是两个独立维度：首阶段数据库行使用 `initializing`、`failed`、`unknown`、`playable`、`abandoned`；物理 attempt 行使用 `pending`、`failed`、`unknown`、`accepted`。UI 可将这些状态投影为等待、生成中等文案，但不能把 UI 状态名当作持久协议。账本仍由 035 的 physical request / settlement 状态表示，不能把“产物已保存”误当费用已结算，也不能把 `unknown` 按零退款。只有显式登记在初始化表中的新周目受新门禁约束；没有初始化行的历史周目继续按 legacy 恢复，不补造目标或档案。

`session_catalog.state` 继续表示 profile、预算、账务对账和修复状态，不与初始化状态做笛卡尔组合。目录摘要可以单独返回初始化状态。普通 `open_target` / session owner 激活与只读恢复 profile 的路径分离：初始化未完成时，普通 owner、行动、普通模型请求及回合恢复都必须拒绝；初始化专用读取只返回已冻结的 profile、剧本输入和明确的初始化 attempt，不提供普通回合能力。服务端的门禁必须覆盖 `Product::open`、`open_selected`、CLI/TUI、已注册 coordinator 和 `ProfileFactory` 的普通冻结入口，不能只靠前端按钮隐藏。

新局创建前先完成 profile、模型能力、双币预算和脚本输入预检；预算尚未配置时，应在创建 run / 产生任何物理请求之前要求用户设置并确认，不创建一个无法继续的半成品 run。初始化专用 `aoidos_llm::schedule::run_generation_once_with_output` 接收已持久化的规范 UUID `requestId`，并强制 `transport_retries=0`、温度 ladder 置空、`max_requests=1`；预算预留、Provider 调用和费用结算沿用同一 requestId。该接口只提供“每次调用最多一个物理请求”的调度原语，不保证相同身份的重复 / 并发调用幂等；调用方须先持久化并原子领取初始化 attempt，防止同一 attempt 被重复派发，重启时将已开始或 unknown 的请求转入恢复而非再次调用。调用方也负责失败 / unknown 状态恢复。玩家显式重试沿用原 run / profile / budget，分配新 requestId，旧 attempt 与费用记录保留。

dispatch 围栏由初始化专用预算端口实现：它在单个 SQLite `IMMEDIATE` 事务内 CAS 领取当前 attempt 的 `dispatch_started` 权，并写入同一 `requestId` 的物理费用预留；价格、额度或结算队列校验失败时两者一起回滚。重复或并发提交同一身份无法再次领取，即使物理账本对相同预留幂等，也不能重新访问 Provider。只有事务提交确认后才可调用 one-shot scheduler。进程在权已领取后退出时，恢复按可能已发送处理，不自动重放；费用预留继续承担原有对账义务。显式 retry 创建新 requestId 和未领取 attempt。

records 文件和 SQLite 不是同一事务。先创建不可玩的 record/header，再持久登记 canonical run、profile / 输入快照、预算身份和初始化状态；崩溃留下的未激活 record 必须可识别并恢复或显式放弃，不能被 legacy scanner 当作可玩旧局。物理请求身份和冻结输入必须在调用 Provider 前持久化。响应整体校验后，以同一 SQLite 事务 CAS 当前 run revision / requestId，保存规范化快照与 hash 并标记 `playable`；失败不得部分写入，也不得先注册普通 owner。成功冻结与 owner 注册之间若发生崩溃，恢复从冻结快照建立 owner，不再调用 Provider。

持久化接口应由 engine 内的初始化 repository 提供 begin / load / prepare-attempt / finish-attempt / finalize 等窄操作；状态跃迁以 runId、requestId 和 revision 作 CAS，重复同结果调用幂等，冲突或旧身份拒绝。跨多表的预算与 attempt 原子写入需抽取 `BillingLedger` 的事务内 helper；不能把会自行开启 SQLite 事务的公开 API 嵌套进外层事务。首个存储契约提交可先只对测试显式构造的初始化 run 启用门禁；在所有入口具备恢复与预算设置能力前，不得切换现有新局入口。

放弃未完成初始化仅停止该 run，保留请求与费用审计；玩家再次勾选并确认完整删除时，复用周目删除 journal 清理本地该局数据，包括未结物理请求和结算 outbox。删除仍校验账务身份范围，不触及共享价目或无法确认归属的记录。

## 输入和生成约束

`cycle-input.json` 只提供维护者明确登记的状态字段和已登记角色资料，不从 Markdown 自然语言推断执行字段。模型可引用这些稳定 ID，但不能创造状态 source、角色 ID、世界事实或任意规则。固定的真值档案不由 LLM 重新编写；没有作者固定档案的角色由开局阶段生成一次，并与目标等初始化产物共同冻结。主角和配角都必须在 run 快照中有档案，不能因为主角由玩家控制而省略其 in-world identity。

最终目标是唯一主线顶层项，开场由固定或生成的首个引导者交代；后续普通任务和连续任务按已提交里程碑追加。任务状态用于叙事导向，不是游戏世界中的事实断言。详见[任务投影](tasks.md)。

## 私有档案保护

初始化快照的角色真值不能被序列化进通用 UI DTO、公开记录、日志或所有模型请求。模型请求按用途最小化投影：生成开场引导语时可获得引导者需要知道的开局资料，但不应因此获得其他角色的秘密或玩家未获知的主角真值。需要同时运用秘密和输出公开文本时，不能仅靠提示词要求模型“不要泄漏”；应采用 engine 侧显式字段筛选、分开的工作输入 / 输出职责，或未来建立独立的秘密推理步骤和公开叙事步骤。

每个快照记录 schema、prompt、locale、script revision、角色档案来源 / revision、profile revision、物理 requestId 和规范化内容 hash。预算与 run 身份在发请求前保存；成功结果只在一个 SQLite 事务中从 initializing 进入 playable。locale 只控制本地化文案，不得改变角色的 canonical ID、真值或初始化结果。
