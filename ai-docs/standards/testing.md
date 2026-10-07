# 测试规范

更新日期：2026-10-07。均为项目约定。

## 测试位置与写法

前端测试在 `tests/web/`，目录镜像 `src-web`；组件行为用 `@vue/test-utils` + happy-dom，IPC mock 模式见[前端约定](frontend.md)。Rust 命令的单测贴近源码（`commands.rs` 的 `mod tests`），业务 crate 的公开 API 测试随 crate 走。异步断言用 `vi.waitFor`；Rust 侧失败路径与正常路径都要有断言。

### Rust 集成测试的位置

- 默认与主流是**内联单测**（源文件内 `#[cfg(test)] mod tests`，夹具用 `#[cfg(test)] pub(crate) mod` 放 src/ 内，如 mythos-llm 的 testserver）：覆盖率门禁 `cargo llvm-cov --workspace --lib` 只统计 lib 目标与内联测试的执行，这一耦合是有意为之——夹具自身的行为路径也受门禁约束。
- 出现真实跨 crate / 跨层场景（引擎协调、命令注册 + 事件投递端到端）时，用 Cargo 惯例位置，两选一：单 crate 的对外行为放**该 crate 自己的 `tests/` 目录**；跨 crate 集成放**专门的测试 crate**（workspace member，`dev-dependencies` 引全部被测方）。`cargo test --workspace` 自动纳入两者，CI 无需改步骤。
- **不建根级 `tests/rust`**：Cargo 不识别工作区级 tests 目录，需要自建 harness 与装配，徒增一层非标准结构；`tests/web` 与 `src-web` 分离是 vitest 生态的惯例，不移植到 Rust 侧。
- 集成测试不进覆盖率口径（`--lib` 不统计独立测试目标）：行为断言归集成测试，行覆盖归内联单测，互不替代；给集成测试补覆盖率属门禁变更，按上节流程先立项。

## 覆盖率门槛（verify 两项）

- 前端 `bun run test:coverage`：v8 provider，只统计逻辑层——`src-web/{utils,stores,composables}` 与组件旁 `use*.ts`；行 / 分支 / 函数 / 语句四项 100%。`main.ts` 是装配、api 是薄封装、`bench/` 是基准，均不入门槛。
- Rust `bun run coverage:rust`：实际参数以根 `package.json` 为唯一来源，统计 workspace 的 lib 目标，要求行覆盖 100%，并输出未覆盖行号；忽略正则 `lib\.rs$` 对所有 crate 生效，不限于 Tauri 装配入口；因此每个 `lib.rs` 只放模块声明 / 薄装配，不放业务逻辑。src-tauri 的 commands / events / ipc 等非忽略文件及业务 crate 的逻辑文件均须足额覆盖。前置组件与安装见[构建与开发](../architecture/build-and-development.md)。

改门槛口径（include 白名单、忽略正则、阈值数字）属于门禁变更：先在 task 里给出理由与新口径的验证结果，再动配置。

## 死代码检查

`bun run knip` 检查未用依赖、导出与文件，配置在 `knip.json`：entry 是 `index.html`、`tests/web/**/*.test.ts`、`src-web/bench/*.ts`。新依赖装了没用、导出无人消费，knip 会拦下；确属工具链需要而 knip 误报时，在 `knip.json` 的 `ignoreDependencies` 登记并在此处或对应 task 注明原因（JSON 不支持注释）。已登记：`tailwindcss`——经 app.css 的 `@import "tailwindcss"` 消费，knip 不追踪 CSS 导入（task 004）。`$schema` 指向 jsDelivr CDN（VS Code 对本地相对路径在部分工作区会按 git: 协议解析而报错；版本钉主版本 `@6` 与 package.json 对齐）。

## 基准

`bun run bench` 跑 `src-web/bench/` 下的 tinybench 示例，用于验证基准链路可用，已纳入 verify（秒级）。性能结论要可靠对比（同机器、同口径、多次采样），临时性结论写进对应 task，不进规范；需要持续跟踪性能时再登记专门 task 扩建基准集。
