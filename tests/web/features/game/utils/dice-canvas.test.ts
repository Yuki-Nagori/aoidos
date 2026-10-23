import { afterEach, describe, expect, it, vi } from "vitest";
import {
  drawDiceAtlas,
  drawFlatDiceCanvas,
  hashStr,
  seededNumber,
} from "../../../../../src-web/features/game/utils/dice-canvas";
import { buildDiceFaces } from "../../../../../src-web/features/game/utils/dice-geometry";
import type { DiceVisualShape } from "../../../../../src-web/features/game/utils/dice-visual";
import { diceSkins } from "../../../../../src-web/features/game/utils/dice-visual";

function createContext() {
  const gradient = { addColorStop: vi.fn() };
  const context = new Proxy(
    {
      createLinearGradient: vi.fn(() => gradient),
      createRadialGradient: vi.fn(() => gradient),
    } as Record<string, unknown>,
    {
      get(target, key: string) {
        if (!(key in target)) target[key] = vi.fn();
        return target[key];
      },
    },
  );
  return { context, gradient };
}

afterEach(() => vi.restoreAllMocks());

describe("骰子画布绘制", () => {
  it("使用稳定种子生成确定的图案随机序列", () => {
    expect(hashStr("mistbell")).toBe(hashStr("mistbell"));
    expect(hashStr("mistbell")).not.toBe(hashStr("jade"));
    const first = seededNumber(321),
      second = seededNumber(321);
    expect([first(), first(), first()]).toEqual([second(), second(), second()]);
    expect(seededNumber(-1)()).toBeGreaterThanOrEqual(0);
  });

  it("绘制各类皮肤纹理、d6 点数与不同骰面的数字", () => {
    const { context, gradient } = createContext();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
      context as unknown as CanvasRenderingContext2D,
    );

    for (const skin of diceSkins) {
      const sides = skin.id === "brass" || skin.id === "bone" ? 6 : skin.id === "crystal" ? 8 : 20;
      const atlas = drawDiceAtlas(buildDiceFaces(sides), sides, skin);
      expect(atlas.width).toBeGreaterThan(0);
      expect(atlas.height).toBe(atlas.width);
    }

    expect(gradient.addColorStop).toHaveBeenCalled();
    expect(context.arc).toHaveBeenCalled();
    expect(context.strokeText).toHaveBeenCalled();
    expect(context.fillText).toHaveBeenCalled();
    expect(context.quadraticCurveTo).toHaveBeenCalled();
    expect(context.filter).toBe("none");
  });

  it("在没有二维上下文时返回空图集且不宣称平面回退成功", () => {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null);
    expect(drawDiceAtlas(buildDiceFaces(6), 6, diceSkins[0]!).width).toBeGreaterThan(0);
    const canvas = document.createElement("canvas");
    expect(drawFlatDiceCanvas(canvas, { count: 1, sides: 6, values: [4] }, diceSkins[0]!)).toBe(
      false,
    );
  });

  it("按视口与像素比绘制平面回退，并覆盖 d6、d8 与 d20 的轮廓", () => {
    const { context } = createContext();
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(
      context as unknown as CanvasRenderingContext2D,
    );
    vi.stubGlobal("devicePixelRatio", 3);
    const canvas = document.createElement("canvas");
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({
      width: 120,
      height: 80,
    } as DOMRect);

    for (const sides of [6, 8, 20] as const) {
      const shape: DiceVisualShape = { count: 2, sides, values: [6, 9] };
      expect(drawFlatDiceCanvas(canvas, shape, diceSkins[0]!)).toBe(true);
      expect(canvas.width).toBe(240);
      expect(canvas.height).toBe(160);
    }
    expect(context.fillText).toHaveBeenCalledWith("6", expect.any(Number), expect.any(Number));
    expect(context.fillText).toHaveBeenCalledWith("9", expect.any(Number), expect.any(Number));
  });
});
