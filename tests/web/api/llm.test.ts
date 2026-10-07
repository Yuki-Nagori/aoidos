import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CmdError } from "../../../src-web/api/store";
import {
  cancelTurn,
  getTurn,
  submitTurn,
  type TurnInput,
  type TurnSnapshot,
  type TurnEnvelope,
  type TurnChunk,
  type TurnDone,
  type TurnFailed,
  type FinishReason,
  type TurnOutcome,
  deleteProfile,
  getKeyStatus,
  listProfiles,
  saveProfile,
  setKey,
  type LlmProfile,
  type ProfileMode,
  type ProxyConfig,
  type Sampling,
} from "../../../src-web/api/llm";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

const narrationSampling: Sampling = { temperature: 1.0, maxTokens: 2048 };
const narrationProxy: ProxyConfig = {
  mode: "manual",
  url: "socks5://127.0.0.1:1080",
  authRef: "proxy-auth",
};
const narrationMode: ProfileMode = "completion";
const narration: LlmProfile = {
  profileId: "narration",
  providerId: "deepseek",
  model: "deepseek-v4-pro",
  mode: narrationMode,
  thinking: false,
  sampling: narrationSampling,
  proxy: narrationProxy,
};

describe("llm IPC", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("profile 列表与保存载荷同型透传，camelCase 形参映射 snake_case", async () => {
    const response = { items: [narration] };
    vi.mocked(invoke).mockResolvedValue(response);
    expect(await listProfiles()).toBe(response);
    expect(invoke).toHaveBeenCalledWith("llm_list_profiles");

    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue(narration);
    expect(await saveProfile(narration)).toBe(narration);
    expect(invoke).toHaveBeenCalledWith("llm_save_profile", { profile: narration });
  });

  it("删除为幂等操作并携带 profileId", async () => {
    const response = { deleted: true };
    vi.mocked(invoke).mockResolvedValue(response);
    expect(await deleteProfile("narration")).toBe(response);
    expect(invoke).toHaveBeenCalledWith("llm_delete_profile", { profileId: "narration" });
  });

  it("凭据状态只回 set / hint，setKey 传 providerId + action", async () => {
    const status = { set: true, hint: "5678" };
    vi.mocked(invoke).mockResolvedValue(status);
    expect(await getKeyStatus("deepseek")).toBe(status);
    expect(invoke).toHaveBeenCalledWith("llm_get_key_status", { providerId: "deepseek" });

    vi.mocked(invoke).mockClear();
    const cleared = { set: false, hint: null };
    vi.mocked(invoke).mockResolvedValue(cleared);
    expect(await setKey("deepseek", "clear")).toBe(cleared);
    expect(invoke).toHaveBeenCalledWith("llm_set_key", { providerId: "deepseek", action: "clear" });
  });

  it("保持命令错误形状，不把凭据失败转换成默认状态", async () => {
    const error: CmdError = { code: "store.io", message: "凭据库不可用" };
    vi.mocked(invoke).mockRejectedValue(error);
    expect(await getKeyStatus("deepseek").catch((reason: unknown) => reason)).toBe(error);
    expect(await setKey("deepseek", "set").catch((reason: unknown) => reason)).toBe(error);
  });
  it("回合命令及终态类型同型透传，空失败保留真实收尾原因", async () => {
    const input: TurnInput = {
      kind: "chat",
      messages: [{ role: "user", content: "本地夹具" }],
      assistantPrefix: "前缀",
    };
    const accepted = { turnId: "turn-id" };
    vi.mocked(invoke).mockResolvedValue(accepted);
    expect(await submitTurn("profile", input, "debug-fixture-v1")).toBe(accepted);
    expect(invoke).toHaveBeenCalledWith("llm_submit", {
      profileId: "profile",
      input,
      guardSpecId: "debug-fixture-v1",
    });
    const finishReason: FinishReason = "length";
    const outcome: TurnOutcome = "failed";
    const snapshot: TurnSnapshot = {
      turnId: accepted.turnId,
      text: "",
      seq: { chunk: 0, done: 0, failed: 1 },
      outcome,
      finishReason,
      error: { code: "llm.empty-output", message: "生成未返回正文" },
    };
    vi.mocked(invoke).mockResolvedValue(snapshot);
    expect(await getTurn(accepted.turnId)).toBe(snapshot);
    expect(invoke).toHaveBeenLastCalledWith("llm_get_turn", { turnId: accepted.turnId });
    const cancelled = { turnId: accepted.turnId, outcome: "cancelled" };
    vi.mocked(invoke).mockResolvedValue(cancelled);
    expect(await cancelTurn(accepted.turnId)).toBe(cancelled);
    expect(invoke).toHaveBeenLastCalledWith("llm_cancel", { turnId: accepted.turnId });
    const failed: TurnEnvelope<TurnFailed> = {
      seq: 1,
      data: {
        turnId: accepted.turnId,
        code: "llm.empty-output",
        message: "生成未返回正文",
        chunkSeq: 0,
        finishReason,
      },
    };
    const chunk: TurnEnvelope<TurnChunk> = {
      seq: 1,
      data: { turnId: accepted.turnId, delta: "text" },
    };
    const done: TurnEnvelope<TurnDone> = {
      seq: 1,
      data: {
        turnId: accepted.turnId,
        outcome: "completed",
        chunkSeq: chunk.seq,
        finishReason: "stop",
      },
    };
    expect(failed.data.finishReason).toBe(snapshot.finishReason);
    expect(done.data.chunkSeq).toBe(chunk.seq);
  });
});
