import {
  onScopeDispose,
  ref,
  shallowRef,
  toValue,
  type DeepReadonly,
  type MaybeRefOrGetter,
} from "vue";
import {
  cancelRound,
  interruptRound,
  regenerate,
  resume,
  rewind,
  submitCheck,
  type PhaseSnapshot,
} from "../api/engine";
import { turnRecoveryError } from "../utils/turn-recovery";
const defaults = { cancelRound, interruptRound, regenerate, resume, rewind, submitCheck };
/** 控制操作使用当时确认的身份；阶段身份变化或卸载后不展示旧响应的错误。 */
export function useGameControls(
  snapshot: MaybeRefOrGetter<DeepReadonly<PhaseSnapshot> | undefined>,
  transport = defaults,
) {
  const busy = ref(false);
  const error = shallowRef<{ code: string; message: string }>();
  let disposed = false;
  onScopeDispose(() => {
    disposed = true;
  });
  async function control(
    action: "cancel" | "interrupt" | "resume" | "check" | "regenerate" | "rewind",
    argument?: string | number,
  ): Promise<boolean> {
    const state = toValue(snapshot);
    if (!state || busy.value || disposed) return false;
    busy.value = true;
    error.value = undefined;
    try {
      if (action === "resume") await transport.resume(state.sessionId);
      else if (action === "cancel" && state.inFlight?.roundId)
        await transport.cancelRound(state.sessionId, state.inFlight.roundId);
      else if (action === "interrupt" && state.inFlight?.roundId && typeof argument === "string")
        await transport.interruptRound(state.sessionId, state.inFlight.roundId, argument);
      else if (action === "regenerate") {
        const roundId = state.inFlight?.roundId ?? state.lastOperation?.roundId;
        if (!roundId) return false;
        await transport.regenerate(state.sessionId, roundId);
      } else if (action === "rewind" && typeof argument === "number")
        await transport.rewind(state.sessionId, argument);
      else if (action === "check" && state.inFlight?.roundId && state.check)
        await transport.submitCheck(state.sessionId, state.inFlight.roundId, state.check.planId);
      else return false;
      return true;
    } catch (failure) {
      const current = toValue(snapshot);
      if (
        !disposed &&
        current &&
        state.sessionId === current.sessionId &&
        state.stateEpoch === current.stateEpoch &&
        state.phaseRevision === current.phaseRevision
      )
        error.value = turnRecoveryError(failure);
      return false;
    } finally {
      if (!disposed) busy.value = false;
    }
  }
  return { busy, error, control };
}
