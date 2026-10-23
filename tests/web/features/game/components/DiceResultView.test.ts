import { mount } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import DiceResultView from "../../../../../src-web/features/game/components/DiceResultView.vue";
import DiceSkinPicker from "../../../../../src-web/features/game/components/DiceSkinPicker.vue";
import CheckDiceTray from "../../../../../src-web/features/game/components/CheckDiceTray.vue";
import { i18n } from "../../../../../src-web/i18n/index";

// 输入测试不创建 GPU；像素绘制由真实浏览器截图单独验收。
beforeEach(() => {
  vi.stubGlobal(
    "IntersectionObserver",
    class {
      observe() {}
      disconnect() {}
    },
  );
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const baseProps = {
  expression: "2d6",
  rolls: [],
  scriptId: "mistbell",
  interactive: true,
  motion: "idle" as const,
};

describe("骰子交互", () => {
  it("PBR 首帧准备前隐藏画布且不显示方形数字占位", () => {
    const wrapper = mount(DiceResultView, {
      props: {
        ...baseProps,
        motion: "result",
        rolls: [
          { sides: 6, value: 4 },
          { sides: 6, value: 6 },
        ],
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.attributes("data-renderer")).toBe("pending");
    expect(wrapper.find("canvas").attributes("style")).toContain("visibility: hidden");
    expect(wrapper.findAll(".record-dice-value")).toHaveLength(0);
    expect(wrapper.findAll(".record-dice-pending")).toHaveLength(0);
    wrapper.unmount();
  });
  it("短按和蓄力保持原型力度，甩动按最近轨迹计算方向", async () => {
    vi.useFakeTimers({ toFake: ["setInterval", "clearInterval", "performance"] });
    const wrapper = mount(DiceResultView, { props: baseProps, global: { plugins: [i18n] } });
    wrapper.element.setPointerCapture = vi.fn();
    await wrapper.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    await wrapper.trigger("pointerup");
    expect(wrapper.emitted("submit")?.[0]).toEqual([{ direction: [-1, 0.3], power: 0.35 }]);
    await wrapper.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    await vi.advanceTimersByTimeAsync(900);
    await wrapper.trigger("pointerup");
    expect(wrapper.emitted("submit")?.[1]).toEqual([{ direction: [-1, 0.3], power: 1 }]);
    await wrapper.trigger("pointerdown", { button: 0, pointerId: 1, clientX: 10, clientY: 10 });
    await vi.advanceTimersByTimeAsync(40);
    await wrapper.trigger("pointermove", { pointerId: 1, clientX: 90, clientY: 10 });
    await wrapper.trigger("pointerup");
    expect(wrapper.emitted("submit")?.[2]).toEqual([{ direction: [1, -0], power: 1 }]);
    wrapper.unmount();
  });

  it("设置页试掷从等待切换为滚动，在可视化落定后显示结果", async () => {
    const wrapper = mount(DiceSkinPicker, {
      props: { selected: "bone", disabled: false },
      global: { plugins: [i18n] },
    });
    const preview = () => wrapper.find(".dice-preview-stage").findComponent(DiceResultView);
    expect(preview().props("motion")).toBe("idle");
    expect(preview().props("showPop")).toBe(true);
    await wrapper.findAll(".dice-preview-actions button")[1]!.trigger("click");
    expect(preview().props("motion")).toBe("rolling");
    expect(preview().props("effect")).toBe("crit");
    expect(preview().classes()).not.toContain("dice-effect-crit");
    preview().vm.$emit("settled", { values: [6, 6], effect: "crit" });
    await wrapper.vm.$nextTick();
    expect(preview().props("motion")).toBe("result");
    expect(preview().classes()).toContain("dice-effect-crit");
    wrapper.unmount();
  });
  it("Enter 和 Space 只在等待手动输入时提交，忽略长按重复与输入法", async () => {
    const wrapper = mount(DiceResultView, { props: baseProps, global: { plugins: [i18n] } });
    await wrapper.trigger("keydown", { key: "Enter", repeat: true });
    await wrapper.trigger("keydown", { key: " ", isComposing: true });
    expect(wrapper.emitted("submit")).toBeUndefined();
    await wrapper.trigger("keydown", { key: "Enter" });
    await wrapper.trigger("keydown", { key: " " });
    expect(wrapper.emitted("submit")).toEqual([
      [{ direction: [-1, 0.3], power: 0.6 }],
      [{ direction: [-1, 0.3], power: 0.6 }],
    ]);
    await wrapper.setProps({ motion: "rolling" });
    await wrapper.trigger("keydown", { key: "Enter" });
    expect(wrapper.emitted("submit")).toHaveLength(2);
    wrapper.unmount();
  });

  it("保留最近甩动手势用于引擎事实返回后的动画", async () => {
    const wrapper = mount(CheckDiceTray, {
      props: {
        expression: "2d6",
        mode: "manual",
        status: "waiting",
        disabled: false,
        scriptId: "mistbell",
        skin: "bone",
        scriptSkinEnabled: true,
        shortcut: "Ctrl+Enter",
        rolls: [],
        modifierTotal: 0,
      },
      global: { plugins: [i18n] },
    });
    const gesture = { direction: [0.8, -0.6], power: 0.9 };
    wrapper.findComponent(DiceResultView).vm.$emit("submit", gesture);
    await wrapper.setProps({
      status: "rolling",
      rolls: [
        { sides: 6, value: 4 },
        { sides: 6, value: 6 },
      ],
    });
    expect(wrapper.emitted("submit")).toEqual([[gesture]]);
    expect(wrapper.findComponent(DiceResultView).props("gesture")).toEqual(gesture);
    expect(wrapper.findComponent(DiceResultView).props("motion")).toBe("rolling");
    await wrapper.setProps({ status: "settled" });
    expect(wrapper.findComponent(DiceResultView).props("motion")).toBe("rolling");
    expect(wrapper.attributes("data-state")).toBe("rolling");
    expect(wrapper.emitted("presentationSettled")).toBeUndefined();
    wrapper.findComponent(DiceResultView).vm.$emit("settled", { values: [4, 6], effect: "" });
    await wrapper.vm.$nextTick();
    expect(wrapper.findComponent(DiceResultView).props("motion")).toBe("result");
    expect(wrapper.emitted("presentationSettled")).toHaveLength(1);
    wrapper.findComponent(DiceResultView).vm.$emit("submit", gesture);
    expect(wrapper.emitted("submit")).toHaveLength(1);
    wrapper.unmount();
  });
});
