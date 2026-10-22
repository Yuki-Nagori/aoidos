<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type {
  MemoryAppConfig,
  MemoryPolicyOverrides,
  MemoryPolicyParameters,
  MemoryScriptConfig,
} from "../../../api/memory";
import {
  getMemoryAppConfig,
  getMemoryScriptConfig,
  saveMemoryAppConfig,
  saveMemoryScriptConfig,
} from "../../../api/memory";

const props = defineProps<{ scriptId: string }>();
const { t } = useI18n();
type NumericField = Exclude<keyof MemoryPolicyParameters, "algorithm" | "rateTableVersion">;
type Algorithm = MemoryPolicyParameters["algorithm"];

const numericFields: { key: NumericField; min: number; max: number }[] = [
  { key: "baseStrength", min: 50_000, max: 1_000_000 },
  { key: "reinforcementGain", min: 0, max: 500_000 },
  { key: "reinforcementCap", min: 0, max: 950_000 },
  { key: "strengthCap", min: 100_000, max: 1_000_000 },
  { key: "halfLifeRounds", min: 10, max: 10_000 },
  { key: "repeatWindowRounds", min: 1, max: 10_000 },
  { key: "repeatLimit", min: 1, max: 10 },
  { key: "revivalCooldownRounds", min: 1, max: 10_000 },
  { key: "activeCapacity", min: 100, max: 10_000 },
  { key: "decayRate", min: 1, max: 1_000_000 },
];
const fieldLabels = computed<Record<NumericField, string>>(() => ({
  baseStrength: t("memorySettings.fields.baseStrength"),
  reinforcementGain: t("memorySettings.fields.reinforcementGain"),
  reinforcementCap: t("memorySettings.fields.reinforcementCap"),
  strengthCap: t("memorySettings.fields.strengthCap"),
  halfLifeRounds: t("memorySettings.fields.halfLifeRounds"),
  repeatWindowRounds: t("memorySettings.fields.repeatWindowRounds"),
  repeatLimit: t("memorySettings.fields.repeatLimit"),
  revivalCooldownRounds: t("memorySettings.fields.revivalCooldownRounds"),
  activeCapacity: t("memorySettings.fields.activeCapacity"),
  decayRate: t("memorySettings.fields.decayRate"),
}));

const appConfig = ref<MemoryAppConfig | null>(null);
const appDraft = ref<MemoryPolicyParameters | null>(null);
const scriptConfig = ref<MemoryScriptConfig | null>(null);
const scriptDraft = ref<MemoryPolicyOverrides>({});
const appBusy = ref(false);
const scriptBusyIds = ref(new Set<string>());
const appNumberDraft = ref<Partial<Record<NumericField, string>>>({});
const scriptNumberDraft = ref<Partial<Record<NumericField, string>>>({});
const appInvalidNumbers = ref(new Set<NumericField>());
const scriptInvalidNumbers = ref(new Set<NumericField>());
const appError = ref(false);
const scriptError = ref(false);
const appSaved = ref(false);
const scriptSaved = ref(false);
let scriptLoadRevision = 0;
const appNumericFields = computed(() =>
  numericFields.filter(({ key }) => {
    if (key === "halfLifeRounds") return appDraft.value?.algorithm === "segmented";
    if (key === "decayRate") return appDraft.value?.algorithm === "exponential";
    return true;
  }),
);
const scriptAlgorithm = computed(
  () => scriptDraft.value.algorithm ?? scriptConfig.value?.inherited.algorithm ?? "none",
);
const scriptPreview = computed<MemoryPolicyParameters | null>(() =>
  scriptConfig.value ? { ...scriptConfig.value.inherited, ...scriptDraft.value } : null,
);
const scriptNumericFields = computed(() =>
  numericFields.filter(({ key }) => {
    if (key === "halfLifeRounds") return scriptAlgorithm.value === "segmented";
    if (key === "decayRate") return scriptAlgorithm.value === "exponential";
    return true;
  }),
);
const appHasInvalidNumber = computed(() =>
  appNumericFields.value.some(({ key }) => appInvalidNumbers.value.has(key)),
);
const scriptHasInvalidNumber = computed(() =>
  scriptNumericFields.value.some(({ key }) => scriptInvalidNumbers.value.has(key)),
);

function updateInvalidNumbers(
  invalid: { value: Set<NumericField> },
  activeFields: { key: NumericField }[],
  key: NumericField,
  empty: boolean,
): void {
  const next = new Set(invalid.value);
  if (empty) next.add(key);
  else next.delete(key);
  const active = new Set(activeFields.map((field) => field.key));
  for (const field of next) if (!active.has(field)) next.delete(field);
  invalid.value = next;
}

function updateAppNumber(key: NumericField, event: Event): void {
  const raw = (event.target as HTMLInputElement).value;
  appNumberDraft.value[key] = raw;
  const value = Number(raw);
  const field = numericFields.find((item) => item.key === key)!;
  const invalid = raw.trim() === "" || !Number.isSafeInteger(value) || value < field.min || value > field.max;
  updateInvalidNumbers(appInvalidNumbers, appNumericFields.value, key, invalid);
  if (!invalid && appDraft.value) appDraft.value[key] = value;
}

function updateScriptNumber(key: NumericField, event: Event): void {
  const raw = (event.target as HTMLInputElement).value;
  scriptNumberDraft.value[key] = raw;
  const value = Number(raw);
  const field = numericFields.find((item) => item.key === key)!;
  const invalid = raw.trim() === "" || !Number.isSafeInteger(value) || value < field.min || value > field.max;
  updateInvalidNumbers(scriptInvalidNumbers, scriptNumericFields.value, key, invalid);
  if (!invalid) scriptDraft.value[key] = value;
}

function updateAppAlgorithm(value: string): void {
  if (appDraft.value) appDraft.value.algorithm = value as Algorithm;
  updateInvalidNumbers(appInvalidNumbers, appNumericFields.value, "halfLifeRounds", false);
}

function updateScriptAlgorithm(value: string): void {
  scriptDraft.value.algorithm = value as Algorithm;
  updateInvalidNumbers(scriptInvalidNumbers, scriptNumericFields.value, "halfLifeRounds", false);
}

function restoreAppDefaults(): void {
  if (!appConfig.value || appBusy.value) return;
  appDraft.value = { ...appConfig.value.defaultParameters };
  appNumberDraft.value = {};
  appInvalidNumbers.value = new Set();
  appSaved.value = false;
}

function restoreScriptDefaults(): void {
  if (!scriptConfig.value || scriptBusyIds.value.has(scriptConfig.value.scriptId)) return;
  scriptDraft.value = {};
  scriptNumberDraft.value = {};
  scriptInvalidNumbers.value = new Set();
  scriptSaved.value = false;
}

function toggleOverride(key: NumericField | "algorithm", event: Event): void {
  const enabled = (event.target as HTMLInputElement).checked;
  if (enabled) {
    scriptDraft.value[key] = scriptPreview.value?.[key] as never;
  } else {
    delete scriptDraft.value[key];
  }
  if (key !== "algorithm") {
    delete scriptNumberDraft.value[key];
    updateInvalidNumbers(scriptInvalidNumbers, scriptNumericFields.value, key, false);
  }
}

async function loadApp(preserveDraft = false): Promise<void> {
  appError.value = false;
  try {
    const current = await getMemoryAppConfig();
    appConfig.value = current;
    if (!preserveDraft || !appDraft.value) {
      appDraft.value = { ...current.parameters };
      appNumberDraft.value = {};
      appInvalidNumbers.value = new Set();
    }
  } catch {
    appError.value = true;
  }
}

async function loadScript(
  scriptId: string,
  revision: number,
  preserveDraft = false,
): Promise<void> {
  const pendingOverrides = preserveDraft ? { ...scriptDraft.value } : null;
  scriptError.value = false;
  scriptSaved.value = false;
  if (!scriptId) {
    scriptConfig.value = null;
    scriptDraft.value = {};
    scriptInvalidNumbers.value = new Set();
    return;
  }
  scriptConfig.value = null;
  if (!preserveDraft) scriptDraft.value = {};
  try {
    const current = await getMemoryScriptConfig(scriptId);
    if (revision !== scriptLoadRevision || scriptId !== props.scriptId) return;
    scriptConfig.value = current;
    scriptDraft.value = pendingOverrides ?? { ...current.overrides };
    if (!pendingOverrides) {
      scriptInvalidNumbers.value = new Set();
      scriptNumberDraft.value = {};
    }
  } catch {
    if (revision === scriptLoadRevision) {
      if (pendingOverrides) scriptDraft.value = pendingOverrides;
      scriptError.value = true;
    }
  }
}

async function saveApp(): Promise<void> {
  if (!appConfig.value || !appDraft.value || appBusy.value || appHasInvalidNumber.value) return;
  const parameters = { ...appDraft.value };
  appBusy.value = true;
  appError.value = false;
  appSaved.value = false;
  try {
    const saved = await saveMemoryAppConfig(appConfig.value.configVersion, parameters);
    appConfig.value = {
      configVersion: saved.configVersion,
      algorithmVersion: appConfig.value.algorithmVersion,
      parameters,
      defaultParameters: appConfig.value.defaultParameters,
    };
    const scriptId = props.scriptId;
    const revision = ++scriptLoadRevision;
    await loadScript(scriptId, revision, true);
    appSaved.value = true;
  } catch {
    appError.value = true;
  } finally {
    appBusy.value = false;
  }
}

async function saveScript(): Promise<void> {
  const current = scriptConfig.value;
  if (!current || scriptBusyIds.value.has(current.scriptId) || scriptHasInvalidNumber.value) return;
  const scriptId = current.scriptId;
  const revision = scriptLoadRevision;
  scriptBusyIds.value = new Set(scriptBusyIds.value).add(scriptId);
  scriptError.value = false;
  scriptSaved.value = false;
  try {
    const saved = await saveMemoryScriptConfig(
      current.scriptId,
      current.configVersion,
      current.appConfigVersion,
      scriptDraft.value,
    );
    if (props.scriptId === scriptId && scriptLoadRevision === revision) {
      const refreshRevision = ++scriptLoadRevision;
      await loadScript(scriptId, refreshRevision);
      if (
        props.scriptId === scriptId &&
        scriptLoadRevision === refreshRevision &&
        scriptConfig.value?.configVersion === saved.configVersion
      ) {
        scriptSaved.value = true;
      }
    }
  } catch {
    if (props.scriptId === scriptId && scriptLoadRevision === revision) scriptError.value = true;
  } finally {
    const busyIds = new Set(scriptBusyIds.value);
    busyIds.delete(scriptId);
    scriptBusyIds.value = busyIds;
  }
}

onMounted(() => void loadApp());
watch(
  () => props.scriptId,
  (scriptId) => {
    scriptLoadRevision += 1;
    void loadScript(scriptId, scriptLoadRevision);
  },
  { immediate: true },
);
</script>

<template>
  <div class="memory-settings-groups">
    <p class="note">{{ t("memorySettings.explanation") }}</p>
    <section class="memory-settings-group">
      <div class="memory-settings-heading">
        <h4>{{ t("memorySettings.appDefaults") }}</h4>
        <span v-if="appConfig" class="tag mono">v{{ appConfig.configVersion }}</span>
      </div>
      <p v-if="appError" role="alert">
        {{ t("memorySettings.error") }}
        <button type="button" class="btn ghost btn-compact" @click="loadApp(true)">
          <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M20 12a8 8 0 1 1-2.4-5.7" />
            <path d="M20 4v4.5h-4.5" />
          </svg>
          {{ t("common.retry") }}
        </button>
      </p>
      <form v-if="appDraft" class="memory-settings-form" @submit.prevent="saveApp">
        <fieldset class="settings-choice-group">
          <legend>{{ t("memorySettings.algorithm") }}</legend>
          <div class="settings-segmented">
            <label
              v-for="option in [
                { value: 'none', label: t('memorySettings.algorithms.none') },
                { value: 'exponential', label: t('memorySettings.algorithms.exponential') },
                { value: 'segmented', label: t('memorySettings.algorithms.segmented') },
              ]"
              :key="option.value"
              :class="{
                'is-selected': appDraft.algorithm === option.value,
                'is-disabled': appBusy,
              }"
            >
              <input
                type="radio"
                name="memory-app-algorithm"
                :value="option.value"
                :checked="appDraft.algorithm === option.value"
                :disabled="appBusy"
                @change="updateAppAlgorithm(option.value)"
              />
              <span>{{ option.label }}</span>
            </label>
          </div>
        </fieldset>
        <p class="note">{{ t("memorySettings.algorithmHint") }}</p>
        <div class="memory-parameter-grid">
          <label v-for="field in appNumericFields" :key="field.key" class="memory-number-field">
            <span>{{ fieldLabels[field.key] }}</span>
            <input
              type="number"
              class="memory-number-input"
              step="1"
              :min="field.min"
              :max="field.max"
              required
              :value="appNumberDraft[field.key] ?? appDraft[field.key]"
              :disabled="appBusy"
              @input="updateAppNumber(field.key, $event)"
            />
          </label>
        </div>
        <p v-if="appHasInvalidNumber" role="alert" class="text-sm">
          {{ t("memorySettings.invalidNumber") }}
        </p>
        <div class="memory-settings-actions">
          <button type="button" class="btn" :disabled="appBusy" @click="restoreAppDefaults">
            {{ t("memorySettings.restoreAppDefaults") }}
          </button>
          <button
            type="submit"
            class="btn primary"
            :disabled="appBusy || !appConfig || appHasInvalidNumber"
          >
            {{ appBusy ? t("memorySettings.saving") : t("memorySettings.save") }}
          </button>
        </div>
        <p v-if="appSaved" role="status" class="memory-settings-saved">
          {{ t("memorySettings.saved") }}
        </p>
      </form>
      <p v-if="!appError && !appDraft" role="status">{{ t("common.loading") }}</p>
    </section>

    <section class="memory-settings-group">
      <div class="memory-settings-heading">
        <h4>{{ t("memorySettings.scriptOverrides", { script: scriptId }) }}</h4>
      </div>
      <p v-if="scriptError" role="alert">
        {{ t("memorySettings.error") }}
        <button
          type="button"
          class="btn ghost btn-compact"
          @click="loadScript(scriptId, ++scriptLoadRevision, true)"
        >
          <svg class="i sm" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M20 12a8 8 0 1 1-2.4-5.7" />
            <path d="M20 4v4.5h-4.5" />
          </svg>
          {{ t("common.retry") }}
        </button>
      </p>
      <form v-if="scriptConfig" class="memory-settings-form" @submit.prevent="saveScript">
        <p class="note">
          {{ t("memorySettings.inherited", { version: scriptConfig.appConfigVersion }) }}
        </p>
        <fieldset class="settings-choice-group">
          <legend>
            <label class="memory-override-label">
              <input
                type="checkbox"
                :checked="scriptDraft.algorithm !== undefined"
                :disabled="scriptBusyIds.has(scriptConfig.scriptId)"
                @change="toggleOverride('algorithm', $event)"
              />
              {{ t("memorySettings.fields.algorithm") }}
            </label>
          </legend>
          <div class="settings-segmented">
            <label
              v-for="option in [
                { value: 'none', label: t('memorySettings.algorithms.none') },
                { value: 'exponential', label: t('memorySettings.algorithms.exponential') },
                { value: 'segmented', label: t('memorySettings.algorithms.segmented') },
              ]"
              :key="option.value"
              :class="{
                'is-selected': (scriptDraft.algorithm ?? scriptConfig.inherited.algorithm) === option.value,
                'is-disabled': scriptBusyIds.has(scriptConfig.scriptId) || scriptDraft.algorithm === undefined,
              }"
            >
              <input
                type="radio"
                :name="`memory-script-algorithm-${scriptConfig.scriptId}`"
                :value="option.value"
                :checked="(scriptDraft.algorithm ?? scriptConfig.inherited.algorithm) === option.value"
                :disabled="scriptBusyIds.has(scriptConfig.scriptId) || scriptDraft.algorithm === undefined"
                @change="updateScriptAlgorithm(option.value)"
              />
              <span>{{ option.label }}</span>
            </label>
          </div>
        </fieldset>
        <p class="note">{{ t("memorySettings.algorithmHint") }}</p>
        <div class="memory-parameter-grid">
          <label
            v-for="field in scriptNumericFields"
            :key="field.key"
            class="memory-number-field"
          >
            <span class="memory-override-label">
              <input
                type="checkbox"
                :checked="scriptDraft[field.key] !== undefined"
                :disabled="scriptBusyIds.has(scriptConfig.scriptId)"
                @change="toggleOverride(field.key, $event)"
              />
              {{ fieldLabels[field.key] }}
            </span>
            <input
              type="number"
              class="memory-number-input"
              step="1"
              :min="field.min"
              :max="field.max"
              required
              :value="scriptNumberDraft[field.key] ?? scriptDraft[field.key] ?? scriptConfig.inherited[field.key]"
              :disabled="scriptBusyIds.has(scriptConfig.scriptId) || scriptDraft[field.key] === undefined"
              @input="updateScriptNumber(field.key, $event)"
            />
          </label>
        </div>
        <p v-if="scriptHasInvalidNumber" role="alert" class="text-sm">
          {{ t("memorySettings.invalidNumber") }}
        </p>
        <div class="memory-settings-actions">
          <p v-if="scriptSaved" role="status" class="memory-settings-saved">
            {{ t("memorySettings.saved") }}
          </p>
          <button
            type="button"
            class="btn"
            :disabled="scriptBusyIds.has(scriptConfig.scriptId)"
            @click="restoreScriptDefaults"
          >
            {{ t("memorySettings.restoreScriptDefaults") }}
          </button>
          <button
            type="submit"
            class="btn primary"
            :disabled="scriptBusyIds.has(scriptConfig.scriptId) || scriptHasInvalidNumber"
          >
            {{
              scriptBusyIds.has(scriptConfig.scriptId) ? t("memorySettings.saving") : t("memorySettings.save")
            }}
          </button>
        </div>
      </form>
      <p v-if="!scriptError && !scriptConfig" role="status">{{ t("common.loading") }}</p>
    </section>
  </div>
</template>
