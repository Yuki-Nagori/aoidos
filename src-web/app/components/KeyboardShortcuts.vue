<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { shortcutGroups } from "./shortcut-presentation";

const { t } = useI18n();
const query = ref("");
const closeButton = ref<HTMLButtonElement>();
const viewButton = ref<HTMLButtonElement>();
const dialogOpen = ref(false);
const isMac = /Macintosh|Mac OS X|iPhone|iPad/.test(navigator.userAgent);
const groups = computed(() =>
  shortcutGroups.map((group) => ({ ...group, label: t(group.labelKey) })),
);
const dialogGroups = computed(() => {
  const needle = query.value.trim().toLocaleLowerCase();
  return groups.value
    .map((group) => ({
      ...group,
      rows: group.rows.filter((row) =>
        !needle ||
        [t(row.labelKey), t(row.noteKey), ...row.keys.map(displayKey)]
          .join(" ")
          .toLocaleLowerCase()
          .includes(needle),
      ),
    }))
    .filter((group) => group.rows.length > 0);
});
function displayKey(key: string): string {
  const names: Record<string, string> = {
    ArrowUp: "↑",
    ArrowDown: "↓",
    Escape: "Esc",
    Enter: isMac ? "↩" : "Enter",
    Tab: isMac ? "⇥" : "Tab",
    Mod: isMac ? "⌘" : "Ctrl",
    Shift: isMac ? "⇧" : "Shift",
    Alt: isMac ? "⌥" : "Alt",
  };
  return key
    .split("+")
    .map((part) => names[part] ?? part)
    .join(isMac ? "" : "+");
}

async function close(): Promise<void> {
  dialogOpen.value = false;
  await nextTick();
  viewButton.value?.focus();
}

function onDialogKeydown(event: KeyboardEvent): void {
  event.stopPropagation();
  if (event.key === "Escape" && !isComposingEvent(event)) {
    event.preventDefault();
    close();
    return;
  }
  if (event.key === "Tab") {
    const focusable = Array.from(
      (event.currentTarget as HTMLElement).querySelectorAll<HTMLElement>(
        'button:not(:disabled), input:not(:disabled), [href], [tabindex]:not([tabindex="-1"])',
      ),
    );
    const first = focusable[0];
    const last = focusable.at(-1);
    if (event.shiftKey && document.activeElement === first && last) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last && first) {
      event.preventDefault();
      first.focus();
    }
  }
}

function isComposingEvent(event: KeyboardEvent): boolean {
  return event.isComposing || event.key === "Process";
}

watch(dialogOpen, async (isOpen) => {
  if (!isOpen) return;
  await nextTick();
  closeButton.value?.focus();
});
</script>

<template>
  <div class="shortcut-settings">
    <div class="shortcut-settings-intro card">
      <div>
        <strong>{{ t("shortcuts.title") }}</strong>
        <p class="text-sm text-muted">{{ t("shortcuts.notConnected") }}</p>
      </div>
      <button
        ref="viewButton"
        type="button"
        class="btn"
        @click="query = ''; dialogOpen = true"
      >
        <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
          <rect x="3" y="6" width="18" height="12" rx="2.5" />
          <path d="M7 10h.01M10.5 10h.01M14 10h.01M17.5 10h.01M8 14h8" />
        </svg>
        {{ t("shortcuts.viewSheet") }}
        <kbd class="kbd dim" aria-hidden="true">?</kbd>
      </button>
      <button type="button" class="btn" disabled>
        <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
          <path d="M4 12a8 8 0 1 0 2.4-5.7M4 4v4.5h4.5" />
        </svg>
        {{ t("shortcuts.reset") }}
      </button>
    </div>
    <div class="shortcut-groups shortcut-groups-settings">
      <section v-for="group in groups" :key="group.id" class="shortcut-group">
        <h3>{{ group.label }}</h3>
        <div
          v-for="row in group.rows"
          :key="row.id"
          class="shortcut-row shortcut-row-settings"
        >
          <div class="shortcut-row-copy">
            <strong>{{ t(row.labelKey) }}</strong>
            <span>{{ t(row.noteKey) }}</span>
          </div>
          <div class="shortcut-row-keys">
            <template v-for="(key, index) in row.keys" :key="key">
              <span v-if="index" class="shortcut-separator">/</span>
              <kbd class="kbd">{{ displayKey(key) }}</kbd>
            </template>
            <span v-if="row.keys.length === 0" class="tag">{{ t("shortcuts.unbound") }}</span>
          </div>
          <button v-if="!row.locked" type="button" class="btn ghost" disabled>
            {{ t("shortcuts.change") }}
          </button>
          <span
            v-else
            class="shortcut-lock"
            :aria-label="t('shortcuts.locked')"
            :title="t('shortcuts.locked')"
          >
            <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
              <rect x="5" y="11" width="14" height="9" rx="2" />
              <path d="M8 11V8a4 4 0 0 1 8 0v3" />
            </svg>
          </span>
        </div>
      </section>
    </div>
    <p class="shortcut-note text-sm text-muted">{{ t("shortcuts.settingsNote") }}</p>
  </div>

  <Teleport to="body">
    <div v-if="dialogOpen" class="shortcut-scrim">
      <section
        class="shortcut-dialog glass"
        role="dialog"
        aria-modal="true"
        :aria-label="t('shortcuts.title')"
        @keydown="onDialogKeydown"
      >
        <header class="shortcut-dialog-header">
          <div class="shortcut-dialog-title">
            <span class="shortcut-dialog-icon" aria-hidden="true">
              <svg class="i lg" viewBox="0 0 24 24" fill="none" aria-hidden="true">
                <rect x="3" y="6" width="18" height="12" rx="2.5" />
                <path d="M7 10h.01M10.5 10h.01M14 10h.01M17.5 10h.01M8 14h8" />
              </svg>
            </span>
            <strong>{{ t("shortcuts.title") }}</strong>
          </div>
          <button
            ref="closeButton"
            type="button"
            class="btn icon ghost"
            :aria-label="t('common.close')"
            @click="close"
          >
            <svg class="i" viewBox="0 0 24 24" aria-hidden="true">
              <path d="M6 6l12 12M18 6 6 18" />
            </svg>
          </button>
        </header>
        <label class="shortcut-search">
          <span class="sr-only">{{ t("shortcuts.filter") }}</span>
          <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
            <circle cx="11" cy="11" r="6" />
            <path d="m20 20-4.5-4.5" />
          </svg>
          <input
            v-model="query"
            type="search"
            :aria-label="t('shortcuts.filter')"
            :placeholder="t('shortcuts.filter')"
          />
        </label>
        <div class="shortcut-groups shortcut-groups-dialog">
          <section v-for="group in dialogGroups" :key="group.id" class="shortcut-group">
            <h3>{{ group.label }}</h3>
            <div v-for="row in group.rows" :key="row.id" class="shortcut-row">
              <div class="shortcut-row-copy">
                <strong>{{ t(row.labelKey) }}</strong>
                <span>{{ t(row.noteKey) }}</span>
              </div>
              <div class="shortcut-row-keys">
                <template v-for="(key, index) in row.keys" :key="key">
                  <span v-if="index" class="shortcut-separator">/</span>
                  <kbd class="kbd">{{ displayKey(key) }}</kbd>
                </template>
                <span v-if="row.keys.length === 0" class="tag">{{ t("shortcuts.unbound") }}</span>
              </div>
              <span class="tag" :class="{ violet: row.status === 'adopted' }">
                {{ t(row.statusKey) }}
              </span>
            </div>
          </section>
        </div>
        <footer class="shortcut-dialog-footer">
          <span class="text-sm text-muted">{{ t("shortcuts.imeNote") }}</span>
          <button type="button" class="btn" @click="close">{{ t("common.close") }}</button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>
