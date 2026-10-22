import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it } from "vitest";
import type { RecordItem } from "../../../../../src-web/api/records";
import RecordFeed from "../../../../../src-web/features/game/components/RecordFeed.vue";
import { i18n } from "../../../../../src-web/i18n/index";

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
        body: { playerId: "player", text: "Open the door.", mode: "outOfCharacter" },
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
      {
        recordSeq: 5,
        createdAt: "2026-10-10T00:00:04Z",
        kind: "characterSpeech",
        body: {
          speakerId: "keeper",
          text: "Wait before you open it.",
          turnId: "turn-keeper",
          outcome: "completed",
          finishReason: "stop",
        },
      },
      {
        recordSeq: 6,
        createdAt: "2026-10-10T00:00:05Z",
        kind: "narration",
        body: {
          text: "The lamp leans toward the door.",
          turnId: "turn-narrator",
          outcome: "completed",
          finishReason: "stop",
        },
      },
      {
        recordSeq: 7,
        createdAt: "2026-10-10T00:00:06Z",
        kind: "check",
        body: { diceSeq: 2, dc: 9, result: "success", ruleId: "probeDoor", planId: "plan" },
      },
    ];
    const wrapper = mount(RecordFeed, {
      props: {
        items,
        characters: [{ id: "keeper", canonicalName: "Lantern keeper" }],
        olderCursor: "cursor",
        loadingOlder: false,
        canRewind: true,
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("Open the door.");
    expect(wrapper.text()).toContain("17");
    expect(wrapper.text()).toContain("Lantern keeper");
    expect(wrapper.text()).toContain("The lamp leans toward the door.");
    expect(wrapper.text()).toContain("DC 9");
    expect(wrapper.find(".record-player-bubble").exists()).toBe(true);
    expect(wrapper.find(".record-character-bubble").exists()).toBe(true);
    expect(wrapper.find(".record-narrator").exists()).toBe(true);
    expect(wrapper.find(".record-entry").exists()).toBe(false);
    expect(wrapper.text()).toContain("Out of character");
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

  it("shows a bounded unknown check result without replacing the whole record", () => {
    const wrapper = mount(RecordFeed, {
      props: {
        items: [
          {
            recordSeq: 1,
            createdAt: "2026-10-20T00:00:00Z",
            kind: "check",
            body: {
              diceSeq: 99,
              result: "futureResult" as never,
              ruleId: "test",
              planId: "plan",
            },
          },
        ],
        loadingOlder: false,
        canRewind: false,
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.find(".record-result").text()).toBe("Unknown check result: futureResult");
    expect(wrapper.text()).not.toContain("current version cannot display");
    wrapper.unmount();
  });

  it("renders bounded system messages instead of the generic compatibility placeholder", () => {
    const wrapper = mount(RecordFeed, {
      props: {
        items: [
          {
            recordSeq: 1,
            createdAt: "2026-10-20T00:00:00Z",
            kind: "system",
            body: { code: "roundAccepted", message: "回合已提交。\u0000\u0007", data: {} },
          },
          {
            recordSeq: 2,
            createdAt: "2026-10-20T00:00:01Z",
            kind: "system",
            body: { code: "futureSystemEvent", message: " ", data: {} },
          },
          {
            recordSeq: 3,
            createdAt: "2026-10-20T00:00:02Z",
            kind: "system",
            body: {
              code: "longSystemEvent",
              message: `${"x".repeat(1_999)}😀${"y".repeat(10)}`,
              data: {},
            },
          },
        ],
        loadingOlder: false,
        canRewind: false,
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.text()).toContain("回合已提交。");
    expect(wrapper.findAll("li")[0]!.find("p").text()).toBe("回合已提交。");
    expect(wrapper.text()).toContain(
      "The system record is preserved, but it has no displayable message.",
    );
    expect(wrapper.text()).not.toContain("This record uses a format this version cannot display.");
    expect(wrapper.text()).not.toContain("futureSystemEvent");
    expect(wrapper.findAll("li")[2]!.find("p").text()).toBe(`${"x".repeat(1_999)}😀…`);
    wrapper.unmount();
  });
});
