# 049 — Windows 测试 I/O 与原生 HMR 超时

- 状态：in-progress
- 依赖：048
- 优先级：P1
- 创建 / 更新：2026-10-13 / 2026-10-13

## 目标与背景

048 合并后，Windows 普通 Rust workspace 测试仍有明显运行时间波动；PR #144 的成功运行中 memory 测试为 358.92 秒，合并后 main 运行中为 157.07 秒。慢项集中在 memory 仓储少数反复执行 SQLite WAL 提交与原子文件 `sync_all` 的压力用例，不能据此关闭所有测试夹具的同步要求。

合并后 main run #38343926466 的 Rust workspace 测试通过，但原生 HMR 步骤在 WebView2 发出 Started 导航事件前触发内部 20 秒 watchdog。成功的 PR run #38342661503 从启动原生测试到 Started 也耗时 16.28 秒；当前计时还把 WebView 初始化时间算入 CSS 更新预算。

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

- [ ] 慢测试逐项有行为目的和夹具选择依据；只有不验证持久性的测试改用内存 SQLite。
- [ ] 恢复压力用例只保留必要的硬边界规模；减少重复写入时，分页和不可恢复状态断言仍成立。
- [ ] WebView 创建、HMR 导航与 CSS 更新分别有独立、有界的超时及可定位阶段的日志；不引入自动重试。
- [ ] Windows CI 复验成功；Rust 测试与 native 验收分开记录耗时，并与 PR #144 / main run #38343926466 的对应步骤比较。
- [ ] 代码、注释与文档同步；`bun run verify` 通过，独立 review 完成并处理发现。

## 验证记录

- 2026-10-13，PR #144 Windows Rust workspace：memory 358.92 秒，engine 153.53 秒；全部通过。
- 2026-10-13，main run #38343926466 Windows Rust workspace：memory 157.07 秒，engine 81.22 秒；全部通过；native HMR 内部 20 秒 watchdog 失败。
- 2026-10-13，本机 `bun run verify`：通过；Rust 覆盖 31,693 / 31,806 行（99.64%），Vitest 349 项通过。
- 2026-10-13，本机 `aoidos-memory`：219 项通过，12.48 秒；别名来源容量用例单独运行 0.03 秒。
- 2026-10-13，优化恢复压力用例后，`aoidos-memory` 219 项通过，9.02 秒；长无效链单测从本机 6.40 秒降至 3.67 秒（约 43%）。该测试仍跨过 256 条扫描上限，并确认操作分页时不检查条目、最终标记为 unavailable。
- 2026-10-13，密集预备组测试改为仅保留 48 来源的最严场景及内存 SQLite；仍逐组调用正式 `prepare` / `write_versions`，并断言分页不会提前应用下一组。单测约 2.55 秒。
- 2026-10-13，本机 `hmr-platform --no-run`：编译通过；未在本机启动桌面 WebView，Windows 原生运行结果待 CI 验证。
- 2026-10-13，`gpt-6.1-sol` / `medium` 独立复核：发现并修复创建阶段退出无硬保障及 Vite 子进程过早清理；复核通过。
