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

## 输入和生成约束

`cycle-input.json` 只提供维护者明确登记的状态字段和已登记角色资料，不从 Markdown 自然语言推断执行字段。模型可引用这些稳定 ID，但不能创造状态 source、角色 ID、世界事实或任意规则。固定的真值档案不由 LLM 重新编写；没有作者固定档案的角色由开局阶段生成一次，并与目标等初始化产物共同冻结。主角和配角都必须在 run 快照中有档案，不能因为主角由玩家控制而省略其 in-world identity。

最终目标是唯一主线顶层项，开场由固定或生成的首个引导者交代；后续普通任务和连续任务按已提交里程碑追加。任务状态用于叙事导向，不是游戏世界中的事实断言。详见[任务投影](tasks.md)。

## 私有档案保护

初始化快照的角色真值不能被序列化进通用 UI DTO、公开记录、日志或所有模型请求。模型请求按用途最小化投影：生成开场引导语时可获得引导者需要知道的开局资料，但不应因此获得其他角色的秘密或玩家未获知的主角真值。需要同时运用秘密和输出公开文本时，不能仅靠提示词要求模型“不要泄漏”；应采用 engine 侧显式字段筛选、分开的工作输入 / 输出职责，或未来建立独立的秘密推理步骤和公开叙事步骤。

每个快照记录 schema、prompt、locale、script revision、角色档案来源 / revision、profile revision、物理 requestId 和规范化内容 hash。预算与 run 身份在发请求前保存；成功结果只在一个 SQLite 事务中从 initializing 进入 playable。locale 只控制本地化文案，不得改变角色的 canonical ID、真值或初始化结果。
