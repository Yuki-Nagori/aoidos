# 测试规范

更新日期：2026-10-07。均为项目约定。

## 测试位置与写法

前端测试在 `tests/web/`，目录镜像 `src-web`；组件行为用 `@vue/test-utils` + happy-dom，IPC mock 模式见[前端约定](frontend.md)。Rust 命令的单测贴近源码（`commands.rs` 的 `mod tests`），业务 crate 的公开 API 测试随 crate 走。异步断言用 `vi.waitFor`；Rust 侧失败路径与正常路径都要有断言。

### Rust 集成测试的位置

- 默认使用**内联单测**（源文件内 `#[cfg(test)] mod tests`，夹具用 `#[cfg(test)] pub(crate) mod` 放 src/ 内，如 mythos-llm 的 testserver）：覆盖率门禁只统计 lib 目标与内联测试的执行，这一耦合是有意为之——夹具自身的行为路径也受门禁约束。
- 出现真实跨 crate / 跨层场景（引擎协调、命令注册 + 事件投递端到端）时，用 Cargo 惯例位置，两选一：单 crate 的对外行为放**该 crate 自己的 `tests/` 目录**；跨 crate 集成放**专门的测试 crate**（workspace member，`dev-dependencies` 引全部被测方）。`cargo test --workspace` 自动纳入两者，CI 无需改步骤。
- 根 `tests/rust` 不放松散源码；三平台原生能力验证放 `tests/rust/native-platform` 独立测试 crate（workspace member），Cargo 显式登记集成 target。其 `harness = false` 让真实 AppKit / GTK 在进程主线程执行；`desktop-session` 特性及 `bun run test:native` 显式开启，普通无桌面套件不弹出原生窗口。所有平台都检查确认 / 取消与 OS 凭据读写清，使用固定合成值，不输出明文。
- 集成测试不进覆盖率口径（`--lib` 不统计独立测试目标）：行为断言归集成测试，行覆盖归内联单测，互不替代；给集成测试补覆盖率属门禁变更，按下方门槛变更流程先立项。

## 覆盖率门槛（verify 两项）

- 前端 `bun run test:coverage`：v8 provider，只统计逻辑层——`src-web/{utils,stores,composables}` 与组件旁 `use*.ts`；行 / 分支 / 函数 / 语句四项 100%。`main.ts` 是装配、api 是薄封装、`bench/` 是基准，均不入门槛。
- Rust `bun run coverage:rust`：**逐文件行覆盖门禁**，规则集中在根目录 [`coverage-rust.config.mts`](../../coverage-rust.config.mts)，由 `scripts/coverage-rust.mts` 消费执行（跑 `cargo llvm-cov --workspace --lib --json` 后逐文件裁决）。规则只有三条：
  1. 缺省每文件未覆盖行 = 0，即必须 100%。
  2. 忽略清单（文件名正则）：`lib.rs` 与以平台模块路径匹配的 `platform/windows.rs`、`platform/macos.rs`、`platform/linux.rs`，理由见下方台账。
  3. 逐文件预算：确属工具伪影的文件在配置登记理由与额度（配置登记的文件各有 1 行预算，见台账）。

### Rust 覆盖豁免台账

| 豁免                                      | 理由                                                                                                                                                                                              | 仍受直测覆盖的行为                                                                                                                                                                                                            |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `lib.rs`（所有 crate）                    | 装配入口只放模块声明 / 薄装配，不含业务逻辑                                                                                                                                                       | 无                                                                                                                                                                                                                            |
| `platform/windows.rs`                     | Win32 安全 API 的失败分支——进程令牌 SID 提取中的内存分配 / CopySid 失败、合法 SID 的空值防御——无法在健康进程注入                                                                                  | 空进程 / 空令牌句柄、非法 SID（全零缓冲被 `SetEntriesInAclW` 拒绝）、不存在路径（`SetNamedSecurityInfoW` 拒绝）、降级文件 DACL 结构断言、CredUI 对话框取消路径（WM_CLOSE 关闭真实对话框 → `Ok(None)`）、CredUI 返回码纯映射表 |
| `platform/macos.rs` / `platform/linux.rs` | 真实主线程 AppKit / GTK 适配，无业务存储逻辑                                                                                                                                                      | `test:native` 在真实桌面会话（Linux 可用 Xvfb + Secret Service）检查密码字段、确认 / 取消及 OS 服务；缺会话失败，不以 stub 标通过                                                                                             |
| `proxy.rs` / `llm_commands.rs` 各 1 行    | llvm-cov 合并伪影：测试收尾语句（关停夹具、清理临时目录）在跨二进制合并时计为未覆盖，无法经测试触达（实测数量随编译目标变化，预算不表示当前一定缺失；config / credentials 与共享读取均要求 100%） | 这些文件的真实逻辑全部有直测；预算只覆盖伪影，真实回归会推高未覆盖数照样拦截                                                                                                                                                  |

src-tauri 的 commands / events / ipc 等非忽略文件及业务 crate 的逻辑文件均须足额覆盖。前置组件与安装见[构建与开发](../architecture/build-and-development.md)。

改门槛口径（忽略清单、逐文件预算）属于门禁变更：先在 task 里给出理由与新口径的验证结果，再动 `coverage-rust.config.mts`，并同步本节台账。

## 死代码检查

`bun run knip` 检查未用依赖、导出与文件，配置在 `knip.json`：entry 是 `index.html`、`tests/web/**/*.test.ts`、`src-web/bench/*.ts`。新依赖装了没用、导出无人消费，knip 会拦下；确属工具链需要而 knip 误报时，在 `knip.json` 的 `ignoreDependencies` 登记并在此处或对应 task 注明原因（JSON 不支持注释）。已登记：`tailwindcss`——经 app.css 的 `@import "tailwindcss"` 消费，knip 不追踪 CSS 导入（task 004）。`$schema` 指向 jsDelivr CDN（VS Code 对本地相对路径在部分工作区会按 git: 协议解析而报错；版本钉主版本 `@6` 与 package.json 对齐）。

## 基准

`bun run bench` 跑 `src-web/bench/` 下的 tinybench 示例，用于验证基准链路可用，已纳入 verify（秒级）。性能结论要可靠对比（同机器、同口径、多次采样），临时性结论写进对应 task，不进规范；需要持续跟踪性能时再登记专门 task 扩建基准集。
