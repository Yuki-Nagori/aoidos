import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "happy-dom",
    include: ["tests/web/**/*.test.ts", "tests/scripts/**/*.test.ts"],
    coverage: {
      provider: "v8",
      // 门槛统计可独立验证的规则与 composable；页面级编排及 bench 不计。
      include: ["src-web/features/**/*.ts", "src-web/shared/**/*.ts"],
      exclude: ["src-web/features/game/composables/useGameWorkspace.ts"],
      // Coverage is a floor for review, not a target for exhaustive branch-by-branch tests.
      thresholds: { lines: 90, functions: 90, branches: 90, statements: 90 },
    },
  },
});
