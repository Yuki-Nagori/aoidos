import { effectScope } from "vue";
import { describe, expect, it, vi } from "vitest";
import { useUiPreferences } from "../../../src-web/composables/useUiPreferences";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((accept, fail) => {
    resolve = accept;
    reject = fail;
  });
  return { promise, resolve, reject };
}

describe("useUiPreferences", () => {
  it("does not let a late initial read overwrite an interaction", async () => {
    const read = deferred<{ version: 1; panelPinned: boolean; diceMode: "manual" | "auto" }>();
    const set = vi.fn(async (panelPinned: boolean, diceMode: "manual" | "auto") => ({
      version: 1 as const,
      panelPinned,
      diceMode,
    }));
    const scope = effectScope();
    const state = scope.run(() => useUiPreferences({ get: () => read.promise, set }))!;

    const loading = state.load();
    await state.setPanelPinned(true);
    read.resolve({ version: 1, panelPinned: false, diceMode: "manual" });
    await loading;

    expect(state.panelPinned.value).toBe(true);
    expect(set).toHaveBeenCalledWith(true, "manual");
    scope.stop();
  });

  it("serializes full replacements and keeps the latest intent", async () => {
    const first = deferred<{ version: 1; panelPinned: boolean; diceMode: "manual" | "auto" }>();
    const set = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockImplementationOnce(async (panelPinned: boolean, diceMode: "manual" | "auto") => ({
        version: 1 as const,
        panelPinned,
        diceMode,
      }));
    const scope = effectScope();
    const state = scope.run(() =>
      useUiPreferences({
        get: async () => ({ version: 1, panelPinned: false, diceMode: "manual" }),
        set,
      }),
    )!;
    await state.load();

    const pinning = state.setPanelPinned(true);
    await Promise.resolve();
    const mode = state.setDiceMode("auto");
    expect(set).toHaveBeenCalledTimes(1);
    first.resolve({ version: 1, panelPinned: true, diceMode: "manual" });
    await Promise.all([pinning, mode]);

    expect(set.mock.calls).toEqual([
      [true, "manual"],
      [true, "auto"],
    ]);
    expect(state.panelPinned.value).toBe(true);
    expect(state.diceMode.value).toBe("auto");
    scope.stop();
  });

  it("rolls back a rejected latest intent to the last confirmed values", async () => {
    const scope = effectScope();
    const state = scope.run(() =>
      useUiPreferences({
        get: async () => ({ version: 1, panelPinned: false, diceMode: "manual" }),
        set: async () => {
          throw { code: "store.io", message: "private detail" };
        },
      }),
    )!;
    await state.load();
    await state.setPanelPinned(true);

    expect(state.panelPinned.value).toBe(false);
    expect(state.saveError.value?.code).toBe("store.io");
    expect(state.saveError.value?.message).toBe("private detail");
    scope.stop();
  });
});

it("preserves pending dice intent across initial load and does not reload confirmed state", async () => {
  const get = vi.fn().mockResolvedValue({ version: 1, panelPinned: true, diceMode: "manual" });
  const set = vi.fn(async (panelPinned: boolean, diceMode: "manual" | "auto") => ({
    version: 1 as const,
    panelPinned,
    diceMode,
  }));
  const scope = effectScope();
  const state = scope.run(() => useUiPreferences({ get, set }))!;
  await state.setDiceMode("auto");
  await state.load();
  await state.load();
  expect(get).toHaveBeenCalledOnce();
  await vi.waitFor(() => expect(state.saving.value).toBe(false));
  expect(set).toHaveBeenCalledExactlyOnceWith(true, "auto");
  expect(state.diceMode.value).toBe("auto");
  scope.stop();
});

it("retries failed reads without writing against an unknown baseline", async () => {
  const get = vi
    .fn()
    .mockRejectedValueOnce(new Error("secret"))
    .mockResolvedValueOnce({ version: 1, panelPinned: true, diceMode: "auto" });
  const set = vi.fn();
  const scope = effectScope();
  const state = scope.run(() => useUiPreferences({ get, set }))!;
  await state.load();
  expect(state.loadError.value).toEqual({ code: "store.io", message: "偏好读取或保存失败" });
  await state.setDiceMode("auto");
  expect(set).not.toHaveBeenCalled();
  await state.load();
  expect(state.loadError.value).toBeUndefined();
  expect(state.ready.value).toBe(true);
  expect(state.panelPinned.value).toBe(true);
  scope.stop();
});

it("keeps newer changes queued when an earlier replacement fails", async () => {
  type Pref = { version: 1; panelPinned: boolean; diceMode: "manual" | "auto" };
  const first = deferred<Pref>();
  const set = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockImplementationOnce(async (panelPinned: boolean, diceMode: "manual" | "auto") => ({
      version: 1 as const,
      panelPinned,
      diceMode,
    }));
  const scope = effectScope();
  const state = scope.run(() =>
    useUiPreferences({
      get: async () => ({ version: 1, panelPinned: false, diceMode: "manual" }),
      set,
    }),
  )!;
  await state.load();
  const sending = state.setDiceMode("auto");
  const queued = state.setPanelPinned(true);
  first.reject({ code: "store.io", message: "retry" });
  await Promise.all([sending, queued]);
  expect(set.mock.calls).toEqual([
    [false, "auto"],
    [true, "manual"],
  ]);
  expect(state.panelPinned.value).toBe(true);
  expect(state.diceMode.value).toBe("manual");
  scope.stop();
});

it("does not roll back a newer dice choice when a prior pinning write fails", async () => {
  type Pref = { version: 1; panelPinned: boolean; diceMode: "manual" | "auto" };
  const first = deferred<Pref>();
  const set = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockImplementationOnce(async (panelPinned: boolean, diceMode: "manual" | "auto") => ({
      version: 1 as const,
      panelPinned,
      diceMode,
    }));
  const scope = effectScope();
  const state = scope.run(() =>
    useUiPreferences({
      get: async () => ({ version: 1, panelPinned: false, diceMode: "manual" }),
      set,
    }),
  )!;
  await state.load();
  const sending = state.setPanelPinned(true);
  const queued = state.setDiceMode("auto");
  first.reject(new Error("private"));
  await Promise.all([sending, queued]);
  expect(set.mock.calls).toEqual([
    [true, "manual"],
    [false, "auto"],
  ]);
  expect(state.panelPinned.value).toBe(false);
  expect(state.diceMode.value).toBe("auto");
  scope.stop();
});

it.each(["load", "save", "failure"])(
  "ignores late %s responses after disposal",
  async (operation) => {
    type Pref = { version: 1; panelPinned: boolean; diceMode: "manual" | "auto" };
    const pending = deferred<Pref>();
    const get = vi.fn().mockResolvedValue({ version: 1, panelPinned: false, diceMode: "manual" });
    if (operation === "load") get.mockReturnValue(pending.promise);
    const set = vi.fn().mockReturnValue(pending.promise);
    const scope = effectScope();
    const state = scope.run(() => useUiPreferences({ get, set }))!;
    let sending: Promise<void>;
    if (operation === "load") sending = state.load();
    else {
      await state.load();
      sending = state.setPanelPinned(true);
    }
    scope.stop();
    if (operation === "failure") pending.reject(new Error("secret"));
    else pending.resolve({ version: 1, panelPinned: true, diceMode: "auto" });
    await sending;
    await state.setPanelPinned(false);
    await state.setDiceMode("manual");
    await state.load();
    expect(state.loadError.value).toBeUndefined();
    expect(state.saveError.value).toBeUndefined();
    expect(state.diceMode.value).toBe("manual");
  },
);

it("ignores failed initial reads after disposal", async () => {
  const read = deferred<never>();
  const scope = effectScope();
  const state = scope.run(() => useUiPreferences({ get: () => read.promise, set: vi.fn() }))!;
  const loading = state.load();
  scope.stop();
  read.reject(new Error("private"));
  await loading;
  expect(state.loadError.value).toBeUndefined();
});
