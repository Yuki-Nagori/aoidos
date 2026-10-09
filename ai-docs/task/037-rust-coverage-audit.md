# 037 — Rust 覆盖率预算审计与消除

- 状态：done
- 依赖：019、022、023（审计当前工作树）
- 优先级：P0
- 创建 / 更新：2026-10-09 / 2026-10-09

## 目标与范围

核实 coverage-rust.config.mts 六处逐文件预算的实际来源，补齐真实边界测试，尽可能删除预算；不跳过文件，不改变 workspace lib 统计口径。不代替 023 的完整产品验收。

## 必读

[测试规范](../standards/testing.md)、[Rust 规范](../standards/rust.md)、[注释规范](../standards/comments.md)。

## 实施与验收

- [x] 按 LLVM 函数实例核实差额，记录可定位的来源。
- [x] 补齐采样、缓存和迁移 / 序列化边界，删除已不需要的预算并同步台账。
- [x] bun run verify 十三项通过。
- [x] subagent 独立评审完成，处理问题并复验。

## 验证计划与结果

| 日期       | 命令 / 检查             | 实际结果                                                                                |
| ---------- | ----------------------- | --------------------------------------------------------------------------------------- |
| 2026-10-09 | `bun run coverage:rust` | 72 文件未覆盖 0 行；六处均 100%                                                         |
| 2026-10-09 | `bun run verify`        | 十三项通过，退出码 0；前端四指标 100%，Rust 单测 536 项通过                             |
| 2026-10-09 | subagent 独立只读评审   | 未发现阻塞问题；核对本任务完整 diff、生产调用链、LLVM 源码与当前 JSON，未重复运行 Cargo |

表中零缺口为当时 macOS 本机结果。023 合并后的 [main CI](https://github.com/Yuki-Nagori/aoidos/actions/runs/37837295003) 三平台在凭据命令缺 1 行失败，当时未同步此记录；issues #78 / #79 已要求补记与默认分支复验，不能以 PR 成功覆盖 main 失败。038 合并提交 `853aecc` 的 [main CI](https://github.com/Yuki-Nagori/aoidos/actions/runs/37906896119) 三平台全部通过；旧失败与新证据均保留，预算仍为空。

原始覆盖率 JSON 位于 `target/coverage-rust.json`。完整 verify 首次在插桩引擎进程启动时被系统 SIGKILL；第二次完整运行通过。

## 风险与回退

审计基于 023 实施工作树，只调整测试与预算记录；不改生产行为或覆盖率统计口径。跨平台结果以最终 CI 为准。

## 决策与工作记录

- 2026-10-09：核实六处预算：`sample`、`open_verified`、`line` 的成功 / 失败分散在不同泛型实例；runtime 缺缓存第 17 项淘汰及 checked-only 身份命中；proxy / llm_commands 已足额覆盖。
- 2026-10-09：LLVM `LineCoverageInfo::merge` 取 `max(covered)` / `max(total)`，不求行并集（[LLVM 实现](https://github.com/llvm/llvm-project/blob/main/llvm/tools/llvm-cov/CoverageSummaryInfo.h)，当日查阅）。补同一实例及缓存边界测试、明确等待 lease 释放，删除六处预算；生产代码和统计口径不变。
- 2026-10-09：独立评审核对 diff、调用链和 LLVM 统计，无阻塞；主代理完成最终 verify。三平台验收随 [023 PR](https://github.com/Yuki-Nagori/aoidos/pull/77) 完成，后续 CI 缺口及测试竞态修复记录归 023。

## 完成摘要

六处逐文件预算全部删除，补齐真实缓存边界和同一泛型实例的成功 / 失败测试；生产行为及统计口径保持一致。完整 verify 十三项通过，workspace lib 72 文件未覆盖 0 行，独立评审无阻塞问题。三平台最终复验见 023 的运行记录。
