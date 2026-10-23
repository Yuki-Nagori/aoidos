import { afterEach, describe, expect, it, vi } from "vitest";
import {
  createDiceEngine,
  createDiceSceneCancellation,
  diceRenderer,
  projectDicePoint,
  renderAndSubmitDiceFrame,
  retainDiceRenderer,
  waitForDiceSceneReady,
  wakeDice,
} from "../../../../../src-web/features/game/utils/dice-renderer";

afterEach(() => vi.unstubAllGlobals());

describe("共享骰子渲染器", () => {
  it("先提交 GPU 帧，再读取共享画布进行二维合成", () => {
    const order: string[] = [];
    const ready = renderAndSubmitDiceFrame(
      {
        beginFrame: () => order.push("begin"),
        endFrame: () => order.push("submit"),
      },
      () => {
        order.push("render");
        return true;
      },
    );

    expect(ready).toBe(true);
    expect(order).toEqual(["begin", "render", "submit"]);
  });

  it("场景取消时释放一次资源，并结束仍未完成的 ready 等待", async () => {
    const dispose = vi.fn();
    const cancellation = createDiceSceneCancellation(dispose);
    void cancellation.promise.catch(() => {});
    const ready = waitForDiceSceneReady(new Promise(() => {}), cancellation.promise);
    cancellation.cancel();
    cancellation.cancel();
    await expect(ready).rejects.toThrow("dice-render-cancelled");
    expect(dispose).toHaveBeenCalledOnce();
  });

  it("场景就绪后正常继续", async () => {
    const cancellation = createDiceSceneCancellation(vi.fn());
    await expect(
      waitForDiceSceneReady(Promise.resolve(), cancellation.promise),
    ).resolves.toBeUndefined();
  });

  it("仅在最后一个托盘或预览卸载时释放共享 GPU，重复清理不会提前释放", async () => {
    const dispose = vi.fn();
    diceRenderer.engine = Promise.resolve({ dispose } as unknown as Awaited<
      ReturnType<typeof createDiceEngine>
    >);
    diceRenderer.canvas = document.createElement("canvas");
    diceRenderer.environment = [new Uint8Array(3)];
    const releaseTray = retainDiceRenderer(),
      releasePreview = retainDiceRenderer();
    releaseTray();
    releaseTray();
    await Promise.resolve();
    expect(dispose).not.toHaveBeenCalled();
    releasePreview();
    await Promise.resolve();
    expect(dispose).toHaveBeenCalledOnce();
    expect(diceRenderer.engine).toBeUndefined();
    expect(diceRenderer.canvas).toBeUndefined();
    expect(diceRenderer.environment).toBeUndefined();
    expect(diceRenderer.live.size).toBe(0);
  });
  function engines(supported: Promise<boolean>, init = vi.fn(async () => {})) {
    const dispose = vi.fn();
    const glCanvases: HTMLCanvasElement[] = [];
    const gpuCanvases: HTMLCanvasElement[] = [];
    class GL {
      constructor(canvas: HTMLCanvasElement) {
        glCanvases.push(canvas);
      }
    }
    class GPU {
      static IsSupportedAsync = supported;
      constructor(canvas: HTMLCanvasElement) {
        gpuCanvases.push(canvas);
      }
      initAsync = init;
      dispose = dispose;
    }
    return {
      GL: GL as unknown as Parameters<typeof createDiceEngine>[0],
      GPU: GPU as unknown as Parameters<typeof createDiceEngine>[1],
      dispose,
      glCanvases,
      gpuCanvases,
    };
  }
  it("GPU 就绪后使用 GPU；失败候选释放后，WebGL 回退使用独立画布", async () => {
    const gpu = engines(Promise.resolve(true));
    expect(await createDiceEngine(gpu.GL, gpu.GPU)).toBeInstanceOf(gpu.GPU);
    expect(diceRenderer.canvas?.width).toBe(16);
    const failed = engines(
      Promise.resolve(true),
      vi.fn(async () => {
        throw new Error("adapter lost");
      }),
    );
    expect(await createDiceEngine(failed.GL, failed.GPU)).toBeInstanceOf(failed.GL);
    expect(failed.dispose).toHaveBeenCalledOnce();
    expect(failed.glCanvases[0]).not.toBe(failed.gpuCanvases[0]);
    expect(diceRenderer.canvas).toBe(failed.glCanvases[0]);
    const unavailable = engines(Promise.resolve(false));
    expect(await createDiceEngine(unavailable.GL, unavailable.GPU)).toBeInstanceOf(unavailable.GL);
    expect(unavailable.glCanvases[0]).not.toBe(unavailable.gpuCanvases[0]);
    const rejected = engines(Promise.reject(new Error("unsupported")));
    expect(await createDiceEngine(rejected.GL, rejected.GPU)).toBeInstanceOf(rejected.GL);
  });

  it("多个实例共用一个 RAF，静止实例退出后仍继续绘制活动实例", () => {
    const frames: FrameRequestCallback[] = [];
    const requestFrame = vi.fn((callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    });
    vi.stubGlobal("requestAnimationFrame", requestFrame);
    let rolling = true;
    const moving = vi.fn(() => rolling),
      still = vi.fn(() => false);
    wakeDice(moving);
    wakeDice(still);
    wakeDice(moving);
    expect(requestFrame).toHaveBeenCalledOnce();
    frames[0]!(100);
    expect(still).toHaveBeenCalledOnce();
    expect(diceRenderer.live.has(still)).toBe(false);
    expect(requestFrame).toHaveBeenCalledTimes(2);
    rolling = false;
    frames[1]!(116);
    expect(diceRenderer.live.size).toBe(0);
    expect(requestFrame).toHaveBeenCalledTimes(2);
  });
});

describe("骰子画布投影", () => {
  it("原点位于画布中心，正Y向上，设备像素比同时缩放位置与特效半径", () => {
    const identity = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
    expect(projectDicePoint({ x: 0, y: 0, z: 0 }, identity, 240, 120)).toEqual({ x: 120, y: 60 });
    expect(projectDicePoint({ x: 0.5, y: 0.5, z: 0 }, identity, 240, 120)).toEqual({
      x: 180,
      y: 30,
    });
    expect(projectDicePoint({ x: 0.5, y: 0.5, z: 0 }, identity, 480, 240)).toEqual({
      x: 360,
      y: 60,
    });
  });
  it("透视除法在视口映射之前完成，前后两颗骰子不会共用错位的光晕中心", () => {
    const projection = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1];
    expect(projectDicePoint({ x: 0.5, y: 0.5, z: 1 }, projection, 240, 120)).toEqual({
      x: 150,
      y: 45,
    });
    expect(projectDicePoint({ x: -0.5, y: -0.5, z: 0 }, projection, 240, 120)).toEqual({
      x: 60,
      y: 90,
    });
  });
});
