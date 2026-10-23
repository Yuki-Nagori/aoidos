import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import type { RunPage, RunState, RunSummary } from "../../../src-web/api/runs";
import {
  bindRunProfile,
  deleteRun,
  listRuns,
  openSelectedRun,
  reconcileRunBilling,
  scanLegacyRuns,
} from "../../../src-web/api/runs";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

it("forwards the revision-bound run catalog cursor without rewriting it", async () => {
  const state: RunState = "needs-profile-binding";
  const summary: RunSummary = {
    sessionId: "00000000-0000-4000-8000-000000000001",
    scriptId: "mistbell",
    createdAt: "2026-10-20T00:00:00Z",
    memoryRunId: null,
    billingRunId: null,
    profileId: null,
    revision: 1,
    state,
    active: false,
    deletionOperationId: null,
  };
  const response: RunPage = {
    entries: [summary],
    catalogRevision: 4,
    nextCursor: null,
    scanIncomplete: true,
    scanIssueCount: 2,
    pendingDeletions: [],
    pendingDeletionsTruncated: false,
  };
  const cursor = {
    catalogRevision: 4,
    afterCreatedAt: "2026-10-20T00:00:00Z",
    afterSessionId: "00000000-0000-4000-8000-000000000001",
  };
  vi.mocked(invoke).mockResolvedValue(response);

  expect(await listRuns(cursor, 20)).toBe(response);
  expect(invoke).toHaveBeenCalledWith("run_catalog_page", { cursor, pageSize: 20 });
  expect(await listRuns()).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("run_catalog_page", {
    cursor: null,
    pageSize: 50,
  });
});

it("opens only the explicitly selected run revision", async () => {
  const response = {
    session: {
      sessionId: "00000000-0000-4000-8000-000000000001",
      memoryRunId: "00000000-0000-4000-8000-000000000002",
      profileId: "saved-profile",
      scriptId: "mistbell",
      title: "雾钟地窖",
      selectionMirrorPending: false,
    },
    profile: {
      profileId: "saved-profile",
      providerId: "deepseek",
      model: "deepseek-v4-flash",
      mode: "completion" as const,
      thinking: false,
      sampling: { temperature: 1, maxTokens: 4096 },
      proxy: { mode: "none" as const },
    },
  };
  vi.mocked(invoke).mockResolvedValue(response);

  expect(await openSelectedRun(response.session.sessionId, 7)).toBe(response);
  expect(invoke).toHaveBeenCalledWith("run_open_selected", {
    sessionId: response.session.sessionId,
    expectedRevision: 7,
  });
});

it("forwards explicit deletion confirmation identity and revision", async () => {
  const operation = {
    operationId: "00000000-0000-4000-8000-000000000003",
    sessionId: "00000000-0000-4000-8000-000000000001",
    state: "database-purged" as const,
    catalogRevision: 9,
    diagnosticCode: null,
  };
  vi.mocked(invoke).mockResolvedValue(operation);

  expect(
    await deleteRun(operation.operationId, operation.sessionId, operation.catalogRevision, false),
  ).toBe(operation);
  expect(invoke).toHaveBeenCalledWith("run_delete", {
    operationId: operation.operationId,
    sessionId: operation.sessionId,
    expectedRevision: operation.catalogRevision,
    acknowledgeRunDataLoss: false,
  });
});

it("advances legacy discovery only through the explicit scan action", async () => {
  const result = {
    discovered: 3,
    diagnosed: 1,
    scanIncomplete: true,
    scanIssueCount: 1,
  };
  vi.mocked(invoke).mockResolvedValue(result);

  expect(await scanLegacyRuns()).toBe(result);
  expect(invoke).toHaveBeenCalledWith("run_scan_legacy");
});

it("forwards profile binding with the catalog revision used for confirmation", async () => {
  vi.mocked(invoke).mockResolvedValue(3);

  expect(await bindRunProfile("00000000-0000-4000-8000-000000000001", 2, "deepseek-default")).toBe(
    3,
  );
  expect(invoke).toHaveBeenCalledWith("run_bind_profile", {
    sessionId: "00000000-0000-4000-8000-000000000001",
    expectedRevision: 2,
    profileId: "deepseek-default",
  });
});

it("sends legacy budget reconciliation arguments in the Rust request's top-level shape", async () => {
  const response = {
    revision: 5,
    merge: {
      movedRequests: 2,
      movedWarnings: 1,
      settings: {
        runId: "00000000-0000-4000-8000-000000000001",
        cnyLimit: "20",
        usdLimit: "5",
        revision: 3,
      },
    },
  };
  vi.mocked(invoke).mockResolvedValue(response);

  expect(
    await reconcileRunBilling("00000000-0000-4000-8000-000000000001", 4, {
      cnyLimit: "20",
      usdLimit: "5",
    }),
  ).toBe(response);
  expect(invoke).toHaveBeenCalledWith("run_reconcile_billing", {
    sessionId: "00000000-0000-4000-8000-000000000001",
    expectedRevision: 4,
    selectedLimits: { cnyLimit: "20", usdLimit: "5" },
  });
});
