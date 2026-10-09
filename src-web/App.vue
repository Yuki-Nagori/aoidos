<script setup lang="ts">
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { formatGreeting } from "./utils/greet";

const name = ref("");
const greeting = ref(formatGreeting(""));
const theme = ref<"dark" | "light">("dark");

function toggleTheme(): void {
  theme.value = theme.value === "dark" ? "light" : "dark";
  document.documentElement.dataset.theme = theme.value;
}

async function greet(): Promise<void> {
  try {
    greeting.value = await invoke<string>("greet", { name: name.value });
  } catch {
    // 浏览器预览没有 IPC 通道，回退纯前端问候。
    greeting.value = formatGreeting(name.value);
  }
}
</script>

<template>
  <main class="grid min-h-screen place-items-center p-6">
    <section class="glass-panel">
      <span class="led-dot" aria-hidden="true"></span>
      <h1 class="text-2xl font-semibold tracking-wide text-ink">Aoidos</h1>
      <p class="text-sm text-muted">AI 驱动的剧情跑团</p>
      <form class="flex gap-2" @submit.prevent="greet">
        <input
          v-model="name"
          type="text"
          placeholder="输入名字试试 IPC…"
          class="rounded-lg border border-hairline bg-transparent px-3 py-2 text-ink outline-none"
        />
        <button
          type="submit"
          class="cursor-pointer rounded-lg bg-accent px-4 py-2 text-white transition-colors duration-(--dur-micro) ease-(--ease-signature) hover:bg-accent-violet"
        >
          Greet
        </button>
      </form>
      <p class="text-sm text-muted" data-testid="greeting">{{ greeting }}</p>
      <button
        type="button"
        class="cursor-pointer text-xs text-accent-warm underline-offset-2 hover:underline"
        @click="toggleTheme"
      >
        切换主题（{{ theme === "dark" ? "浅色" : "深色" }}）
      </button>
    </section>
  </main>
</template>
