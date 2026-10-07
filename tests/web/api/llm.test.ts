import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { CmdError } from "../../../src-web/api/store";
import {
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
});
