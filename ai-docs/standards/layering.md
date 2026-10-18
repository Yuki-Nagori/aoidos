# 依赖分层规范

更新日期：2026-10-19。本文规定 Aoidos 的 Rust crate、Tauri 壳、Vue 层及测试代码的依赖方向。实现边界见[架构总览](../architecture/README.md)与[TS / Rust 边界](../architecture/ts-rust-boundary.md)；CI 检查不得替代架构评审。

## Rust crate

依赖箭头表示“左侧依赖右侧”。workspace crate 之间只能使用下表登记的边；对第三方 crate 的依赖不在此表中。Cargo metadata 检查 normal、build 和 dev 依赖，避免测试依赖偷偷形成生产层反向边。

| 层                 | 成员                                                             | 允许依赖                                                                                                        | 职责                                                        |
| ------------------ | ---------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| L1：基础与独立领域 | `aoidos-store`、`aoidos-json`、`aoidos-script`、`aoidos-billing` | 无                                                                                                              | 存储原语、JSON 共用能力、剧本语法及计价规则；不依赖应用编排 |
| L2：领域适配       | `aoidos-theme`、`aoidos-locale`                                  | `aoidos-store`                                                                                                  | 主题 / 语言偏好及其存储适配                                 |
| L2：领域服务       | `aoidos-memory`                                                  | `aoidos-json`、`aoidos-store`                                                                                   | 记忆派生数据、schema 与恢复                                 |
| L2：模型服务       | `aoidos-llm`                                                     | `aoidos-json`、`aoidos-store`                                                                                   | Provider、请求策略、配置与凭据                              |
| L3：应用引擎       | `aoidos-engine`                                                  | L1 与 L2 的全部 crate                                                                                           | 回合、记录和游戏规则的编排；不得依赖 Tauri、Vue 或桌面壳    |
| L4：桌面组合根     | `aoidos`（`src-tauri`）                                          | `aoidos-engine`、`aoidos-billing`、`aoidos-json`、`aoidos-llm`、`aoidos-locale`、`aoidos-store`、`aoidos-theme` | Tauri 装配、IPC / 事件适配和原生窗口连接                    |
| 验证消费者         | `aoidos-native-tests`                                            | `aoidos`、`aoidos-engine`、`aoidos-llm`、`aoidos-locale`、`aoidos-store`                                        | 跨 crate 和桌面会话验证；生产 crate 不得反向依赖测试 crate  |

规则：

- 新增 workspace crate 或 workspace 依赖边前，先更新此表和架构图，再修改 Cargo manifest；不允许循环依赖或未登记边。
- 领域 crate 不引用 `aoidos` / Tauri；Tauri 命令只做载荷、错误与运行时适配，业务规则留在领域 crate。
- `aoidos-engine` 可以依赖领域服务，但领域服务不得反向依赖 engine；需要回调时由上层注入端口。
- dev-dependencies 也受方向约束。测试可依赖被测 crate；生产 crate 不得通过测试依赖形成反向引用。

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
