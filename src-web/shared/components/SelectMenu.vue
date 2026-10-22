<script setup lang="ts" generic="T extends string">
import { computed, nextTick, onMounted, onScopeDispose, ref, useId, watch } from "vue";

interface SelectMenuOption<T extends string = string> {
  value: T;
  label: string;
  description?: string;
  disabled?: boolean;
  marker?: "avatar" | "book";
}

const props = withDefaults(
  defineProps<{
    modelValue: T;
    options: readonly SelectMenuOption<T>[];
    label: string;
    disabled?: boolean;
    placement?: "bottom" | "top";
    variant?: "standard" | "recipient";
    menuTitle?: string;
    footerNote?: string;
  }>(),
  { disabled: false, placement: "bottom", variant: "standard" },
);
const emit = defineEmits<{ "update:modelValue": [value: T] }>();
const root = ref<HTMLElement>();
const trigger = ref<HTMLButtonElement>();
const listbox = ref<HTMLElement>();
const open = ref(false);
const activeIndex = ref(0);
const id = `select-menu-${useId().replaceAll(":", "")}`;
const activeOptionId = computed(() => `${id}-option-${activeIndex.value}`);
const selected = computed(() => props.options.find((option) => option.value === props.modelValue));
const enabledIndices = computed(() =>
  props.options.flatMap((option, index) => (option.disabled ? [] : [index])),
);

watch([open, activeIndex], async ([isOpen]) => {
  if (!isOpen) return;
  await nextTick();
  listbox.value?.querySelector<HTMLElement>('[role="option"].is-active')?.scrollIntoView?.({
    block: "nearest",
  });
});

function nearestEnabled(index: number, direction: 1 | -1): number {
  if (enabledIndices.value.length === 0) return -1;
  for (let offset = 0; offset < props.options.length; offset += 1) {
    const candidate = (index + direction * offset + props.options.length) % props.options.length;
    if (!props.options[candidate]?.disabled) return candidate;
  }
  return -1;
}

function show(): void {
  if (props.disabled || enabledIndices.value.length === 0) return;
  const selectedIndex = props.options.findIndex((option) => option.value === props.modelValue);
  activeIndex.value = nearestEnabled(selectedIndex >= 0 ? selectedIndex : 0, 1);
  open.value = true;
}

function hide(restoreFocus = false): void {
  open.value = false;
  if (restoreFocus) trigger.value?.focus();
}

function choose(option: SelectMenuOption<T>): void {
  if (option.disabled) return;
  emit("update:modelValue", option.value);
  hide(true);
}

function moveActive(direction: 1 | -1): void {
  if (!open.value) {
    show();
    return;
  }
  activeIndex.value = nearestEnabled(activeIndex.value + direction, direction);
}

watch(
  () => props.options,
  () => {
    if (!open.value) return;
    if (activeIndex.value < props.options.length && !props.options[activeIndex.value]?.disabled)
      return;
    activeIndex.value = nearestEnabled(Math.min(activeIndex.value, props.options.length - 1), 1);
    if (activeIndex.value < 0) hide();
  },
);

function onKeydown(event: KeyboardEvent): void {
  if (event.isComposing || event.key === "Process") return;

  switch (event.key) {
    case "ArrowDown":
      event.preventDefault();
      event.stopPropagation();
      moveActive(1);
      break;
    case "ArrowUp":
      event.preventDefault();
      event.stopPropagation();
      moveActive(-1);
      break;
    case "Home":
      if (!open.value) return;
      event.preventDefault();
      event.stopPropagation();
      activeIndex.value = nearestEnabled(0, 1);
      break;
    case "End":
      if (!open.value) return;
      event.preventDefault();
      activeIndex.value = nearestEnabled(props.options.length - 1, -1);
      break;
    case "Enter":
    case " ":
      event.preventDefault();
      event.stopPropagation();
      if (!open.value) show();
      else {
        const option = props.options[activeIndex.value];
        if (option) choose(option);
      }
      break;
    case "Escape":
      if (!open.value) return;
      event.preventDefault();
      event.stopPropagation();
      hide(true);
      break;
    case "Tab":
      hide();
      break;
  }
}

function onDocumentPointerDown(event: PointerEvent): void {
  if (root.value && !root.value.contains(event.target as Node)) hide();
}

onMounted(() => document.addEventListener("pointerdown", onDocumentPointerDown));
onScopeDispose(() => document.removeEventListener("pointerdown", onDocumentPointerDown));
</script>

<template>
  <span
    ref="root"
    class="select-menu"
    :class="[`select-menu-${props.variant}`, { 'is-open': open }]"
  >
    <button
      :id="id"
      ref="trigger"
      type="button"
      role="combobox"
      class="select-menu-trigger"
      :class="{ recip: props.variant === 'recipient', open: open && props.variant === 'recipient' }"
      :aria-label="props.label"
      :aria-expanded="open"
      :aria-controls="open ? `${id}-listbox` : undefined"
      :aria-activedescendant="open ? activeOptionId : undefined"
      aria-haspopup="listbox"
      :disabled="props.disabled"
      @click="open ? hide() : show()"
      @keydown="onKeydown"
    >
      <slot name="trigger">
        <template v-if="props.variant === 'recipient'">
          <span class="muted">{{ props.menuTitle }}</span>
          <b class="select-menu-value">{{ selected?.label ?? props.modelValue }}</b>
          <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
            <path d="m6 9 6 6 6-6" />
          </svg>
        </template>
        <template v-else>
          <span class="select-menu-value">{{ selected?.label ?? props.modelValue }}</span>
          <svg class="select-menu-chevron" viewBox="0 0 16 16" aria-hidden="true">
            <path d="m4 6 4 4 4-4" />
          </svg>
        </template>
      </slot>
    </button>
    <div
      v-if="open"
      ref="listbox"
      :id="`${id}-listbox`"
      class="select-menu-listbox glass"
      :class="[
        `placement-${props.placement}`,
        { 'cmd-menu': props.variant === 'recipient' },
      ]"
      role="listbox"
      :aria-label="props.label"
    >
      <div v-if="props.variant === 'recipient'" class="select-menu-menu-header">
        <span>{{ props.menuTitle }}</span>
        <span class="select-menu-key-hints" aria-hidden="true">
          <kbd class="kbd">↑</kbd>
          <kbd class="kbd">↓</kbd>
          <kbd class="kbd">Enter</kbd>
        </span>
      </div>
      <div
        v-for="(option, index) in props.options"
        :id="`${id}-option-${index}`"
        :key="option.value"
        class="select-menu-option"
        :class="{
          cmd: props.variant === 'recipient',
          'is-active': activeIndex === index,
          'is-selected': props.modelValue === option.value,
          'is-disabled': option.disabled,
          sel: props.variant === 'recipient' && props.modelValue === option.value,
          off: props.variant === 'recipient' && option.disabled,
        }"
        role="option"
        tabindex="-1"
        :aria-selected="props.modelValue === option.value"
        :aria-disabled="option.disabled || undefined"
        @mouseenter="!option.disabled && (activeIndex = index)"
        @mousedown.prevent
        @click.stop="choose(option)"
        @keydown.enter.stop.prevent="choose(option)"
        @keydown.space.stop.prevent="choose(option)"
      >
        <span v-if="option.marker === 'avatar'" class="select-menu-marker select-menu-avatar" />
        <span
          v-else-if="option.marker === 'book'"
          class="select-menu-marker select-menu-book"
          aria-hidden="true"
        >
          <svg class="i sm" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path d="M5 5a2 2 0 0 1 2-2h12v16H7a2 2 0 0 0-2 2zM5 19V5" />
          </svg>
        </span>
        <span class="select-menu-option-copy">
          <span class="select-menu-option-label">{{ option.label }}</span>
          <span v-if="option.description" class="select-menu-option-description">
            {{ option.description }}
          </span>
        </span>
        <svg
          v-if="props.modelValue === option.value"
          class="select-menu-check"
          viewBox="0 0 16 16"
          aria-hidden="true"
        >
          <path d="m3 8 3.2 3.2L13 4.8" />
        </svg>
      </div>
      <p v-if="props.footerNote" class="select-menu-footer-note">{{ props.footerNote }}</p>
    </div>
  </span>
</template>
