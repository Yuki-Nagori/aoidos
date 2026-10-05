import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  listBackups,
  getMigration,
  type CmdError,
  type BackupItem,
  type MigrationSnapshot,
} from "../../../src-web/api/store";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));

describe("store IPC", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });

  it("保留备份时间戳原始精度，并使用无参数命令", async () => {
    const item: BackupItem = {
      path: "backups/storage-v1-1700000000000000001.sqlite",
      version: 1,
      nanos: "1700000000000000001",
      size: 128,
    };
    const response = { items: [item] };
    vi.mocked(invoke).mockResolvedValue(response);
    expect(await listBackups()).toBe(response);
    expect(invoke).toHaveBeenCalledWith("store_list_backups");
  });

  it("接收新安装的 idle 快照", async () => {
    const response: MigrationSnapshot = { from: 0, to: 0, phase: "idle" };
    vi.mocked(invoke).mockResolvedValue(response);
    expect(await getMigration()).toBe(response);
    expect(invoke).toHaveBeenCalledWith("store_get_migration");
  });

  it("保持命令错误形状，不将读取失败转换为空快照或列表", async () => {
    const error: CmdError = { code: "store.corrupt", message: "存储数据损坏" };
    vi.mocked(invoke).mockRejectedValue(error);
    expect(await getMigration().catch((reason: unknown) => reason)).toBe(error);
    expect(await listBackups().catch((reason: unknown) => reason)).toBe(error);
  });
});
