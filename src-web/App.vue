<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useProduct } from "./composables/useProduct";
import { useGameView } from "./composables/useGameView";
import { useGameInput } from "./composables/useGameInput";
import { useGameControls } from "./composables/useGameControls";
import { useTheme } from "./composables/useTheme";
import { useLocale } from "./composables/useLocale";

const { t } = useI18n();
const product = useProduct();
const theme = useTheme(product.scriptId);
const locale = useLocale();
const sessionId = computed(() => product.session.value?.sessionId);
const game = useGameView(sessionId);
const { phase, turn, records } = game;
const current = computed(() => phase.state.value.snapshot);
const input = useGameInput(sessionId);
const controls = useGameControls(current);
const error = computed(
  () =>
    product.error.value ??
    input.error.value ??
    controls.error.value ??
    current.value?.lastOperation?.error ??
    turn.view.value.snapshot?.error ??
    phase.state.value.error ??
    records.state.value.error ??
    turn.view.value.recoveryError ??
    phase.connectionError.value ??
    records.connectionError.value ??
    turn.connectionError.value,
);
const committed = computed(() => game.latestPublicRecord.value?.body);
const preview = computed(() =>
  turn.view.value.snapshot?.turnId === game.latestPublicRecord.value?.turnId
    ? undefined
    : turn.view.value.snapshot,
);
const selectedScript = computed(() =>
  product.scripts.value.find((script) => script.scriptId === product.scriptId.value),
);
const errorMessage = computed(() => {
  if (!error.value) return undefined;
  switch (error.value.code) {
    case "app.not-ready":
      return t("errors.appNotReady");
    case "app.not-found":
      return t("errors.appNotFound");
    case "app.bad-request":
      return t("errors.badRequest");
    case "store.corrupt":
      return t("errors.storeCorrupt");
    case "store.io":
      return t("errors.storeIo");
    case "app.event-failed":
      return t("errors.appEventFailed");
    case "app.busy":
      return t("errors.appBusy");
    default:
      return t("errors.unknownWithCode", { code: error.value.code });
  }
});
const localeNotice = computed(() => {
  if (locale.notice.value === "pending") return t("settings.languagePending");
  if (locale.notice.value === "saveFailed") return t("settings.languageSaveFailed");
  if (locale.notice.value === "loadFailed") return t("settings.languageLoadFailed");
  if (locale.fallback.value) return t("settings.languageFallback");
  return undefined;
});
const themeErrorMessage = computed(() => {
  switch (theme.themeError.value) {
    case "storageUnavailable":
      return t("settings.themeErrors.storageUnavailable");
    case "invalidPreference":
      return t("settings.themeErrors.invalidPreference");
    case "themeUnavailable":
      return t("settings.themeErrors.themeUnavailable");
    case "saveFailed":
      return t("settings.themeErrors.saveFailed");
    case "readFailed":
      return t("settings.themeErrors.readFailed");
    case "catalogUnavailable":
      return t("settings.themeErrors.catalogUnavailable");
    default:
      return undefined;
  }
});
const skinErrorMessage = computed(() => {
  switch (theme.skinError.value) {
    case "invalidValue":
      return t("settings.skinErrors.invalidValue");
    case "unsupportedWebView":
      return t("settings.skinErrors.unsupportedWebView");
    case "applyFailed":
      return t("settings.skinErrors.applyFailed");
    case "readFailed":
      return t("settings.skinErrors.readFailed");
    default:
      return undefined;
  }
});
function phaseName(phase: string | undefined): string {
  switch (phase) {
    case "idle":
      return t("game.phases.idle");
    case "generating":
      return t("game.phases.generating");
    case "awaitingCheck":
      return t("game.phases.awaitingCheck");
    case "settling":
      return t("game.phases.settling");
    case "advancing":
      return t("game.phases.advancing");
    default:
      return t("common.unknownError");
  }
}
function outcomeName(outcome: string): string {
  switch (outcome) {
    case "completed":
      return t("game.outcomes.completed");
    case "cancelled":
      return t("game.outcomes.cancelled");
    case "failed":
      return t("game.outcomes.failed");
    default:
      return outcome;
  }
}
function finishReasonName(reason: string): string {
  switch (reason) {
    case "stop":
      return t("game.finishReasons.stop");
    case "guard":
      return t("game.finishReasons.guard");
    case "length":
      return t("game.finishReasons.length");
    default:
      return reason;
  }
}
function recordKindName(kind: string): string {
  switch (kind) {
    case "narration":
      return t("game.recordKinds.narration");
    case "character":
      return t("game.recordKinds.character");
    case "system":
      return t("game.recordKinds.system");
    case "check":
      return t("game.recordKinds.check");
    case "header":
      return t("game.recordKinds.header");
    default:
      return kind;
  }
}
void product.load();
</script>

<template>
  <main class="mx-auto grid min-h-screen max-w-4xl gap-6 p-6 text-ink">
    <section class="glass-panel">
      <h1 class="text-2xl font-semibold">{{ t("common.appName") }}</h1>
      <p class="text-sm text-muted">{{ t("game.intro") }}</p>
      <div class="flex gap-3">
        <label>
          {{ t("settings.theme") }}
          <select
            :value="theme.theme.value"
            :aria-label="t('settings.theme')"
            :disabled="theme.saving.value"
            @change="theme.setTheme(($event.target as HTMLSelectElement).value)"
          >
            <option v-for="option in theme.themes.value" :key="option.id" :value="option.id">
              {{ option.name }}
            </option>
          </select>
        </label>
        <button v-if="product.scriptId.value === 'mistbell'" @click="theme.toggleSkin">
          {{ theme.disabled.value ? t("settings.enableSkin") : t("settings.disableSkin") }}
        </button>
      </div>
      <p v-if="themeErrorMessage" role="status">{{ themeErrorMessage }}</p>
      <p v-if="skinErrorMessage" role="status">{{ skinErrorMessage }}</p>
      <p v-else-if="theme.warningCount.value">
        {{ t("settings.skinWarnings", { count: theme.warningCount.value }) }}
      </p>
      <label>
        {{ t("settings.language") }}
        <select
          :value="locale.choice.value"
          :aria-label="t('settings.language')"
          :disabled="!locale.ready.value || locale.saving.value"
          @change="
            locale.setChoice(
              ($event.target as HTMLSelectElement).value as 'system' | 'zh-Hans' | 'en',
            )
          "
        >
          <option value="system">
            {{
              t("settings.systemLanguageCurrent", {
                locale: t(
                  locale.resolved.value === "zh-Hans" ? "settings.chinese" : "settings.english",
                ),
              })
            }}
          </option>
          <option value="zh-Hans">{{ t("settings.chinese") }}</option>
          <option value="en">{{ t("settings.english") }}</option>
        </select>
      </label>
      <p v-if="localeNotice" role="status">{{ localeNotice }}</p>
      <label>
        {{ t("settings.model") }}
        <select
          v-model="product.model.value"
          :aria-label="t('settings.model')"
          :disabled="product.busy.value"
        >
          <option v-for="model in product.models" :key="model.id" :value="model.id">
            {{ model.label }}
          </option>
        </select></label
      >
      <button :disabled="product.busy.value" @click="product.configureKey">
        {{ t("settings.setApiKey") }}
      </button>
      <p v-if="product.keyStatus.value">
        {{ product.keyStatus.value.set ? t("settings.keySet") : t("settings.keyMissing") }}
      </p>
      <label>
        {{ t("settings.script") }}
        <select v-model="product.scriptId.value" :disabled="product.busy.value">
          <option
            v-for="script in product.scripts.value"
            :key="script.scriptId"
            :value="script.scriptId"
          >
            {{ script.title }}
          </option>
        </select></label
      >
      <div class="flex gap-4">
        <button :disabled="product.busy.value" @click="product.open(false)">
          {{ t("game.restart") }}</button
        ><button :disabled="product.busy.value" @click="product.open(true)">
          {{ t("game.newCycle") }}</button
        ><button @click="game.reconnect">{{ t("common.reconnect") }}</button>
      </div>
      <details v-if="selectedScript">
        <summary>{{ t("settings.credits") }}</summary>
        <pre class="whitespace-pre-wrap text-sm">{{ selectedScript.attributions }}</pre>
      </details>
      <p v-if="errorMessage" role="alert">{{ errorMessage }}</p>
    </section>
    <section v-if="product.session.value" class="glass-panel">
      <h2>{{ product.session.value.title }}</h2>
      <p>
        {{ t("game.phase", { phase: current ? phaseName(current.phase) : t("common.loading") }) }}
      </p>
      <p v-if="current?.resumeRequired">{{ t("game.paused") }}</p>
      <p v-if="game.historyPending.value">{{ t("game.historyPending") }}</p>
      <ol v-else>
        <li v-for="item in records.items.value" :key="item.recordSeq" class="whitespace-pre-wrap">
          <span>{{ t("game.recordKind", { kind: recordKindName(item.kind) }) }}</span
          ><span v-if="item.body && 'text' in item.body">{{ item.body.text }}</span
          ><button v-else-if="item.bodyRef" @click="records.loadBody(item.bodyRef)">
            {{ t("common.readBody") }}</button
          ><span v-else>{{ item.body }}</span>
        </li>
      </ol>
      <pre v-if="preview" class="whitespace-pre-wrap" data-testid="live-text">{{
        preview.text
      }}</pre>
      <pre
        v-if="!game.historyPending.value && records.state.value.body !== undefined"
        class="whitespace-pre-wrap"
        >{{ records.state.value.body }}</pre>
      <p v-if="committed && 'outcome' in committed">
        {{ t("game.generation", { outcome: outcomeName(committed.outcome) }) }}
        <span v-if="'finishReason' in committed && committed.finishReason">{{
          finishReasonName(committed.finishReason)
        }}</span>
      </p>
      <p v-else-if="game.latestPublicRecord.value?.outcome">
        {{ t("game.generation", { outcome: outcomeName(game.latestPublicRecord.value.outcome) }) }}
      </p>
      <p v-else-if="preview?.outcome">
        {{ t("game.generation", { outcome: outcomeName(preview.outcome) }) }}
        {{ finishReasonName(preview.finishReason ?? "") }}
      </p>
      <form class="flex gap-2" @submit.prevent="input.send">
        <textarea
          v-model="input.draft.value"
          :aria-label="t('game.action')"
          class="w-full border border-hairline bg-transparent p-2"
          @compositionstart="input.composing.value = true"
          @compositionend="input.composing.value = false"
        />
        <button
          type="submit"
          :disabled="
            product.busy.value ||
            input.busy.value ||
            !current ||
            current.phase !== 'idle' ||
            current.resumeRequired
          "
        >
          {{ t("game.submitAction") }}
        </button>
      </form>
      <div class="flex gap-4">
        <button
          v-if="current?.inFlight"
          :disabled="controls.busy.value"
          @click="controls.control('cancel')"
        >
          {{ t("game.cancelTurn") }}</button
        ><button
          v-if="current?.resumeRequired"
          :disabled="controls.busy.value"
          @click="controls.control('resume')"
        >
          {{ t("game.resume") }}</button
        ><button
          v-if="current?.check?.status === 'waiting' && current.inFlight?.roundId"
          :disabled="controls.busy.value"
          @click="controls.control('check')"
        >
          {{ t("game.confirmCheck", { expression: current.check.expression }) }}
        </button>
      </div>
    </section>
  </main>
</template>
