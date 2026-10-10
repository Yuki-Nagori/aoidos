import { describe, expect, it } from "vitest";
import {
  previewComposerInput,
  canSubmitComposer,
  transitionPanel,
  type PanelState,
} from "../../../src-web/utils/ui-shell";

describe("panel transitions", () => {
  it.each([
    ["collapsed", "triggerEnter", "collapsed", "open"],
    ["expanded", "triggerEnter", "expanded", "none"],
    ["hover", "panelEnter", "hover", "none"],
    ["collapsed", "panelEnter", "collapsed", "none"],
    ["expanded", "panelLeave", "expanded", "none"],
    ["pinned", "open", "pinned", "none"],
    ["expanded", "interact", "expanded", "none"],
    ["expanded", "close", "collapsed", "none"],
    ["collapsed", "pin", "collapsed", "none"],
    ["collapsed", "unpin", "collapsed", "none"],
    ["collapsed", "openDelay", "hover", "none"],
    ["hover", "panelLeave", "hover", "close"],
    ["hover", "closeDelay", "collapsed", "none"],
    ["hover", "interact", "expanded", "none"],
    ["expanded", "pin", "pinned", "none"],
    ["pinned", "escape", "pinned", "none"],
    ["pinned", "unpin", "expanded", "none"],
  ] as const)("%s + %s -> %s", (state, event, expectedState, timer) => {
    const result = transitionPanel(state, event);
    expect(result.state).toBe(expectedState);
    expect(result.timer).toBe(timer);
  });

  it("does not auto-close while protected and only explicit open takes focus", () => {
    expect(transitionPanel("hover", "closeDelay", true).state).toBe("hover");
    expect(transitionPanel("collapsed", "open")).toMatchObject({
      state: "expanded",
      focusComposer: true,
    });
    expect(transitionPanel("hover", "openDelay").state).toBe("hover");
  });

  it("keeps transitions typed over each panel state", () => {
    const states: PanelState[] = ["collapsed", "hover", "expanded", "pinned"];
    for (const state of states) expect(transitionPanel(state, "triggerLeave").state).toBe(state);
  });
});

describe("composer preview", () => {
  it.each([
    ["玩家行动", "inCharacter"],
    ["/ooc 我离开房间", "outOfCharacter"],
    ["((我在门外低声说))", "outOfCharacter"],
  ] as const)("classifies %s", (text, mode) => {
    expect(previewComposerInput(text).mode).toBe(mode);
  });

  it("treats escaped, incomplete, and unknown slash input safely", () => {
    expect(previewComposerInput("\\/literal").slashActive).toBe(false);
    expect(previewComposerInput("/int").candidates).toContain("interrupt");
    expect(previewComposerInput("/unknown").invalidSlash).toBe(true);
    expect(previewComposerInput("/停止")).toMatchObject({ slashActive: true, invalidSlash: true });
    expect(previewComposerInput("/").candidates).toHaveLength(7);
    expect(previewComposerInput("(( ))")).toMatchObject({
      mode: "outOfCharacter",
      contentRequired: true,
    });
    expect(previewComposerInput("/ooc").contentRequired).toBe(true);
    expect(previewComposerInput("  /ooc 内容")).toMatchObject({
      mode: "inCharacter",
      slashActive: false,
    });
    expect(previewComposerInput("  /stop")).toMatchObject({
      mode: "inCharacter",
      slashActive: false,
    });
    expect(previewComposerInput("  /unknown")).toMatchObject({
      mode: "inCharacter",
      slashActive: false,
    });
    expect(previewComposerInput("/ooc\u0085内容").mode).toBe("outOfCharacter");
    expect(previewComposerInput("/ooc\uFEFF内容").invalidSlash).toBe(true);
    expect(previewComposerInput("/ooc \u0085").contentRequired).toBe(true);
    expect(previewComposerInput("((\u0085))").contentRequired).toBe(true);
    expect(previewComposerInput("((内容))\n").mode).toBe("inCharacter");
  });

  it("parses only registered commands without changing their argument text", () => {
    expect(previewComposerInput("/interrupt  原文 ")).toMatchObject({
      command: { name: "interrupt", argument: "原文 " },
      slashActive: false,
    });
  });
});

describe("composer submit availability", () => {
  const idle = {
    phase: "idle",
    resumeRequired: false,
    needsRecovery: false,
    hasInFlight: false,
    hasLastOperation: true,
  } as const;

  it("keeps valid turn controls clickable outside the ordinary idle phase", () => {
    const generating = { ...idle, phase: "generating", hasInFlight: true };
    const paused = { ...idle, phase: "paused", resumeRequired: true };
    expect(canSubmitComposer(previewComposerInput("/stop"), generating, false)).toBe(true);
    expect(canSubmitComposer(previewComposerInput("/interrupt wait"), generating, false)).toBe(
      true,
    );
    expect(canSubmitComposer(previewComposerInput("/resume"), paused, false)).toBe(true);
    expect(
      canSubmitComposer(previewComposerInput("/resume"), { ...paused, needsRecovery: true }, false),
    ).toBe(false);
  });

  it("blocks unavailable commands and regular actions during recovery or busy work", () => {
    expect(canSubmitComposer(previewComposerInput("/stop"), idle, false)).toBe(false);
    expect(
      canSubmitComposer(previewComposerInput("act"), { ...idle, needsRecovery: true }, false),
    ).toBe(false);
    expect(canSubmitComposer(previewComposerInput("act"), idle, true)).toBe(false);
  });

  it("requires a settled turn and a previous operation before regenerating", () => {
    const regenerate = previewComposerInput("/regenerate");
    expect(canSubmitComposer(regenerate, idle, false)).toBe(true);
    expect(canSubmitComposer(regenerate, { ...idle, hasLastOperation: false }, false)).toBe(false);
    expect(canSubmitComposer(regenerate, { ...idle, phase: "generating" }, false)).toBe(false);
    expect(canSubmitComposer(regenerate, { ...idle, needsRecovery: true }, false)).toBe(false);
    expect(canSubmitComposer(regenerate, { ...idle, resumeRequired: true }, false)).toBe(false);
  });

  it("allows choosing a rewind target only after the turn has settled", () => {
    const rewind = previewComposerInput("/rewind");
    expect(canSubmitComposer(rewind, { ...idle, hasLastOperation: false }, false)).toBe(true);
    expect(canSubmitComposer(rewind, { ...idle, phase: "generating" }, false)).toBe(false);
    expect(canSubmitComposer(rewind, { ...idle, needsRecovery: true }, false)).toBe(false);
    expect(canSubmitComposer(rewind, { ...idle, resumeRequired: true }, false)).toBe(false);
  });

  it.each(["/pin", "/latest"])(
    "keeps local navigation %s available during generation and recovery",
    (command) => {
      const preview = previewComposerInput(command);
      expect(
        canSubmitComposer(
          preview,
          { ...idle, phase: "generating", needsRecovery: true, hasInFlight: true },
          false,
        ),
      ).toBe(true);
      expect(canSubmitComposer(preview, idle, true)).toBe(false);
      expect(canSubmitComposer(preview, undefined, false)).toBe(false);
    },
  );
});

it("treats an ooc prefix with no separator as an unknown command, preserving its argument", () => {
  expect(previewComposerInput("/oocextra")).toMatchObject({
    mode: "inCharacter",
    slashActive: true,
    invalidSlash: true,
  });
  expect(previewComposerInput("/ooc内容")).toMatchObject({
    mode: "inCharacter",
    slashActive: true,
    invalidSlash: true,
  });
});
