import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { describe, expect, it, vi } from "vitest";
import SelectMenu from "../../../../src-web/shared/components/SelectMenu.vue";

const options = [
  { value: "dark", label: "Dark" },
  { value: "disabled", label: "Unavailable", disabled: true },
  { value: "light", label: "Light" },
];

describe("SelectMenu", () => {
  it("renders the selected value and changes it from the custom listbox", async () => {
    const wrapper = mount(SelectMenu, {
      props: { modelValue: "dark", options, label: "Theme" },
    });
    const trigger = wrapper.get('[role="combobox"]');

    expect(trigger.text()).toContain("Dark");
    expect(wrapper.find("select").exists()).toBe(false);
    await trigger.trigger("click");
    expect(wrapper.get('[role="listbox"]')).toBeTruthy();

    await wrapper.get('[role="option"]:nth-child(3)').trigger("click");
    expect(wrapper.emitted("update:modelValue")).toEqual([["light"]]);
    expect(wrapper.find('[role="listbox"]').exists()).toBe(false);
  });

  it("skips disabled options and lets Escape dismiss without changing the value", async () => {
    const wrapper = mount(SelectMenu, {
      props: { modelValue: "dark", options, label: "Theme" },
    });
    const trigger = wrapper.get('[role="combobox"]');

    await trigger.trigger("keydown", { key: "ArrowDown" });
    expect(wrapper.get('[role="option"].is-active').attributes("id")).toContain("option-0");
    await trigger.trigger("keydown", { key: "ArrowDown" });
    expect(wrapper.get('[role="option"].is-active').attributes("id")).toContain("option-2");
    await trigger.trigger("keydown", { key: "Escape" });

    expect(wrapper.find('[role="listbox"]').exists()).toBe(false);
    expect(wrapper.emitted("update:modelValue")).toBeUndefined();
  });

  it("keeps the active keyboard option visible in a scrollable list", async () => {
    const originalScrollIntoView = Object.getOwnPropertyDescriptor(
      HTMLElement.prototype,
      "scrollIntoView",
    );
    const scrollIntoView = vi.fn();
    Object.defineProperty(HTMLElement.prototype, "scrollIntoView", {
      configurable: true,
      value: scrollIntoView,
    });
    const wrapper = mount(SelectMenu, {
      props: { modelValue: "dark", options, label: "Theme" },
    });
    const trigger = wrapper.get('[role="combobox"]');

    await trigger.trigger("click");
    await vi.waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: "nearest" }));

    scrollIntoView.mockClear();
    await trigger.trigger("keydown", { key: "End" });
    const activeOption = wrapper.get('[role="option"].is-active');
    expect(activeOption.attributes("id")).toContain("option-2");
    await vi.waitFor(() => expect(scrollIntoView).toHaveBeenCalledOnce());
    expect(scrollIntoView.mock.contexts.at(-1)).toBe(activeOption.element);
    wrapper.unmount();
    if (originalScrollIntoView) {
      Object.defineProperty(HTMLElement.prototype, "scrollIntoView", originalScrollIntoView);
    } else {
      Reflect.deleteProperty(HTMLElement.prototype, "scrollIntoView");
    }
  });

  it("lets IME processing keep control and confirms the active option with Enter", async () => {
    const wrapper = mount(SelectMenu, {
      props: { modelValue: "dark", options, label: "Theme" },
    });
    const trigger = wrapper.get('[role="combobox"]');
    await trigger.trigger("keydown", { key: "ArrowDown" });
    await trigger.trigger("keydown", { key: "Enter", isComposing: true });

    expect(wrapper.find('[role="listbox"]').exists()).toBe(true);
    expect(wrapper.emitted("update:modelValue")).toBeUndefined();

    await trigger.trigger("keydown", { key: "ArrowDown" });
    await trigger.trigger("keydown", { key: "Enter" });
    expect(wrapper.emitted("update:modelValue")).toEqual([["light"]]);
    expect(wrapper.find('[role="listbox"]').exists()).toBe(false);
  });

  it("closes when a pointer interaction occurs outside the component", async () => {
    const wrapper = mount(SelectMenu, {
      attachTo: document.body,
      props: { modelValue: "dark", options, label: "Theme" },
    });
    await wrapper.get('[role="combobox"]').trigger("click");
    document.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    await nextTick();

    expect(wrapper.find('[role="listbox"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("releases its document listener when unmounted", async () => {
    const addSpy = vi.spyOn(document, "addEventListener");
    const removeSpy = vi.spyOn(document, "removeEventListener");
    const wrapper = mount(SelectMenu, {
      props: { modelValue: "dark", options, label: "Theme" },
    });
    expect(addSpy).toHaveBeenCalledWith("pointerdown", expect.any(Function));
    wrapper.unmount();
    expect(removeSpy).toHaveBeenCalledWith("pointerdown", expect.any(Function));
    addSpy.mockRestore();
    removeSpy.mockRestore();
  });
});
