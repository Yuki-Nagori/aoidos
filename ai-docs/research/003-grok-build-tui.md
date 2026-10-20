# Grok Build TUI：可点击终端界面与事件驱动结构

调查日期：2026-10-20。

## 范围与来源

[xai-org/grok-build](https://github.com/xai-org/grok-build) 是公开的 Rust CLI/TUI 仓库，README 将其描述为全屏、支持鼠标交互且可扩展的终端应用。此次只读查看仓库首页、TUI crate README 与 `Cargo.toml`；没有克隆源码，也没有复制代码。

- [仓库 README](https://github.com/xai-org/grok-build)：说明 CLI/TUI 入口、支持平台、仓库布局与 Apache-2.0 许可。
- [xai-grok-pager README](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/README.md)：介绍 TUI 模块、交互模型与快捷键。
- [xai-grok-pager Cargo.toml](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/Cargo.toml)：可见其使用 Ratatui、Crossterm 事件流与 bracketed paste，以及专用 textarea 组件。

## 可借鉴的设计

1. **鼠标和键盘共用同一动作语义。** 可点击区域应与键盘焦点导航指向同一动作；鼠标不是另一套功能分支。Aoidos 的剧本、回复对象、判定策略、发送、取消、继续和设置等核心操作都应能通过焦点与 Enter 执行，也能用鼠标点击。
2. **把输入映射成动作，再执行副作用。** TUI README 将结构概括为 `input → Action → dispatch → Effect → state update`。Aoidos 可采用更小的版本：终端事件转成界面动作，运行时效果调用 `aoidos-app`，收到事件后更新视图状态。绘制代码不应直接读写 SQLite、凭据或创建 Provider。
3. **按屏幕与会话状态拆分视图。** Grok Build 将应用级视图、每会话视图、输入组件、历史区、弹层与渲染帮助拆开。Aoidos 首版可聚焦剧本/会话选择、对话记录、输入框、工作状态和必要确认层；暂不照搬插件系统、文件搜索、MCP、命令面板或多会话工作台。
4. **输入编辑器需要明确的焦点和粘贴语义。** 示例支持多行输入、历史区切换和 bracketed paste。Aoidos 应以真实终端事件测试中文输入、粘贴、窗口缩放、上下滚动、快捷键冲突与不可用终端降级；不要假设每种终端都支持相同鼠标协议。
5. **先让用户看懂当前可做什么。** 全屏界面应持续显示当前剧本 / 会话、模型、回合阶段、正在运行或等待输入的状态，以及可用快捷键。首轮输入最好提供可编辑的行动提示或示例，让用户知道可以自然语言描述行动；不应把 AI 的建议误当成已提交的游戏事实。

## 对 Aoidos 的取舍

- 复用已定的 `aoidos-app` 作为 CLI、TUI 和 Tauri 共用的应用服务；TUI 只是入口与展示，不作为 GUI 的子进程。领域执行、凭据、账本和持久化不进入 TUI crate。
- 首版采用 Ratatui + Crossterm 值得评估，特别是鼠标点击、键盘焦点、终端事件与渲染布局。引入前先在 `aoidos-cli` 的窄范围原型中确认 Windows/macOS/Linux 终端行为、中文编辑和依赖规模；不要复制 Grok Build 的整套依赖图或 agent 架构。
- 可借鉴 Action/Effect 分离与交互反馈，不照抄 UI 代码。若后续确需移植 Apache-2.0 代码，再单独核对目标文件许可、保留版权与许可声明并记录修改；当前本报告仅记录设计观察。
- 研究结论本身不扩张 058 的实现范围；新增 TUI 交互需求应通过 task 规划，并保持 Rust crate 依赖单向、无循环。

## 尚未通过源码确认的内容

此报告基于公开 README 与 manifest，不评价源码中的事件循环细节、鼠标命中测试质量、无障碍支持或终端兼容性，也不把 README 的功能描述当作 Aoidos 的验收证据。若后续需要比较具体实现，可按目标模块只读克隆仓库或继续检查公开源文件；不需要用户预先克隆。
