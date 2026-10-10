import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";
import type { RecordItem } from "../../../src-web/api/records";
import RecordFeed from "../../../src-web/components/RecordFeed.vue";
import { i18n } from "../../../src-web/i18n";

describe("RecordFeed", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
  });

  it("renders typed rows and forwards bounded history actions", async () => {
    const items: RecordItem[] = [
      {
        recordSeq: 1,
        createdAt: "2026-10-10T00:00:00Z",
        kind: "playerSpeech",
        body: { playerId: "player", text: "Open the door." },
      },
      {
        recordSeq: 2,
        createdAt: "2026-10-10T00:00:01Z",
        kind: "dice",
        body: {
          expression: "1d20",
          rolls: [{ sides: 20, value: 17 }],
          total: 17,
          source: { kind: "player", id: "player" },
          planId: "plan",
          rng: {
            algorithm: "chacha20-v1",
            mappingVersion: 1,
            seed: "seed",
            startCounter: "0",
            endCounter: "1",
          },
          modifiers: [],
        },
      },
      {
        recordSeq: 3,
        createdAt: "2026-10-10T00:00:02Z",
        kind: "system",
        body: { code: "checkPlanned", message: "", data: {} },
      },
      {
        recordSeq: 4,
        createdAt: "2026-10-10T00:00:03Z",
        kind: "recap",
        bodyRef: "body-ref",
      },
    ];
    const wrapper = mount(RecordFeed, {
      props: {
        items,
        olderCursor: "cursor",
        loadingOlder: false,
        canRewind: true,
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("Open the door.");
    expect(wrapper.text()).toContain("17");
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("loadOlder")).toHaveLength(1);
    await wrapper.findAll("button")[1]!.trigger("click");
    expect(wrapper.emitted("rewind")).toEqual([[3]]);
    await wrapper.findAll("button")[2]!.trigger("click");
    expect(wrapper.emitted("loadBody")).toEqual([["body-ref"]]);
    wrapper.unmount();
  });

  it("renders preview, terminal metadata, and an empty state", () => {
    const wrapper = mount(RecordFeed, {
      props: {
        items: [],
        loadingOlder: false,
        canRewind: false,
        preview: { text: "partial", outcome: "failed", finishReason: "guard" },
        expandedBody: "expanded",
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("No records yet");
    expect(wrapper.text()).toContain("partial");
    expect(wrapper.text()).toContain("expanded");
    expect(wrapper.text()).toContain("guard");
    wrapper.unmount();
  });
});
