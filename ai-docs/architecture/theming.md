# 主题与剧本皮肤

更新 / 官方资料核验日期：2026-10-10。[009](../task/009-theming-design.md) 的设计定稿，承接 [issue #12](https://github.com/Yuki-Nagori/aoidos/issues/12)；实现由 [026](../task/026-theming-impl.md) 承接，目前进行中。本文维护 token 目录、主题偏好与皮肤校验 / 应用管线。颜色与视觉默认值归 [UI 风格](../standards/ui.md)，布局和交互归 [界面结构](ui-shell.md)，精确命令 / 错误形状归[通信契约](ipc-contract.md)，存储与路径规则归[存储基建](storage.md)。

## 分层与真源

采用方案 B：语义 CSS custom properties 是唯一运行时真源，声明在 `:root` / `:root[data-theme]`；Tailwind 的 `@theme inline` 仅将它们映射为工具类。组件消费语义 token，不以 Tailwind 默认色板或 `dark:` 分支再维护一套颜色。普通排版工具类可继续使用，皮肤只能写目录中明确允许的语义 token，不能写入 Tailwind 映射别名（如 `--color-*` / `--spacing-*`）或其他未登记变量；不能仅凭变量前缀判定权限。

主题以单一、平级的 `ThemeId` 建模：`dark`、`light`、`light-purple` 与模板登记主题都是同一类可选主题，不再区分 `ThemeMode` / `ThemePreset` 层级。ID 使用 1–64 字节小写 ASCII slug：字母或数字开头结尾，中间只允许字母、数字和单连字符；内置 ID 保留，模板 ID 以稳定 `scriptId` 为前缀避免冲突。每个 ID 对应一套完整默认 token 和 `colorScheme: "light" | "dark"`。`colorScheme` 只提示原生控件 / 浏览器绘制，不是主题分类、父主题或偏好字段，不得从 ID 推断。模板主题须在受信任的模板元数据中显式登记，不从剧本文本或 CSS selector 推断，且不改变 039 的原文解析边界。ID 冲突时拒绝注册，不按加载顺序覆盖。

每个主题只有一个默认值真源：内置主题以应用 CSS 为源，模板主题以受信任元数据中的完整 token 常量为源。构建产物（主题 CSS 与 Rust 快照）均从这两种源生成，不能手动维护副本。模板元数据还提供稳定 ID、显示名称和 `colorScheme`；默认值经同一类型目录校验。缺少 / 非法 token 或重复 ID 会使该主题登记失败。运行时 `theme.css` 仅提供可写 token 的覆盖，不定义主题默认值，也不自行创建主题。026 的只读登记夹具验证这条路径；任意剧本包导入仍在本任务范围之外。Rust 目录向前端提供主题选项与描述，使设置界面不另维护第二份主题清单。

004 实现应用默认 token 与映射；026 的 Rust 业务 crate `aoidos-theme` 维护目录元数据、类型化校验与规范输出，存储操作通过 store 访问，不能直接持有 SQL 连接。前端只应用 Rust 交出的值、维护显示代次，不读取 theme.css 或补一套解析器。实现状态以本 task 的验收记录为准。

实现时目录元数据逐项列出 name、域编号、valueKind、skinWritable、alias?，Rust 用封闭目录表；默认 CSS 与它做集合互校。内置默认值只在应用 CSS 维护，模板默认值只在模板元数据维护；Rust 目录只定义类型 / 约束，不复制色板。004 先用设计目录夹具检查实际 CSS 与 Tailwind 映射；026 落地 Rust 目录后，由其导出同一夹具并补齐三方互校，不要求 004 等待 026。Rust 测试核验导出来自唯一目录，避免手写多份白名单。目录产物在同一提交更新，不依赖联网生成器。Rust 解析 var 的默认环境来自 CSS 构建导出的按 ThemeId 索引快照：026 经根 bun 脚本提取、规范化并嵌入，verify 检查快照新鲜；不能手写第二份默认色值。默认 CSS 所需 token 均为可独立解析的常量，快照与实际 CSS 互校。

## 首版 token 目录

首版收录 ui.md 已具名的 token，另给其已有的状态色角色补齐名称；不为约百个 token 的旧估计预建条目。编号只用于目录审计，不进 CSS 名称或运行时协议，不重用被移除的编号。

| 编号 | 语义名称           | 类型         | 皮肤可写 | Tailwind 映射 / 消费                     |
| ---- | ------------------ | ------------ | -------- | ---------------------------------------- |
| C01  | `--ink`            | Color        | 是       | `--color-ink`                            |
| C02  | `--muted`          | Color        | 是       | `--color-muted`                          |
| C03  | `--accent`         | Color        | 是       | `--color-accent`                         |
| C04  | `--accent-violet`  | Color        | 是       | `--color-accent-violet`                  |
| C05  | `--accent-warm`    | Color        | 是       | `--color-accent-warm`                    |
| C06  | `--panel`          | Color        | 是       | `--color-panel`                          |
| C07  | `--bubble-user`    | Color        | 是       | `--color-bubble-user`                    |
| C08  | `--bubble-persona` | Color        | 是       | `--color-bubble-persona`                 |
| C09  | `--hairline`       | Color        | 是       | `--color-hairline`                       |
| C10  | `--app-bg`         | GradientList | 是       | 组件 background-image 直接消费           |
| C11  | `--led-active`     | Color        | 否       | `--color-led-active`                     |
| C12  | `--diff-add`       | Color        | 否       | `--color-diff-add`                       |
| C13  | `--diff-remove`    | Color        | 否       | `--color-diff-remove`                    |
| C14  | `--warning`        | Color        | 否       | `--color-warning`                        |
| C15  | `--danger`         | Color        | 否       | `--color-danger`                         |
| S01  | `--record-left`    | PixelLength  | 是       | `--spacing-record-left`                  |
| M01  | `--ease-signature` | Easing       | 是       | 组件 transition-timing-function 直接消费 |
| M02  | `--dur-micro`      | Duration     | 是       | 组件 transition-duration 直接消费        |

域含义：C 色彩 / 背景、T 字体、S 间距、R 圆角、E 阴影 / 玻璃、M 动效、Z 层级。T / R / E / Z 首版没有具名条目，不接受凭域前缀猜测的变量；后续登记 T / R / E 可配置既有视觉属性，Z 始终不可由皮肤覆盖。字体栈、字号与圆角目前仍由 ui.md 的组件规则定义，不宣称 v1 皮肤能调整所有字体和玻璃参数。

状态色的角色和色相固定，各主题可采用不同亮度；警示琥珀、危险色、成功 / 失败对应 diff 色及活动 LED 全部不可覆盖。皮肤也不能改变层叠、舞台 / 面板尺寸、触发时序、inert、可见性或指针处理；008 的面板动画和 reduced-motion 规则优先于可配置的普通微交互。

新增 token 须先在所属设计 / UI 规范登记语义、默认值、类型、边界与可覆盖性，再同步目录、默认 CSS、映射、Rust / TS 类型及测试；独立新增范围建 task。类型 / 布局约束尚未冻结的条目不进入白名单。T / R / E 扩展也须给出解析子集及辅助技术 / 性能验收，不能通用放行 CSS 值。

### Tailwind 接线

以下为映射片段，不是完整 CSS；004 按目录为需要工具类的项生成别名，标为组件直接消费的项不进入 `@theme`。先清默认颜色命名空间，再使用 inline 引用。不要重设整个 `--*` 命名空间，以免一并删掉普通排版和响应式默认值。[Tailwind 官方主题说明](https://tailwindcss.com/docs/theme#referencing-other-variables)

```css
@import "tailwindcss";

@theme {
  --color-*: initial;
}

@theme inline {
  --color-ink: var(--ink);
  --color-accent: var(--accent);
  --color-warning: var(--warning);
  --spacing-record-left: var(--record-left);
}
```

`--app-bg` 是渐变列表，不能别名为 `--color-app-bg`；Tailwind 本身支持 `--ease-*` 主题命名空间，但本项目 M01 `--ease-signature` 只在 `:root` 定义，由组件通过 `transition-timing-function: var(--ease-signature)` 直接消费，不进入 `@theme`，不生成 `ease-signature` 工具类，也不做同名 `var()` 自引用；M02 `--dur-micro` 同样直接消费。默认 token 需即使未出现在工具类中也保持存在，不能依赖 Tailwind 按使用量生成它们。开发测试验证 `text-ink` 等编译为语义 var 消费，旧调色板类不再可用；不只比较类名字符串。

## 应用主题偏好与防闪

应用级偏好保存一个平级 `ThemeId`，内置选项包括 `dark`、`light`、`light-purple`；当前剧本显式登记的主题也进入同一选择列表。默认 `dark`，不跟随系统，不使用 localStorage 或浏览器探测决定主题。026 拥有 ThemePreference 的 Rust 读写，SQLite 在实例锁内按 011 规则迁移；它是独立配置项，写主题不完整替换 022 的 panelPinned / diceMode。UiPreferences 的现有两字段契约保持不变。非法格式 / 损坏值为 `store.corrupt`；语法合法但当前未登记或不可用的 ID 视为 `themeUnavailable`，保留已确认偏好，当前视图临时使用 `dark`，不得回写降级值。用户设置新偏好时只接受当前主题目录登记的 ID。

首窗创建顺序如下，026 与 025 的窗口状态插件复用同一个 main 构建入口，不创建第二个主窗口：

```text
Rust：解析数据根 → 持实例锁 / 打开 store → 载入受信任主题目录与完整默认值
    → 读取确认 ThemeId 或默认 dark → 校验当前视图可用主题
    → 冻结 theme / colorScheme bootstrap → 注册 initialization_script → 创建 main
HTML：根节点 → head 首个受信任 bootstrap 设置 data-theme → 默认 CSS → Vue
UI：沿用 bootstrap 主题 → 获取剧本登记主题 → 订阅 / 查询业务状态 → 应用对应皮肤
```

当前 main 由配置自动创建，026 落地时改为受控创建，保留标签、窗口配置、事件目标与 capability。窗口 / Webview backgroundColor 为 ui.md 深色星云底色，不以透明或白色兜底；该原生底色不属于可覆盖皮肤。防闪验收针对首帧应用内容，原生窗口装载间隙的深底是明确的启动表面。

Tauri 的 initialization_script 在 HTML 解析前运行，documentElement 可能不存在。脚本只注入经目录校验的 ThemeId、对应的 `colorScheme` 和可选 fallbackReason 枚举（storageUnavailable / invalidPreference / themeUnavailable）；HTML head 的首个受信任脚本在根节点创建后、任何应用 CSS / module 前设置 data-theme 与 color-scheme。不能对 null 根直接赋值后就宣称防闪，也不能等 Vue mounted / 异步 invoke 才修色。[Tauri Builder 文档](https://docs.rs/tauri/latest/tauri/webview/struct.WebviewWindowBuilder.html#method.initialization_script)

034 在同一初始化脚本内追加独立 LocaleBootstrap，语言 / 主题各自版本，不二次创建 main；细节见[国际化架构](i18n.md)。026 保留主题主责，不自行实现翻译。

bootstrap 只接受 Rust 主题目录中登记的 ID，来源 / 主 frame 按实际应用协议与精确 dev origin 核验，不把主题文件、路径或未登记字符串拼入 JS。Windows 的子 frame 行为也需防护；生产不加载第三方 frame。普通浏览器开发没有 Rust bootstrap 时确定性用 dark，不读旧网页偏好；未知 ID 回退到 dark 并报告诊断。

首窗防闪不自动覆盖任意文档导航：生产外部导航拒绝，允许的显式重载须先由 Rust 重新读偏好再重建 main；不能在新文档继续用创建时的旧枚举。开发 HMR 保留文档，开发 full reload 的差异单独记录，不伪装为生产防闪已通过。

读取偏好失败不抹除已存值：启动可用 dark 降级，bootstrap 的 fallbackReason 明确通知 UI 这是降级，不能当作确认偏好；界面显示简短诊断并主动 get 核验，既有迁移快照另按其契约消费，不捏造迁移状态；非法格式 / 损坏持久值为 store.corrupt，不回写默认覆盖。语法合法但当前未登记或不可用的 ID 以 themeUnavailable 降级，保留已确认偏好。迁移冻结保持 app.not-ready / 既有诊断规则；恢复后的主题只能在当前视图代次中应用。

### 运行时切换

用户切换 → 串行调用 theme_set_preference → 持久化确认 → 同一帧替换有效皮肤并设置 data-theme / color-scheme。`color-scheme` 由目标主题描述提供，不能通过 ID 推断。磁盘提交不能承诺与 DOM 同帧；同帧仅约束确认后的视觉更新。皮肤加载状态不能阻塞应用主题切换；缺目标主题皮肤时当帧移除旧覆盖，回应用该 ThemeId 的完整默认值。

每应用最多一个主题写入在飞，连点合并为最新意图；响应按请求 / 视图代次确认，不用较旧结果覆盖最新选择。失败保持最近确认的视觉值并提示，写入结果不确定时主动 get 核验，不重复写来猜测；未确认的选择不声称“已保存”。重启只恢复确认偏好，不保存皮肤 CSS 副本或临时 DOM 状态。

## theme.css 格式与预算

Rust 只为已登记的 scriptId 查可信目录映射，读取固定 theme.css，不接受路径参数。script-id 遵循 011；未登记 id 为 app.not-found。026 提供受控目录 / 最小登记夹具，不实现通用剧本 zip 导入；第三方导入未来须先满足路径消毒和安全解包条件，不能仅用 join_under_root 宣称阻止符号链接穿越。

包目录由应用控制、只读且在读取时稳定；拒绝符号链接 / 非普通文件、核验受控根内路径，使用有界读取（上限加 1 字节检测）而非先读完整文件。不以单次 canonicalize 抵御不可信进程的并发替换；不满足稳定目录前提时拒绝加载。theme.css 缺失不是错误，返回 missing；存在但读取失败返回 store.* 并移除旧皮肤。

每个 theme.css 只接受精确的主题 ID 选择器，空白 / 注释及单 / 双引号经 CSS tokenizer 解析后归一化；选择器只能引用当前剧本已登记主题，不接受选择器列表、组合器、伪元素、类 / id、data-script 或其它属性。源文件不提供作用域，data-script 由应用生成。

```css
:root[data-theme="dark"] {
  --accent: #68b8a8;
  --panel: rgba(20, 28, 36, 0.8);
  --dur-micro: 160ms;
}

:root[data-theme="light-purple"] {
  --accent: #705697;
}

:root[data-theme="mistbell-dawn"] {
  --accent: #8b5f76;
}
```

缺少某个主题块时，该主题完全使用其应用默认；不借用其他主题块或无主题 :root 块。存在主题块时只覆盖其合法条目，未列出的 token 继承该主题应用默认；空块 / 全部条目被丢弃仍可返回 valid + warnings / 空映射。

| 限额                | v1 硬上限                                  |
| ------------------- | ------------------------------------------ |
| 原文件              | 32 KiB UTF-8，禁止 NUL / 无效 UTF-8        |
| 规则块 / 总声明数   | 8 块 / 128 条，重复与非法条目也计数        |
| 单值 / 嵌套深度     | 1024 字节 / 8 层，包括 var / 函数          |
| 渐变层 / 每层 stops | 4 层 / 2–8 个                              |
| 返回 warnings       | 32 条；超出仅置 warningsTruncated=true     |
| 完整 IPC 返回       | 64 KiB 序列化 JSON，超限整份作废           |
| 客户端缓存          | 当前剧本的验证输出有界缓存；切换清退旧输出 |

不监视任意文件变化，不持续轮询。当前 script 的显式重载重新校验，sourceHash 是原字节 SHA-256，只作身份 / 陈旧检查，不是信任背书。零大小文件视为无覆盖 valid；带 BOM 的 UTF-8 首个 BOM 可去除后解析，hash 仍按原字节计算。

## 校验、引用与规范化

依赖候选 cssparser；它提供 tokenizer / parser，026 必须在其上实现本项目子集，不能把“库解析成功”视为皮肤合法。lightningcss 非首版必需依赖，不为压缩输出再引入一套值解释器。[cssparser 官方 Parser 文档](https://docs.rs/cssparser/latest/cssparser/struct.Parser.html)

| valueKind    | 首版合法语法 / 范围                                                                                                                                                                |
| ------------ | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Color        | #RGB / #RGBA / #RRGGBB / #RRGGBBAA；逗号型 rgb / rgba（通道整数 0–255，alpha 0–1）、hsl / hsla（hue 0–360，saturation / lightness 0–100%，alpha 0–1）；transparent；输出 #rrggbbaa |
| GradientList | 仅 linear-gradient（0–360deg）或 radial-gradient（circle at x% y%，0–100%）；每个 stop 为 Color + 1–2 个 0–100% 非递减位置，最多四层；不允许 repeating 或外部图                    |
| PixelLength  | --record-left 的有限数值 4–24px，不接受 rem、负值或百分比                                                                                                                          |
| Duration     | --dur-micro 的有限数值 80–800ms；可输入等价 s，规范输出 ms；不改变面板 / IME / 收起保护时序                                                                                        |
| Easing       | linear / ease / ease-in / ease-out / ease-in-out，或 cubic-bezier 的四个有限数值均在 0–1；不接受 steps / 任意函数                                                                  |

Color 先按上述范围解析为 sRGB / alpha，再将各通道乘 255 并取最近整数（半单位向上），输出八位小写 hex；hue=360 等价 0。所有数值先拒绝非有限值，不靠浏览器截断非法范围。

所有类型可整值使用 var(--目录名)，不接受 fallback 参数；渐变 stop 的 Color 也可引用 Color。引用必须同 valueKind，不引用 Tailwind 别名或未知变量；不可写的状态 Color 可被读取，但不能被覆盖。普通 CSS-wide 关键字、currentColor、calc / min / max / clamp / attr / env / url / image-set / src / expression 均不在子集。皮肤不能通过值间接请求文件、字体、网络或改变布局控制属性。

按 tokenizer 解码名称 / 函数后判定，不能 regex 搜索 url 作为唯一防线；大小写、转义、注释和未知 / 嵌套函数都须穿过类型解析。CSS custom property 名大小写敏感，函数 / 单位按合法语法归一化。禁止 !important；普通属性、未知 token、受保护 token、错误类型、非法值或引用，作为单条 warning 丢弃，不让浏览器修错。

每个主题按文件顺序取最后一个通过单条检查的候选；单条检查失败的重复不抹去之前候选。先建立可写条目的引用图，检查循环 / 深度 / 类型，再针对该主题默认值解析 var。循环成员及依赖该循环 / 错误引用的条目一并丢弃，其他无关条目继续生效；最终引用解析失败的覆盖回应该主题默认值，不回溯到更早的同名候选，也不借用本次被丢弃的皮肤值。最终输出只含已解析的类型化常量，不保留 var / 原始值 / 注释；即使返回值再次拼装 CSS，也不会重新启动引用或资源解析。

结构级错误整份 theme.invalid-skin：未知 / 越界选择器、任何 at 规则（包括 @import / @font-face / @media / @property）、嵌套规则、坏括号 / 不完整字符串 / tokenizer 错误，以及任何文件 / 计数 / 深度硬限超出。即使 cssparser 或浏览器能容错，也不接受结构错误前面的半份输出。单个完整声明的语义失败才允许局部丢弃。

```text
load(scriptId):
  lookup registered immutable root; bounded-read fixed theme.css
  missing -> empty maps; IO -> typed store error
  reject invalid encoding / structural errors / exceeded budgets
  parse only exact selectors for themes registered by this script, counting every declaration
  for declaration: decode; check catalog / protection / grammar / !important
    valid -> remember candidate; invalid -> bounded warning
  for each ThemeId: resolve typed reference graph against that theme's defaults, dropping invalid dependencies
  serialize constants in catalog order; enforce full JSON budget
  return scriptId / sourceHash / ThemeId-indexed maps / bounded warnings
```

warnings 只有固定 code、theme?、token?、line?，token 仅目录名或最多 64 字节的受限 ASCII 候选，非安全名称省略，不回传原值、CSS 片段或磁盘路径。代码包含 unknown-token / protected-token / invalid-declaration / invalid-value / invalid-reference / cyclic-reference；line 是源文件 1 起行号。warning 只是诊断，不持久化成玩家记录，也不改变场景 / 判定状态。

### 黄金走查

下表只验证设计结果，026 应转为 Rust / Web 夹具；没有当前运行解析器。

| 输入 / 场景                                           | 预期                                                              |
| ----------------------------------------------------- | ----------------------------------------------------------------- |
| `dark` 中 --accent: #abc                              | 常量 #aabbccff；`light` 与其他主题保持各自默认                    |
| 同名 #abc 后跟 2px                                    | 第二条 invalid-value，保留第一候选                                |
| --accent: var(--muted)                                | 读取当前 ThemeId 的有效 muted 或其默认值，输出 Color 常量         |
| accent / muted 相互引用                               | 两者 cyclic-reference，依赖者一并丢弃；不恢复更早候选             |
| --ink: var(--dur-micro)                               | invalid-reference，不安装跨类型引用                               |
| var(--unknown) 或带 fallback                          | invalid-reference / invalid-value，按失败阶段丢条，不交浏览器补救 |
| 每个内置主题 --app-bg 的全部默认渐变，起始 stop 为 0% | 满足 GradientList 子集；默认快照和照抄的皮肤值均可解析            |
| --app-bg 的 stop 使用无单位 0                         | invalid-value；不扩大仅允许百分比位置的子集                       |
| --ease-signature: cubic-bezier(0.2,0.85,0.2,1)        | Easing 常量；仅组件直接消费，不生成 @theme 别名                   |
| --warning: #000                                       | protected-token；原状态色保留                                     |
| --dur-micro: 79ms / 80ms / 800ms / 801ms              | invalid-value / 接受 / 接受 / invalid-value                       |
| --record-left: 0px / 4px / 24px / 25px                | invalid-value / 接受 / 接受 / invalid-value                       |
| 转义 url 函数、image-set、env 或 !important           | 单条拒绝；同块无关合法项仍生效，无资源请求                        |
| :root 无主题、选择器列表或嵌套规则                    | theme.invalid-skin，整份无输出                                    |
| @import / @media / @font-face / @property             | theme.invalid-skin，不交 CSSOM 静默忽略                           |
| 无效 UTF-8 / 第 129 声明 / 第 9 层嵌套                | theme.invalid-skin，即使多余声明后来会被丢弃                      |
| 选中主题缺少皮肤块                                    | 移除旧覆盖，使用当前主题完整默认值，不沿用其他主题                |
| 切换剧本后旧请求成功 / 用户关闭皮肤                   | 旧请求不安装；关闭后主题切换不重新启用                            |

## 前端应用、隔离与回退

收到 Rust 已验证的 tokens 后，前端用固定模板生成 :root[data-theme][data-script] 规则。scriptId 只能取已登记 ASCII 标识，不能拼任意输入；前端不接收原 CSS、任意 selector 或 HTML。优先 CSSStyleSheet.replaceSync + adoptedStyleSheets；不能依赖 replaceSync 静默丢掉 @import 提供安全校验。[replaceSync 官方说明](https://developer.mozilla.org/en-US/docs/Web/API/CSSStyleSheet/replaceSync)

皮肤样式不进 Tailwind 的低优先级 layer；生成规则位于应用组件样式之外并以受控根属性限定，只改变 custom properties。不得在 root.style 长期残留覆盖导致移除样式后仍不回默认，也不得把样式表插入顺序当唯一作用域边界。replaceSync 后核验规则数量 / token 集合，异常回默认，不安装部分样式。

切换 script、显式关闭皮肤或卸载均递增 skinEpoch；先清退旧皮肤 / data-script，再开始最多一个加载请求；请求中切换只合并最新目标，旧响应仅完成清理、不应用。安装与 data-script 设置在同一帧；切换主题选择对应已验证结果，缺块立即移除覆盖。卸载只移除自己拥有的 stylesheet / 监听，不改其它 adoptedStyleSheets。[adoptedStyleSheets 官方说明](https://developer.mozilla.org/en-US/docs/Web/API/Document/adoptedStyleSheets)

| 结果                                | 当前视图处理                                                  |
| ----------------------------------- | ------------------------------------------------------------- |
| missing / 无合法覆盖 / 缺当前主题块 | 应用该主题默认值，不复用旧 script / 其他主题                  |
| valid + 单条 warnings               | 应用合法常量；非阻塞、可展开的短诊断                          |
| theme.invalid-skin / store.*        | 清退皮肤，保持应用已确认主题，提供手动重试 / 诊断             |
| 旧 skinEpoch / 已卸载               | 忽略响应，不覆盖当前状态                                      |
| 构造样式表 API / CSP 不可用         | 回应用默认并说明皮肤样式不可用；不放宽 CSP 或绕过为原文件注入 |

原生 API 支持需按 Windows WebView2 / macOS WKWebView / Linux webkit2gtk 实测，happy-dom 不代替平台证据。CSS.registerProperty 仅是未来可选增强，v1 不依赖它；不使用 VueUse useColorMode 或额外主题状态库。每个登记主题由默认 CSS 保证，皮肤降级不会锁死主题切换。

## 威胁模型与 CSP

第三方与内置皮肤走同一校验；可信来源不绕白名单。威胁包含选择器 / 布局劫持、外部资源探测、字体或图像请求、变量循环 / 解析耗尽和陈旧响应串用。禁止包内及外部图引用、字体加载、任意规则 / JS；颜色 / 字体可读性与内容真实性是不同问题。

CSP 是兜底，不替代 Rust 校验。026 在现存尚未设置 CSP 的窗口上接入受限生产策略：script / style 仅允许受信任应用及必要 nonce / hash，img-src 只允许应用内资源，font-src 仅应用内字体，connect-src 保留确需的 Tauri IPC；不为皮肤开放网络、data / blob 图像、unsafe-eval 或资产协议。dev 精确 Vite origin / HMR 另配，不能把 dev 放宽项带入发布包。v1 本地字体依系统栈，不下载网络字体。[Tauri CSP 官方说明](https://v2.tauri.app/security/csp/)

026 验证构造样式表与实际 CSP 共存、favicon / IPC / 开发热更新正常，记录三平台证据；尚未实现时不在本文给出一份未经运行的完整配置。未来受控剧情资产需独立任务冻结路径 / MIME / CSP 权限，不借换肤开放任意读取。

对比度告警在导入 / 开发时可选，首版不阻止加载；受保护状态色不能保证任意背景上都可读。透明色与玻璃 / 渐变组合须在实际合成背景上测试并给出文字 / 图标，不能仅检查两个色值就宣称可访问性通过。未知皮肤仍可通过不合适色值降低可读性，用户可暂时关闭当前皮肤回应用默认，离开该剧本前保持关闭，主题切换不重新启用；不新增持久偏好字段，无须改主题偏好或删除剧本。

## 对接与实现验收

004 先接语义真源 / inline / 清默认色板；026 接校验器、主题偏好、首窗 bootstrap、命令及样式生命周期；025 消费主题设置与皮肤接口，完整界面不复制校验 / 存储。026 可用受控夹具和最小主题入口验证，不等待完整游戏 UI；025 依赖 026 避免二次构建 main。

026 必须覆盖每个内置主题与模板登记主题、缺套回退、坏结构整份失败 / 单条丢弃、非法重复、变量循环与依赖、转义 url、恶意 selector / at 规则、输入 / 返回预算、有界 IO / 符号链接前提、旧响应和样式表清退、偏好保存失败及重载 bootstrap。目录 / 默认 CSS / Tailwind 映射 / Rust 白名单共同检查，首帧无错误主题与三平台 CSP / CSSOM 单独留证。最终 `bun run verify` 十三项通过。

009 的设计走查与参考例已完成；Rust CSS 解析器、浏览器渲染、首帧 / 三平台 CSP 的实施证据由进行中的 026 持续记录，不以候选依赖文档代替这些验收。
