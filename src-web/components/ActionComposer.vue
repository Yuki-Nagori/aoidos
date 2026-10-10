<script setup lang="ts">
import { ref } from "vue";
import { useI18n } from "vue-i18n";
import type { ComposerPreview } from "../utils/ui-shell";

const props = defineProps<{
  modelValue: string;
  preview: ComposerPreview;
  submitDisabled: boolean;
  submitLabel: string;
  tooLarge: boolean;
  slashMenuDismissed: boolean;
  rewindSelectorOpen: boolean;
}>();
const emit = defineEmits<{
  "update:modelValue": [value: string];
  submit: [];
  keydown: [event: KeyboardEvent];
  compositionStart: [];
  compositionEnd: [];
  chooseCommand: [command: string];
}>();
const { t } = useI18n();
const textarea = ref<HTMLTextAreaElement>();

function focus(): void {
  textarea.value?.focus();
}

defineExpose({ focus });
</script>

<template>
  <form class="flex items-end gap-2" @submit.prevent="emit('submit')">
    <textarea
      ref="textarea"
      :value="props.modelValue"
      :aria-label="t('game.action')"
      :placeholder="t('game.actionPlaceholder')"
      :maxlength="32768"
      class="min-h-20 w-full resize-y rounded-lg border border-hairline bg-transparent p-3 focus-visible:outline-2 focus-visible:outline-accent"
      @input="emit('update:modelValue', ($event.target as HTMLTextAreaElement).value)"
      @keydown="emit('keydown', $event)"
      @compositionstart="emit('compositionStart')"
      @compositionend="emit('compositionEnd')"
    />
    <button
      type="submit"
      class="rounded-lg bg-accent px-4 py-2 font-medium text-on-accent disabled:opacity-50"
      :disabled="props.submitDisabled"
    >
      {{ props.submitLabel }}
    </button>
  </form>
  <p v-if="props.tooLarge" class="mt-2 text-xs text-danger" role="alert">
    {{ t("game.actionTooLarge") }}
  </p>
  <div
    v-if="props.preview.slashActive && !props.slashMenuDismissed"
    class="mt-2 rounded-lg border border-hairline p-2"
    role="listbox"
    :aria-label="t('game.commandSuggestions')"
  >
    <p v-if="props.preview.invalidSlash" class="text-xs text-warning">
      {{ t("game.unknownCommand") }}
    </p>
    <button
      v-for="candidate in props.preview.candidates"
      :key="candidate"
      type="button"
      role="option"
      class="mr-2 text-sm text-ink underline"
      @click="emit('chooseCommand', candidate)"
    >
      {{ t("game.slashCommand", { command: candidate }) }}
    </button>
  </div>
  <p
    v-else-if="props.rewindSelectorOpen || props.preview.command?.name === 'rewind'"
    class="mt-2 text-xs text-muted"
  >
    {{ t("game.chooseRewindTarget") }}
  </p>
  <p v-else-if="props.preview.contentRequired" class="mt-2 text-xs text-warning">
    {{ t("game.contentRequired") }}
  </p>
  <p v-else-if="props.preview.mode === 'outOfCharacter'" class="mt-2 text-xs text-muted">
    {{ t("game.outOfCharacter") }}
  </p>
  <p
    v-else-if="
      props.preview.command?.name === 'interrupt' && !props.preview.command.argument.trim()
    "
    class="mt-2 text-xs text-warning"
  >
    {{ t("game.interruptNeedsText") }}
  </p>
</template>
