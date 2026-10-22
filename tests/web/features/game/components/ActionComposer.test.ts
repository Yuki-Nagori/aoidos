import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";
import ActionComposer from "../../../../../src-web/features/game/components/ActionComposer.vue";
import { i18n } from "../../../../../src-web/i18n/index";
import { previewComposerInput } from "../../../../../src-web/features/game/utils/ui-shell";

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
    expect(wrapper.find(".game-composer-input").exists()).toBe(true);
    expect(wrapper.find(".game-composer-key-hints").text()).toContain("Enter");
    expect(wrapper.find(".game-composer-key-hints").text()).toContain("send");
    expect(wrapper.find(".game-composer-key-hints").text()).toContain("Shift+Enter");
    expect(wrapper.find(".game-composer-key-hints").text()).toContain("newline");
    expect(wrapper.find(".game-composer-top .game-composer-key-hints").exists()).toBe(false);
    expect(wrapper.find(".game-composer-foot .game-composer-key-hints").exists()).toBe(true);
    expect(wrapper.find(".game-composer-foot .game-composer-submit-icon").exists()).toBe(true);
    expect(wrapper.get('button[type="submit"]').text()).toBe("Submit action");
    expect(wrapper.get('button[type="submit"]').classes()).toContain("game-composer-submit");
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

  it("renders the designed command menu and supports arrow navigation and Tab selection", async () => {
    const preview = previewComposerInput("/o");
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "/o",
        preview,
        availableCommands: ["ooc", "interrupt", "pin", "latest"],
        submitDisabled: true,
        submitLabel: "Submit action",
        tooLarge: false,
        slashMenuDismissed: false,
        rewindSelectorOpen: false,
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.get('[role="listbox"]').classes()).toContain("game-command-menu");
    expect(wrapper.get("textarea").attributes("aria-controls")).toBe(
      wrapper.get('[role="listbox"]').attributes("id"),
    );
    expect(wrapper.findAll('[role="option"]')).toHaveLength(8);
    const selected = wrapper.get('[role="option"][aria-selected="true"]');
    expect(selected.text()).toContain("/ooc");
    expect(wrapper.get("textarea").attributes("aria-activedescendant")).toBe(
      selected.attributes("id"),
    );
    expect(wrapper.get('[role="option"][aria-disabled="true"]').text()).toContain("/stop");
    expect(wrapper.get('[role="option"][aria-selected="true"]').text()).toContain(
      "Speak out of character",
    );

    const textarea = wrapper.get("textarea");
    await textarea.trigger("keydown", { key: "ArrowDown" });
    expect(wrapper.get('[role="option"][aria-selected="true"]').text()).toContain("/interrupt");
    await textarea.trigger("keydown", { key: "Tab" });
    expect(wrapper.emitted("chooseCommand")).toEqual([["interrupt"]]);
    expect(wrapper.emitted("keydown")).toBeUndefined();
    wrapper.unmount();
  });

  it("does not trap Tab when the matching command is unavailable", async () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "/s",
        preview: previewComposerInput("/s"),
        availableCommands: ["ooc", "pin", "latest"],
        submitDisabled: true,
        submitLabel: "Submit action",
        tooLarge: false,
        slashMenuDismissed: false,
        rewindSelectorOpen: false,
      },
      global: { plugins: [i18n] },
    });

    await wrapper.get("textarea").trigger("keydown", { key: "Tab" });
    expect(wrapper.emitted("chooseCommand")).toBeUndefined();
    expect(wrapper.emitted("keydown")).toHaveLength(1);
    expect(wrapper.find('[role="option"][aria-selected="true"]').exists()).toBe(false);
    wrapper.unmount();
  });

  it("offers the scenario opening action as an editable draft without submitting it", async () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "",
        suggestedAction: "I look around.",
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
    const suggestion = wrapper.get('button[type="button"]');
    expect(suggestion.text()).toContain("I look around.");
    await suggestion.trigger("click");
    expect(wrapper.emitted("update:modelValue")).toEqual([["I look around."]]);
    expect(wrapper.emitted("submit")).toBeUndefined();
    wrapper.unmount();
  });

  it("offers only the supplied in-scene recipients and defaults to narration", async () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "",
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
        presenceHasScene: true,
        presenceReady: true,
        presenceCharacters: [
          { id: "keeper", kind: "character", scope: "global", canonicalName: "Mira" },
        ],
      },
      global: { plugins: [i18n] },
    });

    const select = wrapper.get('[role="combobox"]');
    expect(select.text()).toContain("Narration");
    await select.trigger("click");
    expect(wrapper.findAll('[role="option"]').map((option) => option.text())).toEqual([
      "Narration",
      "Mira",
    ]);
    await wrapper.findAll('[role="option"]')[1]!.trigger("click");
    expect(wrapper.emitted("selectRecipient")).toEqual([["keeper"]]);
    wrapper.unmount();
  });

  it("moves in-flight controls into the composer footer", async () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "interrupt text",
        preview: {
          mode: "inCharacter",
          slashActive: false,
          candidates: [],
          invalidSlash: false,
          contentRequired: false,
        },
        submitDisabled: true,
        submitLabel: "Submit action",
        tooLarge: false,
        slashMenuDismissed: false,
        rewindSelectorOpen: false,
        inFlight: true,
        interruptAvailable: true,
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.find(".game-composer-key-hints").exists()).toBe(false);
    expect(wrapper.find(".game-composer-submit").exists()).toBe(false);
    expect(wrapper.findAll(".game-composer-foot .game-composer-secondary")).toHaveLength(2);
    await wrapper
      .findAll(".game-composer-secondary")
      .find((button) => button.text().includes("Interrupt"))!
      .trigger("click");
    await wrapper
      .findAll(".game-composer-secondary")
      .find((button) => button.text().includes("Stop"))!
      .trigger("click");
    expect(wrapper.emitted("interrupt")).toHaveLength(1);
    expect(wrapper.emitted("stop")).toHaveLength(1);
    wrapper.unmount();
  });

  it("keeps command submission available while a turn is in flight", () => {
    const wrapper = mount(ActionComposer, {
      props: {
        modelValue: "/latest",
        preview: {
          mode: "inCharacter",
          slashActive: false,
          candidates: [],
          command: { name: "latest", argument: "" },
          invalidSlash: false,
          contentRequired: false,
        },
        submitDisabled: false,
        submitLabel: "Jump to latest",
        tooLarge: false,
        slashMenuDismissed: false,
        rewindSelectorOpen: false,
        inFlight: true,
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.find(".game-composer-submit").text()).toContain("Jump to latest");
    expect(wrapper.find(".game-composer-actions").exists()).toBe(false);
    wrapper.unmount();
  });
});
