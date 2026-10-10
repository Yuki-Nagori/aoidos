import { onScopeDispose, readonly, ref, shallowRef } from "vue";
import { getUiPreferences, setUiPreferences, type UiPreferences } from "../api/store";

interface Transport {
  get: typeof getUiPreferences;
  set: typeof setUiPreferences;
}
const defaults: Transport = { get: getUiPreferences, set: setUiPreferences };
type PreferenceError = { code: string; message: string };

/** 串行写入完整偏好对象；较旧确认不会覆盖随后产生的用户意图。 */
export function useUiPreferences(transport: Transport = defaults) {
  const panelPinned = ref(false);
  const diceMode = ref<UiPreferences["diceMode"]>("manual");
  const ready = ref(false);
  const saving = ref(false);
  const loadError = shallowRef<PreferenceError>();
  const saveError = shallowRef<PreferenceError>();
  let confirmed: UiPreferences = { version: 1, panelPinned: false, diceMode: "manual" };
  const desired = { panelPinned: false, diceMode: "manual" as UiPreferences["diceMode"] };
  let dirtyPanel = false;
  let dirtyDice = false;
  let writing: Promise<void> | undefined;
  let disposed = false;
  onScopeDispose(() => {
    disposed = true;
  });

  async function drain(): Promise<void> {
    if (writing) return writing;
    const operation = (async () => {
      if (!ready.value || loadError.value || disposed) return;
      saving.value = true;
      saveError.value = undefined;
      while (
        !disposed &&
        (desired.panelPinned !== confirmed.panelPinned || desired.diceMode !== confirmed.diceMode)
      ) {
        const submitted = { ...desired };
        try {
          const saved = await transport.set(submitted.panelPinned, submitted.diceMode);
          if (disposed) return;
          confirmed = saved;
          if (desired.panelPinned === submitted.panelPinned && dirtyPanel)
            desired.panelPinned = saved.panelPinned;
          if (desired.diceMode === submitted.diceMode && dirtyDice)
            desired.diceMode = saved.diceMode;
          panelPinned.value = desired.panelPinned;
          diceMode.value = desired.diceMode;
        } catch (failure) {
          if (disposed) return;
          const error = safeError(failure);
          if (desired.panelPinned === submitted.panelPinned) {
            desired.panelPinned = confirmed.panelPinned;
            panelPinned.value = confirmed.panelPinned;
          }
          if (desired.diceMode === submitted.diceMode) {
            desired.diceMode = confirmed.diceMode;
            diceMode.value = confirmed.diceMode;
          }
          if (
            desired.panelPinned === confirmed.panelPinned &&
            desired.diceMode === confirmed.diceMode
          ) {
            saveError.value = error;
            break;
          }
          saveError.value = error;
        }
      }
    })().finally(() => {
      if (!disposed) saving.value = false;
      writing = undefined;
    });
    writing = operation;
    return operation;
  }

  async function load(): Promise<void> {
    if (ready.value || disposed) return;
    try {
      const saved = await transport.get();
      if (disposed) return;
      confirmed = saved;
      if (!dirtyPanel) desired.panelPinned = saved.panelPinned;
      if (!dirtyDice) desired.diceMode = saved.diceMode;
      panelPinned.value = desired.panelPinned;
      diceMode.value = desired.diceMode;
      loadError.value = undefined;
      ready.value = true;
      if (dirtyPanel || dirtyDice) void drain();
    } catch (failure) {
      if (!disposed) loadError.value = safeError(failure);
    }
  }

  function setPanelPinned(value: boolean): Promise<void> {
    if (disposed) return Promise.resolve();
    dirtyPanel = true;
    desired.panelPinned = value;
    panelPinned.value = value;
    return drain();
  }

  function setDiceMode(value: UiPreferences["diceMode"]): Promise<void> {
    if (disposed) return Promise.resolve();
    dirtyDice = true;
    desired.diceMode = value;
    diceMode.value = value;
    return drain();
  }

  return {
    panelPinned: readonly(panelPinned),
    diceMode: readonly(diceMode),
    ready: readonly(ready),
    saving: readonly(saving),
    loadError: readonly(loadError),
    saveError: readonly(saveError),
    load,
    setPanelPinned,
    setDiceMode,
  };
}

function safeError(value: unknown): PreferenceError {
  if (
    value &&
    typeof value === "object" &&
    "code" in value &&
    "message" in value &&
    typeof value.code === "string" &&
    typeof value.message === "string"
  )
    return { code: value.code, message: value.message };
  return { code: "store.io", message: "偏好读取或保存失败" };
}
