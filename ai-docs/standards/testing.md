# 测试规范

更新日期：2026-10-09。均为项目约定。

## 测试位置与写法

前端测试在 `tests/web/`，目录镜像 `src-web`；组件行为用 `@vue/test-utils` + happy-dom，IPC mock 模式见[前端约定](frontend.md)。Rust 命令的单测贴近源码（`commands.rs` 的 `mod tests`），业务 crate 的公开 API 测试随 crate 走。异步断言用 `vi.waitFor`；Rust 侧失败路径与正常路径都要有断言。

### Rust 集成测试的位置

- 默认使用**内联单测**（源文件内 `#[cfg(test)] mod tests`，夹具用 `#[cfg(test)] pub(crate) mod` 放 src/ 内，如 mythos-llm 的 testserver）：测试体较大时可拆成模块目录下的 `tests.rs`，由原模块的 `#[cfg(test)] mod tests` 加载，仍是单元测试，不因文件名成为 Cargo 集成 target。覆盖率门禁只统计 lib 目标与内联测试的执行，这一耦合是有意为之——夹具自身的行为路径也受门禁约束。
- 出现真实跨 crate / 跨层场景（引擎协调、命令注册 + 事件投递端到端）时，用 Cargo 惯例位置，两选一：单 crate 的对外行为放**该 crate 自己的 `tests/` 目录**；跨 crate 集成放**专门的测试 crate**（workspace member，`dev-dependencies` 引全部被测方）。`cargo test --workspace` 自动纳入两者，CI 无需改步骤。
- 根 `tests/rust` 不放松散源码；三平台原生能力验证放 `tests/rust/native-platform` 独立测试 crate（workspace member），Cargo 显式登记集成 target。其 `harness = false` 让真实 AppKit / GTK 在进程主线程执行；`desktop-session` 特性及 `bun run test:native` 显式开启，普通无桌面套件不弹出原生窗口。聚合入口执行该 crate 全部桌面会话 target：原生确认 / 取消与 OS 凭据读写清，以及真实主 Webview 的命令 / 事件往返；均使用本地合成夹具，不输出密钥、不发送收费请求。单目标排查通过 `bun run test:native --test <target>`，不新增逐目标根脚本。手写 `webview/`、`tauri.conf.json` 入库；根入口先构建实际产品消费者到 `gen/webview/`，生成的整个 `gen/` 从 Git / ESLint 排除。夹具不得复制一套消费算法来代替产品模块。
- 集成测试不进覆盖率口径（`--lib` 不统计独立测试目标）：行为断言归集成测试，行覆盖归内联单测，互不替代；给集成测试补覆盖率属门禁变更，按下方门槛变更流程先立项。

## 覆盖率门槛（verify 两项）

- 前端 `bun run test:coverage`：v8 provider，只统计逻辑层——`src-web/{utils,stores,composables}` 与组件旁 `use*.ts`；行 / 分支 / 函数 / 语句四项 100%。`main.ts` 是装配、api 是薄封装、`bench/` 是基准，均不入门槛。
- Rust `bun run coverage:rust`：**逐文件行覆盖门禁**，规则集中在根目录 [`coverage-rust.config.mts`](../../coverage-rust.config.mts)，由 `scripts/coverage-rust.mts` 消费执行（跑 `cargo llvm-cov --workspace --lib --json` 后逐文件裁决）。规则只有三条：
  1. 缺省每文件未覆盖行 = 0，即必须 100%。
  2. 忽略清单（文件名正则）：`lib.rs` 与以平台模块路径匹配的 `platform/windows.rs`、`platform/macos.rs`、`platform/linux.rs`，理由见下方台账。
  3. 逐文件预算：确属工具伪影的文件才在配置登记理由与额度；当前清单为空，全部非忽略文件要求 100%。

### Rust 覆盖豁免台账

| 豁免                                      | 理由                                                                                                             | 仍受直测覆盖的行为                                                                                                                                                                                                            |
| ----------------------------------------- | ---------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lib.rs`（所有 crate）                    | 装配入口只放模块声明 / 薄装配，不含业务逻辑                                                                      | 无                                                                                                                                                                                                                            |
| `platform/windows.rs`                     | Win32 安全 API 的失败分支——进程令牌 SID 提取中的内存分配 / CopySid 失败、合法 SID 的空值防御——无法在健康进程注入 | 空进程 / 空令牌句柄、非法 SID（全零缓冲被 `SetEntriesInAclW` 拒绝）、不存在路径（`SetNamedSecurityInfoW` 拒绝）、降级文件 DACL 结构断言、CredUI 对话框取消路径（WM_CLOSE 关闭真实对话框 → `Ok(None)`）、CredUI 返回码纯映射表 |
| `platform/macos.rs` / `platform/linux.rs` | 真实主线程 AppKit / GTK 适配，无业务存储逻辑                                                                     | `test:native` 在真实桌面会话（Linux 可用 Xvfb + Secret Service）检查密码字段、确认 / 取消及 OS 服务；缺会话失败，不以 stub 标通过                                                                                             |

当前逐文件预算为空，审计依据见 [037](../task/037-rust-coverage-audit.md)。LLVM 的函数实例组统计不求覆盖行并集；不同实例分别覆盖成功 / 失败时，segments 全绿仍可能存在 summary 缺口，须核实原始函数数据并补同一实例的边界测试。

src-tauri 的 commands / events / ipc 等非忽略文件及业务 crate 的逻辑文件均须足额覆盖。前置组件与安装见[构建与开发](../architecture/build-and-development.md)。

改门槛口径（忽略清单、逐文件预算）属于门禁变更：先在 task 里给出理由与新口径的验证结果，再动 `coverage-rust.config.mts`，并同步本节台账。

## 覆盖率缺口修复

覆盖率缺口由 subagent 分工修复：主代理提供文件范围、缺口证据和验收条件，避免交叉修改；测试须验证真实边界，不以镜像实现、跳过文件或放宽门槛代替。工具差额先核实原始数据，不能仅凭 segments 全绿认定为伪影。

主代理整合并核验修复，复验受影响检查及完整 `bun run verify`；交付前由未承担对应修复的 subagent 独立复核，在 task 记录来源、修复与实际结果。

## 独立评审

每个 task 实现与验证完成后，使用 subagent 独立 review；处理发现并复验后才交付，不为小修复额外建 task，也不以评审代替 verify / CI。

评审覆盖完整 diff 与调用链：职责 / 依赖无环、代码 / 注释规范、真实测试边界、跨端类型及文档一致。主代理提供范围、验收和验证证据，核验可定位的发现；实质修复须再次复核。task 记录最终结果与验证限制，仍有阻塞时不标完成。

## 死代码检查

`bun run knip` 检查未用依赖、导出与文件，配置在 `knip.json`：entry 是 `index.html`、`tests/web/**/*.test.ts`、`src-web/bench/*.ts` 与原生夹具 `tests/rust/native-platform/webview/main.ts`。新依赖装了没用、导出无人消费，knip 会拦下；确属工具链需要而 knip 误报时，在 `knip.json` 的 `ignoreDependencies` 登记并在此处或对应 task 注明原因（JSON 不支持注释）。已登记：`tailwindcss`——经 app.css 的 `@import "tailwindcss"` 消费，knip 不追踪 CSS 导入（task 004）。`$schema` 指向 jsDelivr CDN（VS Code 对本地相对路径在部分工作区会按 git: 协议解析而报错；版本钉主版本 `@6` 与 package.json 对齐）。

## 基准

`bun run bench` 跑 `src-web/bench/` 下的 tinybench 示例，用于验证基准链路可用，已纳入 verify（秒级）。性能结论要可靠对比（同机器、同口径、多次采样），临时性结论写进对应 task，不进规范；需要持续跟踪性能时再登记专门 task 扩建基准集。
