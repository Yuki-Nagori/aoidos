import { onScopeDispose, ref, shallowRef } from "vue";
import { listProfiles, saveProfile, setKey, type LlmProfile, type KeyStatus } from "../api/llm";
import { listScripts, openSession, type OpenedSession, type ScriptInfo } from "../api/engine";
import { turnRecoveryError } from "../utils/turn-recovery";

interface Transport {
  profiles: typeof listProfiles;
  scripts: typeof listScripts;
  save: typeof saveProfile;
  key: typeof setKey;
  open: typeof openSession;
}
const defaults: Transport = {
  profiles: listProfiles,
  scripts: listScripts,
  save: saveProfile,
  key: setKey,
  open: openSession,
};
/** 最小产品入口只登记配置及会话；异步操作不因卸载更新状态，不隐式生成或续费。 */
export function useProduct(transport: Transport = defaults) {
  const profiles = shallowRef<LlmProfile[]>([]);
  const scripts = shallowRef<ScriptInfo[]>([]);
  const profileId = ref("");
  const scriptId = ref("mistbell");
  const models = [
    { id: "deepseek-flash", label: "DeepSeek V4.1 Flash" },
    { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro" },
  ];
  const model = ref("deepseek-flash");
  const session = shallowRef<OpenedSession>();
  const keyStatus = shallowRef<KeyStatus>();
  const busy = ref(false);
  const error = shallowRef<{ code: string; message: string }>();
  let disposed = false;
  onScopeDispose(() => {
    disposed = true;
  });
  async function run(operation: () => Promise<void>): Promise<void> {
    if (busy.value || disposed) return;
    busy.value = true;
    error.value = undefined;
    try {
      await operation();
    } catch (failure) {
      if (!disposed) error.value = turnRecoveryError(failure);
    } finally {
      if (!disposed) busy.value = false;
    }
  }
  function load(): Promise<void> {
    return run(async () => {
      const [saved, bundled] = await Promise.all([transport.profiles(), transport.scripts()]);
      if (disposed) return;
      profiles.value = saved.items;
      scripts.value = bundled;
      profileId.value =
        saved.items.find(
          (profile) => profile.providerId === "deepseek" && profile.model === model.value,
        )?.profileId ?? "";
    });
  }
  async function selectProfile(): Promise<string> {
    const existing = profiles.value.find(
      (profile) => profile.providerId === "deepseek" && profile.model === model.value,
    );
    if (existing) return existing.profileId;
    const previous = profiles.value.find(
      (profile) => profile.providerId === "deepseek" && profile.profileId === "default",
    );
    const saved = await transport.save({
      profileId: "default",
      providerId: "deepseek",
      model: model.value,
      mode: previous?.mode ?? "completion",
      thinking: false,
      sampling: previous?.sampling ?? { temperature: 1, maxTokens: 2048 },
      proxy: previous?.proxy ?? { mode: "system" },
    });
    if (!disposed)
      profiles.value = [
        ...profiles.value.filter((profile) => profile.profileId !== saved.profileId),
        saved,
      ];
    return saved.profileId;
  }
  function configureKey(): Promise<void> {
    return run(async () => {
      const status = await transport.key("deepseek", "set");
      if (!disposed) keyStatus.value = status;
    });
  }
  function open(startNew: boolean): Promise<void> {
    return run(async () => {
      const selected = await selectProfile();
      if (disposed) return;
      const opened = await transport.open(selected, scriptId.value, startNew);
      if (!disposed) {
        profileId.value = selected;
        session.value = opened;
      }
    });
  }
  return {
    profiles,
    scripts,
    profileId,
    scriptId,
    model,
    session,
    keyStatus,
    busy,
    error,
    load,
    models,
    configureKey,
    open,
  };
}
