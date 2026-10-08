import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { expect, it, vi } from "vitest";
import {
  getRecordPage,
  getRecordView,
  getRecordBody,
  listenRecords,
} from "../../../src-web/api/records";
import type { RecordNotification } from "../../../src-web/api/records";
import { getUiPreferences, setUiPreferences, listenMigration } from "../../../src-web/api/store";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

it("passes opaque record references and preferences without local rewriting", async () => {
  const response = { opaque: true };
  vi.mocked(invoke).mockResolvedValue(response);
  expect(await getRecordPage("session", "cursor", 20)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("engine_get_record_page", {
    sessionId: "session",
    cursor: "cursor",
    limit: 20,
  });
  expect(await getRecordView("session", 10)).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("engine_get_record_view", {
    sessionId: "session",
    limit: 10,
  });
  expect(await getRecordBody("session", "body-ref", "body-cursor")).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("engine_get_record_body", {
    sessionId: "session",
    bodyRef: "body-ref",
    cursor: "body-cursor",
  });
  expect(await getUiPreferences()).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("store_get_ui_preferences");
  expect(await setUiPreferences(true, "auto")).toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("store_set_ui_preferences", {
    panelPinned: true,
    diceMode: "auto",
  });
});

it("binds listeners to main and forwards complete envelopes", async () => {
  const handler = vi.fn();
  const off = vi.fn();
  vi.mocked(listen).mockResolvedValue(off);
  expect(await listenRecords(handler)).toBe(off);
  const [name, callback, options] = vi.mocked(listen).mock.calls.at(-1)!;
  expect(name).toBe("engine:record:appended");
  expect(options).toEqual({ target: { kind: "AnyLabel", label: "main" } });
  const payload: RecordNotification = {
    seq: 1,
    data: { viewEpoch: "epoch", sessionId: "session", recordSeq: 1, kind: "narration" },
  };
  callback({ event: name, id: 1, payload });
  expect(handler).toHaveBeenLastCalledWith(payload);
  expect(await listenMigration("failed", handler)).toBe(off);
  const [migrationName, migrationCallback] = vi.mocked(listen).mock.calls.at(-1)!;
  expect(migrationName).toBe("store:migration:failed");
  const failure = {
    seq: 2,
    data: { migrationId: "flow", to: 1, code: "store.io", message: "无法打开" },
  };
  migrationCallback({ event: migrationName, id: 2, payload: failure });
  expect(handler).toHaveBeenLastCalledWith(failure);
});
