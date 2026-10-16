import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [vue()],
  test: {
    environment: "happy-dom",
    include: ["tests/web/**/*.test.ts", "tests/scripts/**/*.test.ts"],
    coverage: {
      provider: "v8",
      // 门槛只统计逻辑层，装配与 bench 不计（与 Rust 侧口径对齐）。
      include: [
        "src-web/utils/**/*.ts",
        "src-web/stores/**/*.ts",
        "src-web/composables/**/*.ts",
        "src-web/components/**/use*.ts",
        "src-web/views/**/use*.ts",
      ],
      // Coverage is a floor for review, not a target for exhaustive branch-by-branch tests.
      thresholds: { lines: 90, functions: 90, branches: 90, statements: 90 },
    },
  },
});
