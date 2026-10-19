<script setup lang="ts">
import { useI18n } from "vue-i18n";

const { t } = useI18n();
defineProps<{ title: string }>();
const emit = defineEmits<{ close: [] }>();
</script>

<template>
  <div class="settings-section-heading">
    <h3>{{ title }}</h3>
    <button
      type="button"
      class="settings-section-close"
      :aria-label="t('common.close')"
      @click="emit('close')"
    >
      <span aria-hidden="true">×</span>
    </button>
  </div>
</template>
