import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { UnlistenFn } from "@tauri-apps/api/event";

type Source = { kind: string; id: string };
type Terminal =
  | { outcome: "completed"; finishReason: "stop" | "guard" | "length" }
  | { outcome: "cancelled" }
  | {
      outcome: "failed";
      error: { code: string; message: string };
      finishReason?: "stop" | "guard" | "length";
    };

/** 仅块专属字段；Rust 保留未知可选字段，未知 kind 不展开到 UI。 */
type RecordBody =
  | {
      playerId: string;
      text: string;
      mode?: "inCharacter" | "outOfCharacter";
      contentRange?: { start: number; end: number };
    }
  | ({ text: string; turnId: string; speakerId?: string } & Terminal)
  | {
      expression: string;
      rolls: { sides: number; value: number }[];
      total: number;
      source: Source;
      planId: string;
      rng: {
        algorithm: "chacha20-v1";
        mappingVersion: 1;
        seed: string;
        startCounter: string;
        endCounter: string;
      };
      modifiers: { value: number; source: Source }[];
    }
  | {
      diceSeq: number;
      dc?: number;
      result: "success" | "costlySuccess" | "failure" | "criticalSuccess" | "criticalFailure";
      ruleId: string;
      planId: string;
    }
  | {
      code: string;
      message: string;
      relatedSeq?: number;
      turnId?: string;
      data: Record<string, unknown>;
    }
  | {
      fromSeq: number;
      throughSeq: number;
      text: string;
      sourceHash: string;
      estimatorVersion: 1 | 2;
      origin: "manual" | "background";
    };

/** RecordItem 同型形状，超大 / 兼容正文只含不透明 bodyRef。 */
export interface RecordItem {
  recordSeq: number;
  kind: string;
  createdAt: string;
  body?: RecordBody;
  bodyRef?: string;
  turnId?: string;
  outcome?: "completed" | "cancelled" | "failed";
}

export interface RecordPage {
  sessionId: string;
  items: RecordItem[];
  nextCursor?: string;
  lastSeq: number;
  lastRecordSeq: number;
  viewEpoch: string;
}
export interface RecordView extends RecordPage {
  needsRecovery: boolean;
  inFlight?: { turnId: string; recordSeq: number };
}
export interface RecordBodyPage {
  text: string;
  nextCursor?: string;
}
interface RecordAppended {
  sessionId: string;
  viewEpoch: string;
  recordSeq: number;
  kind: string;
  turnId?: string;
}
export interface RecordNotification {
  seq: number;
  data: RecordAppended;
}

/** 获取固定读取边界的历史页；前端不解析 cursor。 */
export function getRecordPage(
  sessionId: string,
  cursor?: string,
  limit?: number,
): Promise<RecordPage> {
  return invoke("engine_get_record_page", { sessionId, cursor, limit });
}
/** 获取可替换最新窗口的有界快照，历史页基线不能替代它。 */
export function getRecordView(sessionId: string, limit?: number): Promise<RecordView> {
  return invoke("engine_get_record_view", { sessionId, limit });
}
/** 按 UTF-8 边界读取一段正文，切换引用时放弃旧请求响应。 */
export function getRecordBody(
  sessionId: string,
  bodyRef: string,
  cursor?: string,
): Promise<RecordBodyPage> {
  return invoke("engine_get_record_body", { sessionId, bodyRef, cursor });
}
/** 仅监听主窗口记录身份事件，生命周期由 composable 所有。 */
export function listenRecords(handler: (event: RecordNotification) => void): Promise<UnlistenFn> {
  return listen<RecordNotification>("engine:record:appended", (event) => handler(event.payload), {
    target: { kind: "AnyLabel", label: "main" },
  });
}
