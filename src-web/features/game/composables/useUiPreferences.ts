import { onScopeDispose, readonly, ref, shallowRef } from "vue";
import type { UiPreferences } from "../../../api/store";
import type { DiceSkinId } from "../utils/dice-visual";
import { getUiPreferences, setUiPreferences } from "../../../api/store";
import { safeError } from "../../../shared/utils/error";

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
  const diceSkin = ref<UiPreferences["diceSkin"]>("auto");
  const ready = ref(false);
  const saving = ref(false);
  const loadError = shallowRef<PreferenceError>();
  const saveError = shallowRef<PreferenceError>();
  let confirmed: UiPreferences = {
    version: 1,
    panelPinned: false,
    diceMode: "manual",
    diceSkin: "auto",
  };
  const desired = {
    panelPinned: false,
    diceMode: "manual" as UiPreferences["diceMode"],
    diceSkin: "auto" as UiPreferences["diceSkin"],
  };
  let dirtyPanel = false;
  let dirtyDice = false;
  let dirtySkin = false;
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
        (desired.panelPinned !== confirmed.panelPinned ||
          desired.diceMode !== confirmed.diceMode ||
          desired.diceSkin !== confirmed.diceSkin)
      ) {
        const submitted = { ...desired };
        try {
          const saved = await transport.set(
            submitted.panelPinned,
            submitted.diceMode,
            submitted.diceSkin,
          );
          if (disposed) return;
          confirmed = saved;
          if (desired.panelPinned === submitted.panelPinned && dirtyPanel)
            desired.panelPinned = saved.panelPinned;
          if (desired.diceMode === submitted.diceMode && dirtyDice)
            desired.diceMode = saved.diceMode;
          if (desired.diceSkin === submitted.diceSkin && dirtySkin)
            desired.diceSkin = saved.diceSkin;
          panelPinned.value = desired.panelPinned;
          diceMode.value = desired.diceMode;
          diceSkin.value = desired.diceSkin;
        } catch (failure) {
          if (disposed) return;
          const error = safeError(failure, "偏好读取或保存失败", "store.io");
          if (desired.panelPinned === submitted.panelPinned) {
            desired.panelPinned = confirmed.panelPinned;
            panelPinned.value = confirmed.panelPinned;
          }
          if (desired.diceMode === submitted.diceMode) {
            desired.diceMode = confirmed.diceMode;
            diceMode.value = confirmed.diceMode;
          }
          if (desired.diceSkin === submitted.diceSkin) {
            desired.diceSkin = confirmed.diceSkin;
            diceSkin.value = confirmed.diceSkin;
          }
          if (
            desired.panelPinned === confirmed.panelPinned &&
            desired.diceMode === confirmed.diceMode &&
            desired.diceSkin === confirmed.diceSkin
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
      if (!dirtySkin) desired.diceSkin = saved.diceSkin;
      panelPinned.value = desired.panelPinned;
      diceMode.value = desired.diceMode;
      diceSkin.value = desired.diceSkin;
      loadError.value = undefined;
      ready.value = true;
      if (dirtyPanel || dirtyDice || dirtySkin) void drain();
    } catch (failure) {
      if (!disposed) loadError.value = safeError(failure, "偏好读取或保存失败", "store.io");
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

  function setDiceSkin(value: DiceSkinId): Promise<void> {
    if (disposed) return Promise.resolve();
    dirtySkin = true;
    desired.diceSkin = value;
    diceSkin.value = value;
    return drain();
  }

  return {
    panelPinned: readonly(panelPinned),
    diceMode: readonly(diceMode),
    diceSkin: readonly(diceSkin),
    ready: readonly(ready),
    saving: readonly(saving),
    loadError: readonly(loadError),
    saveError: readonly(saveError),
    load,
    setPanelPinned,
    setDiceMode,
    setDiceSkin,
  };
}
