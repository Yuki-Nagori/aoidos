import { mount } from "@vue/test-utils";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RecordItem } from "../../../../../src-web/api/records";
import RecordFeed from "../../../../../src-web/features/game/components/RecordFeed.vue";
import { i18n } from "../../../../../src-web/i18n/index";

describe("RecordFeed", () => {
  beforeEach(() => {
    i18n.global.locale.value = "en";
  });

  it("copies a failed request code and delegates recovery to the existing host actions", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    const retry = vi.fn();
    const rewind = vi.fn();
    const wrapper = mount(RecordFeed, {
      props: {
        items: [],
        loadingOlder: false,
        preview: {
          text: "Partial narration",
          outcome: "failed",
          error: { code: "provider.timeout", message: "Timed out" },
        },
        onRetry: retry,
        onRequestRewind: rewind,
        actionsBusy: true,
      },
      global: { plugins: [i18n] },
    });
    const card = wrapper.get(".record-failure");
    expect(wrapper.text()).toContain("Partial narration");
    expect(card.text()).toContain("Timed out");
    expect(
      card
        .findAll(".record-failure-actions button")
        .every((button) => button.attributes("disabled") !== undefined),
    ).toBe(true);
    await wrapper.setProps({ actionsBusy: false });
    await card.get(".record-failure-code button").trigger("click");
    expect(writeText).toHaveBeenCalledWith("provider.timeout");
    await card.get(".record-failure-actions .primary").trigger("click");
    await card.get(".record-failure-actions .ghost").trigger("click");
    expect(retry).toHaveBeenCalledTimes(1);
    expect(rewind).toHaveBeenCalledTimes(1);
    wrapper.unmount();
    Reflect.deleteProperty(navigator, "clipboard");
  });

  it("does not render trailing blank lines while narration is still streaming", () => {
    const wrapper = mount(RecordFeed, {
      props: { items: [], loadingOlder: false, preview: { text: "First line.\n\n\n" } },
      global: { plugins: [i18n] },
    });

    expect(wrapper.get(".record-live").element.textContent).toBe("First line.");
    wrapper.unmount();
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
        body: {
          diceSeq: 2,
          dc: 9,
          result: "success",
          ruleId: "pbta-explore-v1",
          planId: "plan",
        },
      },
    ];
    const wrapper = mount(RecordFeed, {
      props: {
        items,
        characters: [{ id: "keeper", canonicalName: "Lantern keeper" }],
        olderCursor: "cursor",
        loadingOlder: false,
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).toContain("Open the door.");
    expect(wrapper.text()).toContain("17");
    expect(
      wrapper.findAll(".record-machine .record-dice-value").map((value) => value.text()),
    ).toEqual(["17"]);
    expect(wrapper.find("canvas").exists()).toBe(false);
    expect(wrapper.find(".record-dice-visual").exists()).toBe(false);
    expect(wrapper.text()).toContain("Lantern keeper");
    expect(wrapper.text()).toContain("The lamp leans toward the door.");
    expect(wrapper.text()).not.toContain("DC 9");
    const checkRow = wrapper.findAll(".record-row")[6]!;
    expect(checkRow.text()).toContain("Exploration check");
    expect(checkRow.find(".record-result").text()).toBe("Success");
    expect(checkRow.text()).not.toContain("1d20");
    expect(checkRow.text()).not.toContain("17");
    expect(wrapper.find(".record-player-bubble").exists()).toBe(true);
    expect(wrapper.find(".record-character-bubble").exists()).toBe(true);
    expect(wrapper.find(".record-narrator").exists()).toBe(true);
    expect(wrapper.find(".record-entry").exists()).toBe(false);
    expect(wrapper.text()).toContain("Out of character");
    await wrapper.get("button").trigger("click");
    expect(wrapper.emitted("loadOlder")).toHaveLength(1);
    await wrapper.findAll("button")[1]!.trigger("click");
    expect(wrapper.emitted("loadBody")).toEqual([["body-ref"]]);
    wrapper.unmount();
  });

  it("renders a live preview without a misleading empty state", () => {
    const wrapper = mount(RecordFeed, {
      props: {
        items: [],
        loadingOlder: false,
        preview: { text: "partial" },
        expandedBody: "expanded",
      },
      global: { plugins: [i18n] },
    });
    expect(wrapper.text()).not.toContain("No records yet");
    expect(wrapper.text()).toContain("partial");
    expect(wrapper.get(".record-list").find(".record-live-row").exists()).toBe(true);
    expect(wrapper.text()).toContain("expanded");
    expect(wrapper.findAll(".record-terminal")).toHaveLength(0);
    wrapper.unmount();
  });

  it("omits internal terminal and engine-confirmation status from the story feed", () => {
    const wrapper = mount(RecordFeed, {
      props: {
        items: [
          {
            recordSeq: 1,
            createdAt: "2026-10-22T00:00:00Z",
            kind: "narration",
            body: {
              text: "The stair descends into darkness.",
              turnId: "turn-1",
              outcome: "completed",
              finishReason: "stop",
            },
          },
          {
            recordSeq: 2,
            createdAt: "2026-10-22T00:00:01Z",
            kind: "system",
            body: { code: "roundCompleted", message: "引擎确认回合事实", data: {} },
          },
          {
            recordSeq: 3,
            createdAt: "2026-10-22T00:00:02Z",
            kind: "narration",
            body: {
              text: "The response could not be completed.",
              turnId: "turn-2",
              outcome: "failed",
              error: { code: "llm.unavailable", message: "Provider unavailable." },
            },
          },
        ],
        loadingOlder: false,
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.findAll(".record-terminal")).toHaveLength(0);
    expect(wrapper.findAll(".record-system")).toHaveLength(0);
    expect(wrapper.text()).toContain("The stair descends into darkness.");
    expect(wrapper.text()).not.toContain("Normal end");
    expect(wrapper.text()).not.toContain("引擎确认回合事实");
    expect(wrapper.text()).toContain(i18n.global.t("game.operationFailed"));
    expect(wrapper.text()).toContain("Provider unavailable.");
    expect(wrapper.text()).toContain("Diagnostic code: llm.unavailable");
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

  it("hides generic engine facts while preserving actionable check status", () => {
    const wrapper = mount(RecordFeed, {
      props: {
        items: [
          [1, "roundAccepted"],
          [2, "checkPlanned"],
          [3, "worldApplied"],
          [4, "worldApplied"],
        ].map(([recordSeq, code]) => ({
          recordSeq: Number(recordSeq),
          createdAt: `2026-10-20T00:00:0${recordSeq}Z`,
          kind: "system",
          turnId: "turn-1",
          body: {
            code: String(code),
            message: "引擎确认回合事实",
            turnId: "turn-1",
            data: {},
          },
        })),
        loadingOlder: false,
      },
      global: { plugins: [i18n] },
    });

    expect(wrapper.findAll(".record-system")).toHaveLength(0);
    expect(wrapper.findAll(".record-row")).toHaveLength(1);
    expect(wrapper.text()).toContain(i18n.global.t("game.checkPlanned"));
    wrapper.unmount();
  });

  it("selects a rewind target from a record row without rendering a button", async () => {
    const wrapper = mount(RecordFeed, {
      props: {
        rewindSelectorOpen: true,
        items: [
          {
            recordSeq: 8,
            createdAt: "2026-10-20T00:00:08Z",
            kind: "system",
            body: { code: "checkPlanned", message: "", data: {} },
          },
        ],
        loadingOlder: false,
      },
      global: { plugins: [i18n] },
    });
    const row = wrapper.find(".record-row");
    expect(row.attributes("tabindex")).toBe("0");
    expect(row.find("button").exists()).toBe(false);
    await row.trigger("click");
    expect(wrapper.emitted("rewind")).toEqual([[8]]);
    wrapper.unmount();
  });
});
