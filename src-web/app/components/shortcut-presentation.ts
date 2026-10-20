/**
 * 052 的快捷键展示数据。057 接入唯一动作注册表后会以其取代此视图清单。
 */
type ShortcutPresentationRow = {
  id: string;
  labelKey: string;
  noteKey: string;
  statusKey: string;
  keys: string[];
  status: "adopted" | "documented" | "unbound";
  locked?: boolean;
};

type ShortcutPresentationGroup = {
  id: string;
  labelKey: string;
  rows: ShortcutPresentationRow[];
};

export const shortcutGroups: ShortcutPresentationGroup[] = [
  {
    id: "global",
    labelKey: "shortcuts.groups.global",
    rows: [
      {
        id: "settings",
        labelKey: "shortcuts.actions.settings",
        noteKey: "shortcuts.notes.settings",
        statusKey: "shortcuts.status.adopted",
        keys: ["Mod+,"],
        status: "adopted",
      },
      {
        id: "help",
        labelKey: "shortcuts.actions.help",
        noteKey: "shortcuts.notes.help",
        statusKey: "shortcuts.status.adopted",
        keys: ["?", "Mod+/"],
        status: "adopted",
      },
      {
        id: "dismiss",
        labelKey: "shortcuts.actions.dismiss",
        noteKey: "shortcuts.notes.dismiss",
        statusKey: "shortcuts.status.documented",
        keys: ["Escape"],
        status: "documented",
        locked: true,
      },
    ],
  },
  {
    id: "stage",
    labelKey: "shortcuts.groups.stage",
    rows: [
      {
        id: "open-panel",
        labelKey: "shortcuts.actions.openPanel",
        noteKey: "shortcuts.notes.openPanel",
        statusKey: "shortcuts.status.documented",
        keys: ["Mod+Shift+M"],
        status: "documented",
      },
      {
        id: "pin-panel",
        labelKey: "shortcuts.actions.pinPanel",
        noteKey: "shortcuts.notes.pinPanel",
        statusKey: "shortcuts.status.adopted",
        keys: ["Mod+Shift+P"],
        status: "adopted",
      },
      {
        id: "latest",
        labelKey: "shortcuts.actions.latest",
        noteKey: "shortcuts.notes.latest",
        statusKey: "shortcuts.status.adopted",
        keys: ["End"],
        status: "adopted",
      },
    ],
  },
  {
    id: "input",
    labelKey: "shortcuts.groups.input",
    rows: [
      {
        id: "send",
        labelKey: "shortcuts.actions.send",
        noteKey: "shortcuts.notes.send",
        statusKey: "shortcuts.status.documented",
        keys: ["Enter"],
        status: "documented",
        locked: true,
      },
      {
        id: "newline",
        labelKey: "shortcuts.actions.newline",
        noteKey: "shortcuts.notes.newline",
        statusKey: "shortcuts.status.documented",
        keys: ["Shift+Enter"],
        status: "documented",
        locked: true,
      },
      {
        id: "roll",
        labelKey: "shortcuts.actions.roll",
        noteKey: "shortcuts.notes.roll",
        statusKey: "shortcuts.status.adopted",
        keys: ["Mod+Enter"],
        status: "adopted",
      },
      {
        id: "stop",
        labelKey: "shortcuts.actions.stop",
        noteKey: "shortcuts.notes.stop",
        statusKey: "shortcuts.status.unbound",
        keys: [],
        status: "unbound",
      },
      {
        id: "interrupt",
        labelKey: "shortcuts.actions.interrupt",
        noteKey: "shortcuts.notes.interrupt",
        statusKey: "shortcuts.status.unbound",
        keys: [],
        status: "unbound",
      },
    ],
  },
  {
    id: "commands",
    labelKey: "shortcuts.groups.commands",
    rows: [
      {
        id: "open-menu",
        labelKey: "shortcuts.actions.openMenu",
        noteKey: "shortcuts.notes.openMenu",
        statusKey: "shortcuts.status.documented",
        keys: ["/"],
        status: "documented",
        locked: true,
      },
      {
        id: "move-candidate",
        labelKey: "shortcuts.actions.moveCandidate",
        noteKey: "shortcuts.notes.moveCandidate",
        statusKey: "shortcuts.status.documented",
        keys: ["ArrowUp", "ArrowDown"],
        status: "documented",
        locked: true,
      },
      {
        id: "complete-candidate",
        labelKey: "shortcuts.actions.completeCandidate",
        noteKey: "shortcuts.notes.completeCandidate",
        statusKey: "shortcuts.status.adopted",
        keys: ["Tab", "Enter"],
        status: "adopted",
        locked: true,
      },
      {
        id: "close-menu",
        labelKey: "shortcuts.actions.closeMenu",
        noteKey: "shortcuts.notes.closeMenu",
        statusKey: "shortcuts.status.documented",
        keys: ["Escape"],
        status: "documented",
        locked: true,
      },
    ],
  },
  {
    id: "dialogs",
    labelKey: "shortcuts.groups.dialogs",
    rows: [
      {
        id: "move-focus",
        labelKey: "shortcuts.actions.moveFocus",
        noteKey: "shortcuts.notes.moveFocus",
        statusKey: "shortcuts.status.documented",
        keys: ["Tab", "Shift+Tab"],
        status: "documented",
        locked: true,
      },
      {
        id: "close-dialog",
        labelKey: "shortcuts.actions.closeDialog",
        noteKey: "shortcuts.notes.closeDialog",
        statusKey: "shortcuts.status.documented",
        keys: ["Escape"],
        status: "documented",
        locked: true,
      },
      {
        id: "activate",
        labelKey: "shortcuts.actions.activate",
        noteKey: "shortcuts.notes.activate",
        statusKey: "shortcuts.status.adopted",
        keys: ["Enter"],
        status: "adopted",
        locked: true,
      },
      {
        id: "move-section",
        labelKey: "shortcuts.actions.moveSection",
        noteKey: "shortcuts.notes.moveSection",
        statusKey: "shortcuts.status.adopted",
        keys: ["ArrowUp", "ArrowDown"],
        status: "adopted",
        locked: true,
      },
    ],
  },
];
