import type {
  RecordBodyPage,
  RecordItem,
  RecordNotification,
  RecordPage,
  RecordView,
} from "../api/records";

const byteLength = (value: unknown) => new TextEncoder().encode(JSON.stringify(value)).length;
const sequence = (value: number) => Number.isSafeInteger(value) && value >= 0;
type RecoveryError = { code: string; message: string };
export interface RecordState {
  view?: RecordView;
  pages: RecordPage[];
  historyStale: boolean;
  bodyRef?: string;
  body?: string;
  recovering: boolean;
  needsRecovery: boolean;
  error?: RecoveryError;
}
export interface RecordTransport {
  view(sessionId: string): Promise<RecordView>;
  page(sessionId: string, cursor?: string): Promise<RecordPage>;
  body(sessionId: string, bodyRef: string, cursor?: string): Promise<RecordBodyPage>;
}
function recoveryError(error: unknown): RecoveryError {
  if (
    error &&
    typeof error === "object" &&
    "code" in error &&
    "message" in error &&
    typeof error.code === "string" &&
    typeof error.message === "string"
  )
    return { code: error.code, message: error.message };
  return { code: "app.event-failed", message: "记录恢复失败，可主动重试" };
}
/** 只确认有界页；事件基线与物理记录位置分别校验。 */
export function validRecordPage(page: RecordPage, sessionId: string): boolean {
  return (
    page.sessionId === sessionId &&
    !!page.viewEpoch &&
    sequence(page.lastSeq) &&
    sequence(page.lastRecordSeq) &&
    page.items.length <= 200 &&
    byteLength(page) <= 512 * 1024 &&
    page.items.every(
      (item, index) =>
        sequence(item.recordSeq) &&
        item.recordSeq > 0 &&
        item.recordSeq <= page.lastRecordSeq &&
        (index === 0 || item.recordSeq < page.items[index - 1]!.recordSeq),
    )
  );
}
/** 正式块替换同 turnId 的预览；不会因 LLM / 记录事件同时到达显示两份正文。 */
export function previewIsCommitted(
  view: RecordView | undefined,
  turnId: string,
  recordSeq: number,
): boolean {
  return !!view?.items.some((item) => item.recordSeq === recordSeq && item.turnId === turnId);
}

/** 最新窗口优先于缓存历史页；同一物理身份在展示工作集中只出现一次。 */
export function recordItems(state: Pick<RecordState, "view" | "pages">): RecordItem[] {
  const items = new Map<number, RecordItem>();
  for (const page of state.pages) {
    for (const item of page.items) items.set(item.recordSeq, item);
  }
  for (const item of state.view?.items ?? []) items.set(item.recordSeq, item);
  return [...items.values()].sort((a, b) => b.recordSeq - a.recordSeq);
}

/** 记录视图恢复及有界缓存；最后所有事件丢失仍须主动恢复，不开启定时轮询。 */
export function createRecordRecovery(
  transport: RecordTransport,
  publish: (state: RecordState) => void,
) {
  let id: string | undefined;
  let generation = 0;
  let connected = false;
  let observed = 0;
  let pending: Promise<void> | undefined;
  let cache: RecordNotification[] = [];
  const reads = new Map<string, Promise<RecordView>>();
  const retiredEpochs: string[] = [];
  let bodyGeneration = 0;
  let pageRead: Promise<RecordPage> | undefined;
  let bodyRead: Promise<RecordBodyPage> | undefined;
  let state: RecordState = {
    pages: [],
    historyStale: false,
    recovering: false,
    needsRecovery: false,
  };
  const emit = () => publish({ ...state, pages: [...state.pages] });
  const current = (token: number, session: string) =>
    token === generation && session === id && connected;
  async function refresh(token: number, session: string) {
    try {
      for (let pass = 0; pass < 2; pass++) {
        const previous = reads.get(session);
        if (previous) await previous.catch(() => undefined);
        if (!current(token, session)) return;
        if (reads.size >= 32) throw { code: "app.busy", message: "未完成的记录读取已达上限" };
        const requestedEpochs = new Set(cache.map((event) => event.data.viewEpoch));
        const request = Promise.resolve().then(() => transport.view(session));
        reads.set(session, request);
        let candidate: RecordView;
        try {
          candidate = await request;
        } finally {
          reads.delete(session);
        }
        if (!current(token, session)) return;
        if (
          !validRecordPage(candidate, session) ||
          retiredEpochs.includes(candidate.viewEpoch) ||
          (state.view?.viewEpoch === candidate.viewEpoch &&
            candidate.lastSeq < state.view.lastSeq) ||
          (state.view && candidate.lastRecordSeq < state.view.lastRecordSeq)
        )
          throw { code: "app.event-failed", message: "记录快照不能覆盖确认基线" };
        const epochChanged = candidate.viewEpoch !== state.view?.viewEpoch;
        if (state.view && epochChanged) {
          retiredEpochs.push(state.view.viewEpoch);
          if (retiredEpochs.length > 16) retiredEpochs.shift();
          state.pages = [];
          state.bodyRef = undefined;
          state.body = undefined;
          bodyGeneration++;
        }
        if (epochChanged)
          observed = Math.max(
            candidate.lastSeq,
            ...cache
              .filter((event) => event.data.viewEpoch === candidate.viewEpoch)
              .map((event) => event.seq),
          );
        const otherEpochHint = cache.some(
          (event) =>
            event.data.viewEpoch !== candidate.viewEpoch &&
            !retiredEpochs.includes(event.data.viewEpoch),
        );
        const lateOtherEpoch = (event: RecordNotification) =>
          event.data.viewEpoch !== candidate.viewEpoch &&
          !retiredEpochs.includes(event.data.viewEpoch) &&
          !requestedEpochs.has(event.data.viewEpoch);
        state.historyStale = state.pages.some((page) => page.lastSeq < candidate.lastSeq);
        state.view = candidate;
        cache = cache.filter(
          (event) =>
            (event.data.viewEpoch === candidate.viewEpoch && event.seq > candidate.lastSeq) ||
            lateOtherEpoch(event),
        );
        state.needsRecovery =
          observed > candidate.lastSeq ||
          (pass === 0 && otherEpochHint) ||
          cache.some(lateOtherEpoch);
        state.error = undefined;
        emit();
        if (!state.needsRecovery) break;
      }
    } catch (error) {
      if (current(token, session)) {
        state.error = recoveryError(error);
        state.needsRecovery = true;
      }
    } finally {
      if (current(token, session)) {
        pending = undefined;
        state.recovering = false;
        emit();
      }
    }
  }
  function recover(): Promise<void> {
    if (!connected || !id) return Promise.resolve();
    if (pending) return pending;
    const token = generation,
      session = id;
    const request = Promise.resolve().then(() => refresh(token, session));
    pending = request;
    state.recovering = true;
    emit();
    return request;
  }
  function select(sessionId: string | undefined) {
    if (id === sessionId) return;
    id = sessionId;
    generation++;
    bodyGeneration++;
    pending = undefined;
    cache = [];
    observed = 0;
    retiredEpochs.length = 0;
    state = { pages: [], historyStale: false, recovering: false, needsRecovery: false };
    emit();
    if (connected) void recover();
  }
  function receive(event: RecordNotification) {
    if (
      !id ||
      event.data.sessionId !== id ||
      !event.data.viewEpoch ||
      retiredEpochs.includes(event.data.viewEpoch) ||
      !sequence(event.seq) ||
      event.seq === 0 ||
      !sequence(event.data.recordSeq) ||
      event.data.recordSeq === 0
    )
      return;
    if (
      (!pending &&
        connected &&
        event.data.viewEpoch === state.view?.viewEpoch &&
        event.seq <= state.view.lastSeq) ||
      cache.some((old) => old.data.viewEpoch === event.data.viewEpoch && old.seq === event.seq)
    )
      return;
    if (event.data.viewEpoch === state.view?.viewEpoch) observed = Math.max(observed, event.seq);
    if (cache.length === 32) cache = [];
    cache.push(event);
    state.needsRecovery = true;
    if (connected) void recover();
  }
  function connect() {
    connected = true;
    return recover();
  }
  function disconnect() {
    connected = false;
    cache = [];
    observed = 0;
    generation++;
    bodyGeneration++;
    pending = undefined;
    state.recovering = false;
    emit();
  }
  async function loadPage(cursor?: string) {
    if (!id || !connected || !state.view) return;
    if (pageRead) return;
    const session = id,
      token = generation,
      epoch = state.view.viewEpoch;
    try {
      const request = Promise.resolve().then(() => transport.page(session, cursor));
      pageRead = request;
      let page: RecordPage;
      try {
        page = await request;
      } finally {
        pageRead = undefined;
      }
      if (!current(token, session) || epoch !== state.view?.viewEpoch) return;
      if (!validRecordPage(page, session) || page.viewEpoch !== epoch)
        throw { code: "app.bad-request", message: "历史页身份已失效" };
      const key = page.items.map((item) => item.recordSeq).join(",");
      state.pages = state.pages.filter(
        (old) => old.items.map((item) => item.recordSeq).join(",") !== key,
      );
      state.pages.push(page);
      if (state.pages.length > 4) state.pages.shift();
      state.historyStale = state.pages.some((old) => old.lastSeq < state.view!.lastSeq);
      emit();
    } catch (error) {
      if (current(token, session) && epoch === state.view?.viewEpoch) {
        state.error = recoveryError(error);
        emit();
      }
    }
  }
  async function loadBody(bodyRef: string) {
    if (!id || !connected || !state.view) return;
    const session = id,
      token = generation,
      epoch = state.view.viewEpoch,
      bodyToken = ++bodyGeneration;
    state.bodyRef = bodyRef;
    state.body = "";
    emit();
    const cursors = new Set<string>();
    let cursor: string | undefined;
    let count = 0;
    try {
      do {
        if (bodyRead) await bodyRead.catch(() => undefined);
        if (
          !current(token, session) ||
          epoch !== state.view?.viewEpoch ||
          bodyToken !== bodyGeneration
        )
          return;
        const request = Promise.resolve().then(() => transport.body(session, bodyRef, cursor));
        bodyRead = request;
        let page: RecordBodyPage;
        try {
          page = await request;
        } finally {
          bodyRead = undefined;
        }
        count++;
        if (
          !current(token, session) ||
          epoch !== state.view?.viewEpoch ||
          bodyToken !== bodyGeneration
        )
          return;
        if (
          count > 65 ||
          (page.nextCursor && page.text.length === 0) ||
          new TextEncoder().encode(page.text).length > 32 * 1024 ||
          new TextEncoder().encode(state.body! + page.text).length > 2 * 1024 * 1024 ||
          (page.nextCursor && cursors.has(page.nextCursor))
        )
          throw { code: "app.event-failed", message: "正文分段超过限制或未前进" };
        state.body += page.text;
        cursor = page.nextCursor;
        if (cursor) cursors.add(cursor);
        emit();
      } while (cursor);
    } catch (error) {
      if (
        current(token, session) &&
        epoch === state.view?.viewEpoch &&
        bodyToken === bodyGeneration
      ) {
        state.error = recoveryError(error);
        emit();
      }
    }
  }
  function clearBody(): void {
    bodyGeneration++;
    state.bodyRef = undefined;
    state.body = undefined;
    emit();
  }
  return { select, receive, connect, disconnect, recover, loadPage, loadBody, clearBody };
}
