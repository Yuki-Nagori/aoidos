import { onScopeDispose, ref, shallowRef, toValue, type MaybeRefOrGetter } from "vue";
import { submitInput } from "../api/engine";
import { turnRecoveryError } from "../utils/turn-recovery";

/** 输入原文在接纳后才清空；IME 组合输入、重复提交和卸载迟到结果不改草稿。 */
export function useGameInput(
  sessionId: MaybeRefOrGetter<string | undefined>,
  submit = submitInput,
) {
  const draft = ref("");
  const composing = ref(false);
  const busy = ref(false);
  const error = shallowRef<{ code: string; message: string }>();
  let disposed = false;
  onScopeDispose(() => {
    disposed = true;
  });
  async function send(): Promise<void> {
    const id = toValue(sessionId),
      text = draft.value;
    if (!id || !text.trim() || composing.value || busy.value || disposed) return;
    busy.value = true;
    error.value = undefined;
    try {
      await submit(id, text);
      if (!disposed && id === toValue(sessionId) && draft.value === text) draft.value = "";
    } catch (failure) {
      if (!disposed && id === toValue(sessionId)) error.value = turnRecoveryError(failure);
    } finally {
      if (!disposed) busy.value = false;
    }
  }
  return { draft, composing, busy, error, send };
}
