export interface DiceRollFact {
  sides: number;
  value: number;
}

export interface DiceVisualShape {
  count: number;
  sides: 6 | 8 | 20;
  values: readonly number[];
}

export function diceCheckTotal(
  rolls: readonly DiceRollFact[],
  modifierTotal: number,
  authoritativeTotal?: number,
): number {
  return authoritativeTotal ?? rolls.reduce((sum, roll) => sum + roll.value, 0) + modifierTotal;
}

export type DiceSkinId =
  | "auto"
  | "theme"
  | "obsidian"
  | "brass"
  | "jade"
  | "crystal"
  | "bone"
  | "mistbell";

export interface DiceVisualSkin {
  id: Exclude<DiceSkinId, "auto">;
  face: string;
  face2: string;
  pattern: "facet" | "brushed" | "veins" | "speckle" | "mist";
  patternColor: string;
  numeral: string;
  numeralStroke: string;
  edge: string;
  particle: string;
  metalness: number;
  roughness: number;
  clearcoat: number;
  opacity: number;
  d6: "pips" | "numerals";
  font: "serif" | "sans";
  bindScripts?: readonly string[];
}

export const diceSkins: readonly DiceVisualSkin[] = [
  {
    id: "theme",
    face: "theme:--accent",
    face2: "theme:--x-solid",
    pattern: "facet",
    patternColor: "theme:--accent",
    numeral: "theme:--on-accent",
    numeralStroke: "rgba(0,0,0,.25)",
    edge: "theme:--accent",
    particle: "theme:--accent-warm",
    metalness: 0.05,
    roughness: 0.38,
    clearcoat: 0.6,
    opacity: 1,
    d6: "numerals",
    font: "serif",
  },
  {
    id: "obsidian",
    face: "#1d1a24",
    face2: "#0b0a0e",
    pattern: "facet",
    patternColor: "#5b4d78",
    numeral: "#e9cf8c",
    numeralStroke: "#3a2a0a",
    edge: "#6a5b86",
    particle: "#ffcf6b",
    metalness: 0.15,
    roughness: 0.16,
    clearcoat: 1,
    opacity: 1,
    d6: "numerals",
    font: "serif",
  },
  {
    id: "brass",
    face: "#c9a24f",
    face2: "#7a5a22",
    pattern: "brushed",
    patternColor: "#f3d98f",
    numeral: "#2a1b06",
    numeralStroke: "#f7e3a6",
    edge: "#f0d58a",
    particle: "#ffd879",
    metalness: 0.92,
    roughness: 0.3,
    clearcoat: 0.2,
    opacity: 1,
    d6: "pips",
    font: "serif",
  },
  {
    id: "jade",
    face: "#3f9472",
    face2: "#1f5a44",
    pattern: "veins",
    patternColor: "#bfe8cf",
    numeral: "#f6f2e4",
    numeralStroke: "#18402f",
    edge: "#2a6b52",
    particle: "#a8f0c8",
    metalness: 0,
    roughness: 0.28,
    clearcoat: 0.8,
    opacity: 1,
    d6: "numerals",
    font: "serif",
  },
  {
    id: "crystal",
    face: "rgba(178,214,255,0.30)",
    face2: "rgba(120,170,235,0.16)",
    pattern: "facet",
    patternColor: "rgba(255,255,255,.55)",
    numeral: "#ffffff",
    numeralStroke: "#2d5d9a",
    edge: "#e4f1ff",
    particle: "#bfe3ff",
    metalness: 0,
    roughness: 0.04,
    clearcoat: 1,
    opacity: 0.999,
    d6: "numerals",
    font: "sans",
  },
  {
    id: "bone",
    face: "#ece2c8",
    face2: "#cdbd96",
    pattern: "speckle",
    patternColor: "#a8946b",
    numeral: "#3b2a18",
    numeralStroke: "rgba(255,248,230,.6)",
    edge: "#a8946b",
    particle: "#f6e7c1",
    metalness: 0,
    roughness: 0.78,
    clearcoat: 0,
    opacity: 1,
    d6: "pips",
    font: "serif",
  },
  {
    id: "mistbell",
    face: "#2f4f52",
    face2: "#162628",
    pattern: "mist",
    patternColor: "#a9a0d8",
    numeral: "#d5a36f",
    numeralStroke: "#10191a",
    edge: "#477d80",
    particle: "#a9a0d8",
    metalness: 0.35,
    roughness: 0.34,
    clearcoat: 0.7,
    opacity: 1,
    d6: "numerals",
    font: "serif",
    bindScripts: ["mistbell"],
  },
];

const supportedSides = new Set([6, 8, 20]);
const diceScriptBinding = new Map(
  diceSkins.flatMap((skin) =>
    (skin.bindScripts ?? []).map((scriptId) => [scriptId, skin.id] as const),
  ),
);

export function diceVisualShape(
  expression: string,
  rolls: readonly DiceRollFact[],
  allowPending = false,
): DiceVisualShape | undefined {
  const match = /^(\d{1,2})d(6|8|20)$/i.exec(expression.trim());
  if (!match) return undefined;

  const count = Number(match[1]);
  const sides = Number(match[2]);
  if (!count || count > 4 || !supportedSides.has(sides)) return undefined;
  if (rolls.length !== count && !(allowPending && rolls.length === 0)) return undefined;
  if (rolls.some((roll) => roll.sides !== sides || roll.value < 1 || roll.value > sides))
    return undefined;

  return {
    count,
    sides: sides as DiceVisualShape["sides"],
    values: rolls.map((roll) => roll.value),
  };
}

export function resolveDiceSkin(
  id: DiceSkinId,
  scriptId: string,
  scriptSkinEnabled: boolean,
): DiceVisualSkin {
  const resolvedId =
    id === "auto"
      ? scriptSkinEnabled
        ? (diceScriptBinding.get(scriptId) ?? "theme")
        : "theme"
      : id;
  return diceSkins.find((skin) => skin.id === resolvedId) ?? diceSkins[0]!;
}

/** 读取骰子宿主继承的主题颜色，并让浏览器解析 color-mix 与 var。 */
export function resolveDiceSkinColor(value: string, fallback: string, host?: Element): string {
  if (!value.startsWith("theme:")) return value;
  const resolved = getComputedStyle(host ?? document.documentElement)
    .getPropertyValue(value.slice(6))
    .trim();
  if (!resolved || !CSS.supports("color", resolved)) return fallback;
  if (/^color-mix|^var\(/.test(resolved)) {
    const probe = document.createElement("span");
    probe.style.color = resolved;
    document.body.appendChild(probe);
    const color = getComputedStyle(probe).color;
    probe.remove();
    return color;
  }
  return resolved;
}
