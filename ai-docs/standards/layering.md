# 依赖分层规范

更新日期：2026-10-20。本文规定 Aoidos 的 Rust crate、Tauri 壳、Vue 层及测试代码的依赖方向。实现边界见[架构总览](../architecture/README.md)与[TS / Rust 边界](../architecture/ts-rust-boundary.md)；CI 检查不得替代架构评审。

## Rust crate

依赖箭头表示“左侧依赖右侧”。workspace crate 之间只能使用下表登记的边；对第三方 crate 的依赖不在此表中。Cargo metadata 检查 normal、build 和 dev 依赖，避免测试依赖偷偷形成生产层反向边。尚未实施的 crate 或依赖边会在表中明确标注；实施时先更新清单和 `lint:layers`，再改 manifest。

| 层                  | 成员                                                             | 允许依赖                                                                               | 职责                                                                     |
| ------------------- | ---------------------------------------------------------------- | -------------------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| L1：基础与独立领域  | `aoidos-store`、`aoidos-json`、`aoidos-script`、`aoidos-billing` | 无                                                                                     | 存储原语、JSON 共用能力、剧本语法及计价规则；不依赖应用编排              |
| L2：领域适配        | `aoidos-theme`、`aoidos-locale`                                  | `aoidos-store`                                                                         | 主题 / 语言偏好及其存储适配                                              |
| L2：领域服务        | `aoidos-memory`                                                  | `aoidos-json`、`aoidos-store`                                                          | 记忆派生数据、schema 与恢复                                              |
| L2：模型服务        | `aoidos-llm`                                                     | `aoidos-json`、`aoidos-store`                                                          | Provider、请求策略、配置与凭据                                           |
| L3：应用引擎        | `aoidos-engine`                                                  | L1 与 L2 的全部 crate                                                                  | 回合、记录和游戏规则的编排；不得依赖 Tauri、Vue 或桌面壳                 |
| L4：共享应用服务    | `aoidos-app`（058 实施中）                                       | `aoidos-engine`、`aoidos-llm`、`aoidos-store`                                          | profile / 凭据冻结、数据根注入、共享产品与运行时装配；不依赖入口 adapter |
| L5：桌面与 CLI 入口 | `aoidos`（`src-tauri`）、`aoidos-cli`（`src-cli`）               | 均依赖 `aoidos-app`；入口 adapter 可依赖实现 engine 事件端口及共享数据根锁所需 crate   | Tauri IPC / 平台事件或 CLI 参数 / 终端输入及展示；入口相互独立           |
| 验证消费者          | `aoidos-native-tests`                                            | `aoidos`、`aoidos-app`、`aoidos-engine`、`aoidos-llm`、`aoidos-locale`、`aoidos-store` | 跨入口、共享应用层与桌面会话验证；生产 crate 不得反向依赖测试 crate      |

规则：

- 新增 workspace crate 或 workspace 依赖边前，先更新此表和架构图，再修改 Cargo manifest；不允许循环依赖或未登记边。
- 领域 crate 不引用 `aoidos` / Tauri；Tauri 命令只做载荷、错误与运行时适配，业务规则留在领域 crate。
- `aoidos-engine` 可以依赖领域服务，但领域服务不得反向依赖 engine；需要回调时由上层注入端口。
- dev-dependencies 也受方向约束。测试可依赖被测 crate；生产 crate 不得通过测试依赖形成反向引用。
- CLI / TUI 的业务操作统一通过 `aoidos-app` 提供的产品与运行服务；`src-cli` 对 engine / store 的直接依赖仅用于实现 engine 定义的平台事件端口、读取有界记录视图和持有共享实例锁，不在入口复制规则或状态机。

## 共享游戏应用层与 CLI / TUI

游戏运行入口采用“并列入口 adapter → 共享应用服务 → 领域引擎”的单向依赖。`aoidos-app` 已加入 workspace，负责组合 GUI 与 CLI 共用的 profile / 凭据服务、回合协调器、诊断状态、代理快照、`Product` 与 engine runtime；Tauri 和 CLI 各自解析数据根并注入窗口或终端事件。共享服务只组合现有能力，不实现第二套回合、账本或恢复状态机。Tauri GUI 与 CLI/TUI 都在自己的进程内调用这套 Rust 接口，GUI 不启动 CLI 子进程，也不通过 CLI 转发 IPC。

```text
aoidos (src-tauri) ──┐
                    ├→ aoidos-app ──→ aoidos-engine ──→ L1 / L2 领域 crate
aoidos-cli (src-cli) ┘       ├────────→ aoidos-llm
                            └────────→ aoidos-store

aoidos-native-tests → { aoidos, aoidos-app, 被测领域 crate }
```

箭头表示左侧依赖右侧；测试 crate 位于末端并可依赖被测入口、共享应用层和生产 crate，不形成生产层的反向依赖。

应用层构造接收宿主解析的数据根，保持 Tauri 现有数据位置兼容；CLI 默认使用同一产品数据根。应用层统一执行 profile 文件锁、密钥库访问、配置冻结与运行时装配，不复制各入口的 profile 冻结逻辑。操作系统凭据后端继续由 `aoidos-llm` 提供；Tauri 的原生对话框与 CLI 的终端输入分别留在入口，不让应用层等待 UI 输入。

依赖边只允许入口 `L5 → aoidos-app → aoidos-engine / aoidos-llm / aoidos-store`，以及 engine 依赖已登记的 L1/L2；L1/L2/L3 不得反向引用 `aoidos-app`，Tauri 与 CLI 之间不得存在边。应用层若需要入口能力，必须通过窄小的平台无关端口由入口注入；不得由 engine 或 app 反向依赖 adapter。拆分时先登记 Cargo 依赖边并扩展 `lint:layers`，再迁移装配；未迁移的 Tauri 功能可以保留已有直连，不得以此把游戏运行逻辑留在命令层。

## Web 层

```text
main.ts → app/App.vue → app/components（跨 feature 的页面组合）
                          ├─→ features/<feature>/components、composables、utils
                          └─→ shared（多个 feature 共用的基础能力）
features/<feature> → api、i18n、shared
shared → 第三方库、浏览器标准 API 与无运行时依赖的 API 类型
api → Tauri IPC API 与载荷类型
```

`app/` 是唯一可以直接装配多个 feature 的层；feature 之间不导入对方实现，需要协作时由 app 通过 props、事件或回调连接。`shared/` 不得导入 app 或任何 feature；可以引用 `api/` 的纯类型契约，不得依赖其运行时调用；单 feature 逻辑留在该 feature 的 `utils/` / `composables/`。`api/` 不得导入 Vue 组件或 composable。不要为搬文件引入 barrel 转发或第二套状态源。

## 测试层

- `tests/web/` 镜像 `src-web/`（如 `tests/web/features/game/` 对应 `src-web/features/game/`），`tests/scripts/` 镜像 `scripts/`；测试可导入生产模块，生产模块不得导入测试代码。
- Rust 私有实现测试留在 crate 源码单元测试；跨 crate / 原生会话验证放专用测试 crate。
- 影响范围选择只改变执行集合，不改变断言、覆盖率阈值和平台验收要求。selector 不确定时执行全量。

## 自动检查

`bun run lint:layers` 通过 `cargo metadata` 检查 workspace crate 清单和允许依赖边。新增 crate 未登记、超出允许边或任一依赖方向反转都会失败。CI 对涉及 Rust 清单 / 源码的变更运行此检查；本地 `bun run verify` 也包含此项。

测试、构建、格式和 lint selector 共用同一份变更路径解析结果。PR 在三平台构建 Web 产物及受影响 Tauri 应用，并按受影响 crate / 文件运行测试；格式、lint、Clippy 与 coverage 等质量检查只在 Ubuntu 执行。相关原生验收仅在 WebView 夹具直接或传递输入、原生测试输入或 Rust 消费闭包受影响时运行。Nightly 在三平台完整构建并运行 Vitest、workspace Rust 与原生测试，Ubuntu nightly 运行完整 `bun run verify`，包括覆盖率、knip、bench 和 rustdoc。共享配置、空 diff、路径未知或元数据解析失败时保守回退全量。
