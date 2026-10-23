# 053 — 设计：记忆检索与共用搜索组件

- 状态：done
- 依赖：031
- 优先级：P2
- 创建 / 更新：2026-10-19 / 2026-10-19

## 目标与背景

052 对照 `aoidos-art` 发现记忆列表搜索入口尚未接入。当前 API 只支持有界分页；在浏览器中过滤已加载页会漏结果。本文定稿服务端有界检索协议，并定义可复用的 Vue 搜索输入组件。组件只复用输入交互，不把记忆领域查询抽象进 `shared/`。

## 必读

[记忆架构](../architecture/data/memory.md) · [通信契约](../architecture/core/ipc-contract.md) · [界面架构](../architecture/interface/ui-shell.md) · [前端规范](../standards/frontend.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md)。

## 范围与非目标

范围：当前周目的已提交记忆条目检索、筛选、快照分页、派生索引与失效规则；Vue 共用受控搜索输入组件及 memory feature 的组合方式。

非目标：不搜索原始对话 / 素材 / 供应商响应，不查询其他周目或无效历史分支，不改记忆规则与正文存储，不添加近似名称自动匹配、语义向量检索、后台轮询或通用搜索结果 / 请求框架。

## 设计决策

### 检索语义

- Rust 在当前 `runId` 与已确认 `historyRevision` 下检索当前版本；范围包括 active、archived 条目，不包括 unavailable 条目。来源失效、正文损坏或恢复中的版本不能因搜索绕过 `memory_get_entry` 的校验；打开结果仍走原有详情命令。
- 文本只匹配已提交版本的 `summary`、当前权威实体规范名及玩家确认且仍有效的别名。字段筛选支持主体类别、记忆维度、态度、变更类型和 active / archived 状态。未知枚举拒绝，不按展示文案筛选；不搜索来源正文、证据文本、文件路径、ID 或模型原始输出。
- `query` 由 Rust 统一执行 NFC、首尾空白移除及连续空白折叠；大小写按 Unicode 小写规范化。查询是字面子串，不解释 SQL / FTS 运算符，不做拼音、翻译、模糊或语义匹配。空查询可与筛选组合；空查询且无筛选时由 UI 使用既有列表命令。
- 查询最多 128 个 Unicode scalar value、512 UTF-8 字节；非法 UTF-8、超长输入、非法筛选或页大小返回 `app.bad-request`，不自动截断。匹配顺序按条目创建序号升序，稳定且不暗示相关性。
- 动态 NPC 使用当前有效实体目录中的规范名；别名仅使用当前周目、scope 与因果来源仍有效且由玩家明确确认的登记。近似拼写不做自动纠正；名称变更与别名更新推进搜索索引 revision。

### 索引、预算与游标

- SQLite 派生投影分为一条当前版本 entry 行与独立名称行（规范名 / 当前有效别名）；entry 投影与 `memory_entries` 版本 / 状态、别名来源有效性及 entity revision 同事务维护。投影不复制来源文本、证据正文或路径。单次搜索最多访问 256 个 entry 行、1024 个名称行、读取 256 KiB 名称文本、1 MiB 原始投影字节和 4 MiB 归一化工作字节；读取 SQLite 文本前用 `length(CAST(value AS BLOB))` 核对真实 UTF-8 字节数及字段上限，再分配字符串。原始名称最多 256 字节、归一化名称最多 1024 字节；原始摘要最多 4096 字节、归一化摘要最多 16 KiB；查询原始输入最多 512 字节、归一化结果最多 2048 字节。归一化长度单独校验，不复用原文字节上限；合法输入若归一化后仍超过上限，按输入超限拒绝，不标为存储损坏。其他字段沿用现存严格 schema 上限。超长或损坏投影返回 `store.corrupt`，不跳过名称或静默截断。名称行扫描游标独立记录当前 entry rowid 与名称 rowid，可在单个 entry 的别名集合中断后续扫，不遗漏、不重复。索引缺失 / 版本不兼容时按可信数据迁移重建；无法重建则返回 `store.*`，不退化为前端部分页搜索。
- 每次 IPC 最多返回 32 个 `MemoryEntrySearchItem`，完整序列化响应（items、cursor 与 envelope）最多 64 KiB，单项最多 32 KiB。搜索 DTO 只含 entryId、主体 kind / id / 规范名、dimension、attitude、change、state、summary 和有界修订标识，不附带完整来源 / evidence 集合；各字段沿用严格 schema 上限（ID ≤64 ASCII 字节、主体名 ≤256 字节、summary ≤4096 字节），枚举为固定集合。该形状保证每条当前 schema 合法记录均可装入单项预算：含 4096 个需 JSON 转义字符的摘要最多占 24,576 字节，其余 DTO 字段和 JSON 封装仍在 32 KiB 内。序列化预算不足以加入下一命中时，cursor 停在该 entry 之前，下一页重检该 entry；不得把合法记录判为损坏。单项超限仅表示其违反已声明的持久字段约束或 DTO 上限，才返回 `store.corrupt`。实现须用最大合法转义摘要及最大元数据验证单项能放入 32 KiB，响应整体不超过 64 KiB。entry 扫描或名称扫描 / 字节预算用尽时，返回已完成项及 cursor；即使当前页 0 命中，cursor 也表示仍有工作，只有 null 才表示搜索完成。不得因稀疏命中在同一命令中无限扫描。
- 新增 `searchRevision`，同一 run 的当前版本、状态、实体名或确认别名变化时与投影在同一事务递增。游标绑定 `runId`、`historyRevision`、`searchRevision`、查询 / 筛选规范化摘要、entry / 名称两组固定 SQLite rowid 上界及双层扫描位置。任一绑定值改变或 query/filter 不同，返回 stale-query 错误并要求从第一页刷新；不得续接混合快照。
- 游标无匹配时也按已扫描 entry / name rowid 前进；如果名称预算在当前 entry 中耗尽，cursor 继续该 entry 剩余名称，完成名称检查后才移动到下一 entry。序列化响应预算先于下一命中耗尽时不推进越过该命中。新建条目不进入旧快照；已存在条目发生变化会使整个旧游标失效。UI 不后台轮询、不自动遍历所有页；玩家点击“更多结果”才请求下一扫描页。
- 后端每条扫描路径均有固定行数、真实读取字节数、名称行数 / 文本字节数和响应序列化上限；现有记忆摘要上限为 4096 字节。索引只为当前版本提供检索，不改变不可变历史版本、正文 hash 或来源核验。

### IPC 与 UI

新增 `memory_search_entries`：

```ts
memory_search_entries({
  runId,
  query,
  filters: { state?, subjectKind?, dimension?, attitude?, change? },
  cursor,
  limit,
}) -> {
  entries: MemoryEntrySearchItem[],
  nextCursor: MemoryEntrySearchCursor | null,
  historyRevision,
  searchRevision,
}
```

Rust DTO 为权威并使用 camelCase、拒绝未知字段；TypeScript 仅镜像 Rust。页大小为 1–32。游标显式包含 run / 两种 revision、rowid 快照上界 / 位置、queryHash 与 filtersHash；不携带原始查询或敏感正文。过期游标使用 `app.bad-request` 且 `detail.reason=staleSearchCursor`；存储 / 索引失败保留 `store.*`。目标 run 不存在沿用 `app.not-found`。

`src-web/shared/components/SearchInput.vue` 仅提供必填可访问名称、受控字符串值、placeholder、disabled、输入事件和清除操作；清除按钮有本地化名称，清除后焦点留在输入框。组件不负责防抖、提交、筛选 chips、请求、分页、加载 / 错误 / 空状态、查询规范化或结果缓存。原生输入的 Enter / Escape 语义交给调用方，不截断 IME 组字。

memory feature 持有 debounce（建议 250ms）、filters、请求代次、cursor 与结果状态；输入或筛选变化立即清空旧结果 / cursor，递增 generation 并丢弃迟到响应。结果响应绑定 runId、historyRevision、searchRevision；周目或 historyRevision 改变时重置。搜索错误与无结果分开显示，cursor 非空时显示“继续扫描 / 更多结果”，包括本页零命中的情况。既有未筛选列表仍可走 `memory_list_entries`。

### 关键验收用例

服务端需覆盖：摘要子串与 Unicode NFC / 大小写（含 `İ` 等小写后 UTF-8 膨胀值）、最大原文字段归一化后仍合法、SQLite BLOB 字节长度检查、最大合法且含大量需 JSON 转义字符的摘要可单项序列化、空查询和纯筛选、literal 特殊字符、不同状态 / 维度 / 主体筛选、短页及零命中续游标、跨 run / history / search revision / query / filter 的游标拒绝、页面内新增 / 更新 / 别名变化使旧 cursor 失效、超过预算仍稳定续扫、来源失效或 unavailable 条目不绕过详情校验、损坏索引迁移失败关闭、DTO 未知字段拒绝。

前端需覆盖：受控更新 / 清除 / label 和焦点、IME 输入、debounce 清理、query/filter 变化清空结果、迟到页与 run/revision 变化丢弃、cursor 存在但零命中时不显示最终空态、卸载释放计时器；共享组件不承担记忆领域逻辑。

## 依赖与实施规划

031 已提供权威记忆条目查询及错误形状；053 定稿后由 [055](055-memory-search-impl.md) 实现 Rust 派生索引与搜索 IPC，并接入 `shared/SearchInput.vue` 和 memory feature。055 依赖 031、034、052、053；不改 031 已有命令语义，仅新增搜索能力。

## 验收标准

- [x] 查询范围、字段、Unicode 处理、排序、预算、续扫与过期语义完整定义。
- [x] SQLite 投影与权威版本一致性、隐私边界、动态实体 / 已确认别名语义明确。
- [x] Rust / TypeScript IPC 形状、稳定错误、共享输入组件职责和 UI 生命周期明确。
- [x] 根据独立评审修正续扫位置、Unicode 膨胀预算与合法单项序列化边界；独立复核通过，真实 DTO 序列化上界由 055 测试兑现。

## 决策与工作记录

- 2026-10-19：从 052 登记检索缺口；将“完整结果”放在 Rust 查询协议，不在已加载页中做局部过滤。
- 2026-10-19：用户提出使用通用搜索组件。确定组件只复用输入交互，领域筛选与结果状态留在 memory feature；没有第二类消费者前不建设通用搜索框架。
- 2026-10-19：基于现有 sqlite / memory model 定稿有界 SQLite 投影分页、run / history / search revision 绑定、动态 NPC 规范名与确认别名、隐私与 Unicode 字面子串规则；实现登记为 055。

## 完成摘要

搜索协议与共用受控搜索输入组件职责已定稿；独立复核通过。055 承接索引、IPC、UI 与行为验收实现，其中真实 DTO 序列化上界仍须由实现测试验证。
