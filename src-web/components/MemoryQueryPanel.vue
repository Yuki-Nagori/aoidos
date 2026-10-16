<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type {
  MemoryEntry,
  MemoryEntryCursor,
  MemoryEntryDetails,
  MemoryKnowledgeEvidence,
  MemoryQuestion,
  MemoryQuestionCandidate,
  MemoryQuestionCursor,
  MemorySourceRef,
} from "../api/memory";
import {
  getMemoryEntry,
  listMemoryEntries,
  listMemoryQuestions,
  resolveMemoryQuestion,
} from "../api/memory";

const props = defineProps<{ runId: string; historyRevision: number }>();
const { t } = useI18n();
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
const PAGE_SIZE = 32;
const MAX_VISIBLE = 128;
const questions = ref<MemoryQuestion[]>([]);
const entries = ref<MemoryEntry[]>([]);
const questionCursor = ref<MemoryQuestionCursor | null>(null);
const entryCursor = ref<MemoryEntryCursor | null>(null);
const questionBusy = ref(false);
const entryBusy = ref(false);
const detailBusy = ref(false);
const error = ref(false);
const answering = ref(new Set<string>());
const proposalNames = ref<Record<string, string>>({});
const details = ref<MemoryEntryDetails | null>(null);
let generation = 0;

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
    details.value = null;
    questionBusy.value = false;
    entryBusy.value = false;
    detailBusy.value = false;
    answering.value = new Set();
    proposalNames.value = {};
    error.value = false;
    void loadQuestions();
    void loadEntries();
  },
  { immediate: true },
);
</script>

<template>
  <details class="glass-panel">
    <summary>{{ t("memoryQuery.title") }}</summary>
    <p v-if="error" role="alert">
      {{ t("memoryQuery.error") }}
      <button type="button" class="underline" @click="retry">
        {{ t("memoryQuery.reload") }}
      </button>
    </p>
    <section class="grid gap-2 p-2">
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

    <section class="grid gap-2 p-2">
      <div class="flex items-center justify-between gap-2">
        <h2 class="font-semibold">{{ t("memoryQuery.entries") }}</h2>
        <button type="button" :disabled="entryBusy" @click="loadEntries()">
          {{ entryBusy ? t("memoryQuery.loading") : t("memoryQuery.refresh") }}
        </button>
      </div>
      <p v-if="!entries.length && !entryBusy" class="text-sm text-muted">
        {{ t("memoryQuery.noneEntries") }}
      </p>
      <article v-for="entry in entries" :key="entry.entryId" class="rounded border border-line p-3">
        <button type="button" :disabled="detailBusy" @click="openEntry(entry)">
          {{ t("memoryQuery.details") }} · {{ entry.entryId }}
        </button>
      </article>
      <button v-if="entryCursor" type="button" :disabled="entryBusy" @click="loadEntries(true)">
        {{ t("memoryQuery.next") }}
      </button>
      <article v-if="details" class="grid gap-2 rounded border border-line p-3" aria-live="polite">
        <h3 class="font-semibold">{{ details.version.summary }}</h3>
        <p class="text-sm text-muted">
          {{ details.version.subject.kind }} · {{ details.version.dimension }}
        </p>
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
      </article>
    </section>
  </details>
</template>
