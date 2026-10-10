import {
  onScopeDispose,
  computed,
  ref,
  shallowRef,
  toValue,
  watch,
  type MaybeRefOrGetter,
} from "vue";
import { submitInput } from "../api/engine";
import { turnRecoveryError } from "../utils/turn-recovery";

const MAX_DRAFT_SESSIONS = 5;
const MAX_DRAFT_BYTES = 32 * 1024;
interface DraftTicket {
  sessionId?: string;
  generation: number;
  text: string;
}

/** 输入原文在接纳后才清空；IME 组合输入、重复提交和卸载迟到结果不改草稿。 */
export function useGameInput(
  sessionId: MaybeRefOrGetter<string | undefined>,
  submit = submitInput,
) {
  const draft = ref("");
  const composing = ref(false);
  const busy = ref(false);
  const error = shallowRef<{ code: string; message: string }>();
  const draftBytes = computed(() => new TextEncoder().encode(draft.value).byteLength);
  const tooLarge = computed(() => draftBytes.value > MAX_DRAFT_BYTES);
  let disposed = false;
  let generation = 0;
  const drafts = new Map<string, string>();
  function remember(id: string, text: string): void {
    drafts.delete(id);
    if (new TextEncoder().encode(text).length <= MAX_DRAFT_BYTES) drafts.set(id, text);
    while (drafts.size > MAX_DRAFT_SESSIONS) drafts.delete(drafts.keys().next().value!);
  }
  watch(
    () => toValue(sessionId),
    (id, previous) => {
      generation++;
      busy.value = false;
      error.value = undefined;
      if (previous) remember(previous, draft.value);
      draft.value = id ? (drafts.get(id) ?? "") : "";
      if (id && draft.value) remember(id, draft.value);
    },
    { immediate: true, flush: "sync" },
  );
  watch(
    draft,
    (text) => {
      const id = toValue(sessionId);
      if (id) remember(id, text);
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    disposed = true;
    generation++;
  });
  async function send(): Promise<void> {
    const id = toValue(sessionId),
      text = draft.value,
      token = generation;
    if (!id || !text.trim() || tooLarge.value || composing.value || busy.value || disposed) return;
    busy.value = true;
    error.value = undefined;
    try {
      await submit(id, text);
      if (!disposed && token === generation && id === toValue(sessionId) && draft.value === text)
        draft.value = "";
    } catch (failure) {
      if (!disposed && token === generation && id === toValue(sessionId))
        error.value = turnRecoveryError(failure);
    } finally {
      if (!disposed && token === generation) busy.value = false;
    }
  }
  function captureDraft(text = draft.value): DraftTicket {
    return { sessionId: toValue(sessionId), generation, text };
  }
  function clearDraftIfCurrent(ticket: DraftTicket): void {
    if (
      ticket.sessionId &&
      !disposed &&
      ticket.generation === generation &&
      ticket.sessionId === toValue(sessionId) &&
      draft.value === ticket.text
    )
      draft.value = "";
  }
  return {
    draft,
    draftBytes,
    tooLarge,
    composing,
    busy,
    error,
    send,
    captureDraft,
    clearDraftIfCurrent,
  };
}
