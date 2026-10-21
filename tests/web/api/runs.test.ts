import { invoke } from "@tauri-apps/api/core";
import { expect, it, vi } from "vitest";
import type { RunPage, RunState, RunSummary } from "../../../src-web/api/runs";
import { bindRunProfile, listRuns } from "../../../src-web/api/runs";

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
  };
  const response: RunPage = {
    entries: [summary],
    catalogRevision: 4,
    nextCursor: null,
    scanIncomplete: true,
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
