# 前端约定

更新日期：2026-10-10。除标注官方依据外均为项目约定。

## 逻辑归属

IPC 薄调用与 Rust 同型的 TS 载荷类型放 `src-web/api/`，不在组件中直接 invoke。

组件只做编排与渲染；可复用、可测试的逻辑放 `src-web/utils/` 并配单测，`<script setup>` 顶层不堆过程式逻辑，超过十行就抽函数。全局状态与路由在需求真实出现前不引入（当前无 pinia / vueuse / vue-router）。

Composable 遵循 [Vue 官方约定](https://vuejs.org/guide/reusability/composables.html#conventions-and-best-practices)：`use` 前缀、接受 ref / getter、用 watch 跟踪输入、返回包含 refs 的普通对象。必须在 setup 或活动 effectScope 中同步调用；资源通过 [onScopeDispose](https://vuejs.org/api/reactivity-advanced.html#onscopedispose) 清理。桌面事件订阅不读取挂载后的 DOM，不增加 SSR 承诺或无需求的全局 store。

回合消费者通过 `useLlmTurn` 管理已知 turnId 的订阅与重连；消费和恢复规则在 utils，通过注入端口独立测试。`recoveryError` 表示读取失败，不能覆盖 Rust 回合 outcome / error；产品界面由后续任务接入。

## SFC 与类型

组件一律 `<script setup lang="ts">`；类型导入用 inline 形态 `import { type Foo }`（ESLint `consistent-type-imports` 强制）；局部类型标注允许 `import("module").Type`，规则显式设置 `disallowTypeAnnotations: false`（[官方选项](https://typescript-eslint.io/rules/consistent-type-imports/#disallowtypeannotations)）；函数写显式返回类型，ref 给显式初值。模板内避免 `<img src="/...">` 这类运行时资产解析——happy-dom 测试环境会踩 vite 资产解析错误，需要图形就用内联 SVG。

## 测试

测试目录镜像 `src-web`：`tests/web/utils/turn-consumer.test.ts` 对应 `src-web/utils/turn-consumer.ts`。mock IPC 统一写法：

```ts
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
```

完整示例见 `tests/web/App.test.ts`。异步 UI 断言用 `vi.waitFor`，不用 sleep 凑等待。门槛与口径见[测试规范](testing.md)。

## 排版分工

Prettier 独占排版，ESLint 只管质量规则：Vue 排版类规则（`max-attributes-per-line` 等）已关闭，`eslint-config-prettier` 兜底关停冲突项。格式争议以 `bun run format` 的输出为准，不手调。

## 样式分工

组件样式用 Tailwind 工具类 + `@layer components` 小组件类；token 真源与 app.css 分层结构见[UI 风格](ui.md)禁则与[主题架构](../architecture/theming.md)目录。构建生成的样式集中放 `src-web/styles/generated/`，由根样式入口导入，不放在 `src-web/` 根层。主题源变更后更新并提交生成 CSS；`test` / `test:coverage` 先运行生成一致性检查，`tests/web/appcss.test.ts` 再互校 CSS token 与 Tailwind 映射。不在组件里写色值 / 时长魔法数，改样式统一动 token 或组件类。
