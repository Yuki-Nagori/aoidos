# 测试规范

更新日期：2026-10-19。均为项目约定。

## 测试位置与写法

前端测试在 `tests/web/`，目录镜像 `src-web`；工具脚本的行为测试放 `tests/scripts/`，与 `scripts/` 模块对应；两者都由 Vitest 执行。组件行为用 `@vue/test-utils` + happy-dom，IPC mock 模式见[前端约定](frontend.md)。Rust 命令的单测贴近源码（`commands.rs` 的 `mod tests`），业务 crate 的公开 API 测试随 crate 走。异步断言用 `vi.waitFor`；Rust 侧失败路径与正常路径都要有断言。

### 行为优先

- 测试以外部可观察的结果、状态迁移与安全边界为断言对象；只有公开且持久的协议本身才固定精确表示。避免为触达覆盖行直接断言私有 helper、局部变量或格式布局。
- 高风险路径必须有行为证据：授权与身份校验、持久提交 / 幂等 / 崩溃恢复、计费预留与未知结果、输入大小与损坏数据、事件丢失 / 重放 / 过期快照、原生凭据与 WebView IPC。删除或重写这些测试时，须指出替代测试及其覆盖的可观察行为。
- 每个测试聚焦一条主要规则。共同前置条件可以参数化，但 case 名必须说明具体边界；失败日志应能指出哪条产品约束被破坏。
- 按被验证的语义选择夹具：内存 SQLite 适合规则 / 查询 / 状态迁移；原子写入、同步、锁、迁移备份与崩溃恢复使用真实文件和 SQLite WAL。不要为了速度在所有夹具中关闭同步。
- 容量边界测试只构造证明边界所需的数据（通常为上限加一）；批量准备大量行时使用单个显式事务，避免每行单独 autocommit 把磁盘同步成本混入规则测试。若测试不验证事务提交 / 文件耐久性，使用内存 SQLite；若验证这些语义，保留真实文件和正式提交路径。
- 压力测试只保留能证明独立边界的规模；同一分页 / 容量行为用最严的有效输入验证即可，不重复多个较低负载的全量耐久写入。若要缩短长链，只保留刚越过硬上限的规模，并明确仍断言分页或拒绝结果。
- 覆盖率用于发现未审查的逻辑，不表示测试有价值，也不要求所有分支组合都单测。优先覆盖风险清单与公开行为，再查看覆盖缺口；不得为维持数字复制实现或增加无意义断言。

### Rust 集成测试的位置

- 默认使用**内联单测**（源文件内 `#[cfg(test)] mod tests`，夹具用 `#[cfg(test)] pub(crate) mod` 放 src/ 内，如 aoidos-llm 的 testserver）：测试体较大时可拆成模块目录下的 `tests.rs`，由原模块的 `#[cfg(test)] mod tests` 加载，仍是单元测试，不因文件名成为 Cargo 集成 target。覆盖率门禁只统计 lib 目标与内联测试的执行，这一耦合是有意为之——夹具自身的行为路径也受门禁约束。
- 出现真实跨 crate / 跨层场景（引擎协调、命令注册 + 事件投递端到端）时，用 Cargo 惯例位置，两选一：单 crate 的对外行为放**该 crate 自己的 `tests/` 目录**；跨 crate 集成放**专门的测试 crate**（workspace member，`dev-dependencies` 引全部被测方）。`cargo test --workspace` 自动纳入两者，CI 无需改步骤。
- 根 `tests/rust` 不放松散源码；三平台原生能力验证放 `tests/rust/native-platform` 独立测试 crate（workspace member），Cargo 显式登记集成 target。其 `harness = false` 让真实 AppKit / GTK 在进程主线程执行；`desktop-session` 特性及 `bun run test:native` 显式开启，普通无桌面套件不弹出原生窗口。聚合入口执行该 crate 全部桌面会话 target：原生确认 / 取消与 OS 凭据读写清，以及真实主 Webview 的命令 / 事件往返；均使用本地合成夹具，不输出密钥、不发送收费请求。单目标排查通过 `bun run test:native --test <target>`，不新增逐目标根脚本。手写 `webview/`、`tauri.conf.json` 入库；根入口先构建实际产品消费者到 `gen/webview/`，生成的整个 `gen/` 从 Git / Biome 排除。夹具不得复制一套消费算法来代替产品模块。
- 原生 HMR 夹具将 Vite、就绪探测及 WebView2 固定到 `127.0.0.1`；Windows CI 出现 `localhost` 探测成功但 WebView 没有初始报告的超时后，数值回环地址用于消除主机名解析差异。WebView 创建和页面导航 / 脚本报告各有 60 秒时限，收到初始 ready 后另给 CSS 热更新 20 秒；失败日志区分创建、导航与更新阶段。CSP 仍由产品开发策略派生，并只替换夹具的本地来源地址。
- 集成测试不进覆盖率口径（`--lib` 不统计独立测试目标）：行为断言归集成测试，行覆盖归内联单测，互不替代；给集成测试补覆盖率属门禁变更，按下方门槛变更流程先立项。

## 覆盖率门槛

- 前端 `bun run test:coverage`：v8 provider，统计 `src-web/features/**` 的功能规则 / 状态 composable 与 `src-web/shared/**`；行 / 分支 / 函数 / 语句各至少 90%。`app/`、`GameWorkspace` 页面级跨 composable 编排、API 薄封装和 `bench/` 不计入门槛；关键用户路径仍由组件 / 入口行为测试验证，不能用 coverage 排除代替行为证据。
- Rust `bun run coverage:rust`：`cargo llvm-cov --workspace --lib --json` 统计纳入文件的**聚合行覆盖率至少 90%**，配置在根目录 [`coverage-rust.config.mts`](../../coverage-rust.config.mts)，由 `scripts/coverage-rust.mts` 执行。小文件不单独设 100% 预算，Tauri 宏映射的无源行也不再单独登记；平台适配 / 装配排除见下方台账。
- PR CI 由变更规划、Ubuntu 共享质量门禁和三平台构建 / 受影响测试组成；可复用步骤位于 `.github/actions/`。三平台均构建 Web 产物，并按同一计划运行受影响 Tauri 应用 / Rust crate / 消费者及对应 Vitest 镜像测试；Vue / TS 改动按镜像关系选择 Vitest，找不到对应测试时扩展到全部 Vitest。除 WebView 夹具直接或传递依赖的文件外，纯 Web 改动不触发 Rust 或原生测试。格式、lint 和 Clippy 在 Ubuntu 执行，原生验收仅在 WebView 夹具输入、原生测试输入或 Rust 消费闭包受影响时运行。测试、Biome、dprint、rustfmt 与 Clippy 共用变更计划，Cargo 反向依赖闭包从 metadata 得出；`vitest.config.ts` 变更扩展到全部 Vitest，但不扩大 Cargo 测试范围。其他共享配置、空 diff、未知路径或不可用元数据 / 基线回退全量。Nightly 在三平台构建并运行完整 Vitest、workspace Rust 和原生测试；Rust / Vitest coverage、knip、bench、rustdoc 由 Ubuntu nightly 的完整 `bun run verify` 执行，也可手动触发。`bun run test:rust` 仍是本地完整 workspace 测试入口。覆盖统计平台分工不替代平台行为测试。

### Rust 覆盖豁免台账

| 豁免                                      | 理由                                                                                                             | 仍受直测覆盖的行为                                                                                                                                                                                                            |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lib.rs`（所有 crate）                    | 装配入口只放模块声明 / 薄装配，不含业务逻辑                                                                      | 无                                                                                                                                                                                                                            |
| `platform/windows.rs`                     | Win32 安全 API 的失败分支——进程令牌 SID 提取中的内存分配 / CopySid 失败、合法 SID 的空值防御——无法在健康进程注入 | 空进程 / 空令牌句柄、非法 SID（全零缓冲被 `SetEntriesInAclW` 拒绝）、不存在路径（`SetNamedSecurityInfoW` 拒绝）、降级文件 DACL 结构断言、CredUI 对话框取消路径（WM_CLOSE 关闭真实对话框 → `Ok(None)`）、CredUI 返回码纯映射表 |
| `platform/macos.rs` / `platform/linux.rs` | 真实主线程 AppKit / GTK 适配，无业务存储逻辑                                                                     | `test:native` 在真实桌面会话（Linux 可用 Xvfb + Secret Service）检查密码字段、确认 / 取消及 OS 服务；缺会话失败，不以 stub 标通过                                                                                             |
| `locale_commands/native.rs`               | Wry 菜单 / 托盘需真实主线程与桌面服务，MockRuntime 不创建原生表面；仅隔离 Tauri 原生构造与更新适配               | `test:native` 计划在三平台真实桌面会话创建菜单 / 托盘，切换语言并断言应用菜单标签与标题；IPC WebView 夹具验证语言 bootstrap 首帧。托盘 tooltip 和凭据对话框的双语读取断言未覆盖；命令、存储和更新状态由内联单测覆盖           |

审计依据见 [037](../task/037-rust-coverage-audit.md)。覆盖率口径调整理由、行为风险清单和实测对比见 [048](../task/048-test-quality-ci.md)。

src-tauri 的 commands / events / ipc 等非忽略文件及业务 crate 的逻辑都计入聚合门槛；关键行为还须满足上方风险清单，不能由整体百分比替代。前置组件与安装见[构建与开发](../architecture/build-and-development.md)。

忽略范围和聚合覆盖底线属于门禁配置；调整时先在 task 里给出理由、风险行为证据与验证结果，再同步脚本、配置和本节台账。

## 覆盖率缺口修复

覆盖率缺口由 subagent 分工修复：主代理提供文件范围、缺口证据和验收条件，避免交叉修改；测试须验证真实边界，不以镜像实现、跳过文件或放宽门槛代替。工具差额先核实原始数据，不能仅凭 segments 全绿认定为伪影。

主代理整合并核验修复，复验受影响检查及完整 `bun run verify`；交付前由未承担对应修复的 subagent 独立复核，在 task 记录来源、修复与实际结果。

## 独立评审

每个 task 实现与验证完成后，使用 subagent 独立 review；处理发现并复验后才交付，不为小修复额外建 task，也不以评审代替 verify / CI。

评审覆盖完整 diff 与调用链：职责 / 依赖无环、代码 / 注释规范、真实测试边界、跨端类型及文档一致。主代理提供范围、验收和验证证据，核验可定位的发现；实质修复须再次复核。task 记录最终结果与验证限制，仍有阻塞时不标完成。

## 死代码检查

`bun run knip` 检查未用依赖、导出与文件，配置在 `knip.json`。entry 包含 `index.html`、两种首帧 bootstrap 的 TS 源、`tests/web/**/*.test.ts`、`tests/scripts/**/*.test.ts`、`src-web/bench/*.ts` 与原生夹具 `tests/rust/native-platform/webview/*.ts`；project 覆盖 `src-web/**/*.{ts,vue}` 和 `scripts/**/*.{ts,mts}`。新依赖装了没用、导出无人消费，knip 会拦下；确属工具链需要而 knip 误报时，在 `knip.json` 的 `ignoreDependencies` 登记并在此处或对应 task 注明原因（JSON 不支持注释）。已登记：`tailwindcss`——经 app.css 的 `@import "tailwindcss"` 消费，knip 不追踪 CSS 导入（task 004）。`$schema` 指向 jsDelivr CDN（VS Code 对本地相对路径在部分工作区会按 git: 协议解析而报错；版本钉主版本 `@6` 与 package.json 对齐）。

## 基准

`bun run bench` 跑 `src-web/bench/` 下的 tinybench 示例，用于验证基准链路可用，已纳入 verify（秒级）。性能结论要可靠对比（同机器、同口径、多次采样），临时性结论写进对应 task，不进规范；需要持续跟踪性能时再登记专门 task 扩建基准集。
