# Task 索引

采用「先写 task，再做实现」的工作方式：非平凡改动先从[模板](task/_template.md)建任务并在此登记，再动代码。任务详情是范围、决策与验证证据的主记录，索引只是状态摘要，状态变更时两处一起更新。

## 使用方式

```text
ai-docs/
├── architecture/          # 架构总览与主题文档
├── standards/             # 技术规范与依据
├── ideas/                 # 构想记录：未排期，只 mark 不承诺
├── research/              # 调查报告：读开源仓库攒灵感
├── task-index.md          # 本文件：任务队列与状态
└── task/
    ├── _template.md       # 创建任务时复制
    ├── 001-template-baseline.md
    └── …                  # 后续任务按编号递增，目录树不逐一列举
```

1. 复制[模板](task/_template.md)为 `task/NNN-kebab-case.md`，编号取当前最大编号加一，不复用；填写范围与可判断的验收条件。
2. 在下方队列表登记，核对依赖无环。
3. 开始实现标 in-progress，只做该任务范围；范围变化先改 task。
4. 每次 commit 更新 task 的工作记录与验证；全部验收有证据后与索引一起标 done。提交前检查见[提交规范](standards/commits.md)。
5. 例外：CI 修复、门禁机械修正等平凡改动直接提交，不建 task（提交消息 `Task:` 字段写「无」并注明缘由）。

## 状态约定

| 状态        | 含义                      |
| ----------- | ------------------------- |
| draft       | 缺少范围 / 验收，不能开始 |
| planned     | 已编排，依赖未满足        |
| ready       | 可开始，尚无实现          |
| in-progress | 正在实施                  |
| blocked     | 有具体阻塞，记录解除条件  |
| done        | 验收完成且有证据          |
| deferred    | 暂不排入                  |
| cancelled   | 已取消，保留编号与原因    |

## 任务队列

任务按性质分三类：**设计**（产出规则 / 结构文档，定稿评审后新增对应实现任务）、**实现**（写代码，必须引用已定稿的设计）、**文档 / 基线**（纯文档与工程收尾）。优先级：底层任务 P0，界面任务 P1，远期任务 P2；同优先级内按编号顺序。

### 设计任务

| 编号 | 任务                                                      | 域                    | 依赖          | 状态    |
| ---- | --------------------------------------------------------- | --------------------- | ------------- | ------- |
| 005  | [LLM 接入与护栏](task/005-llm-design.md)                  | **底层·LLM**          | 002           | ready   |
| 006  | [对局记录与上下文](task/006-record-design.md)             | **底层·记录引擎**     | 005、011      | planned |
| 007  | [记忆系统](task/007-memory-design.md)                     | **底层·记忆**（远期） | 005、006、011 | planned |
| 008  | [界面结构与交互](task/008-ui-shell-design.md)             | 界面·交互             | 003           | ready   |
| 009  | [主题系统与剧本皮肤](task/009-theming-design.md)          | 界面·主题             | 004           | planned |
| 010  | [通信契约与工程纪律](task/010-ipc-contract-design.md)     | **底层·契约**         | 002           | done    |
| 011  | [存储与文件基建](task/011-storage-design.md)              | **底层·存储**         | 002           | done    |
| 012  | [回合与阶段状态机](task/012-turn-state-machine-design.md) | **底层·状态机**       | 005、006      | planned |

### 实现任务

| 编号 | 任务                                                                  | 域            | 依赖     | 状态  |
| ---- | --------------------------------------------------------------------- | ------------- | -------- | ----- |
| 004  | [引入 Tailwind v4 并接线 token](task/004-tailwind-v4.md)              | 界面·基建     | 002      | ready |
| 013  | [存储与文件基建 crate](task/013-storage-impl.md)                      | **底层·存储** | 011      | done  |
| 015  | [CmdError 与命令错误形状](task/015-cmd-error-impl.md)                 | **底层·契约** | 010、013 | done  |
| 016  | [IPC 事件基建](task/016-ipc-events-infra.md)                          | **底层·契约** | 010、015 | done  |
| 017  | [store 域命令与迁移事件](task/017-store-commands-migration-events.md) | **底层·存储** | 016、013 | done  |

（各设计的实现任务在定稿后新增，标题附「实现」，必读引用对应设计文档。）

### 文档与基线

| 编号 | 任务                                             | 依赖 | 状态 |
| ---- | ------------------------------------------------ | ---- | ---- |
| 001  | [模板基线](task/001-template-baseline.md)        | —    | done |
| 002  | [Mythos 产品化改名](task/002-rebrand-mythos.md)  | 001  | done |
| 003  | [UI 风格规范落档](task/003-ui-style-standard.md) | 002  | done |
| 014  | [注释规范落档](task/014-comments-standard.md)    | 002  | done |
