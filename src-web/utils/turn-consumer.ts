import {
  type TurnChunk,
  type TurnDone,
  type TurnEnvelope,
  type TurnFailed,
  type TurnSnapshot,
} from "../api/llm";

/** 三种事件独立计数，终态同时携带最后确认的正文基线。 */
export type TurnNotification =
  | { kind: "chunk"; envelope: TurnEnvelope<TurnChunk> }
  | { kind: "done"; envelope: TurnEnvelope<TurnDone> }
  | { kind: "failed"; envelope: TurnEnvelope<TurnFailed> };

const encoder = new TextEncoder();
const maxTextBytes = 256 * 1024;
const maxChunkBytes = 8 * 1024;

/** 不可信或超限事件不得进入有界缓存；正文容量按 Rust 相同的 UTF-8 字节口径。 */
export function validTurnEvent(event: TurnNotification): boolean {
  const { seq } = event.envelope;
  if (!Number.isSafeInteger(seq) || seq <= 0) return false;
  if (event.kind === "chunk") {
    const delta = event.envelope.data.delta;
    return (
      delta.length > 0 &&
      delta.length <= maxChunkBytes &&
      encoder.encode(delta).length <= maxChunkBytes
    );
  }
  const { chunkSeq } = event.envelope.data;
  return Number.isSafeInteger(chunkSeq) && chunkSeq >= 0;
}

/** 消费结果保留上一确认副本；需要恢复时不能乐观追加缺口后的正文。 */
export interface TurnConsumption {
  snapshot: TurnSnapshot;
  needsRecovery: boolean;
}

/** 快照必须保持全部确认基线和已提交前文，不能把终态降回进行中。 */
export function consumeTurnSnapshot(
  current: TurnSnapshot | undefined,
  candidate: TurnSnapshot,
): TurnConsumption | undefined {
  const seq = candidate.seq;
  const validSeq = [seq.chunk, seq.done, seq.failed].every(
    (value) => Number.isSafeInteger(value) && value >= 0,
  );
  const validTerminal =
    candidate.outcome === undefined
      ? candidate.finishReason === undefined && candidate.error === undefined
      : candidate.outcome === "cancelled"
        ? candidate.finishReason === undefined && candidate.error === undefined
        : candidate.outcome === "completed"
          ? candidate.finishReason !== undefined && candidate.error === undefined
          : candidate.error !== undefined &&
            (candidate.error.code !== "llm.empty-output" || candidate.finishReason !== undefined);
  if (
    !validSeq ||
    !validTerminal ||
    candidate.text.length > maxTextBytes ||
    encoder.encode(candidate.text).length > maxTextBytes
  )
    return undefined;
  if (
    current &&
    (candidate.turnId !== current.turnId ||
      !coversSequences(candidate.seq, current.seq) ||
      !candidate.text.startsWith(current.text) ||
      (current.outcome !== undefined &&
        (candidate.outcome !== current.outcome ||
          candidate.finishReason !== current.finishReason ||
          candidate.error?.code !== current.error?.code ||
          candidate.error?.message !== current.error?.message)))
  )
    return undefined;
  return { snapshot: { ...candidate, seq: { ...candidate.seq } }, needsRecovery: false };
}

/** 连续正文才追加；终态正文不完整、超限或协议不一致时先取快照。 */
export function consumeTurnEvent(snapshot: TurnSnapshot, event: TurnNotification): TurnConsumption {
  const { seq, data } = event.envelope;
  if (data.turnId !== snapshot.turnId) return { snapshot, needsRecovery: false };
  if (!validTurnEvent(event)) return { snapshot, needsRecovery: true };
  if (seq <= snapshot.seq[event.kind]) return { snapshot, needsRecovery: false };
  if (snapshot.outcome !== undefined || seq !== snapshot.seq[event.kind] + 1)
    return { snapshot, needsRecovery: true };
  const confirmed = { ...snapshot.seq, [event.kind]: seq };
  if (event.kind === "chunk") {
    const text = snapshot.text + event.envelope.data.delta;
    if (encoder.encode(text).length > maxTextBytes) return { snapshot, needsRecovery: true };
    return { snapshot: { ...snapshot, text, seq: confirmed }, needsRecovery: false };
  }
  if (event.envelope.data.chunkSeq !== snapshot.seq.chunk) return { snapshot, needsRecovery: true };
  const terminal: TurnSnapshot =
    event.kind === "done"
      ? {
          ...snapshot,
          seq: confirmed,
          outcome: event.envelope.data.outcome,
          finishReason:
            event.envelope.data.outcome === "completed"
              ? event.envelope.data.finishReason
              : undefined,
          error: undefined,
        }
      : {
          ...snapshot,
          seq: confirmed,
          outcome: "failed",
          finishReason: event.envelope.data.finishReason,
          error: { code: event.envelope.data.code, message: event.envelope.data.message },
        };
  return consumeTurnSnapshot(snapshot, terminal) ?? { snapshot, needsRecovery: true };
}

/** 三类事件的确认位置分别比较，不以 done 序号代替正文序号。 */
export function coversSequences(
  actual: TurnSnapshot["seq"],
  required: TurnSnapshot["seq"],
): boolean {
  return (
    actual.chunk >= required.chunk &&
    actual.done >= required.done &&
    actual.failed >= required.failed
  );
}
