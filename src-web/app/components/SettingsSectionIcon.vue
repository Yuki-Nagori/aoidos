<script setup lang="ts">
defineProps<{
  name:
    | "appearance"
    | "api"
    | "gameplay"
    | "billing"
    | "price"
    | "balance"
    | "memory"
    | "memory-list"
    | "memory-pending"
    | "scenario";
}>();
</script>

<template>
  <svg class="settings-nav-icon" viewBox="0 0 24 24" fill="none" aria-hidden="true">
    <template v-if="name === 'appearance'">
      <path d="M12 3a9 9 0 1 0 0 18c1.5 0 2-1 2-2s-1-1.5-1-2.5 1-1.5 2-1.5h2a4 4 0 0 0 4-4c0-4.4-4-8-9-8z" />
      <circle cx="7.5" cy="11" r="1" />
      <circle cx="10" cy="7" r="1" />
      <circle cx="15" cy="7" r="1" />
    </template>
    <template v-else-if="name === 'api'">
      <circle cx="8" cy="15" r="4" />
      <path d="m11 12 8-8M16 7l2 2" />
    </template>
    <template v-else-if="name === 'gameplay'">
      <rect x="4" y="4" width="16" height="16" rx="3.5" />
      <circle cx="9" cy="9" r="0.9" fill="currentColor" />
      <circle cx="15" cy="15" r="0.9" fill="currentColor" />
      <circle cx="15" cy="9" r="0.9" fill="currentColor" />
      <circle cx="9" cy="15" r="0.9" fill="currentColor" />
    </template>
    <template v-else-if="name === 'billing'">
      <ellipse cx="12" cy="7" rx="7" ry="3" />
      <path d="M5 7v5c0 1.7 3.1 3 7 3s7-1.3 7-3V7M5 12v5c0 1.7 3.1 3 7 3s7-1.3 7-3v-5" />
    </template>
    <template v-else-if="name === 'price'">
      <path d="M12 5v14M5 12h14" />
    </template>
    <template v-else-if="name === 'balance'">
      <path d="M3 12h3l2-6 4 12 3-9 2 3h4" />
    </template>
    <template v-else-if="name === 'memory'">
      <path d="M9 4a3 3 0 0 0-3 3 3 3 0 0 0-2 5 3 3 0 0 0 2 5 3 3 0 0 0 6 1V5a3 3 0 0 0-3-1zM15 4a3 3 0 0 1 3 3 3 3 0 0 1 2 5 3 3 0 0 1-2 5 3 3 0 0 1-6 1" />
    </template>
    <template v-else-if="name === 'memory-list'">
      <path d="M9 6h11M9 12h11M9 18h11M4.5 6h.01M4.5 12h.01M4.5 18h.01" />
    </template>
    <template v-else-if="name === 'memory-pending'">
      <circle cx="12" cy="12" r="9" />
      <path d="M9.5 9.5a2.5 2.5 0 1 1 3.5 2.3c-.7.3-1 .8-1 1.5v.4M12 16.6v.1" />
    </template>
    <template v-else>
      <path d="M5 5a2 2 0 0 1 2-2h12v16H7a2 2 0 0 0-2 2zM5 19V5" />
    </template>
  </svg>
</template>
