// 三平台实际主 Webview；消费产品模块，测试等待有截止时间，不加入运行期轮询。
import { effectScope, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import {
  getPhase,
  listenPhaseEvent,
  listScripts,
  openSession,
  submitInput,
  cancelRound,
  submitCheck,
  type PhaseSnapshot,
} from "../../../../src-web/api/engine";
import { getTurn, listenTurnEvent } from "../../../../src-web/api/llm";
import {
  getRecordView,
  getRecordPage,
  getRecordBody,
  listenRecords,
} from "../../../../src-web/api/records";
import { setUiPreferences } from "../../../../src-web/api/store";
import { useGameView } from "../../../../src-web/composables/useGameView";
function check(value: boolean, message: string): void {
  if (!value) throw { code: message };
}
async function until(predicate: () => boolean, stage: string): Promise<void> {
  const deadline = Date.now() + 5000;
  while (!predicate()) {
    if (Date.now() > deadline) throw { code: `fixture.timeout.${stage}` };
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}
async function smoke(): Promise<void> {
  let dropped = false,
    live = 0;
  const scope = effectScope();
  const sessionId = ref<string>();
  let game!: ReturnType<typeof useGameView>;
  function release(unlisten: () => void): () => void {
    live++;
    return () => {
      live--;
      unlisten();
    };
  }
  scope.run(() => {
    game = useGameView(sessionId, {
      phase: {
        read: getPhase,
        listen: (name, handler) =>
          listenPhaseEvent(name, (event) => {
            if (!dropped) handler(event);
          }).then(release),
      },
      turn: {
        getTurn,
        listen: (name, handler) =>
          listenTurnEvent(name, (event) => {
            if (!dropped) handler(event);
          }).then(release),
      },
      records: {
        view: getRecordView,
        page: getRecordPage,
        body: getRecordBody,
        listen: (handler) =>
          listenRecords((event) => {
            if (!dropped) handler(event);
          }).then(release),
      },
    });
  });
  const { phase, turn, records } = game;
  const count = (mode?: number): Promise<number> => invoke("product_http", { mode: mode ?? null });
  const terminal = (roundId: string): Promise<PhaseSnapshot> =>
    invoke("product_wait", { sessionId: sessionId.value, roundId });
  try {
    const scripts = await listScripts();
    check(
      scripts[0]?.scriptId === "mistbell" && scripts[0].attributions.includes("Creative Commons"),
      "scenario.credit",
    );
    if (await invoke<boolean>("product_restarting")) {
      const restored = await openSession("native-chat", "mistbell", false);
      sessionId.value = restored.sessionId;
      await game.recover();
      await until(() => !!game.latestPublicRecord.value, "restart.record");
      const body = game.latestPublicRecord.value?.body;
      check(
        !!body && "text" in body && body.text === "丢失的终态已恢复。",
        "fresh-process.disk-body",
      );
      check(
        !!body &&
          "outcome" in body &&
          body.outcome === "completed" &&
          "finishReason" in body &&
          body.finishReason === "stop",
        "fresh-process.disk-terminal",
      );
      check(
        !turn.view.value.recoveryError && !turn.view.value.snapshot && (await count()) === 0,
        "fresh-process.no-memory-ring-or-http",
      );
      return;
    }
    await setUiPreferences(false, "auto");
    const opened = await openSession("native-product", "mistbell", true);
    sessionId.value = opened.sessionId;
    await until(() => phase.state.value.snapshot?.sessionId === opened.sessionId, "open.phase");
    check((await count()) === 0, "open.must-not-request");
    const accepted = await submitInput(opened.sessionId, "我举起火把，听钟声。");
    const done = await terminal(accepted.roundId);
    check(done.lastOperation?.outcome === "completed", "normal.complete");
    // 封口可能早于 Webview 消费阶段事件；完成正文以持久记录为准。
    await game.recover();
    check((game.latestPublicRecord.value?.recordSeq ?? 0) > 0, "normal.current-record");
    const normalBody = game.latestPublicRecord.value?.body;
    check(!!normalBody && "text" in normalBody, "normal.record-body");
    const text = normalBody && "text" in normalBody ? normalBody.text : "";
    check(text === "钟声穿过雾气。🕯️", "unicode.text");
    check((await count()) === 2, "normal.two-children");
    await records.recover();
    check(
      records.items.value.some(
        (item) =>
          item.kind === "narration" && item.body && "text" in item.body && item.body.text === text,
      ),
      "record.snapshot-equality",
    );
    const disk = await invoke<{ kind: string; text?: string }[]>("product_disk", {
      sessionId: opened.sessionId,
    });
    check(
      disk.some((record) => record.kind === "narration" && record.text === text),
      "disk.equality",
    );
    const oldEpoch = done.stateEpoch;
    await invoke("product_restart", { sessionId: opened.sessionId });
    const reopened = await openSession("native-chat", "mistbell", false);
    check(reopened.sessionId === opened.sessionId && (await count()) === 2, "restart.no-http");
    await phase.recover();
    check(phase.state.value.snapshot?.stateEpoch !== oldEpoch, "restart.new-epoch");
    await records.recover();
    check(
      records.items.value.some((item) => item.kind === "narration"),
      "restart.keeps-history",
    );
    const beforeCancel = await count(1);
    const partial = await submitInput(opened.sessionId, "/ooc 取消测试");
    await until(
      () => turn.view.value.snapshot?.text === text && !turn.view.value.snapshot?.outcome,
      "cancel.partial",
    );
    const active = turn.view.value.snapshot!;
    try {
      await submitInput(opened.sessionId, "并发输入");
      throw { code: "busy.missing" };
    } catch (error) {
      check((error as { code: string }).code === "app.busy", "shared.busy");
    }
    const cancelled = await cancelRound(opened.sessionId, partial.roundId);
    check(cancelled.outcome === "cancelled", "cancel.result");
    const sealed = await getTurn(active.turnId);
    check(sealed.text === text && sealed.outcome === "cancelled", "cancel.keeps-prefix");
    check((await count()) === beforeCancel + 1, "cancel.no-retry");
    await game.recover();
    const emptyFloor = game.latestPublicRecord.value?.recordSeq ?? 0;
    const beforeEmpty = await count(2);
    const empty = await submitInput(opened.sessionId, "/ooc 空输出测试");
    const emptyDone = await terminal(empty.roundId);
    check(emptyDone.lastOperation?.error?.code === "llm.empty-output", "empty.error");
    await game.recover();
    check((game.latestPublicRecord.value?.recordSeq ?? 0) > emptyFloor, "empty.current-record");
    const emptyBody = game.latestPublicRecord.value?.body;
    check(
      !!emptyBody &&
        "outcome" in emptyBody &&
        emptyBody.outcome === "failed" &&
        "finishReason" in emptyBody &&
        emptyBody.finishReason === "stop" &&
        "text" in emptyBody &&
        emptyBody.text === "",
      "empty.finish-reason",
    );
    check((await count()) === beforeEmpty + 3, "empty.three-attempt-limit");
    const beforeAuth = await count(3);
    const auth = await submitInput(opened.sessionId, "/ooc 认证失败测试");
    check((await terminal(auth.roundId)).lastOperation?.error?.code === "llm.auth", "auth.error");
    check((await count()) === beforeAuth + 1, "auth.no-retry");
    await game.recover();
    const disconnectFloor = game.latestPublicRecord.value?.recordSeq ?? 0;
    const beforeDisconnect = await count(4);
    const disconnect = await submitInput(opened.sessionId, "/ooc 断流测试");
    check(
      (await terminal(disconnect.roundId)).lastOperation?.error?.code === "llm.aborted",
      "disconnect.error",
    );
    await game.recover();
    check(
      (game.latestPublicRecord.value?.recordSeq ?? 0) > disconnectFloor,
      "disconnect.current-record",
    );
    const disconnectedBody = game.latestPublicRecord.value?.body;
    check(
      !!disconnectedBody &&
        "outcome" in disconnectedBody &&
        disconnectedBody.outcome === "failed" &&
        "text" in disconnectedBody &&
        disconnectedBody.text === text,
      "disconnect.record-prefix",
    );
    check((await count()) === beforeDisconnect + 1, "partial.no-retry");
    const beforeCheck = await count(5);
    await setUiPreferences(false, "manual");
    const checked = await submitInput(opened.sessionId, "我试着打开古老的门。");
    await until(() => phase.state.value.snapshot?.check?.status === "waiting", "manual.check");
    const plan = phase.state.value.snapshot!.check!.planId;
    check((await count()) === beforeCheck + 1, "manual.no-request-before-confirmation");
    await submitCheck(opened.sessionId, checked.roundId, plan);
    check(
      (await terminal(checked.roundId)).lastOperation?.outcome === "completed",
      "manual.confirmed",
    );
    const afterCheck = await count();
    await submitCheck(opened.sessionId, checked.roundId, plan);
    check(
      (await count()) === afterCheck && afterCheck === beforeCheck + 2,
      "manual.no-reroll-or-request",
    );
    await game.recover();
    const slowFloor = game.latestPublicRecord.value?.recordSeq ?? 0;
    await count(0);
    await invoke("product_commit_gate", { action: "arm" });
    const slow = await submitInput(opened.sessionId, "/ooc 慢提交取消测试");
    const deadline = Date.now() + 5000;
    while (!(await invoke<boolean>("product_commit_gate", { action: "read" }))) {
      check(Date.now() < deadline, "slow.enter-timeout");
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    let acknowledged = false;
    const cancelDuringCommit = cancelRound(opened.sessionId, slow.roundId).then((result) => {
      acknowledged = true;
      return result;
    });
    await new Promise((resolve) => setTimeout(resolve, 40));
    check(!acknowledged, "slow.cancel-must-await-confirmed-boundary");
    await invoke("product_commit_gate", { action: "release" });
    const slowCancelled = await cancelDuringCommit;
    await terminal(slow.roundId);
    await game.recover();
    check((game.latestPublicRecord.value?.recordSeq ?? 0) > slowFloor, "slow.current-record");
    const slowBody = game.latestPublicRecord.value?.body;
    check(
      !!slowBody &&
        "text" in slowBody &&
        slowBody.text === text &&
        "outcome" in slowBody &&
        slowBody.outcome === "completed" &&
        slowCancelled.outcome === "cancelled",
      "slow.seal-keeps-bytes",
    );
    const beforeLoss = await count(6);
    dropped = true;
    const lost = await submitInput(opened.sessionId, "/ooc 丢事件测试");
    const lostDone = await terminal(lost.roundId);
    check(lostDone.lastOperation?.outcome === "completed", "lost.backend-complete");
    await game.recover();
    const recovered = game.latestPublicRecord.value;
    const recoveredBody = recovered?.body;
    check(
      !!recoveredBody && "text" in recoveredBody && recoveredBody.text === "丢失的终态已恢复。",
      "lost.current-record-discovery",
    );
    check(
      !!recoveredBody &&
        "outcome" in recoveredBody &&
        recoveredBody.outcome === "completed" &&
        "finishReason" in recoveredBody &&
        recoveredBody.finishReason === "stop",
      "lost.record-terminal",
    );
    check(!turn.view.value.recoveryError, "lost.no-retired-ring-read");
    check((await count()) === beforeLoss + 1, "lost.no-extra-http");
    const finalDisk = await invoke<{ kind: string; text?: string }[]>("product_disk", {
      sessionId: opened.sessionId,
    });
    check(
      finalDisk
        .filter((record) => record.kind === "narration")
        .every(
          (record) =>
            record.text === text || record.text === "" || record.text === "丢失的终态已恢复。",
        ),
      "failure.prefix-persisted",
    );
  } finally {
    scope.stop();
    await until(() => live === 0, "dispose.listeners");
  }
}
void smoke().then(
  () => invoke("product_report", { passed: true, diagnostic: "" }),
  async (error: unknown) => {
    const code =
      typeof error === "object" && error !== null && "code" in error
        ? String(error.code)
        : "unknown";
    await invoke("product_report", { passed: false, diagnostic: code });
  },
);
