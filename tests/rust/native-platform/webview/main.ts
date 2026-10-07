// 真实主 Webview 装载产品消费者，不复制恢复算法；夹具始终运行本地 Rust Provider。
import { createApp, h, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  getTurn,
  submitTurn,
  cancelTurn,
  listenTurnEvent,
  type TurnInput,
  type TurnSnapshot,
} from "../../../../src-web/api/llm";
import { useLlmTurn, type TurnTransport } from "../../../../src-web/composables/useLlmTurn";
import { type TurnNotification } from "../../../../src-web/utils/turn-consumer";

function check(value: boolean, code: string): void {
  if (!value) throw { code };
}

async function smoke(): Promise<void> {
  const turnId = ref<string>();
  let consumer!: ReturnType<typeof useLlmTurn>;
  let requests = 0;
  let callbacks = 0;
  let released = 0;
  let dropAll = false;
  const transport: TurnTransport = {
    getTurn(id) {
      requests += 1;
      return getTurn(id);
    },
    listen(name, handler) {
      return listenTurnEvent(name, (payload) => {
        callbacks += 1;
        if (!dropAll) handler(payload);
      }).then((unlisten) => () => {
        released += 1;
        unlisten();
      });
    },
  };
  const app = createApp({
    setup() {
      consumer = useLlmTurn(turnId, transport);
      return () => h("div", "Mythos consumer fixture");
    },
  });
  app.mount("#app");
  const received: TurnNotification[] = [];
  const observers: (() => void)[] = [];
  let barrier: (() => void) | undefined;
  function completed(): Promise<TurnSnapshot> {
    const current = consumer.view.value.snapshot;
    if (current?.outcome) return Promise.resolve(current);
    return new Promise((resolve) => {
      const stop = watch(
        () => consumer.view.value.snapshot,
        (snapshot) => {
          if (snapshot?.outcome) {
            stop();
            resolve(snapshot);
          }
        },
        { flush: "sync" },
      );
    });
  }
  let unmounted = false;
  let passed = false;
  let diagnostic = "";
  try {
    await consumer.reconnect();
    observers.push(
      await listenTurnEvent("llm:turn:chunk", (envelope) => {
        received.push({ kind: "chunk", envelope });
        if (envelope.data.delta === "unmounted") barrier?.();
      }),
    );
    observers.push(
      await listenTurnEvent("llm:turn:done", (envelope) =>
        received.push({ kind: "done", envelope }),
      ),
    );
    observers.push(
      await listenTurnEvent("llm:turn:failed", (envelope) =>
        received.push({ kind: "failed", envelope }),
      ),
    );
    let rejected = false;
    try {
      await submitTurn(
        "ipc-fixture",
        { kind: "chat", messages: [{ role: "tool", content: "invalid" }] } as unknown as TurnInput,
        "debug-fixture-v1",
      );
    } catch (error) {
      rejected =
        typeof error === "object" &&
        error !== null &&
        "code" in error &&
        error.code === "app.bad-request";
    }
    check(rejected, "invalid-input-not-rejected");
    const first = await submitTurn(
      "ipc-fixture",
      { kind: "completion", prompt: "本地夹具" },
      "debug-fixture-v1",
    );
    turnId.value = first.turnId;
    const snapshot = await completed();
    const cancelled = await cancelTurn(first.turnId);
    const raw = await getTurn(first.turnId);
    check(
      snapshot.text === "本地夹具正文。" &&
        snapshot.outcome === "completed" &&
        snapshot.finishReason === "stop" &&
        raw.text === snapshot.text &&
        cancelled.outcome === "completed",
      "consumer-snapshot-mismatch",
    );
    check(
      received.filter((event) => event.kind === "chunk").length === 1 &&
        received.filter((event) => event.kind === "done").length === 1 &&
        snapshot.seq.chunk === 1 &&
        snapshot.seq.done === 1 &&
        snapshot.seq.failed === 0,
      "event-sequence-mismatch",
    );
    await consumer.reconnect();
    check(
      released === 3 && consumer.view.value.snapshot?.text === snapshot.text,
      "reconnect-mismatch",
    );
    dropAll = true;
    const second = await submitTurn(
      "ipc-fixture",
      { kind: "completion", prompt: "丢失最后所有事件" },
      "debug-fixture-v1",
    );
    turnId.value = second.turnId;
    await consumer.recover();
    const before = requests;
    await invoke("wait_smoke", { turnId: second.turnId });
    check(requests === before, "unexpected-polling");
    await consumer.recover();
    check(
      requests === before + 1 &&
        consumer.view.value.snapshot?.outcome === "completed" &&
        consumer.view.value.snapshot.text === "本地夹具正文。",
      "lost-events-recovery-mismatch",
    );
    const beforeUnmount = callbacks;
    app.unmount();
    unmounted = true;
    check(released === 6, "listener-release-mismatch");
    const seen = new Promise<void>((resolve) => {
      barrier = resolve;
    });
    await invoke("emit_smoke", { turnId: second.turnId });
    await seen;
    check(callbacks === beforeUnmount, "unmounted-listener-called");
    passed = true;
  } catch (error) {
    diagnostic =
      typeof error === "object" && error !== null && "code" in error
        ? String(error.code)
        : "ipc-error";
  } finally {
    if (!unmounted) app.unmount();
    for (const unlisten of observers) unlisten();
  }
  await invoke("report_smoke", { passed, diagnostic });
}
void smoke();
