<script setup lang="ts">
import { computed, onMounted, onScopeDispose, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { DiceRollFact, DiceSkinId } from "../utils/dice-visual";
import { diceVisualShape, resolveDiceSkin, resolveDiceSkinColor } from "../utils/dice-visual";
import {
  createDiceEngine,
  createDiceSceneCancellation,
  diceRenderer,
  projectDicePoint,
  renderAndSubmitDiceFrame,
  retainDiceRenderer,
  waitForDiceSceneReady,
  wakeDice,
} from "../utils/dice-renderer";
import type { DiceFaceData } from "../utils/dice-geometry";
import { drawDiceAtlas, drawFlatDiceCanvas, hashStr, seededNumber } from "../utils/dice-canvas";
import { buildDiceFaces, diceFaceUprightAngle } from "../utils/dice-geometry";

const props = withDefaults(
  defineProps<{
    expression: string;
    rolls: readonly DiceRollFact[];
    scriptId: string;
    skin?: DiceSkinId;
    scriptSkinEnabled?: boolean;
    motion?: "idle" | "rolling" | "result";
    effect?: "crit" | "fumble" | "";
    seed?: string;
    interactive?: boolean;
    showPop?: boolean;
    gesture?: { direction: [number, number]; power: number };
    keep?: "high" | "low";
    still?: boolean;
    d6?: "pips" | "numerals";
  }>(),
  {
    skin: "auto",
    scriptSkinEnabled: true,
    motion: "result",
    effect: "",
    seed: "",
    interactive: false,
    showPop: false,
    still: false,
  },
);
const emit = defineEmits<{
  submit: [gesture: { direction: [number, number]; power: number }];
  settled: [result: { values: readonly number[]; effect: "" | "crit" | "fumble" }];
}>();
const { t } = useI18n();

const host = ref<HTMLDivElement>();
const canvas = ref<HTMLCanvasElement>();
const visualStyle = computed(() => ({
  "--dice-count": getShape()?.count ?? props.rolls.length,
  "--dice-pop-ink": "var(--ink)",
  "--glow": resolvedSkin().particle,
}));
const available = ref(false);
const canvasRenderer = ref<"babylon" | "2d">("babylon");
const renderFailed = ref(false);
const popElements = ref<HTMLElement[]>([]);
const visible = ref(false);
let cleanupScene: (() => void) | undefined;
let releaseRenderer: (() => void) | undefined;
const pendingScenes = new Set<() => void>();
let observer: IntersectionObserver | undefined;
let themeObserver: MutationObserver | undefined;
let resizeObserver: ResizeObserver | undefined;
let disposed = false;
let revision = 0;
let activeScene:
  | { render: (time: number) => void; wake: () => void; resize: () => void; dispose: () => void }
  | undefined;
let pointerStart: { x: number; y: number; time: number } | undefined;
let pointerTrail: { x: number; y: number; time: number }[] = [];
let chargeStart = 0;
let chargeTimer: ReturnType<typeof setInterval> | undefined;
const charge = ref(0);
let settledEffectAt = 0;
let settledSent = false;

function getShape() {
  return diceVisualShape(props.expression, props.rolls, props.motion !== "result");
}
const fallbackValues = computed(() => getShape()?.values ?? []);

function resolvedSkin() {
  const raw = resolveDiceSkin(props.skin, props.scriptId, props.scriptSkinEnabled);
  const resolved = { ...raw };
  for (const field of ["face", "face2", "patternColor", "numeral", "numeralStroke", "edge", "particle"] as const) {
    resolved[field] = resolveDiceSkinColor(raw[field], "#888", host.value);
  }
  if (raw.id === "theme") {
    resolved.face2 = mixColor(resolved.face, "#0b0d10", 0.55);
    resolved.face = mixColor(resolved.face, "#0b0d10", 0.12);
    resolved.numeral = "#ffffff";
    resolved.numeralStroke = "rgba(0,0,0,.35)";
    resolved.edge = mixColor(resolved.edge, "#ffffff", 0.35);
    resolved.patternColor = mixColor(resolved.patternColor, "#ffffff", 0.3);
  }
  resolved.d6 = props.d6 ?? resolved.d6;
  return resolved;
}

function mixColor(from: string, to: string, amount: number): string {
  const context = document.createElement("canvas").getContext("2d");
  if (!context) return from;
  const channels = (color: string) => {
    context.clearRect(0, 0, 1, 1);
    context.fillStyle = color;
    context.fillRect(0, 0, 1, 1);
    return context.getImageData(0, 0, 1, 1).data;
  };
  const a = channels(from), b = channels(to);
  return `rgb(${[0, 1, 2].map((index) => Math.round(a[index]! + (b[index]! - a[index]!) * amount)).join(",")})`;
}

type DiceFace = DiceFaceData;
async function makeScene(
  targetCanvas: HTMLCanvasElement,
  instance: number,
  onContextLost: () => void,
): Promise<{
  render: (time: number) => void;
  wake: () => void;
  resize: () => void;
  dispose: () => void;
}> {
  const [
    engineModule,
    gpuModule,
    sceneModule,
    cameraModule,
    lightModule,
    directionalModule,
    vectorModule,
    colorModule,
    materialModule,
    textureModule,
    meshModule,
    vertexModule,
    lineModule,
    cubeModule,
  ] = await Promise.all([
    import("@babylonjs/core/Engines/engine"),
    import("@babylonjs/core/Engines/webgpuEngine"),
    import("@babylonjs/core/scene"),
    import("@babylonjs/core/Cameras/targetCamera"),
    import("@babylonjs/core/Lights/hemisphericLight"),
    import("@babylonjs/core/Lights/directionalLight"),
    import("@babylonjs/core/Maths/math.vector"),
    import("@babylonjs/core/Maths/math.color"),
    import("@babylonjs/core/Materials/PBR/pbrMaterial"),
    import("@babylonjs/core/Materials/Textures/dynamicTexture"),
    import("@babylonjs/core/Meshes/mesh"),
    import("@babylonjs/core/Meshes/mesh.vertexData"),
    import("@babylonjs/core/Meshes/Builders/linesBuilder"),
    import("@babylonjs/core/Materials/Textures/rawCubeTexture"),
  ]);
  const shape = getShape();
  if (!shape) throw new Error("unsupported-dice-shape");
  const { Engine } = engineModule;
  const { WebGPUEngine } = gpuModule;
  const { Scene } = sceneModule;
  const { TargetCamera } = cameraModule;
  const { HemisphericLight } = lightModule;
  const { DirectionalLight } = directionalModule;
  const { Vector3, Quaternion } = vectorModule;
  const { Color3, Color4 } = colorModule;
  const { PBRMaterial } = materialModule;
  const { DynamicTexture } = textureModule;
  const { Mesh } = meshModule;
  const { VertexData } = vertexModule;
  const { CreateLineSystem } = lineModule;
  diceRenderer.engine ??= createDiceEngine(Engine, WebGPUEngine);
  const activeEngine = await diceRenderer.engine;
  if (disposed || revision !== instance) throw new Error("dice-render-cancelled");
  const output = targetCanvas.getContext("2d");
  if (!output) throw new Error("dice-output-context-unavailable");
  const resizeOutput = () => {
    const ratio = Math.min(2, window.devicePixelRatio || 1);
    const bounds = targetCanvas.getBoundingClientRect();
    const width = Math.max(16, Math.round(bounds.width * ratio));
    const height = Math.max(16, Math.round(bounds.height * ratio));
    if (targetCanvas.width !== width || targetCanvas.height !== height) { targetCanvas.width = width; targetCanvas.height = height; }
  };
  resizeOutput();
  const scene = new Scene(activeEngine);
  scene.useRightHandedSystem = true;
  const contextLostObserver = activeEngine.onContextLostObservable.add(onContextLost);
  let sceneDisposed = false;
  const disposeBabylonScene = () => {
    if (sceneDisposed) return;
    sceneDisposed = true;
    try {
      scene.dispose();
    } finally {
      activeEngine.onContextLostObservable.remove(contextLostObserver);
    }
  };
  const cancellation = createDiceSceneCancellation(() => {
    disposeBabylonScene();
  });
  // 销毁正在加载字体或纹理的场景时，ready promise 也会被取消，提前接住拒绝。
  void cancellation.promise.catch(() => {});
  pendingScenes.add(cancellation.cancel);
  try {
    scene.clearColor = new Color4(0, 0, 0, 0);
    const camera = new TargetCamera(`dice-camera-${instance}`, new Vector3(0, 0.35, 7.4), scene);
    scene.activeCamera = camera;
    camera.fov = (26 * Math.PI) / 180;
    camera.minZ = 0.1;
    camera.maxZ = 60;
    const ambient = new HemisphericLight(`dice-ambient-${instance}`, new Vector3(0, 1, 0), scene);
    ambient.intensity = 0.55;
    ambient.groundColor = Color3.FromHexString("#2a3240");
    ambient.specular = Color3.Black();
    const lights = [
      [-4, 5, 1.5, "#ffffff", 1.25],
      [3, -1, 4, "#fff1dd", 0.35],
      [-4, 1, -2, "#bfd4ff", 0.6],
    ] as const;
    lights.forEach(([x, y, z, color, intensity], index) => {
      const light = new DirectionalLight(
        `dice-light-${instance}-${index}`,
        new Vector3(-x, -y, -z).normalize(),
        scene,
      );
      light.diffuse = Color3.FromHexString(color);
      light.intensity = intensity * Math.PI * 0.5;
    });
    scene.imageProcessingConfiguration.toneMappingEnabled = true;
    scene.imageProcessingConfiguration.toneMappingType = 1; // 与原型一致的 ACES 色调映射。
    scene.imageProcessingConfiguration.exposure = 1.05;

    // 原型直接使用 32 像素 RGB 环境，保留光板角宽、混色和量化。
    scene.environmentTexture = new cubeModule.RawCubeTexture(scene, roomEnvironment(32), 32, Engine.TEXTUREFORMAT_RGB, Engine.TEXTURETYPE_UNSIGNED_BYTE, true, false);
    scene.environmentIntensity = 1.1;
    const skin = resolvedSkin();
    await document.fonts?.load(`700 87px ${skin.font === "sans" ? "Inter" : "Cinzel"}`);
    if (disposed || revision !== instance) throw new Error("dice-render-cancelled");
    const faces = buildDiceFaces(shape.sides);
    const faceCount = faces.length;
    const atlasColumns = Math.ceil(Math.sqrt(faceCount));
    const atlas = drawDiceAtlas(faces, shape.sides, skin);
    const texture = new DynamicTexture(
      `dice-atlas-${instance}`,
      { width: atlas.width, height: atlas.height },
      scene,
      true,
    );
    const context = texture.getContext();
    context.drawImage(atlas, 0, 0);
    texture.update(true);
    texture.hasAlpha = false;
    texture.gammaSpace = true;
    texture.anisotropicFilteringLevel = 4;
    const material = new PBRMaterial(`dice-material-${instance}`, scene);
    material.albedoTexture = texture;
    material.albedoColor = Color3.White();
    material.metallic = skin.metalness;
    material.roughness = skin.roughness;
    material.useAlphaFromAlbedoTexture = false;
    material.maxSimultaneousLights = 4;
    material.disableDepthWrite = skin.opacity < 1;
    material.alpha = skin.opacity < 1 ? 0.92 : 1;
    material.sideOrientation = 1;
    material.clearCoat.isEnabled = skin.clearcoat > 0;
    material.clearCoat.intensity = skin.clearcoat;
    material.clearCoat.roughness = 0.15;
    material.emissiveColor = Color3.Black();

    const geometries: {
      mesh: InstanceType<typeof Mesh>;
      target: InstanceType<typeof Quaternion>;
      rest: InstanceType<typeof Vector3>;
      axis: InstanceType<typeof Vector3>;
      spins: number;
      direction: InstanceType<typeof Vector3>;
      wobble: InstanceType<typeof Vector3>;
      delay: number;
      duration: number;
    }[] = [];
    const edgeMeshes: InstanceType<typeof Mesh>[] = [];
    const dieSize = shape.sides === 6 ? 0.98 : shape.sides === 8 ? 1.12 : 1;
    for (let index = 0; index < shape.count; index += 1) {
      const positions: number[] = [];
      const normals: number[] = [];
      const uvs: number[] = [];
      const indices: number[] = [];
      const edgeLines: { points: InstanceType<typeof Vector3>[]; key: string }[] = [];
      faces.forEach((face, faceIndex) => {
        const points = face.vertices;
        const coordinates =
          points.length === 3
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
        const polygon = points.map((point, pointIndex) => {
          const [localU, localV] = coordinates[pointIndex]!;
          const u = ((faceIndex % atlasColumns) + localU!) / atlasColumns;
          const v = 1 - (Math.floor(faceIndex / atlasColumns) + 1 - localV!) / atlasColumns;
          positions.push(point.x, point.y, point.z);
          normals.push(face.normal.x, face.normal.y, face.normal.z);
          uvs.push(u, v);
          return positions.length / 3 - 1;
        });
        if (points.length === 4)
          indices.push(
            polygon[0]!,
            polygon[1]!,
            polygon[2]!,
            polygon[0]!,
            polygon[2]!,
            polygon[3]!,
          );
        else indices.push(polygon[0]!, polygon[1]!, polygon[2]!);
        points.forEach((point, pointIndex) => {
          const next = points[(pointIndex + 1) % points.length]!;
          const a = [point.x, point.y, point.z].map((part) => part.toFixed(4)).join(":");
          const b = [next.x, next.y, next.z].map((part) => part.toFixed(4)).join(":");
          const key = [a, b].sort().join("|");
          if (!edgeLines.some((edge) => edge.key === key))
            edgeLines.push({
              key,
              points: [new Vector3(point.x, point.y, point.z), new Vector3(next.x, next.y, next.z)],
            });
        });
      });
      const mesh = new Mesh(`dice-${instance}-${index}`, scene);
      const vertexData = new VertexData();
      vertexData.positions = positions;
      vertexData.indices = indices;
      vertexData.normals = normals;
      vertexData.uvs = uvs;
      vertexData.applyToMesh(mesh, true);
      const bodyMaterial = material.clone(`dice-material-${instance}-${index}`);
      // PBR.clone 会克隆动态图集；未 update 的克隆一直未就绪，必须复用已上传的图集。
      if (bodyMaterial.albedoTexture !== texture) bodyMaterial.albedoTexture?.dispose();
      bodyMaterial.albedoTexture = texture;
      mesh.material = bodyMaterial;
      if (props.keep && props.motion === "result") {
        const kept = shape.values.indexOf(props.keep === "high" ? Math.max(...shape.values) : Math.min(...shape.values));
        if (kept !== index) mesh.material.alpha = 0.35;
      }
      if (skin.opacity < 1) {
        const inner = mesh.clone(`dice-inner-${instance}-${index}`, mesh)!;
        const innerMaterial = new PBRMaterial(`dice-inner-material-${instance}-${index}`, scene);
        innerMaterial.albedoTexture = texture;
        innerMaterial.metallic = 0;
        innerMaterial.roughness = 0.1;
        innerMaterial.alpha = mesh.material.alpha === 0.35 ? 0.35 : 0.85;
        innerMaterial.disableDepthWrite = true;
        innerMaterial.sideOrientation = 1;
        innerMaterial.cullBackFaces = false;
        innerMaterial.backFaceCulling = true;
        innerMaterial.maxSimultaneousLights = 4;
        inner.material = innerMaterial;
        inner.alphaIndex = 1;
        mesh.alphaIndex = 2;
      }
      mesh.scaling.setAll(dieSize);
      const value =
        shape.values[index] ?? (shape.sides === 6 ? [6, 5, 4, 3][index] : shape.sides - index);
      const targetFace =
        value === undefined ? faces[0]! : (faces.find((face) => face.value === value) ?? faces[0]!);
      const poseRandom = seededNumber(hashStr(`${props.seed}|${index}|${shape.sides}`) || 7);
      // 保持 Three 默认 XYZ 欧拉顺序；Babylon 的 YawPitchRoll 顺序不同。
      const tilt = quaternionXYZ(
        -0.32 + (poseRandom() - 0.5) * 0.08,
        0.26 + (poseRandom() - 0.5) * 0.2,
        (poseRandom() - 0.5) * 0.1,
        Quaternion,
      );
      const target = tilt.multiply(quaternionForFace(targetFace, Vector3, Quaternion));
      const random = seededNumber(
        hashStr(`${shape.values.join(",")}|${props.seed}`) + index * 977,
      );
      const axis = new Vector3(random() - 0.5, random() - 0.5, random() - 0.5).normalize();
      const power = Math.max(0.2, Math.min(1, props.gesture?.power ?? 0.6));
      const directionInput = props.gesture?.direction ?? [-1, 0.35];
      const spins = (2.2 + random() * 1.4 + power * 1.2) * Math.PI * 2;
      const direction = new Vector3(
        directionInput[0]! * 4.2 + (random() - 0.5) * 0.6,
        1.4 + directionInput[1]! * 1.5 + random() * 0.5,
        -1.5 * random(),
      );
      const wobble = new Vector3(random() - 0.5, random() - 0.5, 0).normalize();
      geometries.push({
        mesh,
        target,
        rest: Vector3.Zero(),
        axis,
        spins,
        direction,
        wobble,
        delay: index * 90,
        duration: 1250 + power * 450 + index * 120,
      });
      const dieEdges = CreateLineSystem(
        `dice-edges-${instance}-${index}`,
        { lines: edgeLines.map((edge) => edge.points), updatable: false },
        scene,
      );
      dieEdges.parent = mesh;
      dieEdges.position.setAll(0);
      dieEdges.scaling.setAll(1.002);
      dieEdges.color = color3FromCss(skin.edge, Color3);
      dieEdges.alpha = 0.55;
      dieEdges.alphaIndex = 3;
      dieEdges.isPickable = false;
      edgeMeshes.push(dieEdges);
    }
    const relayout = () => {
      const aspect = Math.max(
        0.01,
        targetCanvas.clientWidth / Math.max(1, targetCanvas.clientHeight),
      );
      const cluster = shape.count === 2 && aspect < 1.45;
      geometries.forEach((item, index) => {
        item.rest = cluster
          ? index === 0
            ? new Vector3(-0.74, -0.2, 0.35)
            : new Vector3(0.8, 0.5, -0.55)
          : new Vector3((index - (shape.count - 1) / 2) * 2.35, 0, 0);
      });
      const needWidth = shape.count === 1 ? 2.7 : cluster ? 4.1 : shape.count * 2.35 + 0.9;
      const pop = shape.values.length > 0;
      const distance = Math.max(pop ? 7.6 : 6.5, needWidth / 2 / (Math.tan((13 * Math.PI) / 180) * aspect));
      camera.position.set(0, 0.37, distance);
      camera.setTarget(new Vector3(0, pop ? 0.42 : 0.02, 0));
    };
    relayout();
    const reducedMotion = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
    const duration = Math.max(...geometries.map((item) => item.delay + item.duration * 1.3));
    const startTime = performance.now();
    const liveEffect = () => !reducedMotion && props.effect !== "" && settledEffectAt > 0;
    const shouldAnimate = () => !reducedMotion && (props.motion === "rolling" || (props.motion === "idle" && !props.still) || liveEffect());
    const effectRandom = seededNumber(hashStr(props.effect + shape.values.join()));
    const particles = Array.from({ length: 70 }, () => ({ angle: effectRandom() * Math.PI * 2, speed: 0.6 + effectRandom() * 1.4, life: 0.6 + effectRandom() * 0.7, size: 1 + effectRandom() * 2.2 }));
    let crackX = 0, crackY = 0, crackAngle = effectRandom() * 6.28;
    const crack = Array.from({ length: 7 }, () => {
      crackAngle += (effectRandom() - 0.5) * 1.3;
      crackX += Math.cos(crackAngle) * 0.16; crackY += Math.sin(crackAngle) * 0.16;
      return [crackX, crackY];
    });
    let branchX = crack[2]![0]!, branchY = crack[2]![1]!, branchAngle = crackAngle + 1.6;
    const branch = Array.from({ length: 3 }, () => {
      branchAngle += effectRandom() - 0.5;
      branchX += Math.cos(branchAngle) * 0.13; branchY += Math.sin(branchAngle) * 0.13;
      return [branchX, branchY];
    });
    const render = (time: number) => {
      const elapsed = Math.max(0, time - startTime);
      const seconds = elapsed / 1000;
      const effectAge = liveEffect() ? Math.max(0, (time - settledEffectAt) / 1000) : 0.45;
      material.emissiveColor = props.effect === "crit" && props.motion === "result"
        ? color3FromCss(skin.particle, Color3).scale(0.22)
        : Color3.Black();
      geometries.forEach((item, index) => {
        item.mesh.position.copyFrom(item.rest);
        item.mesh.rotationQuaternion = item.target;
        item.mesh.scaling.setAll(dieSize);
        if (props.motion === "result" && props.effect === "fumble" && liveEffect() && effectAge < 0.4) {
          item.mesh.position.x += Math.sin(effectAge * 70) * 0.06 * (1 - effectAge / 0.4);
        }
        if (reducedMotion) return;
        if (props.motion === "rolling") {
          const progress = Math.max(0, Math.min(1.3, (elapsed - item.delay) / item.duration));
          const u = Math.min(1, progress);
          const remainder = (1 - u) ** 3;
          const height = 1.25 * (1 - u) ** 1.4 * Math.abs(Math.cos(Math.PI * 2.5 * u));
          item.mesh.position.x += item.direction.x * remainder;
          item.mesh.position.z += item.direction.z * remainder;
          item.mesh.position.y += height + Math.max(0, item.direction.y - 1.4) * remainder * 0.3;
          item.mesh.rotationQuaternion = Quaternion.RotationAxis(
            item.axis,
            item.spins * (1 - u) ** 4,
          ).multiply(item.target);
          if (progress > 1) {
            const k = (progress - 1) / 0.3;
            const amplitude = 0.16 * Math.exp(-4 * k) * Math.sin(k * Math.PI * 3) * (1 - k);
            item.mesh.rotationQuaternion = Quaternion.RotationAxis(item.wobble, amplitude).multiply(
              item.target,
            );
          }
        } else if (props.motion === "idle" && !props.still) {
          if (charge.value > 0) {
            const amount = charge.value;
            item.mesh.position.y += 0.35 * amount + 0.04 * Math.sin(seconds * 2);
            item.mesh.scaling.setAll(dieSize * (1 + 0.07 * amount));
            item.mesh.rotationQuaternion = quaternionXYZ(
              Math.sin(seconds * 31 + index) * 0.06 * amount,
              Math.sin(seconds * 27) * 0.08 * amount + 0.2 * amount,
              0,
              Quaternion,
            ).multiply(item.target);
          } else {
            item.mesh.position.y += 0.07 * Math.sin(seconds * 1.5 + index * 0.9);
            item.mesh.rotationQuaternion = quaternionXYZ(
              Math.sin(seconds * 0.7 + index) * 0.06,
              Math.sin(seconds * 0.55 + index) * 0.12,
              0,
              Quaternion,
            ).multiply(item.target);
          }
        }
      });
      activeEngine.setSize(targetCanvas.width, targetCanvas.height);
      const sceneReady = renderAndSubmitDiceFrame(activeEngine, () => {
        scene.render();
        return scene.isReady();
      });
      if (!sceneReady) return;
      const context = output;
      {
        const width = targetCanvas.width, height = targetCanvas.height;
        context.clearRect(0, 0, width, height);
        // 投影与原型一致：直接从当前场景的视图投影矩阵映射到合成画布像素。
        const project = (point: InstanceType<typeof Vector3>) =>
          projectDicePoint(point, scene.getTransformMatrix().m, width, height);
        const dpr = Math.min(2, window.devicePixelRatio || 1);
        popElements.value.forEach((element, index) => {
          const item = geometries[index];
          if (!item) return;
          const point = project(item.mesh.position.add(new Vector3(0, 0.82, 0)));
          element.style.left = `${point.x / dpr}px`;
          element.style.top = `${point.y / dpr + 4}px`;
        });
        const unit = Math.abs(project(new Vector3(1, 0, 0)).x - project(Vector3.Zero()).x);
        geometries.forEach((item, index) => {
          const center = project(
            new Vector3(item.mesh.position.x, item.rest.y - 1.05, item.mesh.position.z),
          );
          const u = Math.max(0, Math.min(1, (elapsed - item.delay) / item.duration));
          const h =
            props.motion === "rolling" && !reducedMotion
              ? 1.25 * (1 - u) ** 1.4 * Math.abs(Math.cos(Math.PI * 2.5 * u))
              : props.motion === "idle" && !reducedMotion
                ? charge.value > 0
                  ? 0.35 * charge.value
                  : (0.07 * (1 + Math.sin(seconds * 1.5 + index * 0.9))) / 2
                : 0;
          const criticalResult = props.effect === "crit" && props.motion === "result";
          const radius = unit * ((criticalResult ? 0.82 : 0.95) - Math.min(0.5, h * 0.25));
          const gradient = context.createRadialGradient(
            center.x,
            center.y,
            1,
            center.x,
            center.y,
            radius,
          );
          gradient.addColorStop(
            0,
            `rgba(0,0,0,${(criticalResult ? 0.3 : 0.42) - Math.min(0.3, h * 0.15)})`,
          );
          gradient.addColorStop(1, "rgba(0,0,0,0)");
          context.save();
          context.translate(center.x, center.y);
          context.scale(1, 0.28);
          context.translate(-center.x, -center.y);
          context.fillStyle = gradient;
          context.beginPath();
          context.arc(center.x, center.y, radius, 0, Math.PI * 2);
          context.fill();
          context.restore();
        });
        // 大成功与失败特效只在骰子落定后显示，避免滚动中提前染色。
        const showEffect = props.motion === "result";
        if (showEffect && props.effect === "crit") {
          geometries.forEach((item) => {
            const center = project(item.mesh.position);
            const gradient = context.createRadialGradient(center.x, center.y, unit * 0.3, center.x, center.y, unit * 1.55 * (liveEffect() ? 1 + 0.2 * Math.exp(-effectAge * 2) * Math.sin(effectAge * 12) : 1));
            gradient.addColorStop(0, skin.particle); gradient.addColorStop(1, "rgba(0,0,0,0)");
            context.save(); context.globalAlpha = 0.38; context.fillStyle = gradient; context.fillRect(0, 0, width, height); context.restore();
          });
        }
        if (diceRenderer.canvas) {
          context.save();
          if (showEffect && props.effect === "fumble") context.filter = "brightness(0.72) saturate(0.35)";
          context.drawImage(diceRenderer.canvas, 0, 0, width, height);
          context.restore();
        }
        if (showEffect && props.effect === "crit") {
          geometries.forEach((item) => {
            const center = project(item.mesh.position);
            context.save(); context.globalCompositeOperation = "lighter";
            particles.forEach((particle) => {
              const k = liveEffect() ? effectAge / particle.life : 0.45;
              if (k > 1) return;
              const radius = unit * (0.6 + particle.speed * 1.3 * (1 - (1 - k) ** 3));
              context.globalAlpha = (1 - k) * 0.9; context.fillStyle = skin.particle;
              context.beginPath(); context.arc(center.x + Math.cos(particle.angle) * radius, center.y + Math.sin(particle.angle) * radius + unit * 0.5 * k * k, particle.size * (width / 240) * (1 - k * 0.5), 0, 7); context.fill();
            });
            context.restore();
          });
        }
        if (showEffect && props.effect === "fumble") {
          geometries.forEach((item) => {
            const center = project(item.mesh.position);
            context.save(); context.lineJoin = "round"; context.lineCap = "round";
            [crack, [...crack.slice(0, 3), ...branch]].forEach((points) => {
              context.beginPath(); context.moveTo(center.x, center.y);
              points.slice(0, Math.max(1, Math.round(points.length * (liveEffect() ? Math.min(1, effectAge / 0.35) : 1)))).forEach(([x, y]) => context.lineTo(center.x + x! * unit, center.y + y! * unit));
              context.strokeStyle = "rgba(0,0,0,.75)"; context.lineWidth = Math.max(2, unit * 0.045); context.stroke();
              context.strokeStyle = "rgba(255,255,255,.35)"; context.lineWidth = Math.max(1, unit * 0.014); context.stroke();
            });
            context.restore();
          });
        }
      }
    };
    resizeOutput();
    activeEngine.setSize(targetCanvas.width, targetCanvas.height);
    // 先绘制以触发 PBR/BRDF 的异步准备，未就绪时持续补帧。
    render(startTime);
    const loop = (time: number) => {
      if (disposed || revision !== instance) return false;
      render(time);
      if (scene.isReady() && ((!shouldAnimate() || reducedMotion) || (props.motion === "rolling" && time - startTime >= duration) || (props.motion === "result" && time - settledEffectAt >= 1600))) {
        geometries.forEach((item) => {
          item.mesh.rotationQuaternion = item.target;
          item.mesh.position.copyFrom(item.rest);
        });
        if (props.motion === "rolling" && !settledSent) {
          settledSent = true;
          emit("settled", { values: shape.values, effect: props.effect });
          return false;
        }
        if (props.motion !== "result" || time - settledEffectAt >= 1600) return false;
      }
      return true;
    };
    wakeDice(loop);
    await waitForDiceSceneReady(scene.whenReadyAsync(), cancellation.promise);
    pendingScenes.delete(cancellation.cancel);
    if (!disposed && revision === instance) render(performance.now());
    return {
      render,
      wake: () => wakeDice(loop),
      resize: () => {
        resizeOutput();
        relayout();
      },
      dispose: () => {
        diceRenderer.live.delete(loop);
        edgeMeshes.forEach((edgeMesh) => edgeMesh.dispose(false, false));
        geometries.forEach((item) => {
          item.mesh.getChildMeshes().forEach((child) => child.material?.dispose(false, false));
          item.mesh.material?.dispose(false, false);
          item.mesh.dispose(false, false);
        });
        material.dispose(false, true);
        texture.dispose();
        disposeBabylonScene();
      },
    };
  } catch (error) {
    pendingScenes.delete(cancellation.cancel);
    disposeBabylonScene();
    throw error;
  }
}

function quaternionForFace(
  face: DiceFace,
  Vector3: typeof import("@babylonjs/core/Maths/math.vector").Vector3,
  Quaternion: typeof import("@babylonjs/core/Maths/math.vector").Quaternion,
): InstanceType<typeof Quaternion> {
  const from = new Vector3(face.normal.x, face.normal.y, face.normal.z);
  const to = new Vector3(0, 0, 1);
  const axis = Vector3.Cross(from, to);
  const cosine = Vector3.Dot(from, to);
  let align: InstanceType<typeof Quaternion>;
  if (axis.lengthSquared() < 1e-8) {
    align =
      cosine < 0 ? Quaternion.RotationAxis(new Vector3(0, 1, 0), Math.PI) : Quaternion.Identity();
  } else {
    axis.normalize();
    align = Quaternion.RotationAxis(axis, Math.acos(Math.max(-1, Math.min(1, cosine))));
  }
  const up = new Vector3(face.up.x, face.up.y, face.up.z);
  const rotatedUp = new Vector3();
  up.rotateByQuaternionToRef(align, rotatedUp);
  const angle = diceFaceUprightAngle(rotatedUp);
  return Quaternion.RotationAxis(to, angle).multiply(align);
}

function roomEnvironment(size: number): Uint8Array[] {
  if (diceRenderer.environment) return diceRenderer.environment;
  const panels = [
    { position: [0, 5.5, -3.5], angle: 0.62, color: [255, 255, 255], intensity: 2.2 },
    { position: [-5.5, 2, -2.5], angle: 0.5, color: [255, 226, 192], intensity: 1.8 },
    { position: [5.5, 1.5, -2.5], angle: 0.5, color: [200, 220, 255], intensity: 1.8 },
    { position: [0, -5.5, 0], angle: 0.75, color: [26, 28, 34], intensity: 1 },
  ].map((panel) => ({ ...panel, direction: panel.position.map((v) => v / Math.hypot(...panel.position)) }));
  diceRenderer.environment = Array.from({ length: 6 }, (_, face) => {
    const data = new Uint8Array(size * size * 3);
    for (let y = 0; y < size; y += 1) for (let x = 0; x < size; x += 1) {
      const u = 2 * (x + 0.5) / size - 1, v = 2 * (y + 0.5) / size - 1;
      const direction = [[1, -v, -u], [-1, -v, u], [u, 1, v], [u, -1, -v], [u, -v, 1], [-u, -v, -1]][face]!;
      const length = Math.hypot(...direction);
      let color = [58, 63, 72];
      for (const panel of panels) {
        const alignment = direction.reduce((sum, value, index) => sum + value / length * panel.direction[index]!, 0);
        const threshold = Math.cos(panel.angle);
        if (alignment > threshold) {
          const weight = Math.min(1, (alignment - threshold) / (1 - threshold) * 3);
          color = color.map((value, index) => value * (1 - weight) + Math.min(255, panel.color[index]! * panel.intensity * 0.5) * weight);
        }
      }
      data.set(color, (y * size + x) * 3);
    }
    return data;
  });
  return diceRenderer.environment;
}

function quaternionXYZ(
  x: number,
  y: number,
  z: number,
  Quaternion: typeof import("@babylonjs/core/Maths/math.vector").Quaternion,
): InstanceType<typeof Quaternion> {
  const c1 = Math.cos(x / 2),
    c2 = Math.cos(y / 2),
    c3 = Math.cos(z / 2);
  const s1 = Math.sin(x / 2),
    s2 = Math.sin(y / 2),
    s3 = Math.sin(z / 2);
  return new Quaternion(
    s1 * c2 * c3 + c1 * s2 * s3,
    c1 * s2 * c3 - s1 * c2 * s3,
    c1 * c2 * s3 + s1 * s2 * c3,
    c1 * c2 * c3 - s1 * s2 * s3,
  );
}

function color3FromCss(
  value: string,
  Color3: typeof import("@babylonjs/core/Maths/math.color").Color3,
): InstanceType<typeof Color3> {
  if (/^#(?:[\da-f]{3}|[\da-f]{6})$/i.test(value)) return Color3.FromHexString(value);
  const canvas = document.createElement("canvas");
  const context = canvas.getContext("2d");
  if (!context) return Color3.White();
  context.fillStyle = value;
  context.fillRect(0, 0, 1, 1);
  const [red, green, blue] = context.getImageData(0, 0, 1, 1).data;
  return new Color3(red! / 255, green! / 255, blue! / 255);
}

function drawFallbackFrame(): boolean {
  const targetCanvas = canvas.value;
  const shape = getShape();
  return !!targetCanvas && !!shape && drawFlatDiceCanvas(targetCanvas, shape, resolvedSkin());
}

async function renderDice(): Promise<void> {
  if (disposed || !visible.value) return;
  if (!canvas.value || !getShape()) {
    // 先作废正在加载的旧场景，避免它稍后失败时仍使用已卸载的画布。
    revision += 1;
    pendingScenes.forEach((cancel) => cancel());
    pendingScenes.clear();
    cleanupScene?.();
    cleanupScene = undefined;
    available.value = false;
    renderFailed.value = false;
    return;
  }
  const currentRevision = ++revision;
  const keepCurrentFrame = available.value;
  pendingScenes.forEach((cancel) => cancel());
  pendingScenes.clear();
  cleanupScene?.();
  cleanupScene = undefined;
  // 重新布置同尺寸骰子时保留上一帧，等新场景首帧就绪后再替换，避免透明闪屏。
  if (!keepCurrentFrame) {
    available.value = false;
    renderFailed.value = false;
  }
  try {
    const instance = currentRevision;
    const contextLost = () => {
      if (currentRevision !== revision) return;
      available.value = false;
      renderFailed.value = true;
      revision += 1;
      cleanupScene?.();
      cleanupScene = undefined;
      if (props.motion === "rolling") emitSettledOnce(getShape()?.values ?? []);
    };
    const created = await makeScene(canvas.value, instance, contextLost);
    if (disposed || currentRevision !== revision) {
      created.dispose();
      return;
    }
    activeScene = created;
    available.value = true;
    canvasRenderer.value = "babylon";
    cleanupScene = () => {
      if (activeScene === created) activeScene = undefined;
      created.dispose();
    };
  } catch {
    if (disposed || currentRevision !== revision) return;
    available.value = drawFallbackFrame();
    canvasRenderer.value = "2d";
    renderFailed.value = !available.value;
    if (props.motion === "rolling") emitSettledOnce(getShape()?.values ?? []);
  }
}

function emitSettledOnce(values: readonly number[]): void {
  if (settledSent) return;
  settledSent = true;
  emit("settled", { values, effect: props.effect });
}

function observeVisibility(): void {
  if (!host.value || typeof IntersectionObserver === "undefined") {
    visible.value = true;
    void renderDice();
    return;
  }
  observer = new IntersectionObserver((entries) => {
    if (entries.some((entry) => entry.isIntersecting)) {
      visible.value = true;
      void renderDice();
    } else {
      visible.value = false;
      revision += 1;
      cleanupScene?.();
      cleanupScene = undefined;
      available.value = false;
    }
  });
  observer.observe(host.value);
}

function startCharge(event: PointerEvent): void {
  if (!props.interactive || props.motion !== "idle" || event.button !== 0 || pointerStart) return;
  const element = event.currentTarget as HTMLElement;
  element.setPointerCapture(event.pointerId);
  pointerStart = { x: event.clientX, y: event.clientY, time: performance.now() };
  pointerTrail = [pointerStart];
  chargeStart = performance.now();
  chargeTimer = setInterval(() => {
    charge.value = Math.min(1, (performance.now() - chargeStart) / 900);
  }, 30);
}

function moveCharge(event: PointerEvent): void {
  if (!pointerStart) return;
  pointerTrail.push({ x: event.clientX, y: event.clientY, time: performance.now() });
  if (pointerTrail.length > 8) pointerTrail.shift();
}

function endCharge(): void {
  if (!pointerStart) return;
  if (chargeTimer) clearInterval(chargeTimer);
  chargeTimer = undefined;
  const start = pointerTrail[0] ?? pointerStart;
  const end = pointerTrail.at(-1) ?? pointerStart;
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const distance = Math.hypot(dx, dy);
  const elapsed = Math.max(16, end.time - start.time);
  if (props.interactive && props.motion === "idle") emit(
    "submit",
    distance > 20
      ? { direction: [dx / distance, -dy / distance], power: Math.min(1, distance / elapsed / 1.4) }
      : { direction: [-1, 0.3], power: 0.35 + 0.65 * charge.value },
  );
  pointerStart = undefined;
  pointerTrail = [];
  charge.value = 0;
}

function onKeydown(event: KeyboardEvent): void {
  if (!props.interactive || props.motion !== "idle" || event.repeat || event.isComposing || event.keyCode === 229 || (event.key !== "Enter" && event.key !== " ")) return;
  event.preventDefault();
  emit("submit", { direction: [-1, 0.3], power: 0.6 });
}

function cancelCharge(): void {
  if (chargeTimer) clearInterval(chargeTimer);
  chargeTimer = undefined;
  pointerStart = undefined;
  pointerTrail = [];
  charge.value = 0;
}

onMounted(() => {
  releaseRenderer = retainDiceRenderer();
  observeVisibility();
  if (typeof MutationObserver !== "undefined") {
    themeObserver = new MutationObserver(() => {
      if (visible.value) void renderDice();
    });
    themeObserver.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "data-script", "style"],
    });
  }
  if (typeof ResizeObserver !== "undefined" && host.value) {
    resizeObserver = new ResizeObserver(() => {
      activeScene?.resize();
      activeScene?.render(performance.now());
    });
    resizeObserver.observe(host.value);
  }
});

watch(() => props.motion, (motion, previous) => {
  if (motion === "rolling") settledSent = false;
  settledEffectAt = motion === "result" && previous === "rolling" ? performance.now() : 0;
  if (motion === "result" && previous === "rolling") activeScene?.wake();
});

watch(
  () => [
    props.expression,
    props.rolls,
    props.scriptId,
    props.skin,
    props.scriptSkinEnabled,
    props.effect,
    props.seed,
    props.keep,
    props.still,
    props.d6,
  ],
  () => {
    if (host.value && visible.value) void renderDice();
  },
  { flush: "post" },
);

onScopeDispose(() => {
  disposed = true;
  revision += 1;
  if (chargeTimer) clearInterval(chargeTimer);
  observer?.disconnect();
  themeObserver?.disconnect();
  resizeObserver?.disconnect();
  cleanupScene?.();
  pendingScenes.forEach((cancel) => cancel());
  pendingScenes.clear();
  releaseRenderer?.();
});
</script>

<template>
  <div
    ref="host"
    class="record-dice-visual"
    :class="{
      'record-dice-fallback': !available,
      'dice-effect-crit': props.effect === 'crit' && props.motion === 'result',
      'dice-effect-fumble': props.effect === 'fumble' && props.motion === 'result',
      'dice-interactive': props.interactive,
    }"
    :data-renderer="available ? canvasRenderer : renderFailed ? 'static' : getShape() ? 'pending' : 'static'"
    :data-motion="props.motion"
    :style="visualStyle"
    role="img"
    :aria-label="`${props.expression}: ${props.rolls.map((roll) => roll.value).join(', ') || t('game.checkTray.rolling')}`"
    @pointerdown="startCharge"
    @pointermove="moveCharge"
    @pointerup="endCharge"
    @pointercancel="cancelCharge"
    @keydown="onKeydown"
  >
    <canvas
      v-if="getShape()"
      ref="canvas"
      class="record-dice-canvas"
      :style="{ visibility: available ? 'visible' : 'hidden' }"
      aria-hidden="true"
    />
    <span
      v-for="(value, index) in renderFailed ? fallbackValues : []"
      :key="index"
      class="record-dice-value"
      aria-hidden="true"
    >
      {{ value }}
    </span>
    <span
      v-for="(roll, index) in props.showPop ? props.rolls : []"
      :key="`pop-${index}`"
      ref="popElements"
      class="dice-number-pop"
      :class="{
        on: props.motion === 'result',
        dim: props.keep && props.rolls.findIndex((die) => die.value === (props.keep === 'high' ? Math.max(...props.rolls.map((entry) => entry.value)) : Math.min(...props.rolls.map((entry) => entry.value)))) !== index,
      }"
      aria-hidden="true"
    >
      {{ roll.value }}
    </span>
    <span
      v-if="props.interactive && charge > 0"
      class="dice-charge-meter"
      :style="{ '--charge': charge }"
      aria-hidden="true"
    />
  </div>
</template>
