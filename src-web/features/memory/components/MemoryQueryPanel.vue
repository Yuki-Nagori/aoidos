<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type {
  MemoryEntry,
  MemoryEntryCursor,
  MemoryEntryDetails,
  MemoryBatchStatus,
  MemoryKnowledgeEvidence,
  MemoryMaterialCursor,
  MemoryQuestion,
  MemoryQuestionCandidate,
  MemoryQuestionCursor,
  MemorySourceRef,
} from "../../../api/memory";
import {
  getMemoryEntry,
  collectMemoryMaterials,
  completeMemoryBatch,
  getActiveMemoryBatch,
  getMemoryBatchStatus,
  listMemoryEntries,
  listMemoryQuestions,
  prepareMemoryBatch,
  resolveMemoryQuestion,
  sendMemoryBatch,
} from "../../../api/memory";

const props = defineProps<{
  runId: string;
  historyRevision: number;
  view?: "all" | "list" | "pending";
}>();
const { t } = useI18n();
const panelView = computed(() => props.view ?? "all");
const classificationLabels = computed(() => ({
  repeat: t("memoryQuery.classifications.repeat"),
  supplement: t("memoryQuery.classifications.supplement"),
  new: t("memoryQuery.classifications.new"),
  correction: t("memoryQuery.classifications.correction"),
  notWorthy: t("memoryQuery.classifications.notWorthy"),
  uncertain: t("memoryQuery.classifications.uncertain"),
}));
const questionKindLabels = computed(() => ({
  entityAssociation: t("memoryQuery.kinds.entityAssociation"),
  memoryClassification: t("memoryQuery.kinds.memoryClassification"),
}));
const evidenceKindLabels = computed<Record<MemoryKnowledgeEvidence["kind"], string>>(() => ({
  observed: t("memoryQuery.evidenceKinds.observed"),
  told: t("memoryQuery.evidenceKinds.told"),
  rumor: t("memoryQuery.evidenceKinds.rumor"),
}));
const entryStateLabels = computed<Record<MemoryEntry["state"], string>>(() => ({
  active: t("memoryQuery.entryStates.active"),
  archived: t("memoryQuery.entryStates.archived"),
  unavailable: t("memoryQuery.entryStates.unavailable"),
}));
const subjectKindLabels = computed<
  Record<MemoryEntryDetails["version"]["subject"]["kind"], string>
>(() => ({
  character: t("memoryQuery.subjectKinds.character"),
  playerPreference: t("memoryQuery.subjectKinds.playerPreference"),
}));
const dimensionLabels = computed<Record<MemoryEntryDetails["version"]["dimension"], string>>(
  () => ({
    story: t("memoryQuery.dimensions.story"),
    relationship: t("memoryQuery.dimensions.relationship"),
    playerPreference: t("memoryQuery.dimensions.playerPreference"),
    foreshadowing: t("memoryQuery.dimensions.foreshadowing"),
  }),
);
const attitudeLabels = computed<Record<MemoryEntryDetails["version"]["attitude"], string>>(
  () => ({
    unexpressed: t("memoryQuery.attitudes.unexpressed"),
    believed: t("memoryQuery.attitudes.believed"),
    doubtful: t("memoryQuery.attitudes.doubtful"),
    denied: t("memoryQuery.attitudes.denied"),
  }),
);
const changeLabels = computed<Record<MemoryEntryDetails["version"]["change"], string>>(() => ({
  new: t("memoryQuery.changes.new"),
  supplement: t("memoryQuery.changes.supplement"),
  correction: t("memoryQuery.changes.correction"),
  restore: t("memoryQuery.changes.restore"),
}));
const batchStateLabels = computed(() => ({
  queued: t("memoryQuery.batchStates.queued"),
  dispatched: t("memoryQuery.batchStates.dispatched"),
  received: t("memoryQuery.batchStates.received"),
  completed: t("memoryQuery.batchStates.completed"),
  rejected: t("memoryQuery.batchStates.rejected"),
  unknown: t("memoryQuery.batchStates.unknown"),
}));
const candidateOutcomeLabels = computed(() => ({
  applied: t("memoryQuery.outcomes.applied"),
  noOp: t("memoryQuery.outcomes.noOp"),
  rejected: t("memoryQuery.outcomes.rejected"),
  pending: t("memoryQuery.outcomes.pending"),
  deferred: t("memoryQuery.outcomes.deferred"),
}));
const candidateReasonLabels = computed<Record<string, string>>(() => ({
  "memory.candidate.review": t("memoryQuery.reasons.review"),
  "memory.candidate.shared_target": t("memoryQuery.reasons.sharedTarget"),
  "memory.candidate.duplicate_anchor": t("memoryQuery.reasons.duplicateAnchor"),
  "memory.operation.rejected": t("memoryQuery.reasons.operationRejected"),
  "policy.capacity_blocked": t("memoryQuery.reasons.capacityBlocked"),
}));
const canPrepareBatch = computed(
  () =>
    !batchStatus.value ||
    batchStatus.value.state === "completed" ||
    batchStatus.value.state === "rejected",
);
const PAGE_SIZE = 32;
const MAX_VISIBLE = 128;
const questions = ref<MemoryQuestion[]>([]);
const entries = ref<MemoryEntry[]>([]);
const questionCursor = ref<MemoryQuestionCursor | null>(null);
const entryCursor = ref<MemoryEntryCursor | null>(null);
const materialCursor = ref<MemoryMaterialCursor | null>(null);
const materialCount = ref<number | null>(null);
const materialBusy = ref(false);
const materialError = ref(false);
const batchId = ref<string | null>(null);
const batchStatus = ref<MemoryBatchStatus | null>(null);
const batchBusy = ref(false);
const batchError = ref(false);
const questionBusy = ref(false);
const entryBusy = ref(false);
const detailBusy = ref(false);
const error = ref(false);
const answering = ref(new Set<string>());
const proposalNames = ref<Record<string, string>>({});
const details = ref<MemoryEntryDetails | null>(null);
let generation = 0;

async function collectMaterials(next = false): Promise<void> {
  const currentRun = props.runId;
  const currentHistoryRevision = props.historyRevision;
  const currentGeneration = generation;
  if (materialBusy.value || !currentRun) return;
  materialBusy.value = true;
  materialError.value = false;
  let cursor = next ? materialCursor.value : null;
  let count = next ? (materialCount.value ?? 0) : 0;
  try {
    for (let pageNumber = 0; pageNumber < 16; pageNumber += 1) {
      const page = await collectMemoryMaterials(currentRun, cursor);
      if (
        currentGeneration !== generation ||
        currentRun !== props.runId ||
        currentHistoryRevision !== props.historyRevision ||
        page.historyRevision !== currentHistoryRevision
      )
        return;
      count += page.eligibleCount;
      cursor = page.nextCursor;
      if (!cursor) break;
    }
    if (currentGeneration !== generation) return;
    materialCount.value = count;
    materialCursor.value = cursor;
  } catch {
    if (currentGeneration === generation) materialError.value = true;
  } finally {
    if (currentGeneration === generation) materialBusy.value = false;
  }
}

async function prepareBatch(): Promise<void> {
  const currentRun = props.runId;
  const currentHistoryRevision = props.historyRevision;
  const currentGeneration = generation;
  if (batchBusy.value || !currentRun) return;
  batchBusy.value = true;
  batchError.value = false;
  let cursor = materialCursor.value;
  let count = cursor ? (materialCount.value ?? 0) : 0;
  try {
    for (let pageNumber = 0; pageNumber < 16; pageNumber += 1) {
      const page = await collectMemoryMaterials(currentRun, cursor);
      if (
        currentGeneration !== generation ||
        currentRun !== props.runId ||
        currentHistoryRevision !== props.historyRevision ||
        page.historyRevision !== currentHistoryRevision
      )
        return;
      count += page.eligibleCount;
      cursor = page.nextCursor;
      if (!cursor) break;
    }
    if (currentGeneration !== generation) return;
    materialCount.value = count;
    materialCursor.value = cursor;
    const prepared = await prepareMemoryBatch(currentRun);
    if (currentGeneration !== generation || currentRun !== props.runId) return;
    batchId.value = prepared.batchId;
    batchStatus.value = await getMemoryBatchStatus(currentRun, prepared.batchId);
  } catch {
    if (currentGeneration === generation) batchError.value = true;
  } finally {
    if (currentGeneration === generation) batchBusy.value = false;
  }
}

async function recoverActiveBatch(): Promise<void> {
  const currentRun = props.runId;
  const currentGeneration = generation;
  if (!currentRun) return;
  try {
    const status = await getActiveMemoryBatch(currentRun);
    if (currentGeneration !== generation || currentRun !== props.runId || !status) return;
    batchId.value = status.batchId;
    batchStatus.value = status;
  } catch {
    if (currentGeneration === generation) batchError.value = true;
  }
}

async function refreshBatch(): Promise<void> {
  const currentRun = props.runId;
  const currentBatch = batchId.value;
  const currentGeneration = generation;
  if (!currentRun || !currentBatch || batchBusy.value) return;
  batchBusy.value = true;
  batchError.value = false;
  try {
    const status = await getMemoryBatchStatus(currentRun, currentBatch);
    if (currentGeneration === generation && currentRun === props.runId)
      batchStatus.value = status;
  } catch {
    if (currentGeneration === generation) batchError.value = true;
  } finally {
    if (currentGeneration === generation) batchBusy.value = false;
  }
}

async function sendBatch(): Promise<void> {
  const currentRun = props.runId;
  const currentBatch = batchId.value;
  const currentGeneration = generation;
  if (!currentRun || !currentBatch || batchBusy.value) return;
  batchBusy.value = true;
  batchError.value = false;
  try {
    const status = await sendMemoryBatch(currentRun, currentBatch);
    if (currentGeneration !== generation || currentRun !== props.runId) return;
    batchStatus.value = status;
    if (status.state === "completed") void loadEntries();
  } catch {
    if (currentGeneration === generation) batchError.value = true;
  } finally {
    if (currentGeneration === generation) batchBusy.value = false;
  }
}

async function completeBatch(): Promise<void> {
  const currentRun = props.runId;
  const currentBatch = batchId.value;
  const currentGeneration = generation;
  if (!currentRun || !currentBatch || batchBusy.value) return;
  batchBusy.value = true;
  batchError.value = false;
  try {
    const status = await completeMemoryBatch(currentRun, currentBatch);
    if (currentGeneration !== generation || currentRun !== props.runId) return;
    batchStatus.value = status;
    if (status.state === "completed") void loadEntries();
  } catch {
    if (currentGeneration === generation) batchError.value = true;
  } finally {
    if (currentGeneration === generation) batchBusy.value = false;
  }
}

function candidateName(candidate: MemoryQuestionCandidate): string {
  if (candidate.kind === "entity") return candidate.displayName ?? candidate.entityId;
  return classificationLabels.value[candidate.classification];
}

function questionName(question: MemoryQuestion): string {
  if (question.draft.mentionedName) return question.draft.mentionedName;
  return questionKindLabels.value[question.draft.identity.kind];
}

function sourceLabel(source: MemorySourceRef): string {
  return t("memoryQuery.sourceRecord", { sequence: source.recordSeq });
}

function evidenceLabel(evidence: MemoryKnowledgeEvidence): string {
  return evidenceKindLabels.value[evidence.kind];
}

async function loadQuestions(next = false): Promise<void> {
  const currentRun = props.runId;
  const currentHistoryRevision = props.historyRevision;
  const currentGeneration = generation;
  if (questionBusy.value || !currentRun) return;
  questionBusy.value = true;
  error.value = false;
  const cursor = next ? questionCursor.value : null;
  try {
    const page = await listMemoryQuestions(currentRun, cursor, PAGE_SIZE);
    if (
      currentGeneration !== generation ||
      currentRun !== props.runId ||
      currentHistoryRevision !== props.historyRevision ||
      page.historyRevision !== currentHistoryRevision
    )
      return;
    questions.value = (next ? [...questions.value, ...page.questions] : page.questions).slice(
      -MAX_VISIBLE,
    );
    questionCursor.value = page.nextCursor;
  } catch {
    if (currentGeneration === generation) error.value = true;
  } finally {
    if (currentGeneration === generation) questionBusy.value = false;
  }
}

async function loadEntries(next = false): Promise<void> {
  const currentRun = props.runId;
  const currentHistoryRevision = props.historyRevision;
  const currentGeneration = generation;
  if (entryBusy.value || !currentRun) return;
  entryBusy.value = true;
  error.value = false;
  const cursor = next ? entryCursor.value : null;
  try {
    const page = await listMemoryEntries(currentRun, cursor, PAGE_SIZE);
    if (
      currentGeneration !== generation ||
      currentRun !== props.runId ||
      currentHistoryRevision !== props.historyRevision ||
      page.historyRevision !== currentHistoryRevision
    )
      return;
    entries.value = (next ? [...entries.value, ...page.entries] : page.entries).slice(-MAX_VISIBLE);
    entryCursor.value = page.nextCursor;
  } catch {
    if (currentGeneration === generation) error.value = true;
  } finally {
    if (currentGeneration === generation) entryBusy.value = false;
  }
}

async function answer(question: MemoryQuestion, selected: MemoryQuestion["answer"]): Promise<void> {
  if (answering.value.has(question.questionId)) return;
  const currentRun = props.runId;
  const currentHistoryRevision = props.historyRevision;
  const currentGeneration = generation;
  answering.value = new Set(answering.value).add(question.questionId);
  error.value = false;
  try {
    const updated = await resolveMemoryQuestion(currentRun, {
      questionId: question.questionId,
      expectedRevision: question.revision,
      expectedHistoryRevision: question.draft.historyRevision,
      answer: selected ?? { kind: "none" },
    });
    if (
      currentGeneration !== generation ||
      currentRun !== props.runId ||
      currentHistoryRevision !== props.historyRevision
    )
      return;
    questions.value = questions.value.map((item) =>
      item.questionId === updated.questionId ? updated : item,
    );
    delete proposalNames.value[question.questionId];
  } catch {
    if (currentGeneration === generation) error.value = true;
  } finally {
    if (currentGeneration === generation) {
      const pending = new Set(answering.value);
      pending.delete(question.questionId);
      answering.value = pending;
    }
  }
}

async function openEntry(entry: MemoryEntry): Promise<void> {
  const currentRun = props.runId;
  const currentHistoryRevision = props.historyRevision;
  const currentGeneration = generation;
  detailBusy.value = true;
  error.value = false;
  try {
    const result = await getMemoryEntry(currentRun, entry.entryId, entry.currentVersionId);
    if (
      currentGeneration === generation &&
      currentRun === props.runId &&
      currentHistoryRevision === props.historyRevision
    )
      details.value = result;
  } catch {
    if (currentGeneration === generation) error.value = true;
  } finally {
    if (currentGeneration === generation) detailBusy.value = false;
  }
}

function toggleEntry(entry: MemoryEntry): void {
  if (details.value?.entry.entryId === entry.entryId) {
    details.value = null;
    return;
  }
  void openEntry(entry);
}

function retry(): void {
  void Promise.all([loadQuestions(), loadEntries()]);
}

watch(
  () => [props.runId, props.historyRevision],
  () => {
    generation += 1;
    questions.value = [];
    entries.value = [];
    questionCursor.value = null;
    entryCursor.value = null;
    materialCursor.value = null;
    materialCount.value = null;
    materialBusy.value = false;
    materialError.value = false;
    batchId.value = null;
    batchStatus.value = null;
    batchBusy.value = false;
    batchError.value = false;
    details.value = null;
    questionBusy.value = false;
    entryBusy.value = false;
    detailBusy.value = false;
    answering.value = new Set();
    proposalNames.value = {};
    error.value = false;
    void loadQuestions();
    void loadEntries();
    void recoverActiveBatch();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  generation += 1;
});
</script>

<template>
  <div class="grid gap-3">
    <h2 v-if="panelView === 'all'" class="font-semibold">{{ t("memoryQuery.title") }}</h2>
    <section v-if="panelView === 'all' || panelView === 'list'" class="grid gap-2">
      <div class="flex flex-wrap items-center gap-2">
        <button
          type="button"
          :disabled="materialBusy"
          @click="collectMaterials(Boolean(materialCursor))"
        >
          {{
            materialBusy
            ? t("memoryQuery.collecting")
            : materialCursor
              ? t("memoryQuery.collectNext")
              : t("memoryQuery.collect")
          }}
        </button>
        <p v-if="materialCount !== null" role="status" class="text-sm text-muted">
          {{ t("memoryQuery.collected", { count: materialCount }) }}
          {{ t("memoryQuery.localOnly") }}
        </p>
      </div>
      <p v-if="materialError" role="alert" class="text-sm text-danger">
        {{ t("memoryQuery.collectError") }}
      </p>
      <p v-if="materialCursor" class="text-sm text-muted">
        {{ t("memoryQuery.collectMore") }}
      </p>
      <div class="flex flex-wrap items-center gap-2">
        <button
          v-if="canPrepareBatch"
          type="button"
          :disabled="batchBusy"
          @click="prepareBatch"
        >
          {{ batchBusy ? t("memoryQuery.processing") : t("memoryQuery.prepareBatch") }}
        </button>
        <button
          v-if="batchId && batchStatus?.state === 'queued'"
          type="button"
          :disabled="batchBusy"
          @click="sendBatch"
        >
          {{ t("memoryQuery.sendBatch") }}
        </button>
        <button
          v-if="batchId && batchStatus?.state === 'received'"
          type="button"
          :disabled="batchBusy"
          @click="completeBatch"
        >
          {{ t("memoryQuery.completeBatch") }}
        </button>
        <button
          v-if="batchId"
          type="button"
          :disabled="batchBusy"
          @click="refreshBatch"
        >
          {{ t("memoryQuery.refreshBatch") }}
        </button>
        <p v-if="batchStatus" role="status" class="text-sm text-muted">
          {{ batchStateLabels[batchStatus.state] }} · {{ batchId }}
        </p>
        <ul
          v-if="batchStatus?.completion?.outcomes.length"
          class="grid w-full gap-1 text-sm text-muted"
        >
          <li v-for="outcome in batchStatus.completion.outcomes" :key="outcome.candidateId">
            {{ outcome.candidateId }} · {{ candidateOutcomeLabels[outcome.outcome] }}
            <span v-if="outcome.reasonCode">· {{
                candidateReasonLabels[outcome.reasonCode]
              }}</span>
          </li>
        </ul>
      </div>
      <p v-if="batchError" role="alert" class="text-sm text-danger">
        {{ t("memoryQuery.batchError") }}
      </p>
    </section>
    <p v-if="error" role="alert">
      {{ t("memoryQuery.error") }}
      <button type="button" class="underline" @click="retry">
        {{ t("memoryQuery.reload") }}
      </button>
    </p>
    <section v-if="panelView === 'all' || panelView === 'pending'" class="grid gap-2">
      <div class="flex items-center justify-between gap-2">
        <h2 class="font-semibold">{{ t("memoryQuery.questions") }}</h2>
        <button type="button" :disabled="questionBusy" @click="loadQuestions()">
          {{ questionBusy ? t("memoryQuery.loading") : t("memoryQuery.refresh") }}
        </button>
      </div>
      <p v-if="!questions.length && !questionBusy" class="text-sm text-muted">
        {{ t("memoryQuery.noneQuestions") }}
      </p>
      <article
        v-for="question in questions"
        :key="question.questionId"
        class="grid gap-2 rounded border border-line p-3"
      >
        <p>{{ questionName(question) }}</p>
        <p v-if="question.status !== 'pending'" class="text-sm text-muted">
          {{
            t(question.status === "resolved" ? "memoryQuery.resolved" : "memoryQuery.invalidated")
          }}
        </p>
        <template v-else>
          <div class="flex flex-wrap gap-2">
            <button
              v-for="candidate in question.draft.candidates"
              :key="JSON.stringify(candidate)"
              type="button"
              :disabled="answering.has(question.questionId)"
              @click="answer(question, { kind: 'selectCandidate', candidate })"
            >
              {{ candidateName(candidate) }}
            </button>
            <button
              type="button"
              :disabled="answering.has(question.questionId)"
              @click="answer(question, { kind: 'none' })"
            >
              {{ t("memoryQuery.none") }}
            </button>
            <button
              type="button"
              :disabled="answering.has(question.questionId)"
              @click="answer(question, { kind: 'skip' })"
            >
              {{ t("memoryQuery.skip") }}
            </button>
          </div>
          <form
            v-if="question.draft.identity.kind === 'entityAssociation'"
            class="flex flex-wrap gap-2"
            @submit.prevent="answer(question, { kind: 'proposeEntity', displayName: proposalNames[question.questionId] ?? '' })"
          >
            <label>
              {{ t("memoryQuery.name") }}
              <input
                v-model="proposalNames[question.questionId]"
                maxlength="128"
                required
                :aria-label="t('memoryQuery.name')"
              />
            </label>
            <button type="submit" :disabled="answering.has(question.questionId)">
              {{ t("memoryQuery.propose") }}
            </button>
          </form>
        </template>
      </article>
      <button
        v-if="questionCursor"
        type="button"
        :disabled="questionBusy"
        @click="loadQuestions(true)"
      >
        {{ t("memoryQuery.next") }}
      </button>
    </section>

    <section v-if="panelView === 'all' || panelView === 'list'" class="grid gap-2">
      <div class="flex items-center justify-between gap-2">
        <h2 class="font-semibold">{{ t("memoryQuery.entries") }}</h2>
        <button type="button" :disabled="entryBusy" @click="loadEntries()">
          {{ entryBusy ? t("memoryQuery.loading") : t("memoryQuery.refresh") }}
        </button>
      </div>
      <p v-if="!entries.length && !entryBusy" class="text-sm text-muted">
        {{ t("memoryQuery.noneEntries") }}
      </p>
      <article
        v-for="entry in entries"
        :key="entry.entryId"
        class="grid gap-2 rounded border border-line p-3"
      >
        <button
          type="button"
          :disabled="detailBusy"
          :aria-expanded="details?.entry.entryId === entry.entryId"
          :aria-controls="`memory-entry-${entry.entryId}`"
          @click="toggleEntry(entry)"
        >
          <span class="font-mono">{{ entry.entryId }}</span>
          <span class="tag">{{ entryStateLabels[entry.state] }}</span>
          <span>{{
            t(details?.entry.entryId === entry.entryId ? "memoryQuery.hideDetails" : "memoryQuery.details")
          }}</span>
        </button>
        <div
          v-if="details?.entry.entryId === entry.entryId"
          :id="`memory-entry-${entry.entryId}`"
          class="grid gap-2"
          aria-live="polite"
        >
          <h3 class="font-semibold">{{ details.version.summary }}</h3>
          <p class="text-sm text-muted">
            <span>{{ subjectKindLabels[details.version.subject.kind] }}</span>
            <code v-if="details.version.subject.kind === 'character'">{{
              details.version.subject.id
            }}</code>
            · {{ dimensionLabels[details.version.dimension] }}
          </p>
          <div class="flex flex-wrap gap-2 text-sm">
            <span class="tag">{{ attitudeLabels[details.version.attitude] }}</span>
            <span class="tag">{{ changeLabels[details.version.change] }}</span>
            <span v-if="details.version.inferred" class="tag">
              {{ t("memoryQuery.inferred") }}
            </span>
          </div>
          <div class="grid gap-1 text-sm">
            <h4 class="font-medium">{{ t("memoryQuery.sources") }}</h4>
            <ul v-if="details.version.sources.length" class="grid gap-1">
              <li
                v-for="source in details.version.sources"
                :key="`${source.runId}:${source.recordSeq}`"
              >
                {{ sourceLabel(source) }}
              </li>
            </ul>
            <p v-else class="text-muted">{{ t("memoryQuery.noSources") }}</p>
            <h4 class="font-medium">{{ t("memoryQuery.evidence") }}</h4>
            <ul v-if="details.version.evidence.length" class="grid gap-1">
              <li v-for="item in details.version.evidence" :key="item.evidenceId">
                {{ evidenceLabel(item) }} · {{ sourceLabel(item.source) }}
              </li>
            </ul>
            <p v-else class="text-muted">{{ t("memoryQuery.noEvidence") }}</p>
          </div>
        </div>
      </article>
      <button v-if="entryCursor" type="button" :disabled="entryBusy" @click="loadEntries(true)">
        {{ t("memoryQuery.next") }}
      </button>
    </section>
  </div>
</template>
