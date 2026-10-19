<script setup lang="ts">
import { computed, nextTick, onMounted, onScopeDispose, ref } from "vue";
import type { PanelState } from "../utils/ui-shell";
import mistbellStageArt from "../assets/mistbell-stage.svg";

const props = defineProps<{
  panelState: PanelState;
  panelOpen: boolean;
  modalOpen: boolean;
  title: string;
  scriptId: string;
  sceneAvailable: boolean;
  sceneLabel: string;
  phase: string;
  phaseLabel: string;
  triggerLabel: string;
  settingsLabel: string;
  pinLabel: string;
  unpinLabel: string;
  closeLabel: string;
}>();
const phaseTone = computed(() =>
  props.phase === "idle" ? "idle" : props.phase === "needsRecovery" ? "warn" : "",
);
const phaseLed = computed(() =>
  props.phase === "needsRecovery" ? "warn" : props.phase === "idle" ? "" : "active",
);
const emit = defineEmits<{
  triggerEnter: [];
  triggerLeave: [];
  panelEnter: [];
  panelLeave: [];
  open: [];
  interact: [];
  escape: [];
  pin: [];
  unpin: [];
  close: [];
  settings: [];
}>();
const trigger = ref<HTMLButtonElement>();
let returningFocus = false;

async function focusTrigger(): Promise<void> {
  returningFocus = true;
  await nextTick();
  trigger.value?.focus();
  await nextTick();
  returningFocus = false;
}
function onTriggerFocus(): void {
  if (!returningFocus) emit("open");
}

function onKeydown(event: KeyboardEvent): void {
  if (
    event.isComposing ||
    event.defaultPrevented ||
    props.modalOpen ||
    isEditingTarget(event.target)
  )
    return;
  if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key.toLowerCase() === "m") {
    event.preventDefault();
    emit("open");
  } else if (event.key === "Escape") {
    const shouldReturnFocus = props.panelState === "expanded";
    emit("escape");
    if (shouldReturnFocus) void focusTrigger();
  }
}
function isEditingTarget(target: EventTarget | null): boolean {
  return (
    target instanceof HTMLElement &&
    (target.isContentEditable ||
      target.matches("input, textarea, select, [contenteditable='true']"))
  );
}
function closePanel(): void {
  emit("close");
  void focusTrigger();
}
onMounted(() => window.addEventListener("keydown", onKeydown));
onScopeDispose(() => window.removeEventListener("keydown", onKeydown));
</script>

<template>
  <section
    class="game-stage"
    :data-panel-state="props.panelState"
  >
    <header class="topbar game-topbar">
      <svg class="logo" viewBox="0 0 24 24" fill="none" aria-hidden="true">
        <circle cx="12" cy="12" r="9" stroke="currentColor" stroke-opacity=".3" />
        <path
          d="M6.5 15.5c2-6 9-9 11-7"
          stroke="var(--accent)"
          stroke-width="1.6"
          stroke-linecap="round"
        />
        <circle cx="17.5" cy="8.5" r="1.6" fill="var(--accent-violet)" />
      </svg>
      <span class="title truncate">{{ props.title }}</span>
      <span class="phase-pill" :class="phaseTone">
        <span class="led" :class="phaseLed" aria-hidden="true" />
        {{ props.phaseLabel }}
      </span>
      <span class="spacer" />
      <slot name="summary" />
      <button
        type="button"
        class="btn icon ghost gear"
        :aria-label="props.settingsLabel"
        @click="emit('settings')"
      >
        <svg class="i" viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="12" cy="12" r="3" />
          <path d="M12 3v2.5M12 18.5V21M3 12h2.5M18.5 12H21M5.6 5.6l1.8 1.8M16.6 16.6l1.8 1.8M5.6 18.4l1.8-1.8M16.6 7.4l1.8-1.8" />
        </svg>
      </button>
      <button
        type="button"
        class="btn icon ghost"
        :aria-label="props.triggerLabel"
        :aria-expanded="props.panelOpen"
        @click="emit('open')"
      >
        <svg class="i" viewBox="0 0 24 24" aria-hidden="true">
          <rect x="4" y="5" width="16" height="14" rx="2.5" />
          <path d="M14 5v14" />
        </svg>
      </button>
    </header>

    <div class="stage-area">
      <section class="stage" :aria-label="props.sceneLabel">
        <img
          v-if="props.sceneAvailable && props.scriptId === 'mistbell'"
          class="stage-art"
          :src="mistbellStageArt"
          alt=""
          aria-hidden="true"
        />
        <div v-else class="stage-art-fallback" aria-hidden="true" />
        <div class="stage-overlay">
          <slot name="scene" />
        </div>
      </section>
    </div>

    <button
      ref="trigger"
      class="panel-edge-trigger"
      :class="{ 'panel-edge-trigger-open': props.panelOpen }"
      type="button"
      :aria-label="props.triggerLabel"
      :aria-hidden="props.panelOpen"
      :tabindex="props.panelOpen ? -1 : 0"
      aria-controls="dialogue-panel"
      :aria-expanded="props.panelOpen"
      @mouseenter="emit('triggerEnter')"
      @mouseleave="emit('triggerLeave')"
      @focus="onTriggerFocus"
      @click="emit('open')"
    >
      <span class="stage-trigger-icon" aria-hidden="true" />
    </button>
    <aside
      id="dialogue-panel"
      class="dialogue-panel"
      aria-labelledby="dialogue-panel-heading"
      :class="{ 'dialogue-panel-open': props.panelOpen }"
      :inert="!props.panelOpen"
      :aria-hidden="!props.panelOpen"
      @mouseenter="emit('panelEnter')"
      @mouseleave="emit('panelLeave')"
      @pointerdown="props.panelState === 'hover' && emit('interact')"
      @focusin="props.panelState === 'hover' && emit('interact')"
    >
      <header class="dialogue-panel-header">
        <slot name="panel-header" />
        <button
          v-if="props.panelState === 'pinned'"
          type="button"
          :aria-label="props.unpinLabel"
          aria-pressed="true"
          @click="emit('unpin')"
        >
          <span class="panel-unpin-icon" aria-hidden="true" />
        </button>
        <button
          v-else
          type="button"
          :aria-label="props.pinLabel"
          aria-pressed="false"
          @click="emit('pin')"
        >
          <span class="panel-pin-icon" aria-hidden="true" />
        </button>
        <button type="button" :aria-label="props.closeLabel" @click="closePanel">
          <span class="panel-close-icon" aria-hidden="true" />
        </button>
      </header>
      <slot name="panel" />
    </aside>
  </section>
</template>
