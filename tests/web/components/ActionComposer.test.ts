import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";
import ActionComposer from "../../../src-web/components/ActionComposer.vue";
import { i18n } from "../../../src-web/i18n";

describe("ActionComposer", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
  });

  it("keeps text editing controlled and emits submit and keyboard events", async () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "hello",
        preview: {
          mode: "inCharacter",
          slashActive: false,
          candidates: [],
          invalidSlash: false,
          contentRequired: false,
        },
        submitDisabled: false,
        submitLabel: "Submit action",
        tooLarge: false,
        slashMenuDismissed: false,
        rewindSelectorOpen: false,
      },
      global: { plugins: [i18n] },
    });
    const textarea = wrapper.get("textarea");
    await textarea.setValue("edited");
    await textarea.trigger("keydown", { key: "Enter" });
    await wrapper.get("form").trigger("submit");
    expect(wrapper.emitted("update:modelValue")).toEqual([["edited"]]);
    expect(wrapper.emitted("keydown")).toHaveLength(1);
    expect(wrapper.emitted("submit")).toHaveLength(1);
    expect(wrapper.get('button[type="submit"]').text()).toBe("Submit action");
    expect(wrapper.get('button[type="submit"]').classes()).toContain("text-on-accent");
    wrapper.unmount();
  });

  it("shows safe slash suggestions and blocks submission while disabled", async () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "/int",
        preview: {
          mode: "inCharacter",
          slashActive: true,
          candidates: ["interrupt"],
          invalidSlash: false,
          contentRequired: false,
        },
        submitDisabled: true,
        submitLabel: "Submit action",
        tooLarge: true,
        slashMenuDismissed: false,
        rewindSelectorOpen: false,
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.find('[role="listbox"]').exists()).toBe(true);
    expect((wrapper.get('button[type="submit"]').element as HTMLButtonElement).disabled).toBe(true);
    await wrapper.get('[role="option"]').trigger("click");
    expect(wrapper.emitted("chooseCommand")).toEqual([["interrupt"]]);
    expect(wrapper.find('[role="alert"]').exists()).toBe(true);
    wrapper.unmount();
  });
});
