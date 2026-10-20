import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it } from "vitest";
import KeyboardShortcuts from "../../../src-web/app/components/KeyboardShortcuts.vue";
import { i18n } from "../../../src-web/i18n/index";

describe("KeyboardShortcuts", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("filters the design preview and closes the reference without claiming bindings", async () => {
    const wrapper = mount(KeyboardShortcuts, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("Keyboard shortcuts");
    expect(wrapper.findAll(".shortcut-row").length).toBeGreaterThan(10);
    expect(wrapper.findAll(".shortcut-row-settings button[disabled]")).toHaveLength(
      wrapper.findAll(".shortcut-row-settings").length,
    );

    await wrapper.get(".shortcut-settings-intro button").trigger("click");
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog).not.toBeNull();
    const search = dialog!.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "Confirm dice roll";
    search.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(dialog!.querySelectorAll(".shortcut-row")).toHaveLength(1);
    expect(dialog!.querySelector(".shortcut-row-keys .kbd")?.textContent).toBeTruthy();

    dialog!.querySelector<HTMLButtonElement>('button[aria-label="Close"]')!.click();
    await nextTick();
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(wrapper.get(".shortcut-settings-intro button").element);
    wrapper.unmount();
  });
});
