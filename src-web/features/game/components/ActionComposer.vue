<script setup lang="ts">
import { computed, nextTick, ref, useId, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { CommandSuggestion, ComposerPreview } from "../utils/ui-shell";
import type { ScenePresenceSnapshot } from "../../../api/engine";
import SelectMenu from "../../../shared/components/SelectMenu.vue";

const props = defineProps<{
  modelValue: string;
  suggestedAction?: string;
  preview: ComposerPreview;
  submitDisabled: boolean;
  submitLabel: string;
  tooLarge: boolean;
  slashMenuDismissed: boolean;
  rewindSelectorOpen: boolean;
  inFlight?: boolean;
  interruptAvailable?: boolean;
  controlDisabled?: boolean;
  interruptDisabled?: boolean;
  resumeAvailable?: boolean;
  availableCommands?: readonly CommandSuggestion[];
  presenceHasScene?: boolean;
  presenceReady?: boolean;
  presenceLoading?: boolean;
  presenceUnavailable?: boolean;
  presenceCharacters?: readonly ScenePresenceSnapshot["characters"][number][];
  selectedCharacterId?: string;
}>();
const emit = defineEmits<{
  "update:modelValue": [value: string];
  submit: [];
  keydown: [event: KeyboardEvent];
  compositionStart: [];
  compositionEnd: [];
  chooseCommand: [command: string];
  selectRecipient: [characterId: string];
  refreshPresence: [];
  stop: [];
  interrupt: [];
}>();
const { t } = useI18n();
const textarea = ref<HTMLTextAreaElement>();
const commandMenu = ref<HTMLElement>();
const id = `game-command-${useId().replaceAll(":", "")}`;
const selectedCandidate = ref<CommandSuggestion>();
const menuOpen = computed(() => props.preview.slashActive && !props.slashMenuDismissed);
const commandDescriptions = computed(() => ({
  ooc: t("game.commandDescriptions.ooc"),
  interrupt: t("game.commandDescriptions.interrupt"),
  stop: t("game.commandDescriptions.stop"),
  resume: t("game.commandDescriptions.resume"),
  regenerate: t("game.commandDescriptions.regenerate"),
  rewind: t("game.commandDescriptions.rewind"),
  pin: t("game.commandDescriptions.pin"),
  latest: t("game.commandDescriptions.latest"),
}));
const availableCommands = computed(
  () => new Set(props.availableCommands ?? props.preview.candidates),
);
const selectableCandidates = computed(() =>
  props.preview.candidates.filter((candidate) => availableCommands.value.has(candidate)),
);

watch(
  () => [props.modelValue, props.preview.activeCandidate, props.availableCommands] as const,
  () => {
    const active = props.preview.activeCandidate;
    selectedCandidate.value = active && availableCommands.value.has(active) ? active : undefined;
  },
  { immediate: true },
);
watch([selectedCandidate, menuOpen], async ([candidate, open]) => {
  if (!candidate || !open) return;
  await nextTick();
  commandMenu.value
    ?.querySelector<HTMLElement>(`#${id}-option-${candidate}`)
    ?.scrollIntoView?.({ block: "nearest" });
});

function chooseCandidate(candidate: CommandSuggestion): void {
  if (!availableCommands.value.has(candidate)) return;
  emit("chooseCommand", candidate);
}

function onTextareaKeydown(event: KeyboardEvent): void {
  if (
    !event.isComposing &&
    props.preview.slashActive &&
    !props.slashMenuDismissed &&
    selectableCandidates.value.length > 0
  ) {
    const index = selectableCandidates.value.indexOf(selectedCandidate.value!);
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const direction = event.key === "ArrowDown" ? 1 : -1;
      const nextIndex =
        index < 0
          ? direction > 0
            ? 0
            : selectableCandidates.value.length - 1
          : (index + direction + selectableCandidates.value.length) %
            selectableCandidates.value.length;
      selectedCandidate.value = selectableCandidates.value[nextIndex];
      return;
    }
    if (
      event.key === "Tab" &&
      selectedCandidate.value &&
      availableCommands.value.has(selectedCandidate.value)
    ) {
      event.preventDefault();
      chooseCandidate(selectedCandidate.value);
      return;
    }
  }
  emit("keydown", event);
}

function focus(): void {
  textarea.value?.focus();
}

defineExpose({ focus });
</script>

<template>
  <form class="game-composer" @submit.prevent="emit('submit')">
    <div
      v-if="props.preview.slashActive && !props.slashMenuDismissed"
      class="game-command-menu"
      :id="`${id}-listbox`"
      ref="commandMenu"
      role="listbox"
      :aria-label="t('game.commandSuggestions')"
    >
      <div class="game-command-menu-head">
        <span>{{ t("game.commandSuggestions") }}</span>
        <span class="game-command-menu-hints" aria-hidden="true">
          <kbd class="kbd">↑</kbd><kbd class="kbd">↓</kbd><kbd class="kbd">Tab</kbd><kbd
            class="kbd"
          >Esc</kbd>
        </span>
      </div>
      <p v-if="props.preview.invalidSlash" class="game-command-menu-empty text-warning">
        {{ t("game.unknownCommand") }}
      </p>
      <button
        v-for="candidate in props.preview.candidates"
        :key="candidate"
        type="button"
        :id="`${id}-option-${candidate}`"
        role="option"
        class="game-command-row"
        :class="{
          'game-command-row-selected': candidate === selectedCandidate,
          'game-command-row-disabled': !availableCommands.has(candidate),
        }"
        :aria-selected="candidate === selectedCandidate"
        :aria-disabled="!availableCommands.has(candidate)"
        :disabled="!availableCommands.has(candidate)"
        @click="chooseCandidate(candidate)"
      >
        <span class="game-command-key">{{ t("game.slashCommand", { command: candidate }) }}</span>
        <span class="game-command-description">{{ commandDescriptions[candidate] }}</span>
        <span v-if="!availableCommands.has(candidate)" class="game-command-state">
          {{ t("game.commandUnavailable") }}
        </span>
        <kbd v-else-if="candidate === selectedCandidate" class="kbd game-command-tab">Tab</kbd>
      </button>
    </div>
    <div class="game-composer-input">
      <div class="game-composer-top">
        <div v-if="props.presenceHasScene" class="game-composer-recipient">
          <SelectMenu
            v-if="props.presenceReady"
            :model-value="props.selectedCharacterId ?? ''"
            :options="[
              { value: '', label: t('game.replyToNarration'), marker: 'book' as const },
              ...(props.presenceCharacters ?? []).map((character) => ({
                value: character.id,
                label: character.canonicalName,
                marker: 'avatar' as const,
              })),
            ]"
            :label="t('game.replyTo')"
            :menu-title="t('game.replyTo')"
            :footer-note="t('game.presenceNote')"
            variant="recipient"
            :disabled="props.submitDisabled"
            placement="top"
            @update:model-value="emit('selectRecipient', $event)"
          />
          <span v-else-if="props.presenceLoading" role="status">{{
            t("game.presenceLoading")
          }}</span>
          <span v-else-if="props.presenceUnavailable" role="status">
            {{ t("game.presenceUnavailable") }}
            <button type="button" class="underline" @click="emit('refreshPresence')">
              {{ t("common.retry") }}
            </button>
          </span>
        </div>
      </div>
      <textarea
        ref="textarea"
        :value="props.modelValue"
        :aria-label="t('game.action')"
        :placeholder="t('game.actionPlaceholder')"
        :maxlength="32768"
        :aria-controls="menuOpen ? `${id}-listbox` : undefined"
        :aria-expanded="menuOpen"
        :aria-activedescendant="menuOpen && selectedCandidate ? `${id}-option-${selectedCandidate}` : undefined"
        class="game-composer-text"
        @input="emit('update:modelValue', ($event.target as HTMLTextAreaElement).value)"
        @keydown="onTextareaKeydown"
        @compositionstart="emit('compositionStart')"
        @compositionend="emit('compositionEnd')"
      />
      <div class="game-composer-foot">
        <span v-if="!props.inFlight" class="game-composer-key-hints" aria-hidden="true">
          <kbd class="kbd">Enter</kbd><span>{{ t("game.composerSendHint") }}</span>
          <kbd class="kbd">Shift+Enter</kbd><span>{{ t("game.composerNewlineHint") }}</span>
        </span>
        <span v-else />
        <div
          v-if="props.inFlight && !props.preview.command"
          class="game-composer-actions"
        >
          <button
            v-if="props.interruptAvailable"
            type="button"
            class="game-composer-secondary"
            :disabled="props.interruptDisabled"
            @click="emit('interrupt')"
          >
            {{ t("game.interrupt") }}
          </button>
          <button
            type="button"
            class="game-composer-secondary"
            :disabled="props.controlDisabled"
            @click="emit('stop')"
          >
            <svg class="game-composer-submit-icon" viewBox="0 0 24 24" aria-hidden="true">
              <rect x="7" y="7" width="10" height="10" rx="2" />
            </svg>
            {{ t("game.stopTurn") }}
          </button>
        </div>
        <button
          v-else
          type="submit"
          class="game-composer-submit"
          :disabled="props.submitDisabled"
        >
          <svg class="game-composer-submit-icon" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M5 12h12M13 6l6 6-6 6" />
          </svg>
          {{ props.submitLabel }}
        </button>
      </div>
    </div>
  </form>
  <button
    v-if="props.suggestedAction && !props.modelValue.trim()"
    type="button"
    class="mt-2 text-left text-xs text-muted underline decoration-dotted underline-offset-4"
    @click="emit('update:modelValue', props.suggestedAction)"
  >
    {{ t("game.trySuggestedAction") }}: {{ props.suggestedAction }}
  </button>
  <p v-if="props.tooLarge" class="mt-2 text-xs text-danger" role="alert">
    {{ t("game.actionTooLarge") }}
  </p>
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
    v-else-if="props.preview.command?.name === 'interrupt' && !props.preview.command.argument.trim()"
    class="mt-2 text-xs text-warning"
  >
    {{ t("game.interruptNeedsText") }}
  </p>
</template>
