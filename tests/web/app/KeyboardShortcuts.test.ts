import { mount } from "@vue/test-utils";
import { nextTick } from "vue";
import { afterEach, describe, expect, it } from "vitest";
import KeyboardShortcuts from "../../../src-web/app/components/KeyboardShortcuts.vue";
import { i18n } from "../../../src-web/i18n/index";

describe("KeyboardShortcuts", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("matches the reference key, lock, and editable row treatment", async () => {
    const wrapper = mount(KeyboardShortcuts, {
      attachTo: document.body,
      global: { plugins: [i18n] },
    });
    expect(wrapper.get(".shortcut-settings-intro p").text()).toBe(
      "Input and accessibility shortcuts are fixed. Other bindings are not connected yet.",
    );
    expect(wrapper.find(".shortcut-settings-intro strong").exists()).toBe(false);
    expect(wrapper.findAll(".shortcut-row").length).toBeGreaterThan(10);
    expect(wrapper.findAll(".shortcut-row-settings .shortcut-lock").length).toBeGreaterThan(5);
    expect(wrapper.findAll(".shortcut-row-settings button[disabled]").length).toBeGreaterThan(5);
    expect(wrapper.find(".shortcut-search").exists()).toBe(false);

    await wrapper.get(".shortcut-settings-intro button").trigger("click");
    const dialog = document.body.querySelector<HTMLElement>('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog!.querySelectorAll(".shortcut-row").length).toBeGreaterThan(10);
    expect(dialog!.querySelector(".shortcut-row-keys .kbd")?.textContent).toBeTruthy();
    expect(dialog!.querySelectorAll(".shortcut-row .tag").length).toBeGreaterThan(5);

    const search = dialog!.querySelector<HTMLInputElement>('input[type="search"]');
    expect(search).not.toBeNull();
    search!.value = "Send";
    search!.dispatchEvent(new Event("input", { bubbles: true }));
    await nextTick();
    expect(dialog!.querySelectorAll(".shortcut-row").length).toBe(1);
    expect(dialog!.textContent).toContain("Send");

    dialog!.querySelector<HTMLButtonElement>('button[aria-label="Close"]')!.click();
    await nextTick();
    expect(document.body.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(wrapper.get(".shortcut-settings-intro button").element);
    wrapper.unmount();
  });
});
