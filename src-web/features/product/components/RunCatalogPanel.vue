<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import {
  bindRunProfile,
  deleteRun,
  listRuns,
  openSelectedRun,
  reconcileRunBilling,
  scanLegacyRuns,
} from "../../../api/runs";
import type { DeletionOperation, OpenedRun, RunCursor, RunSummary } from "../../../api/runs";
import type { ScriptInfo } from "../../../api/engine";
import type { LlmProfile } from "../../../api/llm";
import { safeError } from "../../../shared/utils/error";

const props = defineProps<{
  open: boolean;
  embedded?: boolean;
  scripts: ScriptInfo[];
  profiles: LlmProfile[];
  locale: "zh-Hans" | "en";
}>();
const emit = defineEmits<{ close: []; restore: [run: OpenedRun] }>();
const { t } = useI18n();
const stateLabels = computed<Record<RunSummary["state"], string>>(() => ({
  ready: t("runs.states.ready"),
  "needs-profile-binding": t("runs.states.needs-profile-binding"),
  "billing-not-configured": t("runs.states.billing-not-configured"),
  "billing-reconciliation-required": t("runs.states.billing-reconciliation-required"),
  "recovery-required": t("runs.states.recovery-required"),
  "needs-repair": t("runs.states.needs-repair"),
  "script-unavailable": t("runs.states.script-unavailable"),
  "settlement-pending": t("runs.states.settlement-pending"),
  deleting: t("runs.states.deleting"),
}));
const rows = ref<RunSummary[]>([]);
const nextCursor = ref<RunCursor | null>(null);
const previousCursors = ref<(RunCursor | undefined)[]>([]);
const cursor = ref<RunCursor>();
const revision = ref(0);
const scanIncomplete = ref(false);
const selectedProfileId = ref("");
const loading = ref(false);
const busyRunId = ref<string>();
const error = ref<string>();
const deleteTarget = ref<RunSummary>();
const deleteAcknowledged = ref(false);
const bindTarget = ref<RunSummary>();
const bindAcknowledged = ref(false);
const attentionOnly = ref(false);
const visibleRows = computed(() => attentionOnly.value ? rows.value.filter((run) => run.state !== "ready") : rows.value);
const deletionStages = ["requested", "owner-closed", "files-quarantined", "database-purged", "files-purged", "completed"] as const;
const deletionStageLabels = computed(() => ({
  requested: t("runs.stages.requested"),
  "owner-closed": t("runs.stages.owner-closed"),
  "files-quarantined": t("runs.stages.files-quarantined"),
  "database-purged": t("runs.stages.database-purged"),
  "files-purged": t("runs.stages.files-purged"),
  completed: t("runs.stages.completed"),
}));
function stageClass(state: DeletionOperation["state"], index: number): string {
  const current = deletionStages.indexOf(state as (typeof deletionStages)[number]);
  return index < current ? "done" : index === current ? "current" : "";
}
function profileSummary(run: RunSummary): string {
  const profile = props.profiles.find((item) => item.profileId === run.profileId);
  return profile ? `${profile.providerId} · ${profile.model}` : run.profileId ?? t("runs.noProfile");
}
function chooseBinding(run: RunSummary): void {
  bindTarget.value = run;
  selectedProfileId.value = "";
  bindAcknowledged.value = false;
}
const deletion = ref<DeletionOperation>();
const reconciliationTarget = ref<RunSummary>();
const reconciliationNeedsLimits = ref(false);
const reconciliationCnyLimit = ref("");
const reconciliationUsdLimit = ref("");
const pendingDeletions = ref(new Map<string, DeletionOperation>());
const pendingDeletionTargets = ref(new Map<string, RunSummary>());
const resumableDeletions = computed(() => [...pendingDeletionTargets.value.values()]);
const pageNumber = computed(() => previousCursors.value.length + 1);
const modalPanel = ref<HTMLElement>();
let previousModalFocus: HTMLElement | undefined;
watch(() => Boolean(deleteTarget.value || bindTarget.value || reconciliationTarget.value), async (open) => {
  if (open) {
    previousModalFocus = document.activeElement as HTMLElement;
    await nextTick();
    modalPanel.value?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  } else {
    previousModalFocus?.focus();
  }
});
function onModalKeydown(event: KeyboardEvent): void {
  if (!modalPanel.value) return;
  if (event.key === "Escape") {
    event.stopPropagation();
    if (busyRunId.value || event.isComposing) return;
    event.preventDefault();
    dismissDeleteConfirmation();
    bindTarget.value = undefined;
    reconciliationTarget.value = undefined;
  }
  if (event.key !== "Tab") return;
  const elements = [...modalPanel.value.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)")];
  const first = elements[0];
  const last = elements.at(-1);
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
}
function errorMessage(failure: unknown): string {
  return safeError(failure, t("common.unknownError")).message;
}

watch(
  () => props.open,
  (open) => {
    if (open) void loadPage();
    else {
      deleteTarget.value = undefined;
      bindTarget.value = undefined;
      reconciliationTarget.value = undefined;
      error.value = undefined;
    }
  },
  { immediate: true },
);

function scriptName(run: RunSummary): string {
  const script = props.scripts.find((item) => item.scriptId === run.scriptId);
  return script?.displayNames[props.locale] ?? script?.title ?? run.scriptId;
}

function stateLabel(state: RunSummary["state"]): string {
  return stateLabels.value[state];
}

async function loadPage(target = cursor.value): Promise<void> {
  loading.value = true;
  error.value = undefined;
  try {
    const page = await listRuns(target, 50);
    rows.value = page.entries;
    for (const run of page.entries) {
      if (run.state === "deleting" && run.deletionOperationId) {
        pendingDeletions.value.set(run.sessionId, {
          operationId: run.deletionOperationId,
          sessionId: run.sessionId,
          state: "requested",
          catalogRevision: run.revision,
          diagnosticCode: null,
        });
        pendingDeletionTargets.value.set(run.sessionId, run);
      } else if (pendingDeletionTargets.value.has(run.sessionId)) {
        pendingDeletions.value.delete(run.sessionId);
        pendingDeletionTargets.value.delete(run.sessionId);
      }
    }
    for (const operation of page.pendingDeletions) {
      const run = page.entries.find((entry) => entry.sessionId === operation.sessionId) ?? {
        sessionId: operation.sessionId,
        scriptId: operation.sessionId,
        createdAt: "",
        memoryRunId: null,
        billingRunId: null,
        profileId: null,
        revision: operation.catalogRevision,
        state: "deleting" as const,
        active: false,
        deletionOperationId: operation.operationId,
      };
      pendingDeletions.value.set(run.sessionId, operation);
      pendingDeletionTargets.value.set(run.sessionId, run);
    }
    cursor.value = target;
    nextCursor.value = page.nextCursor;
    revision.value = page.catalogRevision;
    scanIncomplete.value = page.scanIncomplete;
  } catch (failure) {
    error.value = errorMessage(failure);
  } finally {
    loading.value = false;
  }
}

async function refreshCatalog(): Promise<void> {
  previousCursors.value = [];
  await loadPage(undefined);
}

async function scanLegacy(): Promise<void> {
  loading.value = true;
  error.value = undefined;
  try {
    await scanLegacyRuns();
    await refreshCatalog();
  } catch (failure) {
    error.value = errorMessage(failure);
  } finally {
    loading.value = false;
  }
}

async function restore(run: RunSummary): Promise<void> {
  busyRunId.value = run.sessionId;
  error.value = undefined;
  try {
    const opened = await openSelectedRun(run.sessionId, run.revision);
    emit("restore", opened);
    emit("close");
  } catch (failure) {
    error.value = errorMessage(failure);
  } finally {
    busyRunId.value = undefined;
  }
}

async function bindProfile(run: RunSummary): Promise<void> {
  if (!selectedProfileId.value || !bindAcknowledged.value) return;
  busyRunId.value = run.sessionId;
  error.value = undefined;
  try {
    await bindRunProfile(run.sessionId, run.revision, selectedProfileId.value);
    bindTarget.value = undefined;
    await refreshCatalog();
  } catch (failure) {
    error.value = errorMessage(failure);
  } finally {
    busyRunId.value = undefined;
  }
}

async function reconcileBilling(run: RunSummary): Promise<void> {
  if (
    reconciliationNeedsLimits.value &&
    (!validLimit(reconciliationCnyLimit.value) || !validLimit(reconciliationUsdLimit.value))
  ) {
    error.value = t("runs.invalidLimits");
    return;
  }
  busyRunId.value = run.sessionId;
  error.value = undefined;
  try {
    await reconcileRunBilling(
      run.sessionId,
      run.revision,
      reconciliationNeedsLimits.value
        ? { cnyLimit: reconciliationCnyLimit.value, usdLimit: reconciliationUsdLimit.value }
        : undefined,
    );
    reconciliationTarget.value = undefined;
    reconciliationNeedsLimits.value = false;
    reconciliationCnyLimit.value = "";
    reconciliationUsdLimit.value = "";
    await refreshCatalog();
  } catch (failure) {
    const safe = safeError(failure, t("common.unknownError"));
    if (safe.code === "budget.reconciliation-required") {
      reconciliationTarget.value = run;
      reconciliationNeedsLimits.value = true;
    } else {
      error.value = safe.message;
    }
  } finally {
    busyRunId.value = undefined;
  }
}

function validLimit(value: string): boolean {
  return value.length <= 32 && /^(?:0|[1-9]\d*)(?:\.\d{1,9})?$/.test(value);
}

async function confirmDelete(): Promise<void> {
  const run = deleteTarget.value;
  if (!run || (!deletion.value && !deleteAcknowledged.value)) return;
  busyRunId.value = run.sessionId;
  error.value = undefined;
  const existing = pendingDeletions.value.get(run.sessionId);
  const operationId = existing?.operationId ?? crypto.randomUUID();
  try {
    const result = await deleteRun(
      operationId,
      run.sessionId,
      run.revision,
    );
    if (result.state === "completed") {
      deleteTarget.value = undefined;
      deletion.value = undefined;
      pendingDeletions.value.delete(run.sessionId);
      pendingDeletionTargets.value.delete(run.sessionId);
      await refreshCatalog();
    } else {
      deletion.value = result;
      pendingDeletions.value.set(run.sessionId, result);
      pendingDeletionTargets.value.set(run.sessionId, run);
      await refreshCatalog();
    }
  } catch (failure) {
    error.value = safeError(failure, t("common.unknownError")).message;
    // 保留同一 operation ID，使不确定提交和部分失败都能安全重试。
    const pending: DeletionOperation = existing ?? {
      operationId,
      sessionId: run.sessionId,
      state: "requested",
      catalogRevision: run.revision,
      diagnosticCode: null,
    };
    deletion.value = pending;
    pendingDeletions.value.set(run.sessionId, pending);
    pendingDeletionTargets.value.set(run.sessionId, run);
  } finally {
    busyRunId.value = undefined;
  }
}

function confirmTarget(run: RunSummary): void {
  deleteTarget.value = run;
  deleteAcknowledged.value = false;
  deletion.value = pendingDeletions.value.get(run.sessionId);
}

function dismissDeleteConfirmation(): void {
  deleteTarget.value = undefined;
  deletion.value = undefined;
}

async function nextPage(): Promise<void> {
  if (!nextCursor.value) return;
  previousCursors.value.push(cursor.value);
  await loadPage(nextCursor.value);
}

async function previousPage(): Promise<void> {
  if (previousCursors.value.length === 0) return;
  const previous = previousCursors.value.pop();
  await loadPage(previous);
}
</script>

<template>
  <Teleport to="body" :disabled="embedded">
    <div
      v-if="open"
      :class="embedded ? 'run-catalog-embedded' : 'run-catalog-backdrop'"
    >
      <section
        class="run-catalog-dialog"
        :role="embedded ? 'region' : 'dialog'"
        :aria-modal="embedded ? undefined : true"
        :aria-label="t('runs.title')"
        @keydown="onModalKeydown"
      >
        <header class="run-catalog-header">
          <div>
            <h2>{{ t("runs.title") }}</h2>
            <p>{{ t("runs.description") }}</p>
          </div>
          <button
            v-if="!embedded"
            type="button"
            class="btn icon ghost"
            :aria-label="t('common.close')"
            @click="emit('close')"
          >
            ×
          </button>
        </header>

        <fieldset class="run-catalog-filter">
          <legend class="sr-only">{{ t("runs.title") }}</legend>
          <button
            type="button"
            class="btn ghost"
            :aria-pressed="!attentionOnly"
            @click="attentionOnly = false"
          >
            {{ t("runs.filterAll") }}
          </button>
          <button
            type="button"
            class="btn ghost"
            :aria-pressed="attentionOnly"
            @click="attentionOnly = true"
          >
            {{ t("runs.filterAttention") }}
          </button>
        </fieldset>
        <p v-if="scanIncomplete" class="run-catalog-note" role="status">
          {{ t("runs.scanIncomplete") }}
        </p>
        <p v-if="error" class="run-catalog-error" role="alert">{{ error }}</p>
        <section
          v-if="resumableDeletions.length > 0"
          class="run-catalog-pending"
          aria-live="polite"
        >
          <h3>{{ t("runs.pendingCleanup") }}</h3>
          <article
            v-for="run in resumableDeletions"
            :key="run.sessionId"
            class="run-catalog-pending-row"
          >
            <span>
              {{ scriptName(run) }} · {{ run.createdAt || run.sessionId }}
              <small v-if="pendingDeletions.get(run.sessionId)?.diagnosticCode">
                {{
                  t("runs.lastFailure", { code: pendingDeletions.get(run.sessionId)?.diagnosticCode })
                }}
              </small>
            </span>
            <button type="button" class="btn" @click="confirmTarget(run)">
              {{ t("runs.resumeDeletion") }}
            </button>
          </article>
        </section>
        <p v-if="loading" class="run-catalog-empty" role="status">{{ t("common.loading") }}</p>
        <p v-else-if="rows.length === 0" class="run-catalog-empty">{{ t("runs.empty") }}</p>

        <div v-else class="run-catalog-list">
          <article
            v-for="run in visibleRows"
            :key="run.sessionId"
            class="run-catalog-row"
            :class="{ active: run.active }"
          >
            <div class="run-catalog-copy">
              <div class="run-catalog-title">
                <strong>{{ scriptName(run) }}</strong><span v-if="run.active" class="tag accent">{{
                  t("runs.current")
                }}</span><span class="run-state" :class="run.state">{{
                  stateLabel(run.state)
                }}</span>
              </div>
              <span class="mono">{{ run.createdAt }}</span>
              <span>{{ profileSummary(run) }}</span>
              <p v-if="run.state === 'settlement-pending'" class="run-catalog-warning">
                {{ t("runs.deleteBlocked") }}
              </p>
              <ol
                v-if="pendingDeletions.has(run.sessionId)"
                class="run-deletion-stages"
                :aria-label="t('runs.deletionPending')"
              >
                <li
                  v-for="(stage, index) in deletionStages"
                  :key="stage"
                  :class="stageClass(pendingDeletions.get(run.sessionId)!.state, index)"
                >
                  {{ deletionStageLabels[stage] }}
                </li>
              </ol>
            </div>
            <div class="run-catalog-actions">
              <button
                v-if="run.state === 'needs-profile-binding'"
                type="button"
                class="btn primary"
                :disabled="Boolean(busyRunId)"
                @click="chooseBinding(run)"
              >
                {{ t("runs.bindProfile") }}
              </button>
              <button
                v-else-if="run.state === 'billing-reconciliation-required'"
                type="button"
                class="btn"
                :disabled="Boolean(busyRunId)"
                @click="reconciliationTarget = run; reconciliationNeedsLimits = false; reconcileBilling(run)"
              >
                {{ t("runs.reconcileBilling") }}
              </button>
              <button
                v-else-if="['ready', 'recovery-required', 'billing-not-configured', 'settlement-pending'].includes(run.state)"
                type="button"
                class="btn primary"
                :disabled="Boolean(busyRunId)"
                @click="restore(run)"
              >
                {{ t("runs.restore") }}
              </button>
              <button
                v-if="run.state !== 'deleting'"
                type="button"
                class="btn danger"
                :disabled="Boolean(busyRunId) || run.state === 'settlement-pending'"
                @click="confirmTarget(run)"
              >
                {{ t("runs.delete") }}
              </button>
              <button
                v-else
                type="button"
                class="btn"
                :disabled="Boolean(busyRunId)"
                @click="confirmTarget(run)"
              >
                {{ t("runs.resumeDeletion") }}
              </button>
            </div>
          </article>
        </div>

        <footer class="run-catalog-footer">
          <button type="button" class="btn" :disabled="loading" @click="scanLegacy">
            {{ t("runs.scan") }}
          </button>
          <button type="button" class="btn" :disabled="loading" @click="refreshCatalog">
            {{ t("runs.refresh") }}
          </button>
          <span class="spacer" />
          <button
            type="button"
            class="btn"
            :disabled="loading || previousCursors.length === 0"
            @click="previousPage"
          >
            {{ t("common.previous") }}
          </button>
          <span>{{ pageNumber }}</span>
          <button type="button" class="btn" :disabled="loading || !nextCursor" @click="nextPage">
            {{ t("common.next") }}
          </button>
        </footer>

        <div v-if="deleteTarget" class="run-delete-backdrop">
          <section
            ref="modalPanel"
            class="run-delete-confirm"
            role="alertdialog"
            aria-modal="true"
            :aria-label="t('runs.confirmDeleteTitle')"
          >
            <h3>{{ deletion ? t("runs.deletionPending") : t("runs.confirmDeleteTitle") }}</h3>
            <p><strong>{{ scriptName(deleteTarget) }}</strong> · {{ deleteTarget.createdAt }}</p>
            <div class="run-delete-impact">
              <section>
                <h4>{{ t("runs.deleteRemoves") }}</h4>
                <p>{{ t("runs.deleteImpact") }}</p>
              </section>
              <section>
                <h4>{{ t("runs.deleteKeeps") }}</h4>
                <p>{{ t("runs.keepsDescription") }}</p>
              </section>
            </div>
            <p class="run-catalog-note">{{ t("runs.backupNotice") }}</p>
            <p v-if="deletion" class="mono">
              {{ t("runs.deletionStage", { state: deletion.state }) }}
            </p>
            <ol v-if="deletion" class="run-deletion-stages">
              <li
                v-for="(stage, index) in deletionStages"
                :key="stage"
                :class="stageClass(deletion.state, index)"
              >
                {{ deletionStageLabels[stage] }}
              </li>
            </ol>
            <label v-else class="run-acknowledgement"><input
                v-model="deleteAcknowledged"
                type="checkbox"
              />{{ t("runs.deleteAck") }}</label>
            <div class="run-catalog-actions">
              <button
                type="button"
                class="btn"
                :disabled="Boolean(busyRunId)"
                @click="dismissDeleteConfirmation"
              >
                {{ t("common.cancel") }}
              </button>
              <button
                type="button"
                class="btn danger"
                :disabled="Boolean(busyRunId) || (!deletion && !deleteAcknowledged)"
                @click="confirmDelete"
              >
                {{
                  busyRunId ? t("common.loading") : deletion ? t("runs.continueDelete") : t("runs.confirmDelete")
                }}
              </button>
            </div>
          </section>
        </div>

        <div v-if="bindTarget" class="run-delete-backdrop">
          <section
            ref="modalPanel"
            class="run-delete-confirm"
            role="dialog"
            aria-modal="true"
            :aria-label="t('runs.bindProfile')"
          >
            <h3>{{ t("runs.bindProfile") }}</h3>
            <p>{{ scriptName(bindTarget) }}</p>
            <p>{{ t("runs.bindDescription") }}</p>
            <fieldset class="run-profile-options">
              <legend class="sr-only">{{ t("runs.chooseProfile") }}</legend>
              <label
                v-for="profile in profiles"
                :key="profile.profileId"
                class="run-profile-option"
              ><input
                  v-model="selectedProfileId"
                  type="radio"
                  name="run-profile"
                  :value="profile.profileId"
                /><span>{{ profile.profileId }}<small>{{ profile.providerId }} · {{
                      profile.model
                    }}</small></span></label>
            </fieldset>
            <label class="run-acknowledgement"><input
                v-model="bindAcknowledged"
                type="checkbox"
              />{{ t("runs.bindAck") }}</label>
            <div class="run-catalog-actions">
              <button
                type="button"
                class="btn"
                :disabled="Boolean(busyRunId)"
                @click="bindTarget = undefined"
              >
                {{ t("common.cancel") }}
              </button>
              <button
                type="button"
                class="btn primary"
                :disabled="!selectedProfileId || !bindAcknowledged || Boolean(busyRunId)"
                @click="bindProfile(bindTarget)"
              >
                {{ t("runs.bindProfile") }}
              </button>
            </div>
          </section>
        </div>
        <div v-if="reconciliationTarget" class="run-delete-backdrop">
          <section
            ref="modalPanel"
            class="run-delete-confirm"
            role="dialog"
            aria-modal="true"
            :aria-label="t('runs.reconcileBilling')"
          >
            <h3>{{ t("runs.reconcileBilling") }}</h3>
            <p>{{ scriptName(reconciliationTarget) }} · {{ reconciliationTarget.createdAt }}</p>
            <p>{{ t("runs.reconcileBillingDescription") }}</p>
            <div v-if="reconciliationNeedsLimits" class="run-catalog-limit-fields">
              <label>
                {{ t("runs.cnyLimit") }}
                <input v-model="reconciliationCnyLimit" inputmode="decimal" maxlength="32" />
              </label>
              <label>
                {{ t("runs.usdLimit") }}
                <input v-model="reconciliationUsdLimit" inputmode="decimal" maxlength="32" />
              </label>
            </div>
            <div class="run-catalog-actions">
              <button type="button" class="btn" @click="reconciliationTarget = undefined">
                {{ t("common.cancel") }}
              </button>
              <button
                type="button"
                class="btn primary"
                :disabled="Boolean(busyRunId)"
                @click="reconcileBilling(reconciliationTarget)"
              >
                {{ busyRunId ? t("common.loading") : t("runs.reconcileBilling") }}
              </button>
            </div>
          </section>
        </div>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.run-catalog-embedded .run-catalog-dialog { width: 100%; max-height: none; padding: 0; border: 0; background: transparent; box-shadow: none; overflow: visible; }
.run-catalog-filter, .run-catalog-title { display: flex; flex-wrap: wrap; align-items: center; gap: .5rem; }
.run-catalog-filter { min-width: 0; margin: 1rem 0 0; padding: 0; border: 0; }
.run-catalog-filter [aria-pressed="true"] { color: var(--accent); background: color-mix(in srgb, var(--accent) 12%, transparent); }
.run-catalog-row.active { border-color: var(--accent); box-shadow: inset 3px 0 0 var(--accent); }
.run-state { font-size: .6875rem; border: 1px solid var(--hairline); border-radius: 999px; padding: .125rem .5rem; }
.run-state.ready { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 35%, transparent); }
.run-state.needs-repair { color: var(--danger); }
.run-state.deleting { color: var(--accent-violet); }
.run-state.needs-profile-binding, .run-state.settlement-pending, .run-catalog-warning { color: var(--warning); }
.run-catalog-warning { margin: .25rem 0 0; }
.run-deletion-stages { display: flex; list-style: none; padding: 0; gap: .25rem; margin: .75rem 0; }
.run-deletion-stages li { flex: 1; min-width: 0; font-size: .625rem; color: var(--muted); border-top: 4px solid var(--hairline); padding-top: .25rem; overflow-wrap: anywhere; }
.run-deletion-stages li.done { border-color: var(--accent-violet); }
.run-deletion-stages li.current { border-color: var(--accent); color: var(--ink); }
.run-delete-impact { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 1rem; margin-top: 1rem; }
.run-delete-impact h4 { font-size: .75rem; color: var(--muted); }
.run-delete-impact section:first-child h4 { color: var(--danger); }
.run-acknowledgement { display: flex; align-items: flex-start; gap: .5rem; margin-top: 1rem; font-size: .8125rem; }
.run-acknowledgement input, .run-profile-option input { accent-color: var(--accent); margin-top: .2rem; }
.run-profile-options { min-width: 0; margin: 1rem 0 0; padding: 0; border: 0; display: grid; gap: .5rem; }
.run-profile-option { display: flex; gap: .625rem; align-items: flex-start; border: 1px solid var(--hairline); border-radius: .625rem; padding: .75rem; font-size: .8125rem; }
.run-profile-option:has(input:checked) { border-color: var(--accent); background: color-mix(in srgb, var(--accent) 7%, transparent); }
.run-profile-option small { display: block; color: var(--muted); margin-top: .25rem; }
@media (max-width: 36rem) { .run-delete-impact { grid-template-columns: minmax(0, 1fr); } }
</style>
