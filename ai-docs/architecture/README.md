# 架构文档

更新日期：2026-10-24。

本目录按稳定的产品和工程领域组织架构契约。这里说明模块职责、权威数据与跨模块边界；具体实现状态和验收证据以[任务索引](../task-index.md)及各 task 为准，工程约定以[规范索引](../standards/README.md)为准。

## 领域导航

| 领域         | 入口                             | 内容                                                      |
| ------------ | -------------------------------- | --------------------------------------------------------- |
| 核心工程     | [core](core/README.md)           | 技术栈、仓库结构、Rust / TypeScript 边界、IPC、构建与开发 |
| 数据与持久化 | [data](data/README.md)           | 文件与数据库、记录、分支恢复、记忆及其算法                |
| AI 与计费    | [ai](ai/README.md)               | Provider 请求边界、提示构造、凭据、预算与账本             |
| 游戏规则     | [gameplay](gameplay/README.md)   | 角色、回合、周目初始化、任务和结局                        |
| 用户界面     | [interface](interface/README.md) | 界面结构、键盘、主题与语言                                |

## 依赖方向

```text
src-web → src-tauri → aoidos-app / aoidos-engine
                           ├─→ aoidos-billing
                           ├─→ aoidos-llm
                           ├─→ aoidos-memory
                           ├─→ aoidos-store
                           └─→ aoidos-script / aoidos-json
```

业务权威位于 Rust；Vue 展示快照并发起已授权的命令，不自行决定世界事实。数据、AI、界面与游戏规则之间的单向依赖分别在各领域入口定义。平台差异由 Tauri、CLI 和各自的适配层承接，不反向污染领域核心。

## JSON 共用能力

`aoidos-json` 提供有界 UTF-8 / JSON 解码、递归重复键拒绝、DTO 转换、紧凑编码及按对象键排序的规范化；它不依赖业务 crate。字段 schema、版本、未知字段策略、持久化和错误码映射仍由各消费模块负责，交换类型使用 `serde_json`。文件与网络 I/O 不属于此 crate；详见[技术栈](core/tech-stack.md)与[职责边界](core/ts-rust-boundary.md)。

## 设计与实施

架构文档定义目标契约，不能单凭文档存在推断能力已经上线。设计任务、实施主责、平台验收与当前状态统一记录在[任务索引](../task-index.md)和对应 task 中；架构正文只保留稳定规则，避免复制易过期的状态表。
