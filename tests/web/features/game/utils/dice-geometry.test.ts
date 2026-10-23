import { describe, expect, it } from "vitest";
import { Quaternion, Vector3 } from "@babylonjs/core/Maths/math.vector";
import {
  buildDiceFaces,
  diceFaceUprightAngle,
} from "../../../../../src-web/features/game/utils/dice-geometry";

describe("骰子几何", () => {
  it.each([
    [6, 6],
    [8, 8],
    [20, 20],
  ] as const)("d%d 网格有 %d 个完整面和值", (sides, expectedFaces) => {
    const faces = buildDiceFaces(sides);
    expect(faces).toHaveLength(expectedFaces);
    expect(faces.map((face) => face.value).sort((a, b) => a - b)).toEqual(
      Array.from({ length: sides }, (_, index) => index + 1),
    );
    for (const face of faces) {
      const opposite = faces.find((candidate) => candidate.value === sides + 1 - face.value)!;
      const alignment =
        face.normal.x * opposite.normal.x +
        face.normal.y * opposite.normal.y +
        face.normal.z * opposite.normal.z;
      expect(alignment).toBeCloseTo(-1, 5);
    }
  });

  it.each([6, 8, 20] as const)("d%d 每个面都能朝上且数字正立", (sides) => {
    const targetNormal = new Vector3(0, 0, 1);
    for (const face of buildDiceFaces(sides)) {
      const from = new Vector3(face.normal.x, face.normal.y, face.normal.z);
      const axis = Vector3.Cross(from, targetNormal);
      const cosine = Vector3.Dot(from, targetNormal);
      const align =
        axis.lengthSquared() < 1e-8
          ? cosine < 0
            ? Quaternion.RotationAxis(new Vector3(0, 1, 0), Math.PI)
            : Quaternion.Identity()
          : Quaternion.RotationAxis(axis.normalize(), Math.acos(Math.max(-1, Math.min(1, cosine))));
      const rotatedUp = new Vector3(face.up.x, face.up.y, face.up.z);
      rotatedUp.rotateByQuaternionToRef(align, rotatedUp);
      const rotation = Quaternion.RotationAxis(
        targetNormal,
        diceFaceUprightAngle(rotatedUp),
      ).multiply(align);
      const finalNormal = from.rotateByQuaternionToRef(rotation, new Vector3());
      const finalUp = new Vector3(face.up.x, face.up.y, face.up.z).rotateByQuaternionToRef(
        rotation,
        new Vector3(),
      );
      expect(finalNormal.x).toBeCloseTo(0, 5);
      expect(finalNormal.y).toBeCloseTo(0, 5);
      expect(finalNormal.z).toBeCloseTo(1, 5);
      expect(finalUp.x).toBeCloseTo(0, 5);
      expect(finalUp.y).toBeCloseTo(1, 5);
      expect(finalUp.z).toBeCloseTo(0, 5);
    }
  });

  it.each([
    [6, [3, 4, 1, 6, 5, 2]],
    [8, [3, 8, 5, 2, 1, 6, 7, 4]],
    [20, [5, 8, 20, 14, 3, 2, 12, 10, 15, 17, 7, 1, 13, 16, 18, 6, 4, 19, 9, 11]],
  ] as const)("d%d 面序和数值与 Babylon 原型一致", (sides, values) => {
    expect(buildDiceFaces(sides).map((face) => face.value)).toEqual(values);
  });

  it.each([6, 8, 20] as const)("d%d 顶点沿法线逆时针排列，数字朝向首顶点", (sides) => {
    for (const face of buildDiceFaces(sides)) {
      const [a, b, c] = face.vertices.map((point) => new Vector3(point.x, point.y, point.z));
      const winding = Vector3.Cross(b!.subtract(a!), c!.subtract(a!));
      expect(
        Vector3.Dot(winding, new Vector3(face.normal.x, face.normal.y, face.normal.z)),
      ).toBeGreaterThan(0);
      if (sides !== 6) {
        const up = a!
          .subtract(new Vector3(face.center.x, face.center.y, face.center.z))
          .normalize();
        expect(up.x).toBeCloseTo(face.up.x, 12);
        expect(up.y).toBeCloseTo(face.up.y, 12);
        expect(up.z).toBeCloseTo(face.up.z, 12);
      }
    }
  });

  it("D20 与 D8 使用相同的单位外接半径", () => {
    const radius = (sides: 8 | 20) =>
      Math.max(
        ...buildDiceFaces(sides).flatMap((face) =>
          face.vertices.map((vertex) => Math.hypot(vertex.x, vertex.y, vertex.z)),
        ),
      );
    expect(radius(20)).toBeCloseTo(radius(8), 6);
  });
});
