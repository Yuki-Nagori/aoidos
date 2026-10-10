import { mount } from "@vue/test-utils";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "../../src-web/App.vue";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn().mockResolvedValue(() => {}) }));
describe("App", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
  });
  it("展示正式入口，初始化不打开会话或发起生成", async () => {
    vi.mocked(invoke).mockImplementation(async (command) => {
      if (command === "llm_list_profiles") return { items: [] };
      if (command === "engine_list_scripts")
        return [{ scriptId: "mistbell", title: "雾钟地窖", attributions: "CC BY 4.0" }];
      if (command === "theme_get_preference") return { version: 1, theme: "dark" };
      if (command === "theme_list")
        return [
          { id: "dark", name: "深色", colorScheme: "dark" },
          { id: "light", name: "浅色", colorScheme: "light" },
          { id: "light-purple", name: "浅紫", colorScheme: "light" },
        ];
      if (command === "theme_skin_load")
        return {
          scriptId: "mistbell",
          status: "missing",
          tokens: {},
          warnings: [],
          warningsTruncated: false,
        };
      throw new Error("unexpected invocation");
    });
    const wrapper = mount(App);
    await vi.waitFor(() => expect(wrapper.text()).toContain("雾钟地窖"));
    expect(wrapper.text()).toContain("系统窗口");
    const model = wrapper.get('select[aria-label="模型"]');
    expect(model.findAll("option").map((option) => option.text())).toEqual([
      "DeepSeek V4.1 Flash",
      "DeepSeek V4 Pro",
    ]);
    expect((model.element as HTMLSelectElement).value).toBe("deepseek-flash");
    expect(wrapper.find("input").exists()).toBe(false);
    expect(
      vi
        .mocked(invoke)
        .mock.calls.map(([command]) => command)
        .sort(),
    ).toEqual([
      "engine_list_scripts",
      "llm_list_profiles",
      "theme_get_preference",
      "theme_list",
      "theme_skin_load",
    ]);
    wrapper.unmount();
  });
  it("浏览器缺 IPC 明确展示恢复错误，不伪造本地生成结果", async () => {
    vi.mocked(invoke).mockRejectedValue({ code: "app.not-ready", message: "桌面 IPC 不可用" });
    const wrapper = mount(App);
    await vi.waitFor(() => expect(wrapper.get('[role="alert"]').text()).toContain("app.not-ready"));
    wrapper.unmount();
  });
});
