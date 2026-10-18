import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import type { MemoryBatchStatus } from "../../../src-web/api/memory";
import {
  getMemoryAppConfig,
  getMemoryBatchStatus,
  getMemoryEntry,
  getMemoryScriptConfig,
  collectMemoryMaterials,
  listMemoryEntries,
  listMemoryQuestions,
  resolveMemoryQuestion,
  saveMemoryAppConfig,
  saveMemoryScriptConfig,
} from "../../../src-web/api/memory";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

it("forwards memory configuration DTOs without rewriting Rust-owned policy", async () => {
  const response = { configVersion: 2 };
  const batchStatus: MemoryBatchStatus = {
    batchId: "batch-1",
    runId: "run-1",
    historyRevision: 4,
    state: "completed",
    completion: { version: 1, outcomes: [] },
    diagnosticCode: null,
  };
  const parameters = {
    algorithm: "exponential" as const,
    baseStrength: 500_000,
    reinforcementGain: 150_000,
    reinforcementCap: 500_000,
    strengthCap: 1_000_000,
    halfLifeRounds: 100,
    repeatWindowRounds: 20,
    repeatLimit: 2,
    revivalCooldownRounds: 20,
    activeCapacity: 1_000,
    rateTableVersion: 1,
    decayRate: 993_092,
  };
  vi.mocked(invoke).mockResolvedValue(response);

  expect(await getMemoryAppConfig()).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_get_app_config");
  expect(await saveMemoryAppConfig(1, parameters)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_save_app_config", {
    expectedVersion: 1,
    parameters,
  });
  expect(await getMemoryScriptConfig("mistbell")).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_get_script_config", {
    scriptId: "mistbell",
  });

  const overrides = { reinforcementGain: 0, repeatLimit: 3 };
  expect(await saveMemoryScriptConfig("mistbell", 2, 4, overrides)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_save_script_config", {
    scriptId: "mistbell",
    expectedVersion: 2,
    expectedAppVersion: 4,
    overrides,
  });

  const questionCursor = {
    runId: "run-1",
    historyRevision: 8,
    snapshotMaxSequence: 10,
    afterSequence: 4,
  };
  expect(await listMemoryQuestions("run-1", questionCursor, 16)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_list_questions", {
    runId: "run-1",
    cursor: questionCursor,
    limit: 16,
  });

  const resolution = {
    questionId: "question-1",
    expectedRevision: 2,
    expectedHistoryRevision: 8,
    answer: { kind: "none" } as const,
  };
  expect(await resolveMemoryQuestion("run-1", resolution)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_resolve_question", {
    runId: "run-1",
    resolution,
  });

  const entryCursor = {
    runId: "run-1",
    historyRevision: 8,
    snapshotMaxSequence: 12,
    afterSequence: 4,
  };
  expect(await listMemoryEntries("run-1", entryCursor, 16)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_list_entries", {
    runId: "run-1",
    cursor: entryCursor,
    limit: 16,
  });
  expect(await getMemoryEntry("run-1", "entry-2", "version-3")).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("memory_get_entry", {
    runId: "run-1",
    entryId: "entry-2",
    expectedVersionId: "version-3",
  });
  vi.mocked(invoke).mockResolvedValue(batchStatus);
  expect(await getMemoryBatchStatus("run-1", "batch-1")).toBe(batchStatus);
  expect(invoke).toHaveBeenLastCalledWith("memory_get_batch_status", {
    runId: "run-1",
    batchId: "batch-1",
  });
});

it("collects eligible memory sources locally with a history-bound cursor", async () => {
  const response = {
    runId: "run-1",
    historyRevision: 8,
    eligibleCount: 12,
    nextCursor: null,
  };
  const cursor = {
    runId: "run-1",
    historyRevision: 8,
    snapshotMaxRecordSeq: 100,
    afterRecordSeq: 64,
  };
  vi.mocked(invoke).mockResolvedValue(response);

  expect(await collectMemoryMaterials("run-1", cursor)).toBe(response);
  expect(invoke).toHaveBeenCalledWith("memory_collect_materials", { runId: "run-1", cursor });
});
