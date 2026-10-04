# 001 — Herta（黑塔）调查：UI 风格、LLM 配置与记忆系统

- 日期：2026-10-05
- 对象：本地仓库 `C:\Users\26221\game\Herta`（开源版 github.com/PersonaCLI/Herta，代码 MIT；星穹铁道黑塔同人，游戏素材不在 MIT 内）
- 目的：吸收其 UI 风格与 LLM 配置思路；UI 提炼为 [UI 风格规范](../standards/ui.md)，记忆与上下文思路回灌[构想 001](../ideas/001-context-state-decoupling.md)

## 一句话

「给自我以模型」的 AI 伴侣应用：Electron + React，单文件全局 CSS 的玻璃拟态；DeepSeek **completion 模式**做叙事驱动；四段式「自传」prompt 配缓存稳定前缀；梦记忆生态（衰减、驱逐、遗忘回写）。

## 架构总览

pnpm monorepo，十个包：`gui`（Electron 桌面，主产品）、`cli`（终端 REPL）、`app-server`（会话宿主：回合、梦触发、语音、审批）、`herta`（自我：叙事补全、prompt、recap）、`core`（编码后端运行时、权限引擎）、`tools`（工具集）、`knowledge`（梦管线、canon 知识库 SQLite）、`memory`（项目记忆）、`providers`（DeepSeek completion + chat 双工厂）、`website`（复用应用渲染器的介绍站）。

回合流：renderer → IPC → `session-service` → `SessionHost` → `V2ActorDriver.runTurn` → providers（SSE 流式）→ 追加进 TerminalRecord → JSONL 落盘。**TerminalRecord 是唯一基底**：UI 转录、磁盘持久化、LLM prompt、导出四位一体，prompt 只是它的带预算「投影」。

## UI 设计语言

完整提炼见 [UI 风格规范](../standards/ui.md)，要点：

- 单一 8300 行全局 CSS（`reference-ux.css`），无组件库、无 Tailwind；全部 token 是 CSS custom properties，`data-theme` 单属性切浅/深。
- 签名是「玻璃仪表盘悬浮 pastel 星云」：多层 radial 渐变背景预模糊一次，表面全是 18px 圆角半透明玻璃卡 + hairline + inset 顶高光。
- **双 register 排版语法**：人声 = 气泡（用户右灰 15px / 角色左白 16px，共享同一左缘 token）；机器声 = 12px 等宽行 + 7px LED 点（`#4d86ff` 脉冲）+ shimmer 渐变活动文字。系统事件不抢戏，去饱和，唯一的琥珀警示色必须赢。
- 动效是品牌：签名缓动 `cubic-bezier(0.2,0.85,0.2,1)`，140ms 微交互 / 800ms 签名飞行，雾化滚动边缘，33 处 `prefers-reduced-motion` 降级。
- Electron 壳：frameless + 自绘标题栏，主题背景色在构造时设好防冷启动闪色，最小 1280×720。

## LLM 配置

- **Key**：Electron `safeStorage`（Windows DPAPI）加密存 `userData`，IPC 只传 `{set, hint: 后4位, encrypted}`，明文 key 从不跨进程；CLI 降级走 env → cwd 文件 → `~/.herta/keys`。支持运行中热换 key。
- **端点与模型**：`https://api.deepseek.com`；只有两个模型名——`deepseek-v4-pro`（主演）与 `deepseek-flash`（后端/路由/裁判），按 `models.actor` / `models.backend` 分档配置（env > settings.json > 默认）。
- **叙事走 completion 而非 chat**：POST `/beta/completions`，prompt 是单条裸字符串，以 `（我 说）\n` 收尾等模型续写；stop 序列做护栏（闭合标签、伪造用户台词、`"\n### "` 逃跑拦截；DeepSeek 最多 16 个 stop）。客户端再做行首 stop 截断，保证流式输出 == 落库文本。
- **采样**：temperature 默认不设（1.0）；空输出重试阶梯 `[1.1, 1.2, 1.3]`（+0.1 步进——更大步进会「换了个人」）；max_tokens 2048；无 top_p。裁判调用不限 token 而设 60s 死线。
- **流式与重试**：手写 SSE 解析，头阶段 30s 与体空闲 90s 两个独立看门狗；共享重试环（2 次、500ms 指数 ±25% 抖动，只重试网络/429/5xx，TLS 错误不重试）；后端层 `maxRetries: 0`——它的回合循环有自己的 2/4/8/16s 退避，双层重试曾导致一次限流打十二次全量 prompt。
- **设置**：`<workspace>/.herta/settings.json`，极简（dream / backend.thinking / models），改枚举外值自动回退默认。

## Prompt 架构：四段自传 + 缓存纪律

- **四段**：身份（第一人称 bio + 行为指南，编译进安装包）→ 记忆（`### 废案_NN` / `### 记录` 文件按序加载）→ 世界（EnvSet 环境锚，放在最贴近续写点的位置）→ 此刻（TerminalRecord 序列化 + 格式提示 + open tag）。
- **缓存纪律（最值得抄的一条）**：静态前缀会话开始时冻结、字节级不变、永远在 prompt 头部 → provider 的 prompt cache 每轮全命中；一切动态内容 append-only 地长在缓存线以下。少数「为省 token 的重写」（diff 折叠、系统块压缩、思考省略）只发生在缓存线之下。
- **token 预算**：自研校准估算器（**汉字 = 0.65 token**，实测标定；ASCII ÷4）→ 估算工作集 ~200K 才触发 recap：60K 逐字近期尾部 + 有界第一人称 recap（`### 记录：先前`），recap 只允许增补、运行时拼接，防止懒惰模型毁掉背景；熔断 3 次失败 + 每 3 跳探测一次。

## 记忆 / 梦系统

- **存储**：活的记忆是纯文本文件（`### 废案_NN：标题.txt`），`manifest.json` 记账（批判分、复活数、情绪权重）；世界观 canon 用 SQLite + FTS。无向量库。
- **门控（便宜的在前）**：回声强化（确定性：新台词复用 ≥12 连续汉字即强化旧记忆，不调 LLM）→ 值得记（LLM JSON 门，默认拒绝）→ 已记过（标题新颖性 + LLM 复述判定：差的复述转为旧记忆强化）→ 声音忠实（voice ≥0.8、忠实 ≥0.7；**用户台词必须逐字引用真实消息**——凭空捏造的用户对话是不可证伪的污染，结构性拦死）。
- **衰减**：`strength = voiceScore · (1+0.5·charge) · 2^(−Δ天/90) · (1+0.5·ln(1+复活次数))`；容量 27 条，驱逐按最弱，归档不删除；复活即重置衰减钟。
- **遗忘即回写**：记忆死亡时一次 LLM 调用把它的要点重写合并进 600 字上限的「关于用户」页——「夜被忘掉，人留下来」。每次覆写前先备份到 archive。
- **触发**：默认关（烧用户 quota）；闲置 ≥30min + 距上次尝试 ≥1h + 距上次完成 ≥7 天 + 素材门槛（≥5 新会话或单会话 ≥25 轮）；用户回来时在集边界让位。

## 可借鉴处（评估与落点）

本节只做「是否特别适合 Mythos」的评估索引；设计细节以落点文档为准，不在本报告展开。

| 借鉴点                                 | 评估                                                                                     | 落点                                                              |
| -------------------------------------- | ---------------------------------------------------------------------------------------- | ----------------------------------------------------------------- |
| 记录基底 + 投影 + 缓存稳定前缀         | 特别适合：跑团长局比聊天更依赖成本可控；游戏视图与对话面板分离后，记录作为单一基底更关键 | [task 006（对局记录与上下文·设计）](../task/006-record-design.md) |
| completion + stop 护栏                 | 特别适合：叙事延续人设、拦截伪造玩家台词正是跑团刚需                                     | [task 005（LLM 接入与护栏·设计）](../task/005-llm-design.md)      |
| 校准 token 估算 + 工作集压缩           | 特别适合：上下文与对局时长解耦的核心算法                                                 | [task 006](../task/006-record-design.md)                          |
| 记忆生态（门控 / 衰减 / 回写）         | 适合但远期：依赖 005、006 与存储选型                                                     | [task 007（记忆系统·设计）](../task/007-memory-design.md)         |
| 双 register 排版                       | 特别适合，已落档                                                                         | [UI 风格规范](../standards/ui.md) 视觉语法；细则归 task 008       |
| 工程纪律（看门狗 / 原子写 / 密钥隔离） | 适合，属跨切面约束                                                                       | 原则见职责边界；细则见[通信契约](../architecture/ipc-contract.md) |

## 边界与注意

- 只借设计语言与机制，**不取任何游戏素材**（立绘 / 语音 / 文本不在 MIT 授权内）。
- Herta 的 CSS 大量使用 Chromium-only 特性（`@property`、`@starting-style`、`color-mix`）；对本仓库的可用性结论见 [UI 风格规范](../standards/ui.md) 的「平台风险」。
- Windows 文件语义（杀软 / OneDrive 锁、rename 竞争）逼出了它整套原子写 + 自愈层——持久化设计的参照。
