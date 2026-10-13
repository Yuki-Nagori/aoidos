# 049 — Windows 测试 I/O 与原生 HMR 超时

- 状态：done
- 依赖：048
- 优先级：P1
- 创建 / 更新：2026-10-13 / 2026-10-13

## 目标与背景

048 合并后，Windows 普通 Rust workspace 测试仍有明显运行时间波动；PR #144 的成功运行中 memory 测试为 358.92 秒，合并后 main 运行中为 157.07 秒。慢项集中在 memory 仓储少数反复执行 SQLite WAL 提交与原子文件 `sync_all` 的压力用例，不能据此关闭所有测试夹具的同步要求。

合并后 main run #38343926466 的 Rust workspace 测试通过，但原生 HMR 步骤在 WebView2 发出 Started 导航事件前触发内部 20 秒 watchdog。成功的 PR run #38342661503 从启动原生测试到 Started 也耗时 16.28 秒；当前计时还把 WebView 初始化时间算入 CSS 更新预算。

首轮优化 PR #150 的 CI 已通过，但结果没有证明 Windows 时间下降：workspace 357 秒、engine 73.64 秒、memory 170.73 秒、native 66 秒；main 基线分别为 workspace 337 秒、engine 81.22 秒、memory 157.07 秒。memory 时间仍处于历史波动范围，需继续减少分页压力测试中不必要的 WAL 提交，并以更新后的 CI 复测。

## 必读

- [测试规范](../standards/testing.md)
- [048 测试质量与 CI 用时优化](048-test-quality-ci.md)
- [Rust 规范](../standards/rust.md)
- [注释规范](../standards/comments.md)

## 范围与非目标

- 优化有证据支持的 Windows 高 I/O 测试夹具；纯规则 / 容量测试优先用现有内存 SQLite 夹具，耐久性与恢复测试继续验证真实文件、WAL 和同步提交。
- 审查长链和密集恢复压力用例，只有在等价行为断言完整保留时才减少重复耐久 I/O。
- 分离原生 WebView 导航启动预算与页面 ready 后 CSS HMR 更新预算，保留真实 WebView、同文档身份及更新前后断言；不以重试掩盖失败。
- 更新规范与本任务，分别记录 Windows Rust 测试和 native 步骤的 CI 实测耗时。
- 不全局关闭 SQLite synchronous，不删减高风险行为验证，不调整覆盖率门槛。

## 验收标准

- [x] 慢测试逐项有行为目的和夹具选择依据；只有不验证持久性的测试改用内存 SQLite。
- [x] 恢复压力用例只保留必要的硬边界规模；减少重复写入时，分页和不可恢复状态断言仍成立。
- [x] WebView 创建、HMR 导航与 CSS 更新分别有独立、有界的超时及可定位阶段的日志；不引入自动重试。
- [x] Windows CI 复验成功；Rust 测试与 native 验收分开记录耗时，并与 PR #144 / main run #38343926466 的对应步骤比较。
- [x] 代码、注释与文档同步；`bun run verify` 通过，独立 review 完成并处理发现。

## 验证记录

- 2026-10-13，PR #144 Windows Rust workspace：memory 358.92 秒，engine 153.53 秒；全部通过。
- 2026-10-13，main run #38343926466 Windows Rust workspace：memory 157.07 秒，engine 81.22 秒；全部通过；native HMR 内部 20 秒 watchdog 失败。
- 2026-10-13，本机 `bun run verify`：通过；Rust 覆盖 31,693 / 31,806 行（99.64%），Vitest 349 项通过。
- 2026-10-13，本机 `aoidos-memory`：219 项通过，12.48 秒；别名来源容量用例单独运行 0.03 秒。
- 2026-10-13，优化恢复压力用例后，`aoidos-memory` 219 项通过，9.02 秒；长无效链单测从本机 6.40 秒降至 3.67 秒（约 43%）。该测试仍跨过 256 条扫描上限，并确认操作分页时不检查条目、最终标记为 unavailable。
- 2026-10-13，密集预备组测试改为仅保留 48 来源的最严场景及内存 SQLite；仍逐组调用正式 `prepare` / `write_versions`，并断言分页不会提前应用下一组。单测约 2.55 秒。
- 2026-10-13，首轮 PR #150 Windows CI 全部通过。workspace 测试 357 秒，其中 engine 73.64 秒、memory 170.73 秒；native UI / 主 Webview IPC 66 秒。与 main run #38343926466 相比，workspace / memory 没有实测下降，因此继续将只验证分页预算的回合回执、效果回退与跨页恢复用例改为内存 SQLite。提交型夹具通过正式接口写入版本；批量分页夹具直接登记有效正文文件，均由正式恢复路径校验。
- 2026-10-13，本机 `hmr-platform --no-run`：编译通过；未在本机启动桌面 WebView。该条记录于首轮 CI 前；PR #150 两轮 Windows 原生步骤均通过。
- 2026-10-13，`gpt-6.1-sol` / `medium` 独立复核：发现并修复创建阶段退出无硬保障及 Vite 子进程过早清理；复核通过。
- 2026-10-13，PR #150 更新后 CI run #38360606036 三平台全部通过。Windows Rust workspace 312 秒（engine 68.38 秒、memory 120.49 秒），较 main run #38343926466 的 337 秒、81.22 秒、157.07 秒分别缩短 25 秒、12.84 秒、36.58 秒；本轮 native 步骤 85 秒，其中 WebView 创建 5.35 秒，HMR、IPC 和产品集成均通过。与首轮 PR #150 相比，workspace 从 357 秒降至 312 秒、memory 从 170.73 秒降至 120.49 秒；native 总时长含独立编译，单次波动较大，故单列记录。
- 2026-10-13，最新四处内存 SQLite 夹具调整经独立 review 通过；全量 `bun run verify`、`bun run lint:rust:fix` 与提交格式钩子通过，任务验收完成。
