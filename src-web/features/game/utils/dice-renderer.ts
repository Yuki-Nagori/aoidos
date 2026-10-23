// 和原型一致：所有骰子共享 GPU 上下文与一个 RAF，各实例只保留二维合成画布。
type DiceEngine =
  | import("@babylonjs/core/Engines/engine").Engine
  | import("@babylonjs/core/Engines/webgpuEngine").WebGPUEngine;
/** 共享引擎与合成源，避免设置页预览占满浏览器 GPU 上下文配额。 */
export const diceRenderer = {
  engine: undefined as Promise<DiceEngine> | undefined,
  canvas: undefined as HTMLCanvasElement | undefined,
  environment: undefined as Uint8Array[] | undefined,
  live: new Set<(time: number) => boolean>(),
};
let diceFrame = 0;

/** 优先启用 WebGPU，初始化失败后回退到同一共享 WebGL 引擎。 */
export async function createDiceEngine(
  Engine: typeof import("@babylonjs/core/Engines/engine").Engine,
  WebGPUEngine: typeof import("@babylonjs/core/Engines/webgpuEngine").WebGPUEngine,
): Promise<DiceEngine> {
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = 16;
  diceRenderer.canvas = canvas;
  try {
    if (await WebGPUEngine.IsSupportedAsync) {
      const candidate = new WebGPUEngine(canvas, {
        antialias: true,
        powerPreference: "low-power",
        stencil: false,
        audioEngine: false,
        doNotHandleContextLost: true,
      });
      try {
        await candidate.initAsync();
        return candidate;
      } catch {
        candidate.dispose();
      }
    }
  } catch {
    // WebGPU 不可用时沿用同一个共享 WebGL 回退。
  }
  return new Engine(
    canvas,
    true,
    {
      alpha: true,
      preserveDrawingBuffer: true,
      premultipliedAlpha: false,
      stencil: false,
      powerPreference: "low-power",
      doNotHandleContextLost: true,
      audioEngine: false,
    },
    false,
  );
}

/** 将实例加入共享 RAF，直到该实例返回 false。 */
export function wakeDice(callback: (time: number) => boolean): void {
  diceRenderer.live.add(callback);
  if (!diceFrame) diceFrame = requestAnimationFrame(renderDiceFrame);
}

function renderDiceFrame(time: number): void {
  diceFrame = 0;
  diceRenderer.live.forEach((callback) => {
    if (!callback(time)) diceRenderer.live.delete(callback);
  });
  if (diceRenderer.live.size) diceFrame = requestAnimationFrame(renderDiceFrame);
}

/** 与原型的齐次坐标投影一致，统一阴影、光晕和裂纹使用的画布像素坐标。 */
export function projectDicePoint(
  point: { x: number; y: number; z: number },
  matrix: ArrayLike<number>,
  width: number,
  height: number,
): { x: number; y: number } {
  const w = point.x * matrix[3]! + point.y * matrix[7]! + point.z * matrix[11]! + matrix[15]!;
  const x = (point.x * matrix[0]! + point.y * matrix[4]! + point.z * matrix[8]! + matrix[12]!) / w;
  const y = (point.x * matrix[1]! + point.y * matrix[5]! + point.z * matrix[9]! + matrix[13]!) / w;
  return { x: ((x + 1) * width) / 2, y: ((1 - y) * height) / 2 };
}

let rendererConsumers = 0;
/** 托盘和预览共享 GPU；最后一个组件离开后释放引擎、画布缓存及 RAF。 */
export function retainDiceRenderer(): () => void {
  rendererConsumers += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    rendererConsumers -= 1;
    if (rendererConsumers) return;
    diceRenderer.live.clear();
    if (diceFrame) cancelAnimationFrame(diceFrame);
    diceFrame = 0;
    const engine = diceRenderer.engine;
    diceRenderer.engine = undefined;
    diceRenderer.canvas = undefined;
    diceRenderer.environment = undefined;
    void engine?.then(
      (value) => value.dispose(),
      () => {},
    );
  };
}
