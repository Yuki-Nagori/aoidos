# 结局规则与周目落幕

更新日期：2026-10-19。结局设计见 [054](../task/054-ending-flow-design.md)，实现由 [056](../task/056-ending-flow-impl.md) 承接。设计由 054 定稿，功能尚未实现。

## 权威边界

结局事实由 `aoidos-engine` 决定，Vue 与 LLM 都不能结束周目或提交世界事实。固定结局选择页只作为 `aoidos-art` 视觉参考，玩家不从静态结局清单中选正史。

```text
剧本（无 ending-rules.json：沿用现有流程）
  → endingMode=disabled；旧存档无字段时按 legacy-disabled 兼容

剧本字段 / 标签目录（配置 ending-rules.json）
  → 持久化不可玩的 initializing run 与原币预算
  → 可选规则生成请求经 027 / 035 计费
  → 校验并冻结规则快照，再开放新周目输入
  → 可信终局触发条件已提交
  → 引擎按锁定规则求值并冻结标签 / 规范事实
  → endingResolved 经 022 持久化并确认周目终态
  → 可选 AI 基于冻结摘要生成非权威叙事
```

引擎在 enabled run 的规则快照中保存 schema / rule version、scriptId 与剧本 revision / hash、结局规则资源 hash、来源模式、规范化规则 JSON 及 hash。规则、资源与摘要 hash 的字节算法和覆盖字段见[剧本格式](../standards/scenarios.md#结局标签规则)。规则可以由剧本作者预设，或在新周目开始前按剧本白名单生成；静态资源声明与生成后快照复用同一 DTO / schema，并按资源声明或冻结快照用途执行不同的 `rules` 数量及目录一致性校验。生成规则的模型只看到已声明字段、运算符 / 值域和既有 tag catalog，不获得本局运行中的隐藏数值，也不能创造属性、可执行谓词、标签或事实。两种来源使用同一字段类型、范围检查、规范化和持久化流程。初始化先保存不可玩的 run / 预算，随后单事务冻结完整规则并将其激活；运行后重开、恢复、分支回退及剧本更新都沿用原快照。生成失败或结果不确定时不开始游戏、不暗中换规则；供应商物理请求按 027 / 035 记录，未知状态不自动重发。无 `ending-rules.json` 的新 run 冻结 `endingMode=disabled` 并沿用既有生命周期；旧存档缺少该字段时视为 `legacy-disabled`，不补生成规则、不进入 endingResolved 恢复分支。

剧本为每个隐藏数值声明稳定 `attributeId`、类型 / 允许范围 / 可用比较方式；tag catalog 定义稳定 `tagId`、语义事实、公开级别及可选互斥组。规则是类型化比较条件和产生的 tag ID，不是任意代码或自然语言 predicate。同一 tag 由多条规则命中时去重并保留全部匹配依据；互斥 tag 的解决规则必须显式登记，未声明冲突或所需属性缺失时不得猜测 / 当作零。所有顺序稳定且可重放。

只有 `endingMode=enabled` 的 run 在独立、可信剧本 / 世界状态终局条件提交后求值，不因某个 tag 单独命中就结束。其 `sessionEnded` applied 后，engine 以 runId、已提交终局 mutation ID、锁定 rule hash 确定性派生 resolutionId，再追加 `endingResolved` 系统记录并经 022 的 applied 回执提交冻结结果。结果至少绑定规则 hash、终局记录锚点、historyRevision、tag IDs、匹配 rule IDs / versions 和公开事实。`disabled` 与 `legacy-disabled` run 沿用此前结束与恢复语义，不要求或生成 `endingResolved`。原始隐藏数值只用于引擎判断，不进入 UI、公开错误或 AI 请求。标签只决定本次结局可成立的语义事实，不授予额外的世界状态写入权限；只有剧本明确标记可公开的 tag 说明进入玩家摘要与叙事请求。

## 规范摘要与 AI 文本

规范摘要从冻结的公开 tag / 事实按固定顺序经确定性模板生成，是唯一用于描述“发生了什么”的结局正史。需要逐句保证事实的内容只能由模板或剧本作者文本承担。

AI 接收原 resolution 中冻结的规范摘要、公开 tag 作者说明、summaryLocale 与版本化风格参数，生成可选的文学性结局叙事。重试固定以上输入和 summaryHash，只分配新物理 requestId；应用语言切换不改写已冻结摘要或重试输入。它不接触原始隐藏数值 / 私有 tag，不选择结局，不写世界状态；生成文本不进入记忆素材、记忆 evidence、未来回合规则上下文或权威因果投影。自然语言可能与摘要矛盾，prompt、schema 或另一模型都不能可靠证明语义正确；因此界面须区分“结局事实”和“AI 叙事”，保留事实摘要，不把 AI 文本显示为唯一正史。

规范终态不依赖 AI 请求成功。生成失败或额度不足时，周目仍可按冻结事实完成落幕，叙事状态单独标记 failed / pending，并提供显式重试。重试使用原 resolutionId、summaryHash 和新物理 requestId，不重判标签、不重复结束、不自动发送。供应商响应不确定时遵循 005 / 020 / 027 / 035 unknown 恢复规则，不当成零用量。

AI 叙事作为绑定 `resolutionId` 的结局展示产物单独保存，不能作为普通 `narration` 被 033 收集；该记录有独立类型、大小上限、生成状态、requestId 与 summaryHash。持久失败不覆盖规范摘要。通过系统回退改变历史分支时，旧终态只在其原分支有效；恢复选中的周目前缀时保留该周目原规则快照，达到终局条件后才能在新分支生成一条新 resolution。重复命令按 resolution / request 身份幂等。

## 模块与事务方向

```text
scenario resources → aoidos-script / engine catalog → engine ending resolver
engine ending resolver → existing world / record / billing ports
engine → aoidos-llm (rule generation or final prose, only on explicit authorized path)
engine → aoidos-memory (consume committed cycle-end facts; memory never owns ending)
src-tauri commands → engine (wire DTO / platform adapter only)
src-web API → Vue ending view (render authoritative snapshot, explicit retry)
```

配置规则的开局初始化先持久化不可玩的 `initializing` run 和原币预算快照，035 创建同 run 预算后才可发物理生成请求；启动恢复幂等补齐预算和初始化阶段。规则生成失败 / unknown 保留原 run 与账本记录，不激活游戏、不自动重发；放弃不删除费用历史。规则通过对应用途校验后以单事务保存快照并将 run 激活为 `playable`。没有结局规则的 run 记录 disabled mode 并沿用已有创建和结束流程。

仅 enabled run 的可信 `sessionEnded` 事实先由 023 / 022 提交；随后 engine 按该终局锚点和规则快照重建 `endingResolved`。若进程在两步之间崩溃，启动恢复检查有效历史路径，在相同输入下补交同一 resolutionId；pending 事实仍走 022 正常恢复。disabled / legacy-disabled run 不进入该补交步骤。resolution 已 applied 后，AI 叙事才可单独请求并持久化。这个顺序不要求 JSONL 与不同 SQLite 表构成跨资源原子事务；它依赖确定性身份、有效因果锚点和 applied 回执避免重复落幕。恢复不得重新调用模型或改变已冻结事实。标签最终事实提交后，032 只接收其已提交周期边界或允许的公开 tag 引用，不反向依赖 UI、Provider 或计费；035 只结算实际物理 LLM 请求，不决定能否落幕。依赖保持 `aoidos-engine → aoidos-memory / aoidos-llm / aoidos-billing`，无反向调用。

UI 消费权威 `EndingSnapshot`：规则就绪状态、是否满足终局、冻结事实摘要、公开 tag、叙事状态 / 可选文本及脱敏诊断码。页面不能发送自选结局 ID。普通失败只影响 AI 展示产物；仅事实快照损坏或规则集不可验证时阻止继续并要求恢复。

## 对接文档

[剧本格式](../standards/scenarios.md)登记字段与标签规则文件；[阶段机](turn-state-machine.md)登记终局触发位置与恢复阶段；[记录引擎](record-engine.md)登记类型化终局锚点与回退语义；[记忆架构](memory.md)登记落幕摘要 / AI 叙事的素材过滤；[计费架构](billing.md)登记开局规则生成与结局叙事请求计费；[通信契约](ipc-contract.md)登记命令与同型 DTO；具体阶段状态及验收由 056 跟踪。
