import type { DiceFaceData, DiceSides } from "./dice-geometry";
import type { DiceVisualShape, DiceVisualSkin } from "./dice-visual";

export function hashStr(seed: string): number {
  let value = 2166136261;
  for (let index = 0; index < seed.length; index += 1)
    value = Math.imul(value ^ seed.charCodeAt(index), 16777619);
  return value >>> 0;
}

export function seededNumber(seed: number): () => number {
  let value = seed >>> 0;
  return () => {
    value = (value + 0x6d2b79f5) >>> 0;
    let next = value;
    next = Math.imul(next ^ (next >>> 15), next | 1);
    next ^= next + Math.imul(next ^ (next >>> 7), next | 61);
    return ((next ^ (next >>> 14)) >>> 0) / 4294967296;
  };
}

export function drawDiceAtlas(
  faces: DiceFaceData[],
  sides: DiceSides,
  skin: DiceVisualSkin,
): HTMLCanvasElement {
  const G = Math.ceil(Math.sqrt(faces.length)),
    C = 256,
    cv = document.createElement("canvas");
  cv.width = cv.height = G * C;
  const g = cv.getContext("2d");
  if (!g) return cv;
  const R = seededNumber(hashStr(skin.id || "x") + sides);
  const serif =
    skin.font === "sans"
      ? '"Inter", "Helvetica Neue", Arial, sans-serif'
      : '"Cinzel", "Trajan Pro", Georgia, "Times New Roman", serif';
  faces.forEach((f, index) => {
    const ox = (index % G) * C,
      oy = Math.floor(index / G) * C;
    g.save();
    g.translate(ox, oy);
    const poly =
      f.vertices.length === 3
        ? [
            [0.5, 0.934],
            [0.05, 0.155],
            [0.95, 0.155],
          ]
        : [
            [0.03, 0.03],
            [0.97, 0.03],
            [0.97, 0.97],
            [0.03, 0.97],
          ];
    // 填满整个图块，避免 mipmap 与抗锯齿采到图块缝隙。
    g.fillStyle = skin.face2;
    g.fillRect(0, 0, C, C);
    const gr = g.createLinearGradient(0, 0, C * 0.3, C);
    gr.addColorStop(0, skin.face);
    gr.addColorStop(1, skin.face2);
    g.fillStyle = gr;
    g.fillRect(0, 0, C, C);
    g.save();
    g.beginPath();
    poly.forEach((p, i) => {
      const X = p[0]! * C,
        Y = (1 - p[1]!) * C;
      if (i) g.lineTo(X, Y);
      else g.moveTo(X, Y);
    });
    g.closePath();
    g.clip();
    const pc = skin.patternColor;
    if (skin.pattern === "veins" || skin.pattern === "mist") {
      g.strokeStyle = pc;
      g.globalAlpha = skin.pattern === "mist" ? 0.22 : 0.35;
      g.lineCap = "round";
      for (let v = 0; v < (skin.pattern === "mist" ? 5 : 4); v++) {
        g.lineWidth = skin.pattern === "mist" ? 10 + R() * 18 : 1 + R() * 2.5;
        g.beginPath();
        let x = R() * C,
          y = R() * C;
        g.moveTo(x, y);
        for (let s2 = 0; s2 < 5; s2++) {
          x += (R() - 0.3) * C * 0.45;
          y += (R() - 0.5) * C * 0.35;
          g.quadraticCurveTo(x - 30 * R(), y + 30 * R(), x, y);
        }
        if (skin.pattern === "mist") g.filter = "blur(6px)";
        g.stroke();
        g.filter = "none";
      }
      g.globalAlpha = 1;
    } else if (skin.pattern === "speckle") {
      g.fillStyle = pc;
      for (let q = 0; q < 160; q++) {
        g.globalAlpha = 0.15 + R() * 0.35;
        g.beginPath();
        g.arc(R() * C, R() * C, 0.6 + R() * 1.8, 0, 7);
        g.fill();
      }
      g.globalAlpha = 1;
    } else if (skin.pattern === "brushed") {
      g.strokeStyle = pc;
      for (let b = 0; b < 70; b++) {
        g.globalAlpha = 0.05 + R() * 0.12;
        g.lineWidth = 1;
        const yy = R() * C;
        g.beginPath();
        g.moveTo(0, yy);
        g.lineTo(C, yy + (R() - 0.5) * 6);
        g.stroke();
      }
      g.globalAlpha = 1;
    } else if (skin.pattern === "facet") {
      const rg = g.createRadialGradient(C * 0.35, C * 0.3, 4, C * 0.5, C * 0.5, C * 0.8);
      rg.addColorStop(0, pc);
      rg.addColorStop(1, "rgba(0,0,0,0)");
      g.globalAlpha = 0.22;
      g.fillStyle = rg;
      g.fillRect(0, 0, C, C);
      g.globalAlpha = 1;
    }
    // 面内倒角阴影与描边。
    g.lineJoin = "round";
    g.strokeStyle = "rgba(0,0,0,.28)";
    g.lineWidth = 16;
    g.beginPath();
    poly.forEach((p, i) => {
      const X = p[0]! * C,
        Y = (1 - p[1]!) * C;
      if (i) g.lineTo(X, Y);
      else g.moveTo(X, Y);
    });
    g.closePath();
    g.stroke();
    g.strokeStyle = skin.edge;
    g.lineWidth = 5;
    g.stroke();
    g.restore();
    // 数字与点数。
    const cx = 0.5 * C,
      cy = (1 - (f.vertices.length === 3 ? 0.414 : 0.5)) * C,
      val = f.value;
    if (sides === 6 && skin.d6 === "pips") {
      const P: Record<number, number[][]> = {
        1: [[0, 0]],
        2: [
          [-1, -1],
          [1, 1],
        ],
        3: [
          [-1, -1],
          [0, 0],
          [1, 1],
        ],
        4: [
          [-1, -1],
          [1, -1],
          [-1, 1],
          [1, 1],
        ],
        5: [
          [-1, -1],
          [1, -1],
          [0, 0],
          [-1, 1],
          [1, 1],
        ],
        6: [
          [-1, -1],
          [1, -1],
          [-1, 0],
          [1, 0],
          [-1, 1],
          [1, 1],
        ],
      };
      const pips = P[val] ?? [];
      pips.forEach((d) => {
        const X = cx + d[0]! * C * 0.25,
          Y = cy + d[1]! * C * 0.25,
          rad = (val === 1 ? 0.12 : 0.085) * C;
        g.fillStyle = skin.numeralStroke;
        g.beginPath();
        g.arc(X, Y + 2, rad + 2, 0, 7);
        g.fill();
        g.fillStyle = val === 1 && skin.id !== "brass" ? skin.particle : skin.numeral;
        g.beginPath();
        g.arc(X, Y, rad, 0, 7);
        g.fill();
      });
    } else {
      const fs = sides === 20 ? 0.34 : sides === 8 ? 0.4 : 0.52;
      g.font = `700 ${Math.round(fs * C)}px ${serif}`;
      g.textAlign = "center";
      g.textBaseline = "middle";
      const label = String(val);
      g.lineWidth = 7;
      g.strokeStyle = skin.numeralStroke;
      g.lineJoin = "round";
      g.strokeText(label, cx, cy + 2);
      g.fillStyle = skin.numeral;
      g.fillText(label, cx, cy + 2);
      if (val === 6 || val === 9) {
        // 下划线区分 6 与 9。
        const w = fs * C * 0.42,
          uy = cy + fs * C * 0.48;
        g.fillStyle = skin.numeralStroke;
        g.fillRect(cx - w / 2 - 2, uy - 2, w + 4, 9);
        g.fillStyle = skin.numeral;
        g.fillRect(cx - w / 2, uy, w, 5);
      }
    }
    g.restore();
  });
  return cv;
}

export function drawFlatDiceCanvas(
  target: HTMLCanvasElement,
  shape: DiceVisualShape,
  skin: DiceVisualSkin,
): boolean {
  const context = target.getContext("2d");
  if (!context) return false;
  const ratio = Math.min(2, window.devicePixelRatio || 1),
    bounds = target.getBoundingClientRect();
  const width = Math.max(16, Math.round(bounds.width * ratio));
  target.width = width;
  const height = Math.max(16, Math.round(bounds.height * ratio));
  target.height = height;
  const size = Math.min(height * 0.8, (width / shape.count) * 0.8);
  const corners = shape.sides === 6 ? 4 : shape.sides === 8 ? 3 : 6;
  context.clearRect(0, 0, width, height);
  for (let index = 0; index < shape.count; index += 1) {
    const x = (width / (shape.count + 1)) * (index + 1),
      y = height / 2;
    context.beginPath();
    for (let corner = 0; corner < corners; corner += 1) {
      const angle =
        -Math.PI / 2 + (corner * Math.PI * 2) / corners + (corners === 4 ? Math.PI / 4 : 0);
      context.lineTo(x + (Math.cos(angle) * size) / 2, y + (Math.sin(angle) * size) / 2);
    }
    context.closePath();
    context.fillStyle = skin.face;
    context.fill();
    context.strokeStyle = skin.edge;
    context.lineWidth = 3;
    context.stroke();
    context.fillStyle = skin.numeral;
    context.font = `700 ${Math.round(size * 0.36)}px Georgia, serif`;
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.fillText(
      String(
        shape.values[index] ?? (shape.sides === 6 ? [6, 5, 4, 3][index] : shape.sides - index),
      ),
      x,
      y,
    );
  }
  return true;
}
