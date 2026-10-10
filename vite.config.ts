import tailwindcss from "@tailwindcss/vite";
import VueI18nPlugin from "@intlify/unplugin-vue-i18n/vite";
import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

// Tauri 要求固定开发端口，生产构建产物落在 dist/。
export default defineConfig({
  plugins: [
    VueI18nPlugin({
      include: "./locales/**",
      runtimeOnly: true,
      compositionOnly: true,
      dropMessageCompiler: true,
    }),
    vue(),
    tailwindcss(),
  ],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**"] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
});
