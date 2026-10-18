# 前端约定

更新日期：2026-10-19。除标注官方依据外均为项目约定。

## 逻辑归属

IPC 薄调用与 Rust 同型的 TS 载荷类型放 `src-web/api/`，不在组件中直接 invoke。

组件只做编排与渲染；跨功能复用、可独立测试的纯逻辑放 `src-web/shared/utils/` 并配单测，单个功能私有的逻辑留在对应 feature，`<script setup>` 顶层不堆过程式逻辑，超过十行就抽函数。全局状态与路由在需求真实出现前不引入（当前无 pinia / vueuse / vue-router）。

前端目录按职责分层，并逐步采用 feature-first 归档：`app/` 负责启动、根布局与跨功能组合；`features/<feature>/` 持有该功能的页面、组件、composable 与私有逻辑；`shared/` 只放确实被多个功能复用的展示原语、生命周期能力和纯逻辑；现有 `api/`、`i18n/`、`styles/` 保持独立边界。迁移期间先以 [051](../task/051-web-ui-architecture-refactor.md) 的目标结构为准，不为追求整齐而移动无关文件。

依赖只能由应用组合层向功能层、由功能层向共享层 / `api/` / `i18n/` 下行；共享模块不得导入功能模块，功能模块之间不得互相导入实现。跨功能协作由 app 组合时显式传入 props / callbacks 或共享契约完成；只有确有需求且先更新架构与依赖图时才引入路由或全局状态库。`App.vue` 保持应用入口与根组合，不承担多个功能的状态机、业务流程和大段模板；拆分依据是职责与状态生命周期，不设任意文件行数门槛。

Composable 遵循 [Vue 官方约定](https://vuejs.org/guide/reusability/composables.html#conventions-and-best-practices)：`use` 前缀、接受 ref / getter、用 watch 跟踪输入、返回包含 refs 的普通对象。必须在 setup 或活动 effectScope 中同步调用；资源通过 [onScopeDispose](https://vuejs.org/api/reactivity-advanced.html#onscopedispose) 清理。桌面事件订阅不读取挂载后的 DOM，不增加 SSR 承诺或无需求的全局 store。

回合消费者通过 `useLlmTurn` 管理已知 turnId 的订阅与重连；消费和恢复规则在 utils，通过注入端口独立测试。当前产品组合由 `app/App.vue` 组合产品设置、记忆面板和对局工作区；跨 feature 的设置展示留在 `app/components/SetupPanel.vue`。`features/game/` 持有 `GameWorkspace`、舞台、记录、输入与对应状态；其他功能按 `features/<feature>/` 归档。纯面板转换 / 输入预览留在 `features/game/utils/`，多个 feature 共用的格式化与订阅清理位于 `shared/utils/`。`recoveryError` 表示读取失败，不能覆盖 Rust 回合 outcome / error。架构拆分由 051 承接，Aoidos-art 视觉与页面移植由 052 承接；025 的三平台实机与辅助技术验收仍在任务中跟踪。

## SFC 与类型

组件一律 `<script setup lang="ts">`；纯类型依赖用 `import type { Foo }`（Biome `useImportType` 强制分离类型导入）；函数写显式返回类型，ref 给显式初值。模板内避免 `<img src="/...">` 这类运行时资产解析——happy-dom 测试环境会踩 vite 资产解析错误，需要图形就用内联 SVG。

## 测试

测试目录镜像 `src-web`：`tests/web/features/game/utils/turn-consumer.test.ts` 对应 `src-web/features/game/utils/turn-consumer.ts`；应用组合测试放 `tests/web/app/`。mock IPC 统一写法：

```ts
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
```

完整入口测试见 `tests/web/app/App.test.ts`。异步 UI 断言用 `vi.waitFor`，不用 sleep 凑等待。门槛与口径见[测试规范](testing.md)。

## 排版分工

Biome 负责 JavaScript / TypeScript、CSS、JSON 的 lint 与格式化；Vue、HTML、Markdown 和 YAML 由 dprint 格式化。Biome 的 Vue lint 支持仍属实验阶段，项目启用前须保持 `vue-tsc`、组件测试及 i18n 静态检查通过。`bun run lint` 还会运行 `i18n:check`：TypeScript 调用由语法树提取，Vue SFC 通过 Vue 编译器解析脚本与模板表达式，校验静态 key、占位符、未使用键与预留命名空间。格式争议以 `bun run format` 的输出为准，不手调；剧本发布原文按字节冻结并从格式化范围排除。

## 样式分工

组件样式用 Tailwind 工具类 + `@layer components` 小组件类；token 真源与 app.css 分层结构见[UI 风格](ui.md)禁则与[主题架构](../architecture/theming.md)目录。构建生成的样式集中放 `src-web/styles/generated/`，由根样式入口导入，不放在 `src-web/` 根层。主题源变更后更新并提交生成 CSS；`test` / `test:coverage` 先运行生成一致性检查，`tests/web/appcss.test.ts` 再互校 CSS token 与 Tailwind 映射。不在组件里写色值 / 时长魔法数，改样式统一动 token 或组件类。
