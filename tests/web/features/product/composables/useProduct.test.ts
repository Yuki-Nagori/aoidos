import { effectScope } from "vue";
import { expect, it, vi } from "vitest";
import { useProduct } from "../../../../../src-web/features/product/composables/useProduct";
import type { LlmProfile } from "../../../../../src-web/api/llm";
import { getKeyStatus, listProfiles, saveProfile, setKey } from "../../../../../src-web/api/llm";
import { listScripts, openSession } from "../../../../../src-web/api/engine";

vi.mock("../../../../../src-web/api/llm", () => ({
  getKeyStatus: vi.fn(),
  listProfiles: vi.fn(),
  saveProfile: vi.fn(),
  setKey: vi.fn(),
}));
vi.mock("../../../../../src-web/api/engine", () => ({
  listScripts: vi.fn(),
  openSession: vi.fn(),
}));
const profile: LlmProfile = {
  profileId: "default",
  providerId: "deepseek",
  model: "deepseek-flash",
  mode: "completion",
  thinking: false,
  sampling: { temperature: 1, maxTokens: 2048 },
  proxy: { mode: "system" },
};
const scripts = [
  {
    scriptId: "mistbell",
    title: "雾钟地窖",
    displayNames: { en: "Mistbell Cellar", "zh-Hans": "雾钟地窖" },
    attributions: "notice",
  },
];
const opened = {
  sessionId: "session",
  memoryRunId: "memory-run",
  profileId: "default",
  scriptId: "mistbell",
  title: "雾钟地窖",
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (failure: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function transport() {
  return {
    profiles: vi.fn().mockResolvedValue({ items: [profile] }),
    keyStatus: vi.fn().mockResolvedValue({ set: true, hint: "1234" }),
    scripts: vi.fn().mockResolvedValue(scripts),
    save: vi.fn().mockImplementation(async (saved: LlmProfile) => saved),
    key: vi.fn().mockResolvedValue({ set: true, hint: "1234" }),
    open: vi.fn().mockResolvedValue(opened),
  };
}
it("defaults to Flash and exposes supported providers and model choices", async () => {
  vi.mocked(listProfiles).mockResolvedValue({ items: [profile] });
  vi.mocked(getKeyStatus).mockResolvedValue({ set: true, hint: "1234" });
  vi.mocked(listScripts).mockResolvedValue(scripts);
  vi.mocked(setKey).mockResolvedValue({ set: true, hint: null });
  vi.mocked(openSession).mockResolvedValue(opened);
  const scope = effectScope(),
    product = scope.run(() => useProduct())!;
  expect(product.model.value).toBe("deepseek-flash");
  expect(product.providerOptions).toEqual([
    { id: "deepseek", label: "DeepSeek" },
    { id: "opencode-zen", label: "OpenCode Zen" },
    { id: "opencode-go", label: "OpenCode Go" },
  ]);
  expect(product.models.value).toEqual([
    { id: "deepseek-flash", label: "DeepSeek V4.1 Flash" },
    { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro" },
  ]);
  expect(product).not.toHaveProperty("save");
  await product.load();
  expect(product.profileId.value).toBe("default");
  expect(product.scripts.value).toEqual(scripts);
  expect(openSession).not.toHaveBeenCalled();
  await product.configureKey();
  expect(setKey).toHaveBeenLastCalledWith("deepseek", "set");
  expect(product.credentialRevision.value).toBe(1);
  await product.open(false);
  expect(saveProfile).not.toHaveBeenCalled();
  expect(openSession).toHaveBeenLastCalledWith("default", "mistbell", false);
  expect(product.session.value).toEqual(opened);
  scope.stop();
});
it("switches models by reusing matching DeepSeek profiles and never picks another provider", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  const flash = { ...profile, profileId: "flash" },
    pro = {
      ...profile,
      profileId: "pro",
      model: "deepseek-v4-pro",
      mode: "chat" as const,
      thinking: true,
    };
  api.profiles.mockResolvedValue({
    items: [{ ...profile, profileId: "other", providerId: "other" }, flash, pro],
  });
  await product.load();
  expect(product.profileId.value).toBe("flash");
  product.model.value = "deepseek-v4-pro";
  await product.open(true);
  expect(api.open).toHaveBeenLastCalledWith("pro", "mistbell", true);
  expect(product.profileId.value).toBe("pro");
  product.model.value = "deepseek-flash";
  await product.open(false);
  expect(api.open).toHaveBeenLastCalledWith("flash", "mistbell", false);
  expect(product.profiles.value).toEqual([
    { ...profile, profileId: "other", providerId: "other" },
    flash,
    pro,
  ]);
  expect(api.save).not.toHaveBeenCalled();
  scope.stop();
});
it("preserves default sampling, proxy and mode when creating the missing model, retaining unrelated profiles", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  const previous: LlmProfile = {
    ...profile,
    model: "deepseek-v4-pro",
    mode: "chat",
    thinking: true,
    sampling: { temperature: 0.25, maxTokens: 4096 },
    proxy: { mode: "manual", url: "http://localhost:8080", authRef: "proxy-ref" },
  };
  const unrelated = { ...profile, profileId: "other", providerId: "other" };
  api.profiles.mockResolvedValue({ items: [unrelated, previous] });
  await product.load();
  expect(product.profileId.value).toBe("");
  await product.open(true);
  expect(api.save).toHaveBeenCalledExactlyOnceWith({
    ...previous,
    model: "deepseek-flash",
    thinking: false,
  });
  expect(product.profiles.value).toEqual([
    unrelated,
    { ...previous, model: "deepseek-flash", thinking: false },
  ]);
  expect(product.profileId.value).toBe("default");
  scope.stop();
});
it("creates a completion profile automatically for an empty installation and permits native key setup first", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  api.profiles.mockResolvedValue({ items: [] });
  await product.load();
  await product.configureKey();
  expect(api.key).toHaveBeenCalledExactlyOnceWith("deepseek", "set");
  expect(product.keyStatus.value).toEqual({ set: true, hint: "1234" });
  expect(api.save).not.toHaveBeenCalled();
  await product.open(false);
  expect(api.save).toHaveBeenCalledExactlyOnceWith(profile);
  expect(api.open).toHaveBeenCalledExactlyOnceWith("default", "mistbell", false);
  scope.stop();
});
it("selects only a saved profile matching the newly selected provider and default model", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  const matching: LlmProfile = {
    ...profile,
    profileId: "go-flash",
    providerId: "opencode-go",
    model: "deepseek-v4.1-flash",
    mode: "chat",
  };
  api.profiles.mockResolvedValue({
    items: [
      { ...matching, profileId: "other-provider", providerId: "other" },
      { ...matching, profileId: "other-model", model: "gpt-6-luna" },
      matching,
    ],
  });
  await product.load();
  expect(product.profileId.value).toBe("");

  product.providerId.value = "opencode-go";

  expect(product.model.value).toBe("deepseek-v4.1-flash");
  expect(product.profileId.value).toBe("go-flash");
  scope.stop();
});
it("selects OpenCode Go independently with its chat model, credential and profile", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  api.profiles.mockResolvedValue({ items: [] });
  await product.load();
  product.providerId.value = "opencode-go";
  expect(product.models.value).toHaveLength(32);
  expect(product.models.value[0]).toEqual({
    id: "deepseek-v4.1-flash",
    label: "DeepSeek V4.1 Flash",
  });
  expect(product.model.value).toBe("deepseek-v4.1-flash");
  await product.configureKey();
  expect(api.key).toHaveBeenLastCalledWith("opencode-go", "set");
  await product.open(false);
  expect(api.save).toHaveBeenLastCalledWith(
    expect.objectContaining({
      providerId: "opencode-go",
      profileId: "default-opencode-go",
      model: "deepseek-v4.1-flash",
      mode: "chat",
      thinking: false,
    }),
  );
  scope.stop();
});
it("does not let an older provider key-status response replace the current provider", async () => {
  const older = deferred<{ set: boolean; hint: string | null }>();
  const current = deferred<{ set: boolean; hint: string | null }>();
  const api = transport();
  api.keyStatus.mockResolvedValueOnce({ set: false, hint: null });
  const scope = effectScope();
  const product = scope.run(() => useProduct(api))!;
  await product.load();
  api.keyStatus.mockImplementation((provider) =>
    provider === "opencode-go" ? older.promise : current.promise,
  );

  product.providerId.value = "opencode-go";
  product.providerId.value = "opencode-zen";
  current.resolve({ set: false, hint: null });
  await vi.waitFor(() => expect(product.keyStatus.value).toEqual({ set: false, hint: null }));
  older.resolve({ set: true, hint: "1234" });
  await vi.waitFor(() => expect(api.keyStatus).toHaveBeenCalledTimes(3));
  expect(product.keyStatus.value).toEqual({ set: false, hint: null });
  scope.stop();
});
it("exposes the documented OpenCode Zen catalog with DeepSeek V4.1 Flash first", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  api.profiles.mockResolvedValue({ items: [] });
  await product.load();
  product.providerId.value = "opencode-zen";
  expect(product.models.value).toHaveLength(86);
  expect(product.models.value[0]).toEqual({
    id: "deepseek-v4.1-flash",
    label: "DeepSeek V4.1 Flash",
  });
  expect(product.models.value.some((model) => model.id === "gpt-6.1-sol")).toBe(true);
  expect(product.models.value.some((model) => model.id === "gemini-3.8-flash")).toBe(true);
  expect(product.model.value).toBe("deepseek-v4.1-flash");
  scope.stop();
});
it("does not inherit the default of another provider or a differently named DeepSeek profile", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  api.profiles.mockResolvedValue({
    items: [
      { ...profile, providerId: "other" },
      {
        ...profile,
        profileId: "custom",
        model: "deepseek-v4-pro",
        mode: "chat",
        sampling: { temperature: 0, maxTokens: 10 },
      },
    ],
  });
  await product.load();
  await product.open(false);
  expect(api.save).toHaveBeenCalledExactlyOnceWith(profile);
  scope.stop();
});
it("does not open after a failed profile save and retries with a clean error state", async () => {
  const api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  api.save.mockRejectedValueOnce(new Error("private diagnostic"));
  await product.open(true);
  expect(product.error.value?.message).not.toContain("private");
  expect(product.busy.value).toBe(false);
  expect(api.open).not.toHaveBeenCalled();
  expect(product.profiles.value).toEqual([]);
  await product.open(true);
  expect(product.error.value).toBeUndefined();
  expect(product.session.value).toEqual(opened);
  scope.stop();
});
it("serializes loading, key dialogs and opening, preserving a cancelled native key result", async () => {
  const loading = deferred<{ items: LlmProfile[] }>(),
    key = deferred<{ set: boolean; hint: string | null }>(),
    api = transport(),
    scope = effectScope();
  api.profiles.mockReturnValue(loading.promise);
  const product = scope.run(() => useProduct(api))!;
  const load = product.load();
  await product.open(false);
  await product.configureKey();
  expect(api.save).not.toHaveBeenCalled();
  expect(api.key).not.toHaveBeenCalled();
  loading.reject(new Error("private path"));
  await load;
  api.profiles.mockResolvedValue({ items: [profile] });
  await product.load();
  expect(product.error.value).toBeUndefined();
  api.key.mockReturnValue(key.promise);
  const configuring = product.configureKey();
  await product.configureKey();
  await product.open(true);
  expect(api.key).toHaveBeenCalledTimes(1);
  expect(api.open).not.toHaveBeenCalled();
  key.resolve({ set: false, hint: null });
  await configuring;
  expect(product.keyStatus.value).toEqual({ set: false, hint: null });
  scope.stop();
  await product.load();
  expect(api.profiles).toHaveBeenCalledTimes(2);
});
it.each(["load", "save", "key", "open"] as const)(
  "ignores successful %s results after disposal",
  async (operation) => {
    const pending = deferred<unknown>(),
      api = transport(),
      scope = effectScope(),
      product = scope.run(() => useProduct(api))!;
    await product.load();
    if (operation === "load") api.profiles.mockReturnValue(pending.promise);
    else if (operation === "save") {
      product.model.value = "deepseek-v4-pro";
      api.save.mockReturnValue(pending.promise);
    } else if (operation === "key") api.key.mockReturnValue(pending.promise);
    else api.open.mockReturnValue(pending.promise);
    const running =
      operation === "load"
        ? product.load()
        : operation === "key"
          ? product.configureKey()
          : product.open(false);
    // open 先异步选择配置，再发会话命令；卸载发生在真实会话请求已发出之后。
    if (operation === "open") await vi.waitFor(() => expect(api.open).toHaveBeenCalledTimes(1));
    scope.stop();
    const result =
      operation === "load"
        ? { items: [] }
        : operation === "save"
          ? { ...profile, model: "deepseek-v4-pro" }
          : operation === "key"
            ? { set: true, hint: "1234" }
            : opened;
    pending.resolve(result);
    await running;
    expect(product.profiles.value).toEqual([profile]);
    expect(product.profileId.value).toBe("default");
    expect(product.keyStatus.value).toEqual({ set: true, hint: "1234" });
    expect(product.session.value).toBeUndefined();
    if (operation === "save") expect(api.open).not.toHaveBeenCalled();
  },
);
it("ignores rejected operations after disposal", async () => {
  const pending = deferred<unknown>(),
    api = transport(),
    scope = effectScope(),
    product = scope.run(() => useProduct(api))!;
  api.save.mockReturnValue(pending.promise);
  const opening = product.open(false);
  scope.stop();
  pending.reject(new Error("late"));
  await opening;
  expect(product.error.value).toBeUndefined();
});
