import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { RunPage, RunSummary } from "../../../../../src-web/api/runs";
import type { ScriptInfo } from "../../../../../src-web/api/engine";
import RunCatalogPanel from "../../../../../src-web/features/product/components/RunCatalogPanel.vue";
import { i18n } from "../../../../../src-web/i18n/index";

const api = vi.hoisted(() => ({
  bindRunProfile: vi.fn(),
  deleteRun: vi.fn(),
  listRuns: vi.fn(),
  openSelectedRun: vi.fn(),
  reconcileRunBilling: vi.fn(),
  scanLegacyRuns: vi.fn(),
}));
vi.mock("../../../../../src-web/api/runs", () => api);

const run: RunSummary = {
  sessionId: "session-1",
  scriptId: "mistbell",
  createdAt: "2026-10-21T00:00:00Z",
  memoryRunId: "memory-1",
  billingRunId: "session-1",
  profileId: "profile-1",
  revision: 4,
  state: "ready",
  active: false,
  deletionOperationId: null,
};
const script: ScriptInfo = {
  scriptId: "mistbell",
  title: "Mistbell Cellar",
  displayNames: { en: "Mistbell Cellar", "zh-Hans": "雾钟地窖" },
  suggestedActions: { en: "Look around.", "zh-Hans": "环顾四周。" },
  attributions: "notice",
};
function page(entries: RunSummary[], pendingDeletions: RunPage["pendingDeletions"] = []): RunPage {
  return {
    entries,
    nextCursor: null,
    catalogRevision: 7,
    scanIncomplete: false,
    scanIssueCount: 0,
    pendingDeletions,
    pendingDeletionsTruncated: false,
  };
}
function buttonNamed(text: string): HTMLButtonElement {
  const button = [...document.body.querySelectorAll("button")].find(
    (candidate) => candidate.textContent?.trim() === text,
  );
  if (!button) throw new Error(`button not found: ${text}`);
  return button;
}

beforeEach(() => {
  i18n.global.locale.value = "en";
  vi.clearAllMocks();
});
afterEach(() => vi.unstubAllGlobals());

it("embeds run summaries, highlights the active run and warns before deleting unsettled billing", async () => {
  api.listRuns.mockResolvedValue(
    page([
      { ...run, active: true },
      { ...run, sessionId: "pending-run", state: "settlement-pending", active: false },
    ]),
  );
  const wrapper = mount(RunCatalogPanel, {
    props: { open: true, embedded: true, scripts: [script], profiles: [], locale: "en" },
    global: { plugins: [i18n] },
  });
  await vi.waitFor(() => expect(wrapper.findAll(".run-catalog-row")).toHaveLength(2));
  expect(wrapper.find(".run-catalog-backdrop").exists()).toBe(false);
  expect(wrapper.get(".run-catalog-dialog").attributes("role")).toBe("region");
  expect(wrapper.find(".run-catalog-row.active").exists()).toBe(true);
  const pending = wrapper.findAll(".run-catalog-row")[1]!;
  expect(pending.get(".danger").attributes("disabled")).toBeUndefined();
  expect(pending.find(".run-catalog-warning").exists()).toBe(true);
  expect(pending.get(".primary").text()).toBe("Restore to resolve billing");
  expect(pending.get(".run-catalog-warning").text()).toContain("permanently remove those requests");
  await wrapper.findAll(".run-catalog-filter button")[1]!.trigger("click");
  expect(wrapper.findAll(".run-catalog-row")).toHaveLength(1);
  expect(wrapper.find(".run-catalog-row.active").exists()).toBe(false);
  expect(api.deleteRun).not.toHaveBeenCalled();
  wrapper.unmount();
});

it("shows an empty-state message when the attention filter excludes every ready run", async () => {
  api.listRuns.mockResolvedValue({
    ...page([run]),
    nextCursor: { catalogRevision: 7, afterCreatedAt: run.createdAt, afterSessionId: "next-run" },
  });
  const wrapper = mount(RunCatalogPanel, {
    props: { open: true, embedded: true, scripts: [script], profiles: [], locale: "en" },
    global: { plugins: [i18n] },
  });
  await vi.waitFor(() => expect(wrapper.findAll(".run-catalog-row")).toHaveLength(1));

  await wrapper.findAll(".run-catalog-filter button")[1]!.trigger("click");

  expect(wrapper.findAll(".run-catalog-row")).toHaveLength(0);
  expect(wrapper.get(".run-catalog-empty").text()).toBe("No runs on this page need attention.");
  expect(
    wrapper.findAll(".run-catalog-footer button").at(-1)?.attributes("disabled"),
  ).toBeUndefined();
  wrapper.unmount();
});

it("disables deletion when a run's memory identity mapping needs repair", async () => {
  api.listRuns.mockResolvedValue(page([{ ...run, state: "needs-repair" }]));
  const wrapper = mount(RunCatalogPanel, {
    props: { open: true, embedded: true, scripts: [script], profiles: [], locale: "en" },
    global: { plugins: [i18n] },
  });

  await vi.waitFor(() => expect(wrapper.findAll(".run-catalog-row")).toHaveLength(1));

  const row = wrapper.get(".run-catalog-row");
  expect(row.get(".run-catalog-warning").text()).toContain(
    "memory identity mapping is inconsistent",
  );
  expect(row.get(".danger").attributes("disabled")).toBeDefined();
  wrapper.unmount();
});

it("requires both a selected profile and explicit acknowledgement before binding a legacy save", async () => {
  api.listRuns.mockResolvedValue(
    page([{ ...run, profileId: null, state: "needs-profile-binding" }]),
  );
  api.bindRunProfile.mockResolvedValue(5);
  const wrapper = mount(RunCatalogPanel, {
    props: {
      open: true,
      embedded: true,
      scripts: [script],
      profiles: [
        {
          profileId: "saved-profile",
          providerId: "deepseek",
          model: "deepseek-chat",
          mode: "chat",
          thinking: false,
          sampling: { temperature: 1, maxTokens: 1024 },
          proxy: { mode: "system" },
        },
      ],
      locale: "en",
    },
    global: { plugins: [i18n] },
  });
  await vi.waitFor(() => expect(wrapper.find(".run-catalog-row").exists()).toBe(true));
  await wrapper.get(".run-catalog-row .primary").trigger("click");
  const modal = wrapper.get(".run-delete-confirm");
  expect(modal.get(".primary").attributes("disabled")).toBeDefined();
  await modal.get('input[type="radio"]').setValue(true);
  expect(modal.get(".primary").attributes("disabled")).toBeDefined();
  await modal.get('input[type="checkbox"]').setValue(true);
  await modal.get(".primary").trigger("click");
  await vi.waitFor(() =>
    expect(api.bindRunProfile).toHaveBeenCalledWith("session-1", 4, "saved-profile"),
  );
  expect(wrapper.find(".run-delete-confirm").exists()).toBe(false);
  wrapper.unmount();
});

it("keeps a partially completed deletion visible and resumes it with the same operation ID", async () => {
  vi.stubGlobal("crypto", { randomUUID: () => "delete-op-1" });
  api.listRuns.mockResolvedValueOnce(page([run])).mockResolvedValue(page([]));
  api.deleteRun
    .mockResolvedValueOnce({
      operationId: "delete-op-1",
      sessionId: "session-1",
      state: "database-purged",
      catalogRevision: 8,
      diagnosticCode: null,
    })
    .mockResolvedValueOnce({
      operationId: "delete-op-1",
      sessionId: "session-1",
      state: "completed",
      catalogRevision: 8,
      diagnosticCode: null,
    });

  const wrapper = mount(RunCatalogPanel, {
    props: {
      open: false,
      scripts: [script],
      profiles: [],
      locale: "en",
    },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
  await wrapper.setProps({ open: true });
  await vi.waitFor(() => expect(api.listRuns).toHaveBeenCalledTimes(1));

  await buttonNamed("Delete").click();
  expect(buttonNamed("Confirm deletion").disabled).toBe(true);
  (document.body.querySelector(".run-acknowledgement input") as HTMLInputElement).click();
  await vi.waitFor(() => expect(buttonNamed("Confirm deletion").disabled).toBe(false));
  await buttonNamed("Confirm deletion").click();
  await vi.waitFor(() => expect(api.deleteRun).toHaveBeenCalledTimes(1));
  expect(api.deleteRun).toHaveBeenLastCalledWith("delete-op-1", "session-1", 4, true);
  expect(document.body.textContent).toContain("Run cleanup to resume");

  await buttonNamed("Cancel").click();
  expect(document.body.textContent).toContain("Run cleanup to resume");

  await buttonNamed("Resume deletion").click();
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("Run deletion is still in progress"),
  );
  await buttonNamed("Continue deletion").click();
  await vi.waitFor(() => expect(api.deleteRun).toHaveBeenCalledTimes(2));
  expect(api.deleteRun).toHaveBeenLastCalledWith("delete-op-1", "session-1", 4, false);
  expect(document.body.textContent).not.toContain("Run cleanup to resume");
  wrapper.unmount();
  document.body.innerHTML = "";
});

it("retries a pre-journal deletion failure with the run revision, not the catalog revision", async () => {
  vi.stubGlobal("crypto", { randomUUID: () => "delete-op-retry" });
  api.listRuns.mockResolvedValueOnce(page([run])).mockResolvedValue(page([]));
  api.deleteRun
    .mockRejectedValueOnce({ code: "app.busy", message: "A turn is active." })
    .mockResolvedValueOnce({
      operationId: "delete-op-retry",
      sessionId: "session-1",
      state: "completed",
      catalogRevision: 8,
      diagnosticCode: null,
    });

  const wrapper = mount(RunCatalogPanel, {
    props: { open: false, scripts: [script], profiles: [], locale: "en" },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
  await wrapper.setProps({ open: true });
  await vi.waitFor(() => expect(api.listRuns).toHaveBeenCalledTimes(1));
  await buttonNamed("Delete").click();
  (document.body.querySelector(".run-acknowledgement input") as HTMLInputElement).click();
  await vi.waitFor(() => expect(buttonNamed("Confirm deletion").disabled).toBe(false));
  await buttonNamed("Confirm deletion").click();
  await vi.waitFor(() => expect(api.deleteRun).toHaveBeenCalledTimes(1));
  expect(api.deleteRun).toHaveBeenLastCalledWith("delete-op-retry", "session-1", 4, true);

  await buttonNamed("Cancel").click();
  await buttonNamed("Resume deletion").click();
  await buttonNamed("Continue deletion").click();
  await vi.waitFor(() => expect(api.deleteRun).toHaveBeenCalledTimes(2));
  expect(api.deleteRun).toHaveBeenLastCalledWith("delete-op-retry", "session-1", 4, false);
  wrapper.unmount();
  document.body.innerHTML = "";
});

it("hydrates deletion retry from the persisted journal after restart", async () => {
  api.listRuns.mockResolvedValueOnce(
    page(
      [],
      [
        {
          operationId: "delete-op-persisted",
          sessionId: "session-1",
          state: "database-purged",
          catalogRevision: 8,
          diagnosticCode: "store.invalid-path",
        },
      ],
    ),
  );
  api.deleteRun.mockResolvedValue({
    operationId: "delete-op-persisted",
    sessionId: "session-1",
    state: "completed",
    catalogRevision: 8,
    diagnosticCode: null,
  });

  const wrapper = mount(RunCatalogPanel, {
    props: { open: false, scripts: [script], profiles: [], locale: "en" },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
  await wrapper.setProps({ open: true });
  await vi.waitFor(() => expect(api.listRuns).toHaveBeenCalledTimes(1));
  expect(document.body.textContent).toContain("Last recovery error: store.invalid-path");
  await buttonNamed("Resume deletion").click();
  await buttonNamed("Continue deletion").click();
  await vi.waitFor(() => expect(api.deleteRun).toHaveBeenCalledTimes(1));
  expect(api.deleteRun).toHaveBeenLastCalledWith("delete-op-persisted", "session-1", 8, false);
  wrapper.unmount();
  document.body.innerHTML = "";
});

it("allows opening a billing-not-configured run so the player can configure its budget", async () => {
  api.listRuns.mockResolvedValue(page([{ ...run, state: "billing-not-configured" }]));
  api.openSelectedRun.mockResolvedValue({ session: {}, profile: {} });
  const wrapper = mount(RunCatalogPanel, {
    props: { open: false, scripts: [script], profiles: [], locale: "en" },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
  await wrapper.setProps({ open: true });
  await vi.waitFor(() => expect(api.listRuns).toHaveBeenCalledTimes(1));
  await buttonNamed("Continue").click();
  await vi.waitFor(() => expect(api.openSelectedRun).toHaveBeenCalledWith("session-1", 4));
  expect(wrapper.emitted("restore")).toHaveLength(1);
  wrapper.unmount();
  document.body.innerHTML = "";
});

it("asks for explicit final limits only when legacy billing budgets conflict", async () => {
  api.listRuns.mockResolvedValue(page([{ ...run, state: "billing-reconciliation-required" }]));
  api.reconcileRunBilling
    .mockRejectedValueOnce({
      code: "budget.reconciliation-required",
      message: "Historical limits differ.",
    })
    .mockResolvedValueOnce({
      revision: 5,
      merge: { movedRequests: 2, movedWarnings: 1, settings: null },
    });

  const wrapper = mount(RunCatalogPanel, {
    props: {
      open: false,
      scripts: [script],
      profiles: [],
      locale: "en",
    },
    global: { plugins: [i18n] },
    attachTo: document.body,
  });
  await wrapper.setProps({ open: true });
  await vi.waitFor(() =>
    expect(
      [...document.body.querySelectorAll("button")].some(
        (button) => button.textContent?.trim() === "Reconcile legacy billing",
      ),
    ).toBe(true),
  );
  await buttonNamed("Reconcile legacy billing").click();
  await vi.waitFor(() => expect(api.reconcileRunBilling).toHaveBeenCalledTimes(1));
  expect(api.reconcileRunBilling).toHaveBeenLastCalledWith("session-1", 4, undefined);
  await vi.waitFor(() =>
    expect(document.body.textContent).toContain("If historical budget limits conflict"),
  );

  const inputs = document.body.querySelectorAll("input");
  inputs[0]!.value = "12.5";
  inputs[0]!.dispatchEvent(new Event("input", { bubbles: true }));
  inputs[1]!.value = "3";
  inputs[1]!.dispatchEvent(new Event("input", { bubbles: true }));
  const reconcileButtons = [...document.body.querySelectorAll("button")].filter(
    (button) => button.textContent?.trim() === "Reconcile legacy billing",
  );
  await reconcileButtons.at(-1)!.click();
  await vi.waitFor(() => expect(api.reconcileRunBilling).toHaveBeenCalledTimes(2));
  expect(api.reconcileRunBilling).toHaveBeenLastCalledWith("session-1", 4, {
    cnyLimit: "12.5",
    usdLimit: "3",
  });
  wrapper.unmount();
  document.body.innerHTML = "";
});
