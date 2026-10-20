# 057 — 实现全局快捷键与改键

- 状态：planned
- 依赖：025、034、052、055
- 优先级：P2
- 创建 / 更新：2026-10-19 / 2026-10-20

## 目标与背景

实现 `aoidos-art` 基线 `1f19504ebc7390b7d9954b1c597fc3d98b6b7fde` 中已采纳的快捷键体系：统一动作注册表、平台化提示、设置内查看 / 改键与偏好持久化。052 已提供只读速查浮层与设置页视觉骨架；本任务接入真实动作数据、搜索语义、改键操作和持久化，不能把 052 的静态展示误认为运行能力。

## 必读

[快捷键与键盘交互](../architecture/keyboard-shortcuts.md) · [界面交互](../architecture/ui-shell.md) · [架构总览](../architecture/README.md) · [偏好与 IPC 契约](../architecture/ipc-contract.md) · [前端规范](../standards/frontend.md) · [分层规范](../standards/layering.md) · [测试规范](../standards/testing.md) · [注释规范](../standards/comments.md) · [提交规范](../standards/commits.md)。设计对照：本仓库[快捷键与键盘交互](../architecture/keyboard-shortcuts.md)及基线提交中的 `aoidos-art/docs/shortcuts.md`、`screens/shortcuts.html`、`screens/shortcuts-settings.html`。

## 范围与非目标

范围：Rust 权威的 `UiPreferences` v2 keymap schema / v1 迁移 / 冲突校验；共享动作注册表与作用域分发；速查对话框及筛选；设置页快捷键查看、录制、冲突替换、恢复默认；平台键提示、可访问语义和 WebView / 原生菜单接线。

不重新定义回合、输入法和面板协议；不绑定复制 / 粘贴 / 撤销、关闭窗口、刷新、打印、缩放、DevTools、退出、停止、打断、回退、重新生成等系统或破坏性操作；不以浏览器 `KeyboardEvent.code` 代替字符键的 `event.key` 语义。

## 冻结约束

- `Mod` 在 Windows / Linux 映射 Ctrl，在 macOS 映射 Cmd；可见提示跟随平台。
- `event.isComposing`、`event.key === "Process"` 或 composition 守卫期间让所有快捷键让位给输入法；不使用已弃用的 `keyCode`。不能在组字时拦截 Escape、Enter 或组合键。
- 不可编辑焦点中的 `?`、`End` 等单键不进入文本输入；`Mod` 组合不得覆盖系统编辑键。
- 破坏性动作和可能收费动作不绑定。手动掷骰确认可绑定，但仅在合法等待状态生效且忽略长按重复。
- `UiPreferences` v1 数据必须原样保留并迁移；keymap 只持久化与默认不同的项。整份冲突校验失败时拒绝写入，不能保存部分映射。
- 每个全局 / 平台监听在作用域卸载时释放；输入框、菜单、模态和面板按确定层级各响应一次。
- Windows 发布版关闭 WebView2 浏览器快捷键；macOS 菜单设置入口与前端共用同一动作；Linux 的保留键和 WebKitGTK 行为按平台验证。不能只用 DOM 模拟声称原生平台已验收。

## 实施步骤

1. 定义 Rust 偏好 v2、严格 key/action 校验、旧版迁移、动作默认表与稳定序列化；校验重复绑定及系统保留键。
2. 实现唯一快捷键分发器、作用域栈、IME / 编辑器 / `defaultPrevented` 守卫，以及可重复挂载 / 卸载的清理协议。
3. 将现有面板、设置、指令、判定和焦点行为接到动作 ID；确保既有键位兼容，组件不再分散注册同一全局键。
4. 实现速查对话框和设置分区：按平台显示键位、检索动作、录制 / 取消 / 冲突替换、即时保存与失败回滚。
5. 接入 WebView / 原生菜单平台逻辑；按三平台区分 DOM 行为测试和必须实机验证的原生证据。
6. 更新 locales、偏好 IPC 类型、ui-shell / ipc 文档、测试规范和任务索引；独立 review 后修复并验证。

## 验收标准

- [ ] Rust / Tauri / TypeScript 的 v2 偏好结构一致；旧 v1 配置可迁移，未知 action、保留键、重复绑定、非法组合整份拒绝且错误不回显敏感上下文。
- [ ] 按动作 ID 覆盖面板开 / 关、设置、速查、面板钉住、最新记录、判定、指令菜单与设置导航；作用域顺序、输入控件和已消费事件不会重复触发。
- [ ] IME 组合态、`Process` 按键、compositionend 紧随事件、系统编辑键、按住重复、焦点可编辑边界均有行为测试；卸载与重新挂载不泄漏或重复监听。
- [ ] 改键的录制、Escape 取消、冲突替换、保留键拒绝、恢复默认、保存失败回滚和迟到响应均覆盖行为测试。
- [ ] 快捷键速查和设置页遵循 UI / i18n / 可访问规范；平台提示与 `aria-keyshortcuts` 一致，筛选复用 055 的通用 `SearchInput`。
- [ ] Windows WebView2、macOS 菜单 / WKWebView、Linux WebKitGTK 的接线和保留键分别验证；未实机验证的平台保持未验证。
- [ ] Rust 依赖方向无环；代码、中文注释、类型、文档与 task 同步；独立 review 通过；最终 `bun run verify` 全项通过。

## 验证计划与结果

待实施后填写：偏好迁移 / 命令拒绝测试、键位分发 Vitest、速查和改键行为测试、WebView 构建、三平台原生验收及完整 `bun run verify`。

## 风险与回退

改键不得覆盖已有输入契约或系统操作。若平台接线未完成，保留既有已实现键位并隐藏未支持的动作，不展示虚假的可用状态；偏好读取 / 迁移失败时保留旧数据并关闭改键能力。

## 决策与工作记录

- 2026-10-19：根据 `aoidos-art` 快捷键设计登记独立实现任务；原定由本任务负责完整速查 / 设置 UI。
- 2026-10-20：052 按用户要求提前完成速查与设置的视觉层、筛选预览和平台键位呈现；本任务仍负责将其接到动作注册表、有效按键过滤、改键与偏好存储，不能重复创建第二套页面。

## 完成摘要

未开始。
