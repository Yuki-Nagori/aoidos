import { expect, it, vi } from "vitest";
import {
  createRecordRecovery,
  validRecordPage,
  previewIsCommitted,
  recordItems,
  type RecordState,
  type RecordTransport,
} from "../../../src-web/utils/record-recovery";
import type { RecordView, RecordPage, RecordBodyPage } from "../../../src-web/api/records";
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function view(sessionId = "a", lastSeq = 0, epoch = "epoch"): RecordView {
  return {
    sessionId,
    viewEpoch: epoch,
    lastSeq,
    lastRecordSeq: lastSeq,
    items: [],
    needsRecovery: false,
  };
}
function harness(extra: Partial<RecordTransport> = {}) {
  let state: RecordState = {
    pages: [],
    historyStale: false,
    recovering: false,
    needsRecovery: false,
  };
  const transport: RecordTransport = {
    view: vi.fn(async (id) => view(id)),
    page: vi.fn(async (id) => view(id)),
    body: vi.fn(async () => ({ text: "正文" })),
    ...extra,
  };
  const recovery = createRecordRecovery(transport, (next) => {
    state = next;
  });
  return {
    recovery,
    transport,
    get state() {
      return state;
    },
  };
}

it("deduplicates overlapping history and prefers the latest window", () => {
  const item = (recordSeq: number, text = "历史") => ({
    recordSeq,
    kind: "playerSpeech",
    createdAt: "2026-10-08T00:00:00Z",
    body: { playerId: "p", text },
  });
  const latest = { ...view("a", 10), items: [item(9, "窗口"), item(10)] };
  const pages = [
    { ...view("a", 10), items: [item(9), item(8), item(7)] },
    { ...view("a", 10), items: [item(8), item(7), item(6)] },
  ];
  const merged = recordItems({ view: latest, pages });
  expect(merged.map((value) => value.recordSeq)).toEqual([10, 9, 8, 7, 6]);
  expect(merged[1]?.body).toEqual({ playerId: "p", text: "窗口" });
  expect(recordItems({ pages: [] })).toEqual([]);
});

it("reads a near-limit Chinese body through the sixty-fifth UTF-8 segment", async () => {
  const text = "中".repeat(699_049);
  const segments = Array.from({ length: 65 }, (_, index) =>
    text.slice(index * 10922, (index + 1) * 10922),
  );
  const body = vi.fn(async (_session: string, _ref: string, cursor?: string) => {
    const index = Number(cursor ?? "0");
    return { text: segments[index]!, ...(index < 64 ? { nextCursor: String(index + 1) } : {}) };
  });
  const h = harness({ body });
  h.recovery.select("a");
  await h.recovery.connect();
  await h.recovery.loadBody("opaque");
  expect(h.state.error).toBeUndefined();
  expect(h.state.body).toBe(text);
  expect(body).toHaveBeenCalledTimes(65);
});
it("discards an in-flight expanded body when the reader returns to latest", async () => {
  const body = deferred<RecordBodyPage>();
  const h = harness({ body: vi.fn().mockReturnValue(body.promise) });
  h.recovery.select("a");
  await h.recovery.connect();
  const reading = h.recovery.loadBody("opaque");
  await vi.waitFor(() => expect(h.state.bodyRef).toBe("opaque"));
  h.recovery.clearBody();
  body.resolve({ text: "不应重新出现的旧正文" });
  await reading;
  expect(h.state.bodyRef).toBeUndefined();
  expect(h.state.body).toBeUndefined();
});
it("buffers subscription events, coalesces hundreds of notifications and stops bounded recovery", async () => {
  const first = deferred<RecordView>(),
    second = deferred<RecordView>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  const h = harness({ view: read });
  h.recovery.select("a");
  h.recovery.receive({
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "a", recordSeq: 1, kind: "narration" },
  });
  const connected = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  for (let seq = 2; seq <= 200; seq++)
    h.recovery.receive({
      seq,
      data: { viewEpoch: "epoch", sessionId: "a", recordSeq: seq, kind: "narration" },
    });
  expect(h.recovery.recover()).toBe(connected);
  first.resolve(view("a", 1));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  second.resolve(view("a", 200));
  await connected;
  expect(h.state.view?.lastSeq).toBe(200);
  expect(h.state.needsRecovery).toBe(false);
  h.recovery.receive({
    seq: 200,
    data: { viewEpoch: "epoch", sessionId: "a", recordSeq: 200, kind: "narration" },
  });
  expect(read).toHaveBeenCalledTimes(2);
  h.recovery.disconnect();
  await h.recovery.recover();
  expect(read).toHaveBeenCalledTimes(2);
});
it("late responses and same-identity reconnects cannot regress confirmed state", async () => {
  const first = deferred<RecordView>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(view("b", 2));
  const h = harness({ view: read });
  h.recovery.select("a");
  const old = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  h.recovery.select("b");
  await h.recovery.recover();
  first.resolve(view("a", 10));
  await old;
  expect(h.state.view?.sessionId).toBe("b");
  h.recovery.select("b");
  h.recovery.receive({
    seq: 3,
    data: { viewEpoch: "epoch", sessionId: "a", recordSeq: 3, kind: "narration" },
  });
  h.recovery.receive({
    seq: 0,
    data: { viewEpoch: "epoch", sessionId: "b", recordSeq: 1, kind: "narration" },
  });
  h.recovery.receive({
    seq: 4,
    data: { viewEpoch: "epoch", sessionId: "b", recordSeq: 0, kind: "narration" },
  });
  read.mockResolvedValue(view("b", 1));
  await h.recovery.recover();
  expect(h.state.view?.lastSeq).toBe(2);
  expect(h.state.error?.code).toBe("app.event-failed");
  read.mockRejectedValue({ code: "store.io", message: "读取失败", detail: "secret" });
  await h.recovery.recover();
  expect(h.state.error).toEqual({ code: "store.io", message: "读取失败" });
  read.mockRejectedValue("secret");
  await h.recovery.recover();
  expect(h.state.error?.message).not.toContain("secret");
});
it("bounds history to four LRU pages and invalidates pages/body with viewEpoch", async () => {
  let epoch = "one";
  const h = harness({
    view: vi.fn(async () => view("a", 20, epoch)),
    page: vi.fn(async (_id, cursor) => ({
      ...view("a", 10, epoch),
      items: [
        {
          recordSeq: Number(cursor),
          kind: "playerSpeech",
          createdAt: "time",
          body: { playerId: "p", text: "x" },
        },
      ],
    })),
  });
  await h.recovery.loadPage();
  await h.recovery.loadBody("ref");
  h.recovery.select("a");
  await h.recovery.connect();
  for (const cursor of ["1", "2", "3", "4", "5", "3"]) await h.recovery.loadPage(cursor);
  expect(h.state.pages).toHaveLength(4);
  expect(h.state.pages.map((p) => p.items[0]?.recordSeq)).toEqual([2, 4, 5, 3]);
  expect(h.state.historyStale).toBe(true);
  await h.recovery.loadBody("ref");
  expect(h.state.body).toBe("正文");
  epoch = "two";
  await h.recovery.recover();
  expect(h.state.pages).toEqual([]);
  expect(h.state.body).toBeUndefined();
});
it("rejects body loops, oversized payloads and old body/page responses", async () => {
  const body = deferred<RecordBodyPage>(),
    page = deferred<RecordPage>();
  const h = harness({
    body: vi.fn().mockReturnValueOnce(body.promise).mockResolvedValue({ text: "新正文" }),
    page: vi.fn().mockReturnValueOnce(page.promise),
  });
  h.recovery.select("a");
  await h.recovery.connect();
  const oldBody = h.recovery.loadBody("old"),
    oldPage = h.recovery.loadPage("cursor");
  const newBody = h.recovery.loadBody("new");
  await Promise.resolve();
  body.resolve({ text: "旧正文" });
  await oldBody;
  await newBody;
  expect(h.state.body).toBe("新正文");
  h.recovery.select("b");
  await h.recovery.recover();
  page.resolve(view("a"));
  await oldPage;
  expect(h.state.pages).toEqual([]);
  h.transport.body = vi.fn().mockResolvedValue({ text: "字", nextCursor: "loop" });
  await h.recovery.loadBody("loop");
  expect(h.state.error?.code).toBe("app.event-failed");
  h.transport.body = vi.fn().mockResolvedValue({ text: "", nextCursor: "empty" });
  await h.recovery.loadBody("empty");
  expect(h.state.error?.code).toBe("app.event-failed");
  h.transport.body = vi.fn().mockResolvedValue({ text: "x".repeat(32769) });
  await h.recovery.loadBody("large");
  expect(h.state.body).toBe("");
});
it("validates page bounds and replaces previews only by exact identity", () => {
  const v = view("a", 2);
  v.items = [
    {
      recordSeq: 2,
      kind: "narration",
      createdAt: "time",
      turnId: "turn",
      outcome: "completed",
      body: { text: "字", turnId: "turn", outcome: "completed", finishReason: "stop" },
    },
  ];
  expect(validRecordPage(v, "a")).toBe(true);
  expect(previewIsCommitted(v, "turn", 2)).toBe(true);
  expect(previewIsCommitted(undefined, "turn", 2)).toBe(false);
  expect(previewIsCommitted(v, "other", 2)).toBe(false);
  expect(validRecordPage(v, "b")).toBe(false);
  expect(validRecordPage({ ...v, viewEpoch: "" }, "a")).toBe(false);
  expect(validRecordPage({ ...v, lastSeq: -1 }, "a")).toBe(false);
  expect(validRecordPage({ ...v, lastRecordSeq: 1 }, "a")).toBe(false);
  expect(validRecordPage({ ...v, items: [v.items[0]!, v.items[0]!] }, "a")).toBe(false);
  expect(validRecordPage({ ...v, items: Array(201).fill(v.items[0]) }, "a")).toBe(false);
  expect(
    validRecordPage(
      { ...v, items: [{ ...v.items[0]!, body: { playerId: "p", text: "x".repeat(600000) } }] },
      "a",
    ),
  ).toBe(false);
});
it("keeps a physical read single across repeated same-session reconnects and frees saturated identity slots", async () => {
  const pending = deferred<RecordView>();
  const read = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(view("a", 2));
  const h = harness({ view: read });
  h.recovery.select("a");
  const old = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  h.recovery.disconnect();
  const fresh = h.recovery.connect();
  await Promise.resolve();
  expect(read).toHaveBeenCalledTimes(1);
  pending.reject(new Error("old failure"));
  await old;
  await fresh;
  expect(h.state.view?.lastSeq).toBe(2);
  const waits: ReturnType<typeof deferred<RecordView>>[] = [];
  const requests = vi.fn(() => {
    const wait = deferred<RecordView>();
    waits.push(wait);
    return wait.promise;
  });
  const busy = harness({ view: requests });
  busy.recovery.select("session-0");
  void busy.recovery.connect();
  await vi.waitFor(() => expect(waits).toHaveLength(1));
  for (let n = 1; n < 32; n++) {
    busy.recovery.select(`session-${n}`);
    await vi.waitFor(() => expect(waits).toHaveLength(n + 1));
  }
  busy.recovery.select("overflow");
  await busy.recovery.recover();
  expect(busy.state.error?.code).toBe("app.busy");
  expect(requests).toHaveBeenCalledTimes(32);
  waits.forEach((wait) => wait.reject(new Error("release")));
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
  busy.recovery.disconnect();
});
it("bounds single page reads and discards failed responses after switching identity", async () => {
  const failed = deferred<RecordPage>();
  const read = vi
    .fn()
    .mockReturnValueOnce(failed.promise)
    .mockResolvedValue({ ...view("a"), viewEpoch: "wrong" });
  const h = harness({ page: read });
  h.recovery.select("a");
  await h.recovery.connect();
  const pending = h.recovery.loadPage();
  await h.recovery.loadPage("other");
  expect(read).toHaveBeenCalledTimes(1);
  failed.reject({ code: "store.io", message: "failed" });
  await pending;
  expect(h.state.error?.code).toBe("store.io");
  await h.recovery.loadPage();
  expect(h.state.error?.code).toBe("app.bad-request");
  const late = deferred<RecordPage>();
  h.transport.page = vi.fn().mockReturnValue(late.promise);
  const old = h.recovery.loadPage();
  h.recovery.select("b");
  await h.recovery.recover();
  late.reject(new Error("old"));
  await old;
  expect(h.state.error).toBeUndefined();
  const body = deferred<RecordBodyPage>();
  h.transport.body = vi.fn().mockReturnValue(body.promise);
  const oldBody = h.recovery.loadBody("ref");
  h.recovery.select("c");
  await h.recovery.recover();
  body.reject(new Error("old body"));
  await oldBody;
  expect(h.state.error).toBeUndefined();
});

it("retires bounded old epochs and invalidates caches on confirmed forks", async () => {
  const h = harness();
  h.recovery.select("a");
  await h.recovery.connect();
  for (let n = 0; n < 18; n++) {
    h.transport.view = vi.fn().mockResolvedValue(view("a", n, `epoch-${n}`));
    await h.recovery.recover();
  }
  h.transport.view = vi.fn().mockResolvedValue(view("a", 18, "epoch-16"));
  await h.recovery.recover();
  expect(h.state.error?.code).toBe("app.event-failed");
  const first = deferred<RecordView>();
  h.transport.view = vi
    .fn()
    .mockReturnValueOnce(first.promise)
    .mockResolvedValue(view("a", 19, "next"));
  const pending = h.recovery.recover();
  await Promise.resolve();
  h.recovery.receive({
    seq: 19,
    data: { viewEpoch: "next", sessionId: "a", recordSeq: 19, kind: "narration" },
  });
  first.resolve(view("a", 18, "next"));
  await pending;
  expect(h.state.view?.lastSeq).toBe(19);
  expect(h.state.needsRecovery).toBe(false);
  h.recovery.disconnect();
  h.recovery.receive({
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "a", recordSeq: 1, kind: "narration" },
  });
  await h.recovery.connect();
});

it("superseded body readers stop after a failed physical read without issuing old requests", async () => {
  const first = deferred<RecordBodyPage>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue({ text: "最新" });
  const h = harness({ body: read });
  h.recovery.select("a");
  await h.recovery.connect();
  const old = h.recovery.loadBody("old");
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  const superseded = h.recovery.loadBody("middle");
  const latest = h.recovery.loadBody("new");
  first.reject(new Error("old physical failure"));
  await Promise.all([old, superseded, latest]);
  expect(read).toHaveBeenCalledTimes(2);
  expect(h.state.body).toBe("最新");
});

it("a superseded reconnect waits its old physical request then stops before a new read", async () => {
  const first = deferred<RecordView>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockResolvedValue(view("a"));
  const h = harness({ view: read });
  h.recovery.select("a");
  const old = h.recovery.connect();
  await vi.waitFor(() => expect(read).toHaveBeenCalledOnce());
  h.recovery.disconnect();
  const obsolete = h.recovery.connect();
  await Promise.resolve();
  await Promise.resolve();
  h.recovery.disconnect();
  first.resolve(view("a"));
  await Promise.all([old, obsolete]);
  expect(read).toHaveBeenCalledOnce();
  h.recovery.select("b");
  h.recovery.receive({
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "b", recordSeq: 1, kind: "narration" },
  });
  h.recovery.select("a");
  await h.recovery.connect();
});

it("new epoch confirmation retains hints received after the read began", async () => {
  const h = harness();
  h.recovery.select("a");
  await h.recovery.connect();
  const pending = deferred<RecordView>();
  h.transport.view = vi
    .fn()
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValue(view("a", 2, "fork"));
  const ready = h.recovery.recover();
  await Promise.resolve();
  await Promise.resolve();
  h.recovery.receive({
    seq: 2,
    data: { viewEpoch: "fork", sessionId: "a", recordSeq: 2, kind: "narration" },
  });
  pending.resolve(view("a", 1, "fork"));
  await ready;
  expect(h.state.view?.lastSeq).toBe(2);
  expect(h.state.needsRecovery).toBe(false);
  const empty = harness();
  empty.recovery.select("a");
  empty.recovery.connect();
  empty.recovery.select("b");
  empty.recovery.receive({
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "b", recordSeq: 1, kind: "narration" },
  });
  await empty.recovery.recover();
});

it("marks cached history stale on append and recovers after an initial snapshot failure", async () => {
  const h = harness();
  h.recovery.select("a");
  await h.recovery.connect();
  await h.recovery.loadPage();
  h.transport.view = vi.fn().mockResolvedValue(view("a", 1));
  await h.recovery.recover();
  expect(h.state.historyStale).toBe(true);
  const failed = harness({
    view: vi
      .fn()
      .mockRejectedValueOnce(new Error("initial read failed"))
      .mockResolvedValue(view("a", 1)),
  });
  failed.recovery.select("a");
  await failed.recovery.connect();
  expect(failed.state.view).toBeUndefined();
  failed.recovery.receive({
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "a", recordSeq: 1, kind: "narration" },
  });
  await failed.recovery.recover();
  expect(failed.state.view?.lastSeq).toBe(1);
});

it("same-session reopen rejects retired notifications without poisoning the new baseline", async () => {
  const h = harness();
  h.recovery.select("a");
  await h.recovery.connect();
  h.transport.view = vi.fn().mockResolvedValue(view("a", 0, "reopened"));
  await h.recovery.recover();
  h.recovery.receive({
    seq: 99,
    data: { sessionId: "a", viewEpoch: "epoch", recordSeq: 1, kind: "narration" },
  });
  expect(h.state.needsRecovery).toBe(false);
  expect(h.transport.view).toHaveBeenCalledTimes(1);
  h.recovery.receive({
    seq: 99,
    data: { sessionId: "a", viewEpoch: "unseen-old", recordSeq: 1, kind: "narration" },
  });
  await h.recovery.recover();
  expect(h.transport.view).toHaveBeenCalledTimes(3);
  expect(h.state.needsRecovery).toBe(false);
  h.recovery.receive({
    seq: 1,
    data: { sessionId: "a", viewEpoch: "", recordSeq: 1, kind: "narration" },
  });
  expect(h.state.needsRecovery).toBe(false);
});

it("a new epoch event can start at a lower sequence and still request the current view", async () => {
  const h = harness({ view: vi.fn().mockResolvedValue(view("a", 50, "old")) });
  h.recovery.select("a");
  await h.recovery.connect();
  h.transport.view = vi.fn().mockResolvedValue({ ...view("a", 1, "new"), lastRecordSeq: 50 });
  h.recovery.receive({
    seq: 1,
    data: { sessionId: "a", viewEpoch: "new", recordSeq: 50, kind: "system" },
  });
  await h.recovery.recover();
  expect(h.state.view?.viewEpoch).toBe("new");
  expect(h.state.needsRecovery).toBe(false);
});

it("same-epoch hints retain their high-water mark when the notification cache wraps", async () => {
  const h = harness();
  h.recovery.select("a");
  await h.recovery.connect();
  const pending = deferred<RecordView>();
  h.transport.view = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(view("a", 100));
  for (let seq = 1; seq <= 100; seq++)
    h.recovery.receive({
      seq,
      data: { sessionId: "a", viewEpoch: "epoch", recordSeq: seq, kind: "narration" },
    });
  pending.resolve(view("a", 1));
  await h.recovery.recover();
  expect(h.state.view?.lastSeq).toBe(100);
  expect(h.state.needsRecovery).toBe(false);
  expect(h.transport.view).toHaveBeenCalledTimes(2);
});

it("a new epoch arriving during the final bounded read leaves an honest recovery hint", async () => {
  const h = harness();
  h.recovery.select("a");
  await h.recovery.connect();
  const first = deferred<RecordView>(),
    second = deferred<RecordView>();
  const read = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  h.transport.view = read;
  h.recovery.receive({
    seq: 1,
    data: { sessionId: "a", viewEpoch: "epoch", recordSeq: 1, kind: "narration" },
  });
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(1));
  first.resolve(view("a", 0));
  await vi.waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  h.recovery.receive({
    seq: 1,
    data: { sessionId: "a", viewEpoch: "fork", recordSeq: 1, kind: "system" },
  });
  second.resolve(view("a", 1));
  await h.recovery.recover();
  expect(read).toHaveBeenCalledTimes(2);
  expect(h.state.needsRecovery).toBe(true);
  h.transport.view = vi.fn().mockResolvedValue(view("a", 1, "fork"));
  await h.recovery.recover();
  expect(h.state.needsRecovery).toBe(false);
});
