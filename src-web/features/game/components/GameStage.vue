<script setup lang="ts">
import { nextTick, onMounted, onScopeDispose, ref } from "vue";
import type { PanelState } from "../utils/ui-shell";

const props = defineProps<{
  panelState: PanelState;
  panelOpen: boolean;
  modalOpen: boolean;
  title: string;
  sceneLabel: string;
  triggerLabel: string;
  pinLabel: string;
  unpinLabel: string;
  closeLabel: string;
}>();
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
    :aria-label="props.title"
    tabindex="-1"
  >
    <header class="stage-header">
      <h1 class="truncate text-sm font-semibold tracking-wide">{{ props.title }}</h1>
      <span class="stage-live-indicator" aria-hidden="true" />
      <slot name="status" />
    </header>
    <div class="stage-scene" :aria-label="props.sceneLabel">
      <slot name="scene" />
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
