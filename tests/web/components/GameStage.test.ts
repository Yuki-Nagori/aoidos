import { mount } from "@vue/test-utils";
import { h, nextTick } from "vue";
import { describe, expect, it } from "vitest";
import GameStage from "../../../src-web/components/GameStage.vue";

const props = {
  panelState: "expanded" as const,
  panelOpen: true,
  modalOpen: false,
  title: "Mistbell",
  sceneLabel: "Scene",
  triggerLabel: "Open dialogue",
  pinLabel: "Pin dialogue",
  unpinLabel: "Unpin dialogue",
  closeLabel: "Close dialogue",
};

describe("GameStage", () => {
  it("opens with the platform shortcut and returns focus after Escape", async () => {
    const wrapper = mount(GameStage, { props, attachTo: document.body });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "m", ctrlKey: true, shiftKey: true }));
    expect(wrapper.emitted("open")).toHaveLength(1);
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(wrapper.emitted("escape")).toHaveLength(1);
    await nextTick();
    expect(document.activeElement).toBe(wrapper.get(".panel-edge-trigger").element);
    expect(wrapper.emitted("open")).toHaveLength(1);
    wrapper.unmount();
  });

  it("leaves Escape to an open modal and keeps collapsed content inert", async () => {
    const wrapper = mount(GameStage, {
      props: { ...props, modalOpen: true, panelOpen: false, panelState: "collapsed" },
      slots: { panel: "Hidden controls" },
    });
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(wrapper.emitted("escape")).toBeUndefined();
    expect(wrapper.get(".dialogue-panel").attributes("inert")).toBeDefined();
    expect(wrapper.get(".dialogue-panel").text()).toContain("Hidden controls");
    await wrapper.get(".panel-edge-trigger").trigger("click");
    expect(wrapper.emitted("open")).toHaveLength(1);
    wrapper.unmount();
  });

  it("does not capture the stage shortcut from editable controls", () => {
    const wrapper = mount(GameStage, {
      props,
      slots: { panel: () => h("input") },
      attachTo: document.body,
    });
    wrapper.get("input").element.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "m",
        ctrlKey: true,
        shiftKey: true,
        bubbles: true,
      }),
    );
    expect(wrapper.emitted("open")).toBeUndefined();
    wrapper.unmount();
  });

  it("does not close the panel when Escape is pressed in an editable control", () => {
    const wrapper = mount(GameStage, {
      props,
      slots: { panel: () => h("textarea") },
      attachTo: document.body,
    });
    wrapper
      .get("textarea")
      .element.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    expect(wrapper.emitted("escape")).toBeUndefined();
    wrapper.unmount();
  });
});
