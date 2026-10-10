export type PanelState = "collapsed" | "hover" | "expanded" | "pinned";
type PanelTimer = "none" | "open" | "close";
export type PanelEvent =
  | "triggerEnter"
  | "triggerLeave"
  | "panelEnter"
  | "panelLeave"
  | "open"
  | "close"
  | "openDelay"
  | "closeDelay"
  | "interact"
  | "pin"
  | "unpin"
  | "escape";

export interface PanelTransition {
  state: PanelState;
  timer: PanelTimer;
  focusComposer: boolean;
}

/** 将面板交互映射为目标状态和由 composable 持有的唯一计时器效果。 */
export function transitionPanel(
  state: PanelState,
  event: PanelEvent,
  protectedFromClose = false,
): PanelTransition {
  const result = (next = state, timer: PanelTimer = "none", focusComposer = false) => ({
    state: next,
    timer,
    focusComposer,
  });
  switch (event) {
    case "triggerEnter":
      return state === "collapsed" ? result(state, "open") : result();
    case "triggerLeave":
      return state === "collapsed" ? result(state) : result();
    case "panelEnter":
      return state === "hover" ? result(state) : result();
    case "panelLeave":
      return state === "hover" ? result(state, "close") : result();
    case "open":
      return state === "pinned" ? result() : result("expanded", "none", true);
    case "interact":
      return state === "hover" ? result("expanded") : result();
    case "close":
    case "escape":
      return state === "expanded" ? result("collapsed") : result();
    case "openDelay":
      return state === "collapsed" ? result("hover") : result();
    case "closeDelay":
      if (state !== "hover" || protectedFromClose) return result();
      return result("collapsed");
    case "pin":
      return state === "expanded" ? result("pinned") : result();
    case "unpin":
      return state === "pinned" ? result("expanded") : result();
  }
}

type InputMode = "inCharacter" | "outOfCharacter";
type SlashCommand = "interrupt" | "stop" | "resume" | "regenerate" | "rewind" | "pin" | "latest";
const slashCommands: readonly SlashCommand[] = [
  "interrupt",
  "stop",
  "resume",
  "regenerate",
  "rewind",
  "pin",
  "latest",
];

function isRustWhitespace(character: string): boolean {
  const codePoint = character.codePointAt(0)!;
  return (
    (codePoint >= 0x09 && codePoint <= 0x0d) ||
    codePoint === 0x20 ||
    codePoint === 0x85 ||
    codePoint === 0xa0 ||
    codePoint === 0x1680 ||
    (codePoint >= 0x2000 && codePoint <= 0x200a) ||
    codePoint === 0x2028 ||
    codePoint === 0x2029 ||
    codePoint === 0x202f ||
    codePoint === 0x205f ||
    codePoint === 0x3000
  );
}
function trimRustWhitespaceStart(text: string): string {
  let offset = 0;
  for (const character of text) {
    if (!isRustWhitespace(character)) break;
    offset += character.length;
  }
  return text.slice(offset);
}
function isRustWhitespaceOnly(text: string): boolean {
  return [...text].every(isRustWhitespace);
}

export interface ComposerPreview {
  mode: InputMode;
  slashActive: boolean;
  candidates: SlashCommand[];
  command?: { name: SlashCommand; argument: string };
  invalidSlash: boolean;
  contentRequired: boolean;
}

export interface ComposerAvailability {
  phase: string;
  resumeRequired: boolean;
  needsRecovery: boolean;
  hasInFlight: boolean;
  hasLastOperation: boolean;
}

/** Keep command controls available when ordinary action submission is unavailable. */
export function canSubmitComposer(
  preview: ComposerPreview,
  state: ComposerAvailability | undefined,
  blocked: boolean,
): boolean {
  if (blocked || !state || preview.slashActive || preview.invalidSlash || preview.contentRequired)
    return false;

  const command = preview.command;
  if (!command) return state.phase === "idle" && !state.resumeRequired && !state.needsRecovery;

  switch (command.name) {
    case "stop":
      return state.hasInFlight && !state.needsRecovery;
    case "interrupt":
      return state.hasInFlight && Boolean(command.argument.trim()) && !state.needsRecovery;
    case "resume":
      return state.resumeRequired && !state.needsRecovery;
    case "regenerate":
      return (
        state.phase === "idle" &&
        !state.needsRecovery &&
        !state.resumeRequired &&
        state.hasLastOperation
      );
    case "rewind":
      return state.phase === "idle" && !state.needsRecovery && !state.resumeRequired;
    case "pin":
    case "latest":
      return true;
  }
}

/** 仅预览明确支持的输入前缀；提交原文和命令权限仍由调用层 / Rust 决定。 */
export function previewComposerInput(text: string): ComposerPreview {
  if (text.startsWith("\\/"))
    return {
      mode: "inCharacter",
      slashActive: false,
      candidates: [],
      invalidSlash: false,
      contentRequired: false,
    };

  if (text.startsWith("/ooc")) {
    const rest = text.slice(4);
    if (!rest)
      return {
        mode: "outOfCharacter",
        slashActive: false,
        candidates: [],
        invalidSlash: false,
        contentRequired: true,
      };
    if (isRustWhitespace([...rest][0]!)) {
      const content = trimRustWhitespaceStart(rest);
      return {
        mode: "outOfCharacter",
        slashActive: false,
        candidates: [],
        invalidSlash: false,
        contentRequired: isRustWhitespaceOnly(content),
      };
    }
  }

  if (text.startsWith("((") && text.endsWith("))"))
    return {
      mode: "outOfCharacter",
      slashActive: false,
      candidates: [],
      invalidSlash: false,
      contentRequired: isRustWhitespaceOnly(text.slice(2, -2)),
    };

  if (!text.startsWith("/"))
    return {
      mode: "inCharacter",
      slashActive: false,
      candidates: [],
      invalidSlash: false,
      contentRequired: false,
    };

  const match = /^\/([a-z-]*)(?:\s+([\s\S]*))?$/u.exec(text);
  if (!match)
    return {
      mode: "inCharacter",
      slashActive: true,
      candidates: [],
      invalidSlash: true,
      contentRequired: false,
    };
  const name = match[1]!;
  const argument = match[2] ?? "";
  const command = slashCommands.find((candidate) => candidate === name);
  if (command)
    return {
      mode: "inCharacter",
      slashActive: false,
      candidates: [],
      command: { name: command, argument },
      invalidSlash: false,
      contentRequired: false,
    };

  const candidates = slashCommands.filter((candidate) => candidate.startsWith(name));
  return {
    mode: "inCharacter",
    slashActive: true,
    candidates,
    invalidSlash: candidates.length === 0,
    contentRequired: false,
  };
}
