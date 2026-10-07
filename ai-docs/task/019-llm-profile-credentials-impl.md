# 019 — 实现：LLM 配置、原生凭据与代理

- 状态：in-progress
- 依赖：005、013、018
- 优先级：P0
- 创建 / 更新：2026-10-05 / 2026-10-07

## 目标与背景

回合需要可冻结的配置与凭据版本，且现有规则要求明文密钥不进入 Webview。交付可保存的 profile、凭据状态与三种代理模式。

## 必读

[LLM 规则与结构](../architecture/llm.md) · [通信契约](../architecture/ipc-contract.md) · [职责边界](../architecture/ts-rust-boundary.md) · [Rust 规范](../standards/rust.md) · [前端规范](../standards/frontend.md) · [注释规范](../standards/comments.md) · [测试规范](../standards/testing.md) · [提交规范](../standards/commits.md)。依赖任务及后续设计定稿文档从[任务索引](../task-index.md)查阅；规则数值以架构 / 契约原文为准，不在 task 重写。

## 范围与非目标

- profile 校验与持久化，冻结模型 / 调用形态 / 采样 / 代理配置；不可变凭据句柄支持回合内重试。
- Rust 原生密钥输入、OS 凭据库与契约允许的降级保存；设置 / 清除 / 取消、权限与 hint。
- system / none / manual 代理，TLS 正常验证、禁用敏感重定向及脱敏诊断。

非目标：不启动生成回合，不实现 Webview 明文密钥表单，不扩展供应商自动探测。

## 当前进度

- **已落地**：`src-rust/mythos-llm/` 的 `config`（profile 校验 / 持久化 / 冻结）、`credentials`（OS 凭据库优先 + 降级文件 + 单向迁移）、`proxy`（三模式与传输客户端统一构造）、`platform/`（按平台分发的原生能力）；`src-tauri/src/llm_commands.rs` 五条命令；`src-web/api/llm.ts` TS 同型；覆盖率门禁 `coverage-rust.config.mts` + `scripts/coverage-rust.mts`。Windows 原生输入全链路实测。
- **剩余**：Windows 修复后 CI 复验；本机 verify 十三项、lint:rust:fix 与 macOS 原生集成已过，macOS / Linux CI 原生集成已过。

## 前置条件与待决策

先在 Windows / macOS / Linux 验证原生安全输入可行性；如不可行，记录具体阻塞并修订设计后才能更换入口。不得默认放宽明文边界。实现为 Windows CredUI / macOS NSSecureTextField / Linux GTK 密码 Entry；命令层使用同型接口并统一 UI 主线程调度。macOS 本机真实 UI / Keychain 测试通过；Windows / Linux 本轮 CI 复验待完成，不在结果到达前标全部通过。

实现中登记的待决策项（处置结论随条目给出）：

1. **读改写并发锁**：`ProfileStore` 与 `CredentialFile` 的 load → modify → save 无锁，两个命令并发写同一文件会丢一次更新（整文件覆写）。**已处置（闭环）**：设计回写 storage.md「并发与锁」——原子写只保证物理完整，逻辑读改写由命令层进程内锁串行；实现为命令层两把 `Mutex`（profile / key 各一把），CredUI 等待用户输入不持锁，锁中毒忽略（文件操作原子，状态有效）。
2. **降级文件过期副本**：原“OS 读命中后重试清理”仍会在 OS 写失败 / 读成功时取旧值并删除新文件值。**已处置（本轮修正）**：credentials.json v2 在原子文件中登记 OS / file / deleted 权威状态，OS 写成功后只保留无密钥指针，写失败用新文件值，清除先墓碑；读取不迁移、不猜测先后，OS 不可读不当未设置。旧版字典保留其文件值，显式后续写入升级。两后端不可原子提交的失败仍报告不确定，不声称一定未修改。
3. **Sampling 模块归属**：它是传输参数（provider.rs）却被 config 持久化依赖。**已处置（落地）**：迁移到独立词汇模块 `sampling.rs`——被 provider（请求载荷）/ config（持久化）/ schedule（阶梯校验）共同消费，持久化不再反向依赖传输层；单一权威定义不变，serde 形状不变。

## 实施步骤

1. 完成三平台原生输入与凭据库探测，记录选型及失败恢复方式。（macOS 本机通过；Windows / Linux 本轮 CI 待复验）
2. 在 mythos-llm 中实现配置 / Secret 抽象，复用 store 文件工具；Tauri 层只适配原生交互。（完成）
3. 用代理与权限夹具验证冻结、切换、取消和脱敏；同步实际配置结构与 IPC 状态接口。（完成）

## 验收标准

- [x] 设置 / 换 key / clear 不改变已接纳回合的凭据版本；取消输入保留旧值。（freeze 单测：冻结后换 key / clear 不影响副本；CredUI 取消路径直测 + 命令层取消保留旧值单测）
- [x] IPC、配置、错误与日志均不含明文 key / 代理密码；权限与 hint 符合契约，降级保存有明确失败行为。（hint / 状态 JSON 形状、序列化无密钥、错误形状无明文单测；DACL / 0600 权限断言；降级 / 迁移 / 双后端失败单测）
- [x] 三种代理模式互不叠加；HTTPS、重定向和 TLS 分类有夹具验证。（本地代理夹具：manual / none / system 快照互斥、CONNECT 隧道、禁重定向、经代理不可信证书分类为 tls、Proxy-Basic 认证）
- [ ] 三平台原生输入及 OS 凭据库均有实际确认 / 取消 / 读写清记录：macOS 本机及 macOS / Linux CI 已通过；Windows 修复后本轮结果待记录。未验证结果不标通过。
- [x] 待决策三项处置完毕并记录结论：并发锁（设计回写 storage.md + 命令层双锁实现）、权威后端与墓碑（语义回写契约工程纪律 4 + v2 原子发布已实现）、Sampling 归属（迁移至独立 `sampling.rs` 词汇模块，持久化不反向依赖传输层）。
- [ ] 代码、注释、类型、文档与 task 同步；最终状态 `bun run verify` 十三项通过（最终代码 / 文档状态复验后记录结果）。

## 验证计划与结果

本地代理 / TLS fixture、配置与权限测试；原生输入和凭据库做三平台手工检查并记录环境。

| 日期       | 环境 / 命令                                            | 预期                                      | 实际结果                                           |
| ---------- | ------------------------------------------------------ | ----------------------------------------- | -------------------------------------------------- |
| 2026-10-07 | Windows 11；`bun run verify`（十三项）                 | 全项 exit 0                               | 通过（coverage 门禁见下条）                        |
| 2026-10-07 | 代理夹具（`cargo test -p mythos-llm proxy`）           | 三模式互斥、CONNECT / TLS / 重定向        | 16 项通过                                          |
| 2026-10-07 | Windows CredUI 对话框取消直测 + keyring 直测           | 取消 → `Ok(None)`；凭据库读写清正常       | 通过                                               |
| 2026-10-07 | coverage 门禁（`coverage-rust.config.mts` 逐文件预算） | 除登记伪影外全部文件 100%                 | 通过：22 文件、3 行登记伪影在预算内                |
| 2026-10-07 | 本机 macOS；`bun run test:native`                      | 真实 AppKit / Keychain 确认、取消、读写清 | 通过；合成值自动输入 / 关闭，条目已删除            |
| 2026-10-07 | CI 37591267085：macOS / Linux 原生集成                 | 确认 / 取消、OS 凭据读写清                | 两平台通过；Windows 前置路径测试失败，修复后待复验 |

本轮消融：在隔离副本逐项变异、先确认原树测试通过，只把可编译且行为断言失败计为捕捉，结束后还原。第一轮 9/10（数量用例被重复 ID 校验遮蔽）；改为 51 个独立 ID，并使用合法超量 JSON 后，第二轮 11/11 捕捉：权威后端、删除墓碑、OS 错误透传、私有权限顺序、profile 版本 / 重复 / 数量 / 大小、凭据大小、原生输入互斥、重定向禁止。未变异工作的分支源码，不扩大覆盖预算。

## 风险与回退

原生输入和凭据库具有平台差异；探测失败应阻塞该能力，不能静默转为 Webview 输入或不安全存储。回退代码时同步撤销接口 / 依赖与文档；已有存档和凭据不因回退删除。

## 决策与工作记录

- 2026-10-05：按用户要求规划 LLM 相关实现任务，明确依赖、范围和失败路径；本次只登记计划，不实施代码。
- 2026-10-07：Windows 范围实现完成。含：`platform/` 平台适配目录（按用户要求从单文件重构，macOS / Linux 的降级文件权限 0600 真实实现并交 CI unix 测试覆盖，原生输入以 `NativePrompt::Unverified` 显式拒绝，不静默降级为 Webview 明文输入）；覆盖率门禁改造（llvm-cov 跨二进制合并的幽灵未覆盖与 Win32 不可注入分支，方案为 `coverage-rust.config.mts` 逐文件预算 + `scripts/coverage-rust.mts` 执行，口径登记于 testing.md 与 build-and-development.md）；两轮代码 / 注释评审（错误映射具名化、unsafe 补 SAFETY、幽灵未覆盖 5 → 3）。回合命令（llm_submit / cancel / get_turn）归 020。
- 2026-10-07：**按用户决定回退为 in-progress**——原生输入未覆盖三平台就不算完成；macOS / Linux 的实现与真实环境验证由用户在本分支继续。同批把三个实现中识别的设计取舍（读改写并发锁、降级文件过期副本、Sampling 归属）登记进「前置条件与待决策」，完成前处置并记录结论。
- 2026-10-07：本分支整体评审修复私有凭据发布前权限、权威后端 / 删除墓碑、OS 错误吞掉、profile 读取版本 / 重复 ID / 数量 / 大小校验、SOCKS5 特性遗漏、代理 Debug 脱敏、并行测试修改全局环境、Windows SID 释放 / 对齐读取；启动持实例锁，凭据状态读取纳入 key 锁。补失败路径回归与架构 / 注释同步；三平台原生输入已实现；macOS 真实烟测已通过，Windows / Linux 本轮 CI 待复验，暂不标 done。
- 2026-10-07：按用户要求用统一 NativePrompt 接口封装平台差异，Rust / 测试规范同步；原生集成从 examples 迁到 tests/rust/native-platform workspace 测试 crate，覆盖三平台，通过显式 desktop-session 保持 UI 主线程。增加独立输入占用门禁，取消等待不释放仍打开的窗口门禁；CI Linux 用真实 GTK/Xvfb 与 Secret Service，不以模拟后端替代系统服务验收。
- 2026-10-07：按用户要求先 push 触发 CI，再整体审查 / 消融。补合法大 JSON 与独立 profile ID 两类测试盲区，11/11 变异捕捉；CI macOS / Linux 原生测试通过。Windows 将父路径为文件也报 NotFound，读取现区分非法父路径与首次缺项，显式失败且补直接回归。lint:rust:fix、verify 十三项通过；Windows CI 修复后结果待记录。

## 完成摘要

未完成。已落地与剩余见「当前进度」；验收只勾有证据的项，三平台原生输入齐全且待决策三项处置后，与索引一起标 done。
