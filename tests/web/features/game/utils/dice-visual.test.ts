import { describe, expect, it } from "vitest";
import {
  diceCheckTotal,
  diceSkins,
  diceVisualShape,
  resolveDiceSkin,
} from "../../../../../src-web/features/game/utils/dice-visual";

describe("骰子可视化事实映射", () => {
  it("只接受与骰式一致的已提交骰点", () => {
    expect(
      diceVisualShape("2d6", [
        { sides: 6, value: 2 },
        { sides: 6, value: 5 },
      ]),
    ).toEqual({ count: 2, sides: 6, values: [2, 5] });
    expect(diceVisualShape("1d20", [{ sides: 20, value: 17 }])).toEqual({
      count: 1,
      sides: 20,
      values: [17],
    });
    expect(diceVisualShape("1d8", [{ sides: 8, value: 0 }])).toBeUndefined();
    expect(diceVisualShape("2d6", [])).toBeUndefined();
    expect(diceVisualShape("2d6", [], true)).toEqual({ count: 2, sides: 6, values: [] });
  });

  it("未知、超限或与骰点载荷不一致的骰式使用静态记录", () => {
    expect(diceVisualShape("3d6", [{ sides: 6, value: 1 }])).toBeUndefined();
    expect(
      diceVisualShape("2d6+1", [
        { sides: 6, value: 2 },
        { sides: 6, value: 5 },
      ]),
    ).toBeUndefined();
    expect(diceVisualShape("1d100", [{ sides: 100, value: 53 }])).toBeUndefined();
  });

  it("使用引擎最终总值，避免重复累加修正", () => {
    const rolls = [
      { sides: 6, value: 3 },
      { sides: 6, value: 4 },
    ];
    expect(diceCheckTotal(rolls, 2, 9)).toBe(9);
    expect(diceCheckTotal(rolls, 2)).toBe(9);
  });

  it("登记完整皮肤集，并让自动模式按剧本绑定皮肤", () => {
    expect(diceSkins.map((skin) => skin.id)).toEqual([
      "theme",
      "obsidian",
      "brass",
      "jade",
      "crystal",
      "bone",
      "mistbell",
    ]);
    expect(resolveDiceSkin("auto", "mistbell", true)).toMatchObject({
      id: "mistbell",
      face: "#2f4f52",
    });
    expect(resolveDiceSkin("auto", "other", true)).toMatchObject({
      id: "theme",
    });
    expect(resolveDiceSkin("auto", "mistbell", false)).toMatchObject({ id: "theme" });
    expect(resolveDiceSkin("bone", "mistbell", true)).toMatchObject({ id: "bone" });
  });
});
