# UI 风格规范

更新日期：2026-10-06。定义 Mythos 的目标设计语言，提炼自[Herta 调查](../research/001-herta.md)（风格基线数值取自其 `reference-ux.css`，可按 Mythos 品牌调整）。状态：**规划**——当前 `src-web/app.css` 仍是示例样式，尚未按本规范实现；首个真实界面落地时以本规范为准，并把偏差回写进来。

## 设计基调

玻璃拟态仪表盘悬浮在 pastel 星云上：「calm sci-fi」，柔、透、留白多，接近崩铁风格的设备面板而非终端模拟器。内容上分**两个 register**——人声（台词、叙事）是气泡，机器声（掷骰、判定、系统事件）是等宽小字行；机器声永远安静，不与人声抢视觉。

## 主题与 Token

- 全部设计 token 走 CSS custom properties（目录与扩展规则见[主题架构](../architecture/theming.md)），`<html data-theme="light|dark">` 单属性切换；**默认主题为深色**（对局氛围与立绘都是深空色调），浅色用同一套 token 的浅色值，后续加主题切换设置——不做「跟随系统」自动解析。
- 状态色的语义和色相固定，明暗亮度按下表；皮肤不可覆盖状态色，不能另用 LED 蓝与活动紫制造两套口径。
- 背景是签名的组成部分：多层 radial 星云渐变 + 底部 linear，画在 `body` 上预模糊一次（性能决策，不用逐帧 `backdrop-filter`）。
- 表面是玻璃阶梯：半透明白（浅色）/ 深板岩（深色）+ hairline 边框（ink 按透明度）+ `inset 0 1px 0` 顶高光 + 柔和大阴影。

### 色板（按立绘微调）

中性色沿用 Herta 基线（ink / muted / panel / bubble / hairline），强调色相向 `public/icon.png` 立绘靠拢：青绿主强调、紫次强调、橙品牌点缀；状态色的语义和色相固定，明暗亮度按双主题表取值，皮肤不可覆盖。

| Token                          | 浅色                    | 深色                    | 用途                     |
| ------------------------------ | ----------------------- | ----------------------- | ------------------------ |
| `--ink`                        | `#111417`               | `#e6ebf1`               | 主文字                   |
| `--muted`                      | `#6f7782`               | `#7e8894`               | 次级文字                 |
| `--accent`                     | `#1f7a68`               | `#5cc8b4`               | 主强调（青绿，立绘圆盘） |
| `--accent-violet`              | `#6d4fd4`               | `#a88bff`               | 次强调 / 选中（立绘紫）  |
| `--accent-warm`                | `#e8762d`               | `#f0924a`               | 品牌点缀（立绘橙花）     |
| `--panel`                      | `rgba(255,255,255,.72)` | `rgba(22,28,36,.72)`    | 玻璃面板                 |
| `--bubble-user`                | `rgba(236,238,240,.76)` | `rgba(40,48,58,.78)`    | 玩家气泡                 |
| `--bubble-persona`             | `rgba(255,255,255,.64)` | `rgba(26,32,40,.72)`    | 角色 / 叙事气泡          |
| `--hairline`                   | `rgba(17,20,23,.08)`    | `rgba(226,236,246,.09)` | 细边框                   |
| `--led-active`                 | `#8b7cf6`               | 同                      | 机器行活动指示           |
| `--diff-add` / `--diff-remove` | `#3f7d50` / `#a4544c`   | `#6fa86f` / `#c4736b`   | 变更，去饱和             |
| `--warning`                    | `#b45309`               | `#d99a4e`               | **唯一必须赢的警示色**   |
| `--danger`                     | `#9a5b5b`               | `#ff9d92`               | 破坏性操作               |

星云渐变的首版双主题值如下；浅色沿用同一构图，以 pastel 叠在浅底上。这些是待实际 UI 标定的默认初值，由 004 落地后回写偏差。

```css
:root,
:root[data-theme="dark"] {
  --app-bg:
    radial-gradient(circle at 18% 30%, rgba(92, 200, 180, 0.14) 0% 10%, transparent 30%),
    radial-gradient(circle at 78% 18%, rgba(138, 124, 246, 0.14) 0% 8%, transparent 28%),
    radial-gradient(circle at 62% 82%, rgba(232, 118, 45, 0.08) 0% 9%, transparent 26%),
    linear-gradient(135deg, #0c1a1d 0%, #0a1520 46%, #081018 100%);
}

:root[data-theme="light"] {
  --app-bg:
    radial-gradient(circle at 18% 30%, rgba(92, 200, 180, 0.12) 0% 10%, transparent 30%),
    radial-gradient(circle at 78% 18%, rgba(138, 124, 246, 0.1) 0% 8%, transparent 28%),
    radial-gradient(circle at 62% 82%, rgba(232, 118, 45, 0.06) 0% 9%, transparent 26%),
    linear-gradient(135deg, #eef6f4 0%, #f2f4f7 100%);
}
```

改色统一动 token，不动规则。

## 剧本皮肤

每个已登记剧本可携带 theme.css，只覆盖[主题架构](../architecture/theming.md)白名单中的 custom properties。Rust 校验并解析为常量，前端在受控 data-script / data-theme 作用域应用；原文件不进入 Webview。单条语义错误丢弃并告警，结构 / 预算错误整份作废；缺当前主题套或加载失败回应用默认。内置与第三方走同一校验，v1 禁止包内 / 外部图和字体引用。

主题偏好为应用级 dark / light，由 Rust 持久化；首窗 bootstrap、失败确认与皮肤代次按主题架构实施，当前尚未实现。颜色 / 字体 / 圆角等只有登记具名 token 后才可换肤；不允许任意 CSS 选择器、样式或动态资源。

## 字体

- UI 栈：`Inter, ui-sans-serif, system-ui, -apple-system, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif`（比 Herta 显式列出中文字体，避免随机回退）。
- 机器 / 数据栈：`ui-monospace, "SF Mono", Menlo, "Cascadia Code", Consolas, monospace`；计数与时长加 `tabular-nums`。
- 字阶：用户气泡 15px/1.42；角色气泡 16px/1.62；机器行 12px/1.5；微标签 11–12px；导航 eyebrow 10.5px 大写 + `.05em` 字距；代码 13px。

## 布局

- **与 Herta 不同**：主区是**游戏界面**（全幅舞台），对话记录不占居中主位——它是**右侧悬浮面板**，可收起（收起后完全让位给游戏），经悬停屏幕右缘或唤起按钮展开。
- 记录面板本身玻璃化（`--panel` + hairline + 顶高光），收起 / 展开参数以[界面交互规范](../architecture/ui-shell.md)为准；面板内记录列保留 `max-width: 880px` 上限的 measure。
- 气泡与机器行共享同一左缘 token（如 `--record-left: 8px`），视觉上同一条记录线。
- 密度慷慨：行距 ~26px，气泡 `border-radius: 18px`，内边距 15–24px；chrome 用微字号，内容用正文字号。
- 滚动列：自定义贴底跟随 + 边缘雾化渐隐 + 「跳到最新」浮标；长列表考虑 `content-visibility`。

## 视觉语法（按行类型）

- **玩家**：右对齐，冷灰玻璃气泡，15px，无光晕。
- **角色**：左侧白玻璃气泡与名牌，16px，多段落拆叠层气泡，悬停浮现复制 / 时间戳。
- **旁白**：左侧通栏叠层，无名牌 / 引号，比角色泡更浅；段落与事实身份以[界面结构](../architecture/ui-shell.md)为准。
- **机器（骰子、判定、系统、脱敏工作状态）**：通栏 12px 等宽行 + 7px LED 圆点（活动 `#8b7cf6` 脉冲 / 静止灰），运行中的行用文字 shimmer 渐变；结果类行（如 `命中 · 伤害 12`）静态 LED + 等宽汇总。
- 状态徽章克制：警示琥珀是唯一高饱和强调，diff 绿红刻意去饱和，不让机器行盖过台词。

记录块、生成预览、recap、中断和未知 kind 的语义映射见[记录引擎](../architecture/record-engine.md#ui-映射与验收)；一个事实块可渲染多段气泡，不能反向改变持久记录粒度。面板四态、舞台缩放、输入、骰判与异常交互以已定稿的[界面交互规范](../architecture/ui-shell.md)为准，尚未实现。

## 动效

- 签名缓动 `--ease-signature: cubic-bezier(0.2,0.85,0.2,1)`；微交互 `--dur-micro: 140ms`，普通卡片出入场 200–220ms，大型形变 ~800ms；对话面板按 ui-shell 的独立时长。
- 常备动效：LED 脉冲（1.6s）、流式光标 / shimmer（1.1s / 2.8s）；滚动边缘雾化。
- **每一条动效都必须有 `prefers-reduced-motion` 降级**；入场优先 `@starting-style`，动画不阻塞交互。

## 禁则

- 样式选型 **Tailwind v4**（CSS-first，`@tailwindcss/vite` 插件，无 JS 配置，落地见 task 004）：token 真源仍是本规范的 custom properties（`@theme inline` 只做映射，并清除 Tailwind 默认色板），日常排版用工具类；反复出现的模式沉淀 `@layer components` 小组件类（气泡、机器行等），命名 BEM-ish + `is-*` 状态类。不引 UI 组件库。
- chrome（导航、按钮、侧栏）不可选中（`user-select: none`），内容区（气泡、diff、文本）显式恢复 `text`。
- 图标不引库：内联 SVG，描边 14/18px，`stroke-width 1.3–1.5` 圆头，hover 由 CSS 变色加粗。
- 只借 Herta 的设计语言，**不使用其任何美术资产**（游戏素材不在 MIT 内）。

## 平台风险

Herta 的 CSS 依赖 Chromium-only 特性（`@property`、`@starting-style`、`color-mix()`、`:has()`、`content-visibility`）。目标包含 Windows WebView2、macOS WKWebView 与 Linux webkit2gtk，上述特性按 ui-shell 的三平台矩阵逐项验证并提供降级——落地首个界面时在本节记录实测结论。
