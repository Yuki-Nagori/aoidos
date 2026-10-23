export type DiceSides = 6 | 8 | 20;
export interface Vector3Data {
  x: number;
  y: number;
  z: number;
}
export interface DiceFaceData {
  value: number;
  normal: Vector3Data;
  center: Vector3Data;
  up: Vector3Data;
  vertices: Vector3Data[];
}

export function diceFaceUprightAngle(rotatedUp: Vector3Data): number {
  return Math.atan2(rotatedUp.x, rotatedUp.y);
}

const add = (a: Vector3Data, b: Vector3Data): Vector3Data => ({
  x: a.x + b.x,
  y: a.y + b.y,
  z: a.z + b.z,
});
const sub = (a: Vector3Data, b: Vector3Data): Vector3Data => ({
  x: a.x - b.x,
  y: a.y - b.y,
  z: a.z - b.z,
});
const scale = (a: Vector3Data, factor: number): Vector3Data => ({
  x: a.x * factor,
  y: a.y * factor,
  z: a.z * factor,
});
const dot = (a: Vector3Data, b: Vector3Data): number => a.x * b.x + a.y * b.y + a.z * b.z;
const cross = (a: Vector3Data, b: Vector3Data): Vector3Data => ({
  x: a.y * b.z - a.z * b.y,
  y: a.z * b.x - a.x * b.z,
  z: a.x * b.y - a.y * b.x,
});
const normalize = (a: Vector3Data): Vector3Data => scale(a, 1 / Math.hypot(a.x, a.y, a.z));

export function buildDiceFaces(sides: DiceSides): DiceFaceData[] {
  let faces: DiceFaceData[];
  if (sides === 6) {
    faces = [
      { x: 1, y: 0, z: 0 },
      { x: -1, y: 0, z: 0 },
      { x: 0, y: 1, z: 0 },
      { x: 0, y: -1, z: 0 },
      { x: 0, y: 0, z: 1 },
      { x: 0, y: 0, z: -1 },
    ].map((normal) => {
      const up = Math.abs(normal.y) > 0.5 ? { x: 0, y: 0, z: -normal.y } : { x: 0, y: 1, z: 0 };
      const right = normalize(cross(up, normal));
      const center = scale(normal, 0.62);
      const point = (x: number, y: number) =>
        add(center, add(scale(right, x * 0.62), scale(up, y * 0.62)));
      return {
        value: 0,
        normal,
        center,
        up,
        vertices: [point(-1, -1), point(1, -1), point(1, 1), point(-1, 1)],
      };
    });
  } else {
    // 原型的六位小数顶点决定面排序，不能重新按单位球归一化。
    const poly: Record<8 | 20, number[]> = {
      8: [
        0, 1, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 0, -1, 0, 1, 0, 0, 0, -1, 0, 0, 0, -1, 1, 0, 0, 0, 0,
        -1, 0, 1, 0, 1, 0, 0, 0, 1, 0, 0, 0, -1, -1, 0, 0, 0, 0, -1, 0, -1, 0, -1, 0, 0, 0, -1, 0,
        0, 0, 1, -1, 0, 0, 0, 0, 1, 0, 1, 0, -1, 0, 0,
      ],
      20: [
        -0.850651, 0, 0.525731, 0, 0.525731, 0.850651, -0.525731, 0.850651, 0, 0, 0.525731,
        0.850651, 0.525731, 0.850651, 0, -0.525731, 0.850651, 0, 0.525731, 0.850651, 0, 0, 0.525731,
        -0.850651, -0.525731, 0.850651, 0, 0, 0.525731, -0.850651, -0.850651, 0, -0.525731,
        -0.525731, 0.850651, 0, -0.850651, 0, -0.525731, -0.850651, 0, 0.525731, -0.525731,
        0.850651, 0, 0, 0.525731, 0.850651, 0.850651, 0, 0.525731, 0.525731, 0.850651, 0, -0.850651,
        0, 0.525731, 0, -0.525731, 0.850651, 0, 0.525731, 0.850651, -0.850651, 0, -0.525731,
        -0.525731, -0.850651, 0, -0.850651, 0, 0.525731, 0, 0.525731, -0.850651, 0, -0.525731,
        -0.850651, -0.850651, 0, -0.525731, 0.525731, 0.850651, 0, 0.850651, 0, -0.525731, 0,
        0.525731, -0.850651, 0.850651, 0, 0.525731, 0, -0.525731, 0.850651, 0.525731, -0.850651, 0,
        0, -0.525731, 0.850651, -0.525731, -0.850651, 0, 0.525731, -0.850651, 0, -0.525731,
        -0.850651, 0, 0, -0.525731, -0.850651, 0.525731, -0.850651, 0, 0, -0.525731, -0.850651,
        0.850651, 0, -0.525731, 0.525731, -0.850651, 0, 0.850651, 0, -0.525731, 0.850651, 0,
        0.525731, 0.525731, -0.850651, 0, 0.850651, 0, 0.525731, 0, 0.525731, 0.850651, 0,
        -0.525731, 0.850651, 0, -0.525731, 0.850651, -0.850651, 0, 0.525731, -0.525731, -0.850651,
        0, -0.525731, -0.850651, 0, -0.850651, 0, -0.525731, 0, -0.525731, -0.850651, 0, -0.525731,
        -0.850651, 0, 0.525731, -0.850651, 0.850651, 0, -0.525731, 0.850651, 0, -0.525731, 0.525731,
        0.850651, 0, 0.850651, 0, 0.525731,
      ],
    };
    const positions = poly[sides];
    const vertices = Array.from({ length: positions.length / 3 }, (_, index) => ({
      x: positions[index * 3]!,
      y: positions[index * 3 + 1]!,
      z: positions[index * 3 + 2]!,
    }));
    faces = [];
    for (let index = 0; index < vertices.length; index += 3) {
      const a = vertices[index]!;
      let b = vertices[index + 1]!;
      let c = vertices[index + 2]!;
      const center = scale(add(add(a, b), c), 1 / 3);
      let normal = normalize(cross(sub(b, a), sub(c, a)));
      if (dot(normal, center) < 0) {
        [b, c] = [c, b];
        normal = scale(normal, -1);
      }
      faces.push({ value: 0, normal, center, up: normalize(sub(a, center)), vertices: [a, b, c] });
    }
  }

  const ordered = faces
    .map((_, index) => index)
    .sort(
      (a, b) =>
        faces[b]!.normal.y - faces[a]!.normal.y ||
        Math.atan2(faces[a]!.normal.z, faces[a]!.normal.x) -
          Math.atan2(faces[b]!.normal.z, faces[b]!.normal.x),
    );
  const used = new Map<number, number>();
  const d20Sequence = [20, 8, 14, 2, 17, 5, 11, 3, 15, 9];
  let valueIndex = 1;
  for (const index of ordered) {
    if (used.has(index)) continue;
    let opposite = -1;
    let best = 2;
    faces.forEach((candidate, candidateIndex) => {
      if (candidateIndex === index || used.has(candidateIndex)) return;
      const alignment = dot(candidate.normal, faces[index]!.normal);
      if (alignment < best) {
        best = alignment;
        opposite = candidateIndex;
      }
    });
    if (opposite < 0) continue;
    const value = sides === 20 ? d20Sequence[valueIndex - 1]! : valueIndex;
    used.set(index, value);
    used.set(opposite, sides + 1 - value);
    valueIndex += 1;
  }
  faces.forEach((face, index) => {
    face.value = used.get(index) ?? 1;
  });
  return faces;
}
