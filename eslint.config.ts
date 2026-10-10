import eslint from "@eslint/js";
import eslintConfigPrettier from "eslint-config-prettier";
import vueI18n from "@intlify/eslint-plugin-vue-i18n";
import tseslint from "typescript-eslint";
import pluginVue from "eslint-plugin-vue";
import type { Linter } from "eslint";

// .ts 与 .vue 的 <script setup> 共用同一套 TS 规则。
const tsRules: Linter.RulesRecord = {
  "@typescript-eslint/no-empty-object-type": ["error", { allowInterfaces: "with-single-extends" }],
  "@typescript-eslint/consistent-type-imports": [
    "error",
    { prefer: "type-imports", fixStyle: "inline-type-imports", disallowTypeAnnotations: false },
  ],
  "@typescript-eslint/no-unused-vars": [
    "error",
    { argsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
  ],
};

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "coverage/**",
      "public/theme-bootstrap.js",
      "src-tauri/**",
      "tests/rust/native-platform/gen/**",
      "target/**",
      "node_modules/**",
    ],
  },
  eslint.configs.recommended,
  ...tseslint.configs.recommended,
  ...(vueI18n.configs["flat/recommended"] as Linter.Config[]),
  {
    files: ["src-web/**/*.vue", "src-web/**/*.ts"],
    rules: {
      "@intlify/vue-i18n/no-unused-keys": [
        "error",
        {
          src: "src-web",
          extensions: [".vue", ".ts"],
          ignores: ["native.**", "budget.**", "memory.**"],
        },
      ],
      "@intlify/vue-i18n/no-missing-keys-in-other-locales": "error",
    },
  },
  {
    settings: { "vue-i18n": { localeDir: "./locales/*.json", messageSyntaxVersion: "^11.0.0" } },
  },
  { files: ["**/*.ts"], rules: tsRules },
  // SFC 脚本块由 TS 解析器接管；no-undef 读不懂 TS 类型，真值检查归 vue-tsc。
  ...pluginVue.configs["flat/recommended"].map((config): Linter.Config => ({
    ...config,
    files: ["**/*.vue"],
    rules: {
      ...config.rules,
      "no-undef": "off",
      // 排版归 prettier，关掉会打架的 Vue 排版规则。
      "vue/max-attributes-per-line": "off",
      "vue/singleline-html-element-content-newline": "off",
      "vue/html-self-closing": "off",
      "vue/attributes-order": "off",
    },
  })),
  {
    files: ["**/*.vue"],
    languageOptions: {
      parserOptions: { parser: tseslint.parser, extraFileExtensions: [".vue"] },
    },
    rules: tsRules,
  },
  { files: ["src-web/App.vue"], rules: { "vue/multi-word-component-names": "off" } },
  // 必须最后：关停上面全部与 prettier 冲突的规则。
  eslintConfigPrettier,
);
