# 架构总览

[任务索引](../task-index.md) · [规范索引](../standards/README.md)

## 主题导航

| 主题                         | 文档                                   | 范围   |
| ---------------------------- | -------------------------------------- | ------ |
| 选型、版本口径与 IPC         | [技术栈](tech-stack.md)                | 本仓库 |
| 目录与模块归属               | [目录规划](repository-layout.md)       | 本仓库 |
| TS / Rust 职责边界与工程纪律 | [职责边界](ts-rust-boundary.md)        | 本仓库 |
| 存储、目录与原子写基建       | [存储基建](storage.md)                 | 本仓库 |
| 构建、门禁、打包与图标       | [构建与开发](build-and-development.md) | 本仓库 |

## 当前状态

Mythos 是 AI 驱动的剧情跑团桌面应用（产品定位见根 README）。当前为可运行的工程骨架：前端只有 `App.vue` 与 `utils/greet.ts`，Rust 侧只有一个 `greet` 命令，用于演示 `invoke` 往返。状态管理、路由、业务 crate 均未引入，本文档不把它们写成已有能力。路线图见[任务索引](../task-index.md)。

## 分层和依赖方向

```text
src-web（Vue + TypeScript，UI 编排与展示逻辑）
  │ invoke()
  ▼
src-tauri（#[tauri::command] 命令层，保持薄）
  ▼
业务 crate（规划：出现真实业务时加入 Cargo workspace members，命令层只做转发）
```

可复用的前端逻辑放 `src-web/utils/` 并配单测；组件只做编排。跨端类型在 Rust 定型后由 TS 侧同步声明，`invoke` 本身不做校验。

## 文档边界

架构文档说明目标与已验证的边界；项目选型与版本口径集中在[技术栈](tech-stack.md)，不另维护第二份版本表；单次工作的范围、决策与验证证据写在对应 task 里；未排期的产品构想记在 [ideas/](../ideas/)，开源调查记在 [research/](../research/)，都不混入任务队列。
