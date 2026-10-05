# 界面结构与交互

更新 / 官方资料核验日期：2026-10-06。[task 008](../task/008-ui-shell-design.md) 的设计定稿，承接 [issue #11](https://github.com/Yuki-Nagori/mythos/issues/11)；尚未实现。色板、字体与玻璃表面以 [UI 风格](../standards/ui.md)为准，主题 / 皮肤及首窗初始化归[主题架构](theming.md)，本文维护布局、面板状态与操作规则。记录事实归 [006](record-engine.md)，阶段及骰判归 [012](turn-state-machine.md)，跨端类型归[通信契约](ipc-contract.md)。

## 舞台与覆盖层

舞台采用 1920 × 1080 逻辑坐标。可用舞台区为客户端扣除 48px 顶栏后的矩形，缩放 `s = min(width / 1920, height / 1080)`，渲染矩形居中，余边露星云背景。世界图像 / 坐标随 s 缩放，顶栏、对话面板和输入文字使用 CSS 像素，避免小窗口下文字被压小。

面板四态均为覆盖层，不改变舞台尺寸、坐标或布局。层级：背景 → 舞台 → 顶栏 → 面板 / 唤起按钮 → 指令菜单 → 模态确认。舞台坐标换算用实际渲染矩形和 s；指针在 letterbox 或面板内时不生成舞台操作。透明覆盖容器不拦截事件，只有可见控件接收指针。

```text
客户端
├── 顶栏：场景位置、阶段 / 恢复状态、设置入口
├── 舞台区：居中 16:9 舞台；边缘保留星云
└── 覆盖面板：标题与操作 / 有界记录窗口 / composer
```

面板宽度 `min(calc(100vw - 16px), clamp(360px, 32vw, 520px))`，右边距 8px，顶部接顶栏、底部距客户端 8px；v1 不拖拽。高度不足时标题与 composer 保持可操作，记录区独立滚动；禁止整窗横向滚动。常规窗口最小 640 × 480 CSS 像素，恢复到更小显示器 / 高倍缩放时仍按当前客户端裁定宽高，不依赖最小尺寸兜底。

舞台内容使用 aspect-ratio 与容器尺寸计算；不支持容器查询时用 ResizeObserver 更新可用矩形。不把浏览器页面缩放值当设备像素比。当前窗口配置仍为 960 × 640，逻辑分辨率与最小尺寸在界面落地时接入，不在本设计修改配置。

## 面板四态

面板状态属于 UI，独立于游戏 phase。纯函数接受输入事件和保护标志，返回目标状态 / 定时器效果；计时与 DOM 监听放 composable，不在组件中散布 timeout。

| 状态              | 可见性 / 交互                               | 进入与离开                                                                          |
| ----------------- | ------------------------------------------- | ----------------------------------------------------------------------------------- |
| collapsed（收起） | 面板 inert、不可聚焦 / 点选；唤起按钮仍可用 | 初次启动；显式收起或 hover 宽限到期                                                 |
| hover（悬停）     | 预览记录，不自动抢焦点                      | 触发区连续停留 180ms 打开；离开触发区及面板后等 350ms，无保护才收起                 |
| expanded（展开）  | 完整交互，无离开自动收起                    | 点击唤起按钮，或在 hover 中点击 / 聚焦面板；显式关闭、Escape 或安全的面板外点击收起 |
| pinned（钉住）    | 始终覆盖舞台；没有自动解除                  | 用户点击钉住；取消钉住转 expanded；“取消钉住并收起”按钮可直接转 collapsed           |

右侧触发区宽 8px，位于客户端右缘向内 8px、顶栏以下到距底部 16px；不贴 Windows 原生缩放边框，不抢标题栏 / resize hit-test。三平台 / DPI 必须实测；若与边框冲突增加内缩，不能扩大到整条舞台。无精细 hover 指针时禁用触发区，只用至少 32 × 32px 的按钮与键盘入口。

| 当前状态 / 事件                         | 结果                                                       |
| --------------------------------------- | ---------------------------------------------------------- |
| collapsed / PointerEnterTrigger         | 启动 180ms；离开或点击打开时取消旧计时                     |
| collapsed / HoverDelay；仍在触发区      | hover；预览不移动键盘焦点                                  |
| hover / PointerLeaveBoth                | 启动 350ms；重新进入取消                                   |
| hover / CloseDelay；无保护              | collapsed；保护存在时保持并等待保护解除后重计 350ms        |
| hover / 点击内容、输入获得焦点          | expanded；取消全部 hover 计时                              |
| collapsed / Open                        | expanded；按钮点击或快捷键唤起才聚焦 composer              |
| expanded / Pin                          | pinned，保存偏好                                           |
| pinned / Unpin                          | expanded，保存偏好                                         |
| expanded / Close 或 Escape              | 无组字 / 子菜单时 collapsed；组字时保持，先由 IME 消费按键 |
| pinned / Escape、PointerLeave、窗口失焦 | 保持 pinned；Escape 仅关闭子菜单 / 模态                    |
| 任意态 / NewRecord                      | 不自动展开；collapsed 更新未读 LED，打开态按滚动规则处理   |

自动收起保护包括：输入 / 面板控件焦点、IME 组字、非空内容选区、菜单打开、右键操作、拖拽 / pointer capture，以及“流式中且用户正在看”。后者精确定义为窗口聚焦、面板可见且正在跟随最新；用户上滚、离开或窗口失焦后不再因流式永久锁住 hover。保护不导致自动钉住。

展开态的外部点击仅在没有选区、拖拽、组字、菜单和模态时收起；这是用户显式点击，不复用 hover 自动定时器。草稿始终保留。关闭前把面板内焦点移到唤起按钮，再置 inert；关闭过程中不让不可见内容留在 Tab 顺序。[inert 官方说明](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Global_attributes/inert)

钉住偏好按应用保存，重启只恢复 pinned 或 collapsed，不恢复临时 hover / expanded、选区、焦点和定时器。切换对局保留面板偏好，清空旧未读 / 正文缓存；草稿仅按 sessionId 保存在进程内，最多当前及最近 4 局，每份不超过输入预算，退出时不写敏感草稿文件。

## 动效、焦点与滚轮

打开 450ms，收起 280ms，曲线 `cubic-bezier(0.2,0.85,0.2,1)`。收起位置 `translateX(calc(100% + 16px))` / opacity=0，打开 translateX(0) / opacity=1；只改变 transform 与 opacity，不动画宽度、舞台或 backdrop-filter。快速反向从当前计算值继续，不等 animationend 才改状态。

prefers-reduced-motion 时取消位移动画，使用 120ms 淡入淡出；LED 脉冲、文字 shimmer 和流式光标闪烁改为静态状态。控件的可用 / inert 切换按状态即时生效，不由动画结束回调决定；旧回调必须按状态代次忽略。

面板是非模态 aside，有可读标题；按钮提供 aria-expanded / aria-controls，钉住按钮提供 aria-pressed。Tab 在可见界面自然行进，不为面板锁焦点；需要确认破坏性回退时才使用模态并恢复原焦点。新记录不自动抢焦点；状态提示用简短 polite live region，正文流不逐 token 朗读。错误和三档结果同时有文字 / 图标，不只靠颜色区分。

键盘打开面板用 Ctrl+Shift+M（macOS 为 Cmd+Shift+M）；Escape 优先级为 IME → 指令菜单 / 上下文菜单 → 模态 → 非钉住面板。焦点处于 input、textarea、select、contenteditable 或模态时，舞台快捷键全部停用；菜单触发箭头键只由菜单处理。

记录滚动区使用 overscroll-behavior: contain；不要在 window 上统一 preventDefault。面板外滚轮归舞台，面板 / 菜单内滚轮不穿透；面板的非滚动 chrome 消费其区域滚轮，避免操作标题时舞台缩放。键盘 / 指针捕获以及全局快捷键监听在卸载时释放。

## 行类型与事实身份

一正式块可以显示多条视觉段落，身份始终为 sessionId + recordSeq；段落用此身份和递增 ordinal，不作为新事实 seq。连续 LF 形成空行分隔，多段落叠层；流式末尾的空段仅显示光标，不制造空气泡。跨 chunk 的双 LF 也须正确分段；取消 / 失败不丢已提交段落。

| 记录 / 状态                 | 呈现                                                                               |
| --------------------------- | ---------------------------------------------------------------------------------- |
| playerSpeech                | 右侧冷灰泡，原始输入可复制；outOfCharacter 加“场外”微标签                          |
| characterSpeech             | 左侧角色泡 + 名牌；浅色为白玻璃，深色使用对应角色泡 token                          |
| narration                   | 左侧通栏叠层、无名牌 / 引号，比角色泡更浅；不伪装角色台词                          |
| dice                        | 等宽机器行，规范骰式、逐骰与 total；由 Rust 结果渲染                               |
| check                       | 等宽行 + success / costlySuccess / failure 文字徽章；critical 扩展使用注册规则名称 |
| system                      | 灰机器行；错误 / 配额 / 恢复警示使用琥珀，message 已脱敏                           |
| recap                       | 默认折叠“前情提要”，按需取有界正文；不当作角色台词                                 |
| historyFork                 | 注册系统行“已回退”或“已重生成”；正式页只含有效路径，原始历史仍可导出               |
| tombstone / supersede       | 兼容提示，按 006 进入只读；v1 不解释为隐藏旧块 / 展示替代块                        |
| 未知 kind / 未知控制码      | 灰兼容机器行，有界原文查看 / 导出入口；禁止继续游戏                                |
| 流式预览                    | 当前公开 turn 的已提交正文 + 静态 / 闪烁光标；不把 partial journal 作为可见记录行  |
| cancelled / failed / orphan | 保留前文，加“已停止 / 生成失败 / 上回合中断”，不改为 completed                     |

失败判定用去饱和红，costlySuccess 用中性强调，success 用去饱和绿；琥珀留给需要用户处理的错误 / 配额 / 恢复警示。色值归 ui.md，本文不再维护一套色板。

预览按 turnId 对齐，收到正式 appended 后按 turnId / recordSeq 原位替换；取消或封口失败也遵循 006 的确认事实，不因动画删掉正文。historyFork applied 后 viewEpoch 变化，丢弃旧分页 / body 缓存和异步响应，再恢复当前有效窗口；标记新生成块与 fork 的关联时依赖 Rust 因果引用，不用“最近一条旁白”猜测。

## 工作状态与阶段条

顶栏 / composer 旁状态条描述 idle、generating、awaitingCheck、settling、advancing、resumeRequired、needsRecovery，不当作 JSONL 记录行。阶段、场景与公开 turnId 都由 engine_get_phase / 阶段事件给出；同阶段公开 turnId 变化也必须切换正文消费者。正文和历史分别遵循 021 / 022 的监听、快照、缺口与身份规则。

“工作过程”默认折叠为机器状态区域，可展开查看公开阶段、是否正在提议 / 叙事、已确认骰判、当前操作结果和脱敏错误。只使用既有快照公开字段；无法从字段区分的内部步骤不猜测。状态区域为有界当前视图，不新增日志历史；phase 必需提示始终可见，不能藏到折叠内容里。

005 的 reasoning 不累积、不落盘、不投递，v1 叙事关闭 thinking。因此本稿修订 issue 初稿的“展开 thinking 全文”：不提供原始推理或内部提议 JSON。未来若设计公开工作摘要，须先冻结来源 / 载荷 / 预算，不能从 Provider 私有流直连界面。

## Composer 与命令

单一文本框，输入解析规则归 Rust 012；前端纯 util 仅预览输入模式 / 指令候选，两端使用同一黄金例。记录保留 text 原文，不因去空白、换行或剥前缀改变玩家事实；mode 与解析出的内容范围是类型化元数据，prompt 用明确“角色内 / 场外”上下文，不把玩家前缀提升为 system 指令。

| 输入                                         | 行为                                                                               |
| -------------------------------------------- | ---------------------------------------------------------------------------------- |
| 无保留前缀                                   | inCharacter；以当前登记玩家身份提交                                                |
| `/ooc` 后跟空白及非空内容                    | outOfCharacter；`/oocx` 不匹配，只有 `/ooc` 提示补内容                             |
| 整条输入以 `((` 开始、以 `))` 结束且内部非空 | outOfCharacter；只移除解析视图的一层外壳，原文保留；不把正文中的局部括号当模式切换 |
| 空 `(( ))`                                   | 提示补内容，保留草稿，不 invoke / 写记录                                           |
| `/` 或未完成 / 未知 slash 命令               | 打开指令面板并保留草稿；Enter 不发送未知命令，也不写玩家记录                       |
| `/check`                                     | 标“尚未开放”，不作为手动骰判按钮的别名，不接受任意骰式                             |
| 首部 `\/`                                    | 字面 slash 台词；原文保留，不打开指令面板                                          |

slash 菜单 v1 仅提供既有操作快捷入口：/ooc、/stop、/interrupt、/resume、/regenerate、/rewind、/pin、/latest。/stop 等控制命令不成为 playerSpeech；/interrupt 的后续文本作为新输入原文提交；/rewind 打开目标检查点与影响确认，不允许自由输入未经核验的 seq 直接执行。/regenerate 显示骰值复用和可能收费确认。未知 / 禁用候选不触发 invoke；绕过 UI 的非法参数由 Rust 返回 app.bad-request。

Enter 发送，Shift+Enter 换行；event.isComposing 或本地 compositionstart 到 compositionend 期间不发送、不关面板、不选择 slash 候选。发送按钮也遵循组字保护；compositionend 后等待用户下一次显式发送，不用 timeout 自动补发。

先保留草稿并标 submitting，成功接纳且 operationId 已绑定后才清除该版本草稿；用户期间继续编辑时不能清掉新版本。Err / busy 保留内容，轻提示不写入记录、不自动重试。响应丢失时显示“提交结果未确认”，主动取 phase / record view 核验，不凭 text 相同自动重发；对局切换后的旧响应不改当前草稿。

| 引擎状态                            | 输入与操作                                                                           |
| ----------------------------------- | ------------------------------------------------------------------------------------ |
| idle、可写且有场景                  | 可发送；空白输入禁用                                                                 |
| generating / settling 中有公开叙事  | 普通发送不可重入；提供“停止”“打断并说”，裸 Enter 得 busy 轻提示                      |
| generating 中私有提议               | 提供“停止”“打断并说”；无公开 turnId 仍可用 roundId 控制                              |
| awaitingCheck、check.status=waiting | 锁正文发送；manual 显示“掷骰”调用 engine_submit_check；auto 不显示按钮，由 Rust 执行 |
| awaitingCheck、check.status=rolling | 禁用骰按钮，显示“正在判定”；不接受第二次掷骰                                         |
| settling（非叙事）/ advancing       | 锁发送与插话；允许停止 round，等待已开始提交到一致边界                               |
| resumeRequired                      | 显示“继续 / 新行动 / 回退”；继续必须显式确认，新行动先 abandonCheckpoint             |
| needsRecovery / 只读兼容            | 禁止发送、骰判与重生成；提供主动恢复或导出 / 诊断                                    |
| 无活动场景                          | 显示载入 / 已结束提示，不伪造 idle 可发送                                            |

操作优先级为 needsRecovery / 只读 → resumeRequired → phase / check.status。缺计划摘要时主动恢复快照，不臆造 waiting / planId；resumeRequired 优先于 check.status：暂停计划先显示“继续”，取得活动 roundId 后才允许 manual 掷骰；不能因快照含 waiting 就给旧 round 按钮。

默认 manual 骰判；用户可选择 auto，偏好在下一回合接纳时由 Rust 冻结，不热切换正在等待的 plan。UI 只提交 sessionId / roundId / planId 身份，不产生骰值；按钮防连击加 Rust 恰一次保护，取消 / 恢复 / 重生成仍复用已有 dice。重启后的待判定只恢复检查点，不自动掷骰或收费。

## 历史滚动、未读与有界状态

距底部 <= 24px 且无选区 / 正文展开 / 拖拽时，用户视为跟随最新；只在此状态追加或预览增长后贴底。用户上滚超过阈值立即停止跟随，新内容显示“跳到最新”浮标；点击后清未读并跟随。状态变化、错误及骰判不得强制把阅读位置拉到底。

历史加载从顶部哨兵或“更早记录”按钮发起，每个 session 一次分页请求；新页插入前保存顶部可见 recordSeq / 段落与像素偏移，插入后按该锚恢复位置。正文加载 / 窗口缩放也按锚更新；锚被有效路径变化移除时恢复最新窗口并明确提示“历史已回退”。

缓存沿用 006：最新窗口 + 最多 4 页历史、单个 bodyRef、32 个记录事件缓冲。显示区只挂载这些有界页；被淘汰页通过分页重取，不把 content-visibility 当内存上限。单个超大正文按 32 KiB 分段，并遵循磁盘行上限；客户端草稿 / 展开工作状态也有界。

v1 普通历史项用 content-visibility: auto + contain-intrinsic-size 降低绘制成本；活动预览、焦点项、选区和滚动锚保持正常布局。该属性不删除 DOM，屏外内容仍在可访问树，不能替代 inert 或虚拟化。[官方说明](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/content-visibility)

若有界页在最低目标设备上仍出现连续滚动 P95 帧间隔 > 33ms，再评估 TanStack Virtual；引入前验证动态段落高度、选区、贴底和辅助技术，不在设计提交增加依赖。v1 纯文本渲染，HTML / 图片 / 自动外链不执行；轻 Markdown 属于后续可选项，若接入 markdown-it 必须 html:false、禁原始 HTML / 自动外部资源并定义链接打开策略。

未读按“当前有效路径正式块”计数，不按 delta / 段落累计；取消 / failed 封口只计一次。collapsed 收到新块亮微光 LED，显示有界数字 99+，不自动展开。恢复、分页旧块和 fork 重放不重复增加未读；未知丢失区间只显示“有更新”，不能猜出精确数。切回前台 / 主动恢复取 phase 与 record view，最后所有事件丢失的限制如实保留，不加入持续轮询。

## 空态与错误

| 情况                            | 表现 / 恢复                                                                                        |
| ------------------------------- | -------------------------------------------------------------------------------------------------- |
| 无对局 / 未载入场景             | 舞台说明 + 载入入口；面板空态，不调用未存在的业务命令                                              |
| 首次取快照 / 分页               | 有界骨架或加载行；保留已显示事实，单次失败可主动重试                                               |
| 无记录                          | 中性开场提示；不制造虚构玩家 / 旁白块                                                              |
| llm.empty-output                | 无空气泡；按 stop / guard / length 显示“空输出 / 被护栏截断 / 输出上限”，与 failed 事件 / 快照一致 |
| 取消 / 生成失败                 | 保留已确认正文、脱敏错误和恢复入口；重生成不自动执行                                               |
| app.busy / engine.invalid-phase | 保留草稿 / 当前事实，取快照后重新判断可操作项；busy 轻提示                                         |
| engine.no-scene                 | 刷新活动场景 / phase，显示未载入或已结束；禁止发送，不伪造可操作场景                               |
| store.* / needsRecovery         | 琥珀恢复条，禁用修改；显示诊断 / 原始历史导出入口                                                  |
| app.not-ready（迁移冻结）       | 主动查 store_get_migration 诊断；不把它当新安装、版本 0 或空记录                                   |
| app.event-failed / 断开         | 保留内容，按各流快照恢复；不重发正文、不重掷、不无限重试                                           |

错误按 code 分支；message / finishReason 只在已有契约允许时显示，旧快照与 cancelled 不残留上一终态原因。任何原始 prompt、凭据、reasoning 或文件路径都不进入 toast / 工作状态。

## 偏好、依赖与实施边界

拟新增 Rust 拥有的 UiPreferences：version=1、panelPinned=false、diceMode=manual；只保存这两个产品偏好，不持久化焦点 / 滚动 DOM / 草稿。通过 store_get_ui_preferences / store_set_ui_preferences 薄命令读写；SQLite / 受控原子配置存储按 011 既有迁移与文件规则选用，具体 schema 在实现中定型。偏好写入使用应用级单队列，发送时合并两字段的最新意图，至多一个请求在飞，防止完整替换覆盖另一次设置。响应按设置代次处理，失败保留上次确认值并提示，不把乐观状态当已保存；钉住保存失败撤回至已确认偏好，但不清草稿。初始化恢复只应用一次，迟到快照不得覆盖用户已操作的状态。

窗口位置 / 尺寸复用 026 的受控 main 构建入口，使用 tauri-plugin-window-state 的 Rust 装配，插件只负责窗口，不负责面板钉住；恢复到已断开的显示器时约束到当前可见工作区。无须为保存窗口状态把文件权限开放给 Webview，确需 JS 插件命令时按官方权限单独登记。[官方插件文档](https://v2.tauri.app/plugin/window-state/)

ts-rs 仅是未来同型类型生成候选，当前 Rust Serialize 与手写 TS 类型仍同次维护；issue 的“继续 ts-rs”不表示仓库已有它。引入时通过根 Cargo workspace / 锁文件与 CI 验证，不能把生成工具当运行时校验。贴底、IME、hover / 输入预览纯逻辑放 utils 配单测；监听和生命周期放 composable，组件负责编排，不引入全局状态库或组件库。

008 完成设计，[025 界面实现](../task/025-ui-shell-impl.md)承接舞台容器、面板、记录展示、composer 与窗口状态。004 提供样式与 token 底座；021 / 022 / 023 分别提供正文恢复、记录消费与阶段 / 骰判契约，024 验证产品链路；026 提供主题 / 皮肤与首窗 bootstrap；025 消费这些能力，不重复实现恢复、持久化、主题校验或判定。025 的设置视图消费 026；立绘、地图与主题皮肤制作 / 校验不属于 025。

## 实现验收矩阵

| 领域           | 必须覆盖                                                                                                  |
| -------------- | --------------------------------------------------------------------------------------------------------- |
| 布局           | 1920×1080、960×640、640×480、极窄 / 高缩放；四态舞台矩形不变、面板无横向溢出                              |
| 面板           | 180 / 350ms 边界、快进快出、反向动画、焦点 / 组字 / 选区 / 拖拽保护、钉住重启                             |
| 键盘与辅助技术 | Enter / Shift+Enter / IME、Escape 优先级、Tab / inert、无 hover 设备、live region 不逐 token 播报         |
| 记录           | 全部 kind、跨 chunk 双 LF、预览原位封口、未启用控制只读、超大正文、旧 viewEpoch 丢弃                      |
| 输入与骰判     | 模式黄金例、未支持 slash、草稿版本、busy 无记录、手动 / 自动偏好、重复点击不重掷、暂停显式恢复            |
| 恢复           | subscribe / snapshot 竞态、同阶段 turnId 发现、旧响应、缓存上限、最后事件全丢失 / 重连、finishReason 清理 |
| 性能与平台     | 三平台 / DPI、缩放边框、reduced-motion、CSS 降级、滚动锚与 P95、窗口恢复到可见屏幕                        |

设计中的数值是 v1 初值，须上述实测后回写偏差；本文参考例 / 规则检查不替代 Webview、IME 或辅助技术的真实验证。
