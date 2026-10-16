import { flushPromises, mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  MemoryEntry,
  MemoryEntryDetails,
  MemoryQuestion,
  MemoryQuestionPage,
} from "../../../src-web/api/memory";
import MemoryQueryPanel from "../../../src-web/components/MemoryQueryPanel.vue";
import { i18n } from "../../../src-web/i18n";
import {
  getMemoryEntry,
  listMemoryEntries,
  listMemoryQuestions,
  resolveMemoryQuestion,
} from "../../../src-web/api/memory";

vi.mock("../../../src-web/api/memory", () => ({
  getMemoryEntry: vi.fn(),
  listMemoryEntries: vi.fn(),
  listMemoryQuestions: vi.fn(),
  resolveMemoryQuestion: vi.fn(),
}));

const question: MemoryQuestion = {
  questionId: "question-1",
  runId: "run-1",
  questionKey: Array(32).fill(1),
  revision: 2,
  draft: {
    version: 1,
    identity: { kind: "entityAssociation", eventSlot: 1, sources: [] },
    historyRevision: 5,
    entityRevision: 3,
    evidenceRevision: 2,
    mentionedName: "李雷",
    candidates: [{ kind: "entity", entityId: "npc-1", displayName: "米拉" }],
  },
  status: "pending",
  answer: null,
};
const entry: MemoryEntry = {
  entryId: "entry-1",
  currentVersionId: "version-1",
  stateRevision: 1,
  state: "active",
};
const entryDetails: MemoryEntryDetails = {
  entry,
  version: {
    version: 1,
    entryId: entry.entryId,
    versionId: entry.currentVersionId,
    subject: { kind: "character", id: "npc-1" },
    dimension: "story",
    summary: "在钟声响起时守住了地窖入口。",
    attitude: "believed",
    inferred: false,
    change: "new",
    sources: [
      {
        scriptId: "mistbell",
        runId: "run-1",
        sessionId: "session-1",
        recordSeq: 42,
        bodyHash: Array(32).fill(0),
        startByte: 0,
        endByte: 6,
      },
    ],
    evidence: [
      {
        evidenceId: "evidence-1",
        subjectId: "npc-1",
        source: {
          scriptId: "mistbell",
          runId: "run-1",
          sessionId: "session-1",
          recordSeq: 42,
          bodyHash: Array(32).fill(0),
          startByte: 0,
          endByte: 6,
        },
        kind: "told",
        scope: "directDialogue",
        ruleId: "directDialogueV1",
      },
    ],
    sourceSetHash: [],
    evidenceSetHash: [],
    policyHash: [],
  },
};

describe("MemoryQueryPanel", () => {
  beforeEach(() => {
    i18n.global.locale.value = "zh-Hans";
    vi.resetAllMocks();
    vi.mocked(listMemoryQuestions).mockResolvedValue({
      questions: [question],
      nextCursor: null,
      historyRevision: 5,
    });
    vi.mocked(listMemoryEntries).mockResolvedValue({
      entries: [entry],
      nextCursor: null,
      historyRevision: 5,
    });
    vi.mocked(resolveMemoryQuestion).mockResolvedValue({
      ...question,
      status: "resolved",
      answer: { kind: "selectCandidate", candidate: question.draft.candidates[0]! },
    });
    vi.mocked(getMemoryEntry).mockResolvedValue(entryDetails);
  });

  it("answers with the displayed candidate and reads the selected committed version", async () => {
    const wrapper = mount(MemoryQueryPanel, {
      props: { runId: "run-1", historyRevision: 5 },
      global: { plugins: [i18n] },
    });
    await flushPromises();

    const candidateButton = wrapper.findAll("button").find((button) => button.text() === "米拉");
    await candidateButton!.trigger("click");
    await flushPromises();
    expect(resolveMemoryQuestion).toHaveBeenCalledWith("run-1", {
      questionId: "question-1",
      expectedRevision: 2,
      expectedHistoryRevision: 5,
      answer: {
        kind: "selectCandidate",
        candidate: { kind: "entity", entityId: "npc-1", displayName: "米拉" },
      },
    });

    const detailButton = wrapper
      .findAll("button")
      .find((button) => button.text().includes("读取当前记忆"));
    await detailButton!.trigger("click");
    await flushPromises();
    expect(getMemoryEntry).toHaveBeenCalledWith("run-1", "entry-1", "version-1");
    expect(wrapper.text()).toContain("在钟声响起时守住了地窖入口。");
    expect(wrapper.text()).toContain("记录 42");
    expect(wrapper.text()).toContain("直接告知");
    wrapper.unmount();
  });

  it("drops a late page after switching to another run", async () => {
    let resolveOldPage: ((page: MemoryQuestionPage) => void) | undefined;
    vi.mocked(listMemoryQuestions)
      .mockImplementationOnce(() => new Promise((resolve) => (resolveOldPage = resolve)))
      .mockResolvedValueOnce({
        questions: [
          {
            ...question,
            runId: "run-2",
            draft: { ...question.draft, mentionedName: "新周目的问题" },
          },
        ],
        nextCursor: null,
        historyRevision: 0,
      });
    const wrapper = mount(MemoryQueryPanel, {
      props: { runId: "run-1", historyRevision: 5 },
      global: { plugins: [i18n] },
    });
    await vi.waitFor(() => expect(listMemoryQuestions).toHaveBeenCalledTimes(1));
    await wrapper.setProps({ runId: "run-2", historyRevision: 0 });
    await vi.waitFor(() => expect(listMemoryQuestions).toHaveBeenCalledTimes(2));
    resolveOldPage?.({ questions: [question], nextCursor: null, historyRevision: 5 });
    await flushPromises();

    expect(wrapper.text()).toContain("新周目的问题");
    expect(wrapper.text()).not.toContain("李雷");
    wrapper.unmount();
  });

  it("drops a late page after history changes within the same run", async () => {
    let resolveOldPage: ((page: MemoryQuestionPage) => void) | undefined;
    vi.mocked(listMemoryQuestions)
      .mockImplementationOnce(() => new Promise((resolve) => (resolveOldPage = resolve)))
      .mockResolvedValueOnce({
        questions: [
          {
            ...question,
            draft: { ...question.draft, mentionedName: "回退后的问题", historyRevision: 6 },
          },
        ],
        nextCursor: null,
        historyRevision: 6,
      });
    const wrapper = mount(MemoryQueryPanel, {
      props: { runId: "run-1", historyRevision: 5 },
      global: { plugins: [i18n] },
    });
    await vi.waitFor(() => expect(listMemoryQuestions).toHaveBeenCalledTimes(1));
    await wrapper.setProps({ historyRevision: 6 });
    await vi.waitFor(() => expect(listMemoryQuestions).toHaveBeenCalledTimes(2));
    await flushPromises();
    resolveOldPage?.({ questions: [question], nextCursor: null, historyRevision: 5 });
    await flushPromises();

    expect(wrapper.text()).toContain("回退后的问题");
    expect(wrapper.text()).not.toContain("李雷");
    wrapper.unmount();
  });

  it("drops a late detail response after history changes within the same run", async () => {
    let resolveOldDetails: ((details: MemoryEntryDetails) => void) | undefined;
    vi.mocked(getMemoryEntry).mockImplementationOnce(
      () => new Promise((resolve) => (resolveOldDetails = resolve)),
    );
    const wrapper = mount(MemoryQueryPanel, {
      props: { runId: "run-1", historyRevision: 5 },
      global: { plugins: [i18n] },
    });
    await flushPromises();
    await wrapper
      .findAll("button")
      .find((button) => button.text().includes("读取当前记忆"))!
      .trigger("click");
    await vi.waitFor(() => expect(getMemoryEntry).toHaveBeenCalledOnce());

    await wrapper.setProps({ historyRevision: 6 });
    await flushPromises();
    resolveOldDetails?.(entryDetails);
    await flushPromises();

    expect(wrapper.text()).not.toContain("在钟声响起时守住了地窖入口。");
    wrapper.unmount();
  });
});
