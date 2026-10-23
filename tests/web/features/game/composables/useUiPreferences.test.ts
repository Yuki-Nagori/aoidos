import { effectScope } from "vue";
import { describe, expect, it, vi } from "vitest";
import { useUiPreferences } from "../../../../../src-web/features/game/composables/useUiPreferences";
import type { UiPreferences } from "../../../../../src-web/api/store";

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
    const read = deferred<UiPreferences>();
    const set = vi.fn(
      async (
        panelPinned: boolean,
        diceMode: "manual" | "auto",
        diceSkin: UiPreferences["diceSkin"] = "auto",
      ) => ({
        version: 1 as const,
        panelPinned,
        diceMode,
        diceSkin,
      }),
    );
    const scope = effectScope();
    const state = scope.run(() => useUiPreferences({ get: () => read.promise, set }))!;

    const loading = state.load();
    await state.setPanelPinned(true);
    read.resolve({ version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" });
    await loading;

    expect(state.panelPinned.value).toBe(true);
    expect(set).toHaveBeenCalledWith(true, "manual", "auto");
    scope.stop();
  });

  it("serializes full replacements and keeps the latest intent", async () => {
    const first = deferred<UiPreferences>();
    const set = vi
      .fn()
      .mockReturnValueOnce(first.promise)
      .mockImplementationOnce(
        async (
          panelPinned: boolean,
          diceMode: "manual" | "auto",
          diceSkin: UiPreferences["diceSkin"] = "auto",
        ) => ({
          version: 1 as const,
          panelPinned,
          diceMode,
          diceSkin,
        }),
      );
    const scope = effectScope();
    const state = scope.run(() =>
      useUiPreferences({
        get: async () => ({ version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" }),
        set,
      }),
    )!;
    await state.load();

    const pinning = state.setPanelPinned(true);
    await Promise.resolve();
    const mode = state.setDiceMode("auto");
    expect(set).toHaveBeenCalledTimes(1);
    first.resolve({ version: 1, panelPinned: true, diceMode: "manual", diceSkin: "auto" });
    await Promise.all([pinning, mode]);

    expect(set.mock.calls).toEqual([
      [true, "manual", "auto"],
      [true, "auto", "auto"],
    ]);
    expect(state.panelPinned.value).toBe(true);
    expect(state.diceMode.value).toBe("auto");
    scope.stop();
  });

  it("rolls back a rejected latest intent to the last confirmed values", async () => {
    const scope = effectScope();
    const state = scope.run(() =>
      useUiPreferences({
        get: async () => ({ version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" }),
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
  const get = vi
    .fn()
    .mockResolvedValue({ version: 1, panelPinned: true, diceMode: "manual", diceSkin: "auto" });
  const set = vi.fn(
    async (
      panelPinned: boolean,
      diceMode: "manual" | "auto",
      diceSkin: UiPreferences["diceSkin"] = "auto",
    ) => ({
      version: 1 as const,
      panelPinned,
      diceMode,
      diceSkin,
    }),
  );
  const scope = effectScope();
  const state = scope.run(() => useUiPreferences({ get, set }))!;
  await state.setDiceMode("auto");
  await state.load();
  await state.load();
  expect(get).toHaveBeenCalledOnce();
  await vi.waitFor(() => expect(state.saving.value).toBe(false));
  expect(set).toHaveBeenCalledExactlyOnceWith(true, "auto", "auto");
  expect(state.diceMode.value).toBe("auto");
  scope.stop();
});

it("retries failed reads without writing against an unknown baseline", async () => {
  const get = vi
    .fn()
    .mockRejectedValueOnce(new Error("secret"))
    .mockResolvedValueOnce({ version: 1, panelPinned: true, diceMode: "auto", diceSkin: "auto" });
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
  type Pref = UiPreferences;
  const first = deferred<Pref>();
  const set = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockImplementationOnce(
      async (
        panelPinned: boolean,
        diceMode: "manual" | "auto",
        diceSkin: UiPreferences["diceSkin"],
      ) => ({
        version: 1 as const,
        panelPinned,
        diceMode,
        diceSkin,
      }),
    );
  const scope = effectScope();
  const state = scope.run(() =>
    useUiPreferences({
      get: async () => ({ version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" }),
      set,
    }),
  )!;
  await state.load();
  const sending = state.setDiceMode("auto");
  const queued = state.setPanelPinned(true);
  first.reject({ code: "store.io", message: "retry" });
  await Promise.all([sending, queued]);
  expect(set.mock.calls).toEqual([
    [false, "auto", "auto"],
    [true, "manual", "auto"],
  ]);
  expect(state.panelPinned.value).toBe(true);
  expect(state.diceMode.value).toBe("manual");
  scope.stop();
});

it("does not roll back a newer dice choice when a prior pinning write fails", async () => {
  type Pref = UiPreferences;
  const first = deferred<Pref>();
  const set = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockImplementationOnce(
      async (
        panelPinned: boolean,
        diceMode: "manual" | "auto",
        diceSkin: UiPreferences["diceSkin"],
      ) => ({
        version: 1 as const,
        panelPinned,
        diceMode,
        diceSkin,
      }),
    );
  const scope = effectScope();
  const state = scope.run(() =>
    useUiPreferences({
      get: async () => ({ version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" }),
      set,
    }),
  )!;
  await state.load();
  const sending = state.setPanelPinned(true);
  const queued = state.setDiceMode("auto");
  first.reject(new Error("private"));
  await Promise.all([sending, queued]);
  expect(set.mock.calls).toEqual([
    [true, "manual", "auto"],
    [false, "auto", "auto"],
  ]);
  expect(state.panelPinned.value).toBe(false);
  expect(state.diceMode.value).toBe("auto");
  scope.stop();
});

it.each(["load", "save", "failure"])(
  "ignores late %s responses after disposal",
  async (operation) => {
    type Pref = UiPreferences;
    const pending = deferred<Pref>();
    const get = vi
      .fn()
      .mockResolvedValue({ version: 1, panelPinned: false, diceMode: "manual", diceSkin: "auto" });
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
    else pending.resolve({ version: 1, panelPinned: true, diceMode: "auto", diceSkin: "auto" });
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
