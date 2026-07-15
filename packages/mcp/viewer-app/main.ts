/**
 * vcad MCP Apps viewer (SEP-1865 View).
 *
 * Bundled by Vite into a single self-contained HTML resource — no CDN
 * imports at render time. Receives tool results from the host via the
 * official App class, then fetches the GLB preview through the app-only
 * `get_preview_glb` tool so geometry never rides in model-visible results.
 *
 * The render rig mirrors the main app's viewport (ViewportContent.tsx):
 * same key/fill/rim directional lights, the same procedural studio IBL
 * baked through PMREM, the same infinite grid, axis lines, contact
 * shadow, and ACES tone mapping — so a part previewed here reads the
 * same as it does on vcad.io.
 */

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import {
  App,
  type McpUiHostContext,
} from "@modelcontextprotocol/ext-apps";
import { isOpenAiHost, createOpenAiShim } from "./openai-shim";

const statusEl = document.getElementById("status")!;
const errEl = document.getElementById("error")!;
const loadingEl = document.getElementById("loading")!;
const stageEl = document.getElementById("stage")!;
const docLabelEl = document.getElementById("doc-label")!;
const tickerEl = document.getElementById("ticker")!;
const statsEl = document.getElementById("stats")!;
const pulseEl = document.getElementById("pulse")!;
const openBtn = document.getElementById("open-btn") as HTMLButtonElement;
const fullscreenBtn = document.getElementById("fullscreen-btn") as HTMLButtonElement;
const selChipEl = document.getElementById("sel-chip")!;
const selLabelEl = document.getElementById("sel-label")!;
const selClearBtn = document.getElementById("sel-clear") as HTMLButtonElement;
const askBtn = document.getElementById("ask-btn") as HTMLButtonElement;
const askBarEl = document.getElementById("ask-bar")!;
const askPrefixEl = document.getElementById("ask-prefix")!;
const askInput = document.getElementById("ask-input") as HTMLInputElement;
const askSendBtn = document.getElementById("ask-send") as HTMLButtonElement;
const receiptEl = document.getElementById("receipt")!;
const receiptBadgeEl = document.getElementById("receipt-badge")!;
const receiptHashEl = document.getElementById("receipt-hash")!;
const receiptBodyEl = document.getElementById("receipt-body")!;
const receiptRerunBtn = document.getElementById("receipt-rerun") as HTMLButtonElement;
const receiptCloseBtn = document.getElementById("receipt-close") as HTMLButtonElement;
const transportEl = document.getElementById("transport")!;
const tpPlayBtn = document.getElementById("tp-play") as HTMLButtonElement;
const tpScrubEl = document.getElementById("tp-scrub") as HTMLInputElement;
const tpStepEl = document.getElementById("tp-step")!;
const tpSpeedEl = document.getElementById("tp-speed") as HTMLSelectElement;
const tpSparkLineEl = document.getElementById("tp-spark-line")!;
const tpSparkDotEl = document.getElementById("tp-spark-dot")!;
const tpLiveEl = document.getElementById("tp-live") as HTMLButtonElement;
const tpJointsEl = document.getElementById("tp-joints")!;
const tpNoteEl = document.getElementById("tp-note")!;
const ordersEl = document.getElementById("orders")!;
const ordersBodyEl = document.getElementById("orders-body")!;
const ordersCountEl = document.getElementById("orders-count")!;
const ordersFootEl = document.getElementById("orders-foot")!;
const ordersToggleBtn = document.getElementById("orders-toggle") as HTMLButtonElement;

// Hostless dev harness (`#dev*`): selection affordances stay active but
// protocol calls are logged instead of sent.
const devMode = location.hash.startsWith("#dev");

type PulseState = "idle" | "busy" | "ready" | "error";

function setStatus(text: string, pulse: PulseState = "busy"): void {
  loadingEl.classList.remove("hidden");
  statusEl.textContent = text;
  setTicker(text, pulse);
}

function setTicker(text: string, pulse: PulseState): void {
  tickerEl.textContent = text;
  pulseEl.className = pulse === "idle" ? "" : pulse;
}

// ── Theme ────────────────────────────────────────────────────
// The viewer is vcad-branded: the host only chooses dark/light, the
// palette is vcad's own (mirrors packages/app/src/index.css).
let isDark = true;

// ── Scene setup ──────────────────────────────────────────────
// Alpha canvas: the page background is the vcad stage color (--bg), so
// theme switches don't require re-creating the scene.
const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.0;
stageEl.insertBefore(renderer.domElement, stageEl.firstChild);

const scene = new THREE.Scene();

const camera = new THREE.PerspectiveCamera(50, 1, 0.1, 10000);
camera.position.set(50, 50, 50);

const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;
controls.dampingFactor = 0.1;

// ── Lighting — the app's default three-point rig ─────────────
// Same directions, colors, and theme-aware intensities as
// buildDefaultSceneSettings(): positions are -direction * 100.
const key = new THREE.DirectionalLight(new THREE.Color(1, 0.98, 0.95), 1.4);
key.position.set(-50, 80, -40);
scene.add(key);

const fill = new THREE.DirectionalLight(new THREE.Color(0.95, 0.97, 1.0), 0.5);
fill.position.set(30, 40, 20);
scene.add(fill);

const rim = new THREE.DirectionalLight(new THREE.Color(1, 1, 1), 0.35);
rim.position.set(50, 20, -50);
scene.add(rim);

function applyLightIntensities(): void {
  key.intensity = isDark ? 1.4 : 1.2;
  fill.intensity = isDark ? 0.5 : 0.4;
  rim.intensity = isDark ? 0.35 : 0.2;
}

// ── Procedural studio IBL ────────────────────────────────────
// Mirrors the app's default <Environment> Lightformer rig: cool-white
// key softbox, warm + cool fills, bright rim strip, faint floor bounce,
// baked into a PMREM cubemap so metallic reflections read like a
// product shot. Numbers are lifted verbatim from ViewportContent.tsx.
const pmrem = new THREE.PMREMGenerator(renderer);

interface Former {
  intensity: [number, number]; // [dark, light]
  color: [number, number, number];
  position: [number, number, number];
  rotation: [number, number, number];
  scale: [number, number];
}

const LIGHTFORMERS: Former[] = [
  // Key — cool-white ceiling softbox, tilted camera-left.
  { intensity: [3.2, 5.5], color: [1.0, 0.99, 0.97], position: [3, 9, 3], rotation: [-Math.PI / 2 + 0.25, 0, 0.2], scale: [18, 14] },
  // Fill — warmer, camera-right, lower.
  { intensity: [1.3, 2.2], color: [1.0, 0.94, 0.86], position: [10, 2, 4], rotation: [0, -Math.PI / 2, 0.1], scale: [12, 8] },
  // Opposite-side fill — slightly cooler.
  { intensity: [0.9, 1.6], color: [0.96, 0.98, 1.0], position: [-10, 2, 0], rotation: [0, Math.PI / 2, -0.1], scale: [12, 8] },
  // Rim — bright narrow strip behind the camera.
  { intensity: [2.2, 3.6], color: [0.98, 1.0, 1.0], position: [0, 4, -12], rotation: [0, 0, 0], scale: [14, 5] },
  // Faint floor bounce.
  { intensity: [0.25, 0.5], color: [0.92, 0.94, 0.96], position: [0, -8, 0], rotation: [Math.PI / 2, 0, 0], scale: [20, 20] },
];

function buildStudioEnvironment(): void {
  const envScene = new THREE.Scene();
  envScene.background = new THREE.Color(
    ...(isDark ? [0.018, 0.02, 0.024] : [0.62, 0.6, 0.58]) as [number, number, number],
  );
  for (const f of LIGHTFORMERS) {
    const mat = new THREE.MeshBasicMaterial({ side: THREE.DoubleSide });
    mat.color.setRGB(...f.color).multiplyScalar(f.intensity[isDark ? 0 : 1]);
    const plane = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), mat);
    plane.position.set(...f.position);
    plane.rotation.set(...f.rotation);
    plane.scale.set(f.scale[0], f.scale[1], 1);
    envScene.add(plane);
  }
  const target = pmrem.fromScene(envScene, 0, 0.1, 1000);
  scene.environment?.dispose();
  scene.environment = target.texture;
  envScene.traverse((o) => {
    const mesh = o as THREE.Mesh;
    if (mesh.geometry) mesh.geometry.dispose();
    if (mesh.material) (mesh.material as THREE.Material).dispose();
  });
}

// ── Grid — drei <Grid> port (GridPlane.tsx) ──────────────────
// cellSize 10 / sectionSize 100, fadeDistance 500, theme-aware colors.
// Plane follows the camera so the grid reads as infinite.
const GRID_FADE = 500;

const gridUniforms = {
  uCellSize: { value: 10 },
  uSectionSize: { value: 100 },
  uCellColor: { value: new THREE.Color("#2a2a2a") },
  uSectionColor: { value: new THREE.Color("#3a3a3a") },
  uCellThickness: { value: 0.5 },
  uSectionThickness: { value: 1.0 },
  uFadeDistance: { value: GRID_FADE },
};

const gridMaterial = new THREE.ShaderMaterial({
  uniforms: gridUniforms,
  transparent: true,
  depthWrite: false,
  side: THREE.DoubleSide,
  vertexShader: /* glsl */ `
    varying vec3 vWorldPos;
    void main() {
      vec4 wp = modelMatrix * vec4(position, 1.0);
      vWorldPos = wp.xyz;
      gl_Position = projectionMatrix * viewMatrix * wp;
    }
  `,
  fragmentShader: /* glsl */ `
    varying vec3 vWorldPos;
    uniform float uCellSize, uSectionSize, uCellThickness, uSectionThickness, uFadeDistance;
    uniform vec3 uCellColor, uSectionColor;

    float gridLine(vec2 p, float size, float thickness) {
      vec2 r = p / size;
      vec2 grid = abs(fract(r - 0.5) - 0.5) / fwidth(r);
      float line = min(grid.x, grid.y) + 1.0 - thickness;
      return 1.0 - min(line, 1.0);
    }

    void main() {
      float cell = gridLine(vWorldPos.xz, uCellSize, uCellThickness);
      float section = gridLine(vWorldPos.xz, uSectionSize, uSectionThickness);
      float dist = distance(vWorldPos.xz, cameraPosition.xz);
      float fade = 1.0 - smoothstep(0.0, uFadeDistance, dist);
      vec3 color = mix(uCellColor, uSectionColor, step(0.5, section));
      float alpha = max(cell, section) * fade;
      if (alpha < 0.003) discard;
      gl_FragColor = vec4(color, alpha);
      #include <tonemapping_fragment>
      #include <colorspace_fragment>
    }
  `,
});

const grid = new THREE.Mesh(new THREE.PlaneGeometry(GRID_FADE * 4, GRID_FADE * 4), gridMaterial);
grid.rotation.x = -Math.PI / 2;
grid.renderOrder = -2;
scene.add(grid);

// ── Axis lines (GridPlane.tsx) ───────────────────────────────
// RGB convention in Z-up labels: X red, Y green (ground), Z blue (up).
// Shown while orbiting or when there's nothing loaded — same rule as
// the app (`isOrbiting || !hasParts`).
const axes = new THREE.Group();

function axisLine(points: [number, number, number][], colorDark: string, colorLight: string): { line: THREE.Line; dark: string; light: string } {
  const geom = new THREE.BufferGeometry().setFromPoints(points.map((p) => new THREE.Vector3(...p)));
  const mat = new THREE.LineBasicMaterial({ color: colorDark, transparent: true, opacity: 0.7, depthWrite: false });
  const line = new THREE.Line(geom, mat);
  line.renderOrder = -1;
  return { line, dark: colorDark, light: colorLight };
}

const axisDefs = [
  axisLine([[-500, 0, 0], [500, 0, 0]], "#e06c75", "#c94f4f"), // X
  axisLine([[0, 0, -500], [0, 0, 500]], "#98c379", "#5a9a4a"), // Y (ground)
  axisLine([[0, 0, 0], [0, 500, 0]], "#61afef", "#4a7dc9"),    // Z (up)
];
for (const a of axisDefs) axes.add(a.line);
scene.add(axes);

let isOrbiting = false;
let hasModel = false;

function updateAxesVisibility(): void {
  axes.visible = isOrbiting || !hasModel;
}
updateAxesVisibility();

controls.addEventListener("start", () => {
  isOrbiting = true;
  updateAxesVisibility();
});
controls.addEventListener("end", () => {
  isOrbiting = false;
  updateAxesVisibility();
});

// ── Contact shadow ───────────────────────────────────────────
// Cheap stand-in for the app's <ContactShadows>: a radial-gradient
// sprite under the part, sized to the model footprint on load.
const shadowTexture = (() => {
  const size = 256;
  const canvas = document.createElement("canvas");
  canvas.width = canvas.height = size;
  const ctx = canvas.getContext("2d")!;
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  g.addColorStop(0, "rgba(0,0,0,1)");
  g.addColorStop(0.5, "rgba(0,0,0,0.45)");
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  const tex = new THREE.CanvasTexture(canvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  return tex;
})();

const shadowMaterial = new THREE.MeshBasicMaterial({
  map: shadowTexture,
  transparent: true,
  opacity: 0.4,
  depthWrite: false,
});
const contactShadow = new THREE.Mesh(new THREE.PlaneGeometry(1, 1), shadowMaterial);
contactShadow.rotation.x = -Math.PI / 2;
contactShadow.position.y = -0.01;
contactShadow.visible = false;
scene.add(contactShadow);

function applyTheme(): void {
  document.documentElement.classList.toggle("light", !isDark);
  document.body.classList.toggle("light", !isDark);
  applyLightIntensities();
  buildStudioEnvironment();
  gridUniforms.uCellColor.value.set(isDark ? "#2a2a2a" : "#555555");
  gridUniforms.uSectionColor.value.set(isDark ? "#3a3a3a" : "#333333");
  for (const a of axisDefs) {
    (a.line.material as THREE.LineBasicMaterial).color.set(isDark ? a.dark : a.light);
  }
  shadowMaterial.opacity = isDark ? 0.4 : 0.3;
}
applyTheme();

// ── Resize handler ───────────────────────────────────────────
function resize(): void {
  const w = stageEl.clientWidth || window.innerWidth;
  const h = stageEl.clientHeight || window.innerHeight;
  camera.aspect = w / h;
  camera.updateProjectionMatrix();
  renderer.setSize(w, h);
}
new ResizeObserver(resize).observe(stageEl);
resize();

// ── Animation loop ───────────────────────────────────────────
// Sim replay playback hook — assigned by the transport section below.
// Declared here (before animate's first synchronous call) so the rAF
// loop can guard-call it without a TDZ trap.
let simTick: ((deltaSeconds: number) => void) | null = null;
// GLB animation playback — set by loadGlb when the parsed model carries
// glTF animation tracks (render_sequence timelines). Loops all clips.
let glbMixer: THREE.AnimationMixer | null = null;
// Camera-orbit carrier: an empty node named "__camera" whose animated yaw
// encodes the shot's azimuth. We orbit OUR camera from it each frame
// instead of rendering it, so the model stays put while the view sweeps.
let glbCameraNode: THREE.Object3D | null = null;
let glbCameraLastYaw: number | null = null;
let lastFrameMs = performance.now();

function animate(): void {
  requestAnimationFrame(animate);
  const now = performance.now();
  const deltaSeconds = (now - lastFrameMs) / 1000;
  lastFrameMs = now;
  if (simTick) simTick(deltaSeconds);
  if (glbMixer) {
    glbMixer.update(deltaSeconds);
    if (glbCameraNode) {
      // Model-space yaw about Z from the carrier's quaternion.
      const q = glbCameraNode.quaternion;
      const yaw = 2 * Math.atan2(q.z, q.w);
      if (glbCameraLastYaw !== null) {
        // Apply the delta as an orbit around the target about display up
        // (Y) — composes with user drag instead of fighting it.
        const delta = yaw - glbCameraLastYaw;
        if (delta !== 0) {
          const spin = new THREE.Quaternion().setFromAxisAngle(
            new THREE.Vector3(0, 1, 0),
            delta,
          );
          camera.position
            .sub(controls.target)
            .applyQuaternion(spin)
            .add(controls.target);
        }
      }
      glbCameraLastYaw = yaw;
    }
  }
  controls.update();
  renderer.render(scene, camera);
}
animate();

// ── Model group (Z-up to Y-up conversion) ───────────────────
const modelGroup = new THREE.Group();
modelGroup.rotation.x = -Math.PI / 2; // Z-up → Y-up
scene.add(modelGroup);

// ── GLB loading ──────────────────────────────────────────────
const loader = new GLTFLoader();
let currentModel: THREE.Object3D | null = null;

function formatDim(v: number): string {
  return v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2);
}

function updateStats(model: THREE.Object3D, size: THREE.Vector3): void {
  let meshes = 0;
  let tris = 0;
  model.traverse((child) => {
    const mesh = child as THREE.Mesh;
    if (!mesh.isMesh || !mesh.geometry) return;
    meshes++;
    const g = mesh.geometry;
    tris += (g.index ? g.index.count : g.attributes.position?.count ?? 0) / 3;
  });
  const triLabel = tris >= 10_000 ? `${(tris / 1000).toFixed(1)}k` : `${Math.round(tris)}`;
  statsEl.textContent =
    `${meshes} ${meshes === 1 ? "body" : "bodies"} · ${triLabel} tris · ` +
    `${formatDim(size.x)} × ${formatDim(size.y)} × ${formatDim(size.z)} mm`;
}

function clearModel(): void {
  if (glbMixer) {
    glbMixer.stopAllAction();
    glbMixer = null;
  }
  glbCameraNode = null;
  glbCameraLastYaw = null;
  if (!currentModel) return;
  // Restore selection highlights before disposal so we never dispose a
  // clone while the original is detached.
  select(null);
  modelGroup.remove(currentModel);
  currentModel.traverse((child) => {
    const mesh = child as THREE.Mesh;
    if (mesh.geometry) mesh.geometry.dispose();
    if (mesh.material) {
      if (Array.isArray(mesh.material)) mesh.material.forEach((m) => m.dispose());
      else mesh.material.dispose();
    }
  });
  currentModel = null;
}

interface LoadOpts {
  /** Hold the current camera framing instead of re-fitting — used when the
   *  same document re-renders so the part accretes in place. */
  preserveCamera?: boolean;
  /** Runs once the new model is in the scene (re-select, flash, …). */
  afterLoad?: () => void;
}

// The studio IBL is tuned bright for hero metallic CAD parts; on a PCB's
// copper — and on any low-roughness metal — it blows reflections into a wet,
// glassy sheet. Scale every loaded material's environment response down and
// floor the roughness so reflections read as a soft sheen rather than chrome.
// Applied after GLTF import, so it rides on top of whatever the GLB carries
// (including KHR_materials_clearcoat soldermask).
const ENV_MAP_INTENSITY = 0.5;
const MIN_ROUGHNESS = 0.3;

function tameMaterials(root: THREE.Object3D): void {
  root.traverse((child) => {
    const mesh = child as THREE.Mesh;
    if (!mesh.isMesh || !mesh.material) return;
    const mats = Array.isArray(mesh.material) ? mesh.material : [mesh.material];
    for (const m of mats) {
      const std = m as THREE.MeshStandardMaterial;
      // MeshStandard/Physical only — Basic/Line materials have no IBL.
      if (std.envMapIntensity === undefined) continue;
      std.envMapIntensity = ENV_MAP_INTENSITY;
      std.roughness = Math.max(std.roughness, MIN_ROUGHNESS);
      std.needsUpdate = true;
    }
  });
}

function loadGlb(base64Data: string, opts?: LoadOpts): void {
  const binary = atob(base64Data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }

  loader.parse(
    bytes.buffer,
    "",
    (gltf) => {
      // Swap only once the new model has parsed, so a same-document
      // re-render never blinks through an empty scene (and a parse error
      // leaves the current model untouched).
      clearModel();
      currentModel = gltf.scene;
      tameMaterials(currentModel);
      modelGroup.add(currentModel);
      hasModel = true;
      // Autoplay embedded glTF animation tracks (render_sequence output):
      // the agent's dailies loop plays inline with no extra round-trips.
      if (gltf.animations.length > 0) {
        glbMixer = new THREE.AnimationMixer(gltf.scene);
        for (const clip of gltf.animations) {
          glbMixer.clipAction(clip).setLoop(THREE.LoopRepeat, Infinity).play();
        }
        // Camera-orbit carrier node (see the animate-loop hook above).
        glbCameraNode = gltf.scene.getObjectByName("__camera") ?? null;
        glbCameraLastYaw = null;
        if (glbCameraNode) glbCameraNode.visible = false;
      }
      updateAxesVisibility();

      // setFromObject resolves world (Y-up) coordinates — the model sits
      // under modelGroup, which already carries the Z-up → Y-up rotation —
      // so the center/size below are display-space, no further conversion.
      const worldBox = new THREE.Box3().setFromObject(modelGroup);
      const worldSize = worldBox.getSize(new THREE.Vector3());
      const worldCenter = worldBox.getCenter(new THREE.Vector3());

      if (!opts?.preserveCamera) {
        // Fit camera to model and orbit around its center (not the origin).
        const maxDim = Math.max(worldSize.x, worldSize.y, worldSize.z);
        const dist = maxDim * 2;
        controls.target.copy(worldCenter);
        camera.position.set(
          worldCenter.x + dist * 0.7,
          worldCenter.y + dist * 0.7,
          worldCenter.z + dist * 0.7,
        );
        camera.updateProjectionMatrix();
        controls.update();
      }

      // Contact shadow under the footprint (world Y-up space) — rebound on
      // every load so the part stays grounded as its footprint morphs.
      const footprint = Math.max(worldSize.x, worldSize.z) * 1.8;
      contactShadow.scale.set(footprint, footprint, 1);
      contactShadow.position.set(worldCenter.x, -0.01, worldCenter.z);
      contactShadow.visible = true;

      // Stats report kernel Z-up mm: world (x, z, y) → kernel (x, y, z).
      updateStats(
        currentModel,
        new THREE.Vector3(worldSize.x, worldSize.z, worldSize.y),
      );
      loadingEl.classList.add("hidden");
      setTicker("ready", "ready");
      opts?.afterLoad?.();
      renderer.render(scene, camera);
    },
    (error) => {
      console.error("GLB parse error:", error);
      setStatus("error loading model", "error");
    },
  );
}

// ── Part selection — pointing for CAD ────────────────────────
// GLB nodes are named "<part_id>:<name>" by the server (buildPartLabels),
// so a raycast hit maps back to real part identity. Selection drives:
//  1. a local inspector chip (name, id, dims) — works on every host
//  2. ui/update-model-context — silent deixis, so "make this taller"
//     typed in the host chat resolves to the selected part
//  3. an "Ask" composer that sends a part-grounded ui/message
// Both protocol calls are capability-gated via getHostCapabilities().

interface SelectedPart {
  partId: string;
  name: string;
  object: THREE.Object3D;
}

let selected: SelectedPart | null = null;
let lastPushedContext = "";
const SELECT_EMISSIVE = 0xf92672; // --brand

/** Walk up from a raycast hit to the nearest part-labeled ancestor. */
function partInfoFor(object: THREE.Object3D): SelectedPart | null {
  let o: THREE.Object3D | null = object;
  while (o) {
    const m = /^(\d+):(.*)$/.exec(o.name);
    if (m) {
      return { partId: m[1], name: m[2] || `part ${m[1]}`, object: o };
    }
    o = o.parent;
  }
  return null;
}

/** Brand-pink emissive highlight. Materials are cloned per-mesh on
 *  select (GLB materials are shared between same-material parts) and
 *  restored + disposed on deselect. */
function setHighlight(root: THREE.Object3D, on: boolean): void {
  root.traverse((child) => {
    const mesh = child as THREE.Mesh;
    if (!mesh.isMesh) return;
    if (on) {
      // Idempotent: a mesh already highlighted (by selection or a flash)
      // keeps its saved original, so overlapping highlights never lose it.
      if (mesh.userData.origMaterial) return;
      const orig = mesh.material as THREE.MeshStandardMaterial;
      if (!orig?.clone) return;
      const highlighted = orig.clone();
      highlighted.emissive = new THREE.Color(SELECT_EMISSIVE);
      highlighted.emissiveIntensity = 0.28;
      mesh.userData.origMaterial = orig;
      mesh.material = highlighted;
    } else if (mesh.userData.origMaterial) {
      (mesh.material as THREE.Material).dispose();
      mesh.material = mesh.userData.origMaterial as THREE.Material;
      delete mesh.userData.origMaterial;
    }
  });
}

/** Selected part's bbox in kernel Z-up mm (world Y-up → kernel swap). */
function selectedDims(): { x: number; y: number; z: number } | null {
  if (!selected) return null;
  const size = new THREE.Box3().setFromObject(selected.object).getSize(new THREE.Vector3());
  return { x: size.x, y: size.z, z: size.y };
}

function select(info: SelectedPart | null): void {
  if (selected?.object === info?.object) return;
  if (selected) setHighlight(selected.object, false);
  selected = info;
  if (selected) {
    setHighlight(selected.object, true);
    const d = selectedDims();
    const dims = d ? ` · ${formatDim(d.x)} × ${formatDim(d.y)} × ${formatDim(d.z)}` : "";
    selLabelEl.innerHTML = "";
    const b = document.createElement("b");
    b.textContent = selected.name;
    selLabelEl.append(b, ` #${selected.partId}${dims}`);
    selChipEl.classList.add("visible");
  } else {
    selChipEl.classList.remove("visible");
    askBarEl.classList.remove("visible");
  }
  void pushSelectionContext();
}

/** Silent ui/update-model-context push: the host delivers the latest
 *  snapshot alongside the user's next chat message, so bare "this"
 *  references resolve to the selected part. Overwrite semantics mean
 *  deselection must push too (clears the stale pointer). */
async function pushSelectionContext(): Promise<void> {
  const d = selectedDims();
  const payload = {
    document_id: lastDocumentId,
    viewer_selection: selected
      ? {
          part_id: selected.partId,
          name: selected.name,
          bbox_mm: d ? { x: +d.x.toFixed(2), y: +d.y.toFixed(2), z: +d.z.toFixed(2) } : undefined,
        }
      : null,
  };
  const serialized = JSON.stringify(payload);
  if (serialized === lastPushedContext) return;
  lastPushedContext = serialized;

  const sentence = selected
    ? `The user has selected part "${selected.name}" (part_id ${selected.partId})` +
      `${lastDocumentId ? ` of document ${lastDocumentId}` : ""} in the vcad 3D viewer` +
      `${d ? `; its bounding box is ${formatDim(d.x)} × ${formatDim(d.y)} × ${formatDim(d.z)} mm` : ""}. ` +
      `Unqualified references like "this part" mean this selection.`
    : "The user has cleared the part selection in the vcad 3D viewer.";

  if (devMode) {
    console.log("[vcad-viewer:dev] updateModelContext:", payload, sentence);
    return;
  }
  const caps = app.getHostCapabilities();
  if (!caps?.updateModelContext) return;
  try {
    await app.updateModelContext({
      content: [{ type: "text", text: sentence }],
      ...(caps.updateModelContext.structuredContent
        ? { structuredContent: payload }
        : {}),
    });
  } catch (e) {
    console.warn("[vcad-viewer] updateModelContext failed:", e);
  }
}

/** Show capability-gated affordances. Called after connect (and in dev). */
function updateAffordances(): void {
  const caps = devMode ? null : app.getHostCapabilities();
  askBtn.classList.toggle("visible", devMode || Boolean(caps?.message));
}

// Click-to-pick: distinguish a click from an orbit via pointer travel.
const raycaster = new THREE.Raycaster();
const pointerNdc = new THREE.Vector2();
let pointerDown: { x: number; y: number } | null = null;

renderer.domElement.addEventListener("pointerdown", (e) => {
  if (e.button === 0) pointerDown = { x: e.clientX, y: e.clientY };
});

renderer.domElement.addEventListener("pointerup", (e) => {
  if (!pointerDown || e.button !== 0) return;
  const travel = Math.hypot(e.clientX - pointerDown.x, e.clientY - pointerDown.y);
  pointerDown = null;
  if (travel > 5) return; // it was an orbit, not a click

  const rect = renderer.domElement.getBoundingClientRect();
  pointerNdc.set(
    ((e.clientX - rect.left) / rect.width) * 2 - 1,
    -((e.clientY - rect.top) / rect.height) * 2 + 1,
  );
  raycaster.setFromCamera(pointerNdc, camera);
  for (const hit of raycaster.intersectObject(modelGroup, true)) {
    const info = partInfoFor(hit.object);
    if (info) {
      select(info);
      return;
    }
  }
  select(null);
});

window.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  if (askBarEl.classList.contains("visible")) {
    askBarEl.classList.remove("visible");
  } else {
    select(null);
  }
});

selClearBtn.addEventListener("click", () => select(null));

// ── Ask composer — part-grounded ui/message ──────────────────
askBtn.addEventListener("click", () => {
  if (!selected) return;
  askPrefixEl.textContent = `${selected.name} ▸`;
  askBarEl.classList.add("visible");
  askInput.focus();
});

async function sendAsk(): Promise<void> {
  if (!selected) return;
  const text = askInput.value.trim();
  if (!text) return;
  const grounded =
    `About part "${selected.name}" (part_id ${selected.partId}` +
    `${lastDocumentId ? `, document ${lastDocumentId}` : ""}) in the vcad viewer: ${text}`;
  askSendBtn.disabled = true;
  try {
    if (devMode) {
      console.log("[vcad-viewer:dev] sendMessage:", grounded);
    } else {
      const result = await app.sendMessage({
        role: "user",
        content: [{ type: "text", text: grounded }],
      });
      if (result.isError) {
        setTicker("host declined the message", "error");
        return;
      }
    }
    askInput.value = "";
    askBarEl.classList.remove("visible");
    setTicker("sent to chat", "ready");
  } catch (e) {
    console.warn("[vcad-viewer] sendMessage failed:", e);
    setTicker("message failed", "error");
  } finally {
    askSendBtn.disabled = false;
  }
}

askSendBtn.addEventListener("click", () => void sendAsk());
askInput.addEventListener("keydown", (e) => {
  if (e.key === "Enter") void sendAsk();
  if (e.key === "Escape") askBarEl.classList.remove("visible");
  e.stopPropagation(); // keep Escape-in-input from also clearing the selection
});

// ── Flat pattern rendering (sheet_metal_unfold) ──────────────
// Drafting-table view: the cut silhouette and dashed bend centerlines
// drawn on the ground plane with a near-top-down camera. Colors match
// the app's SheetMetalView: cut + bend-up red, bend-down blue.
interface FlatPatternLike {
  panel_outlines_2d?: [number, number][][];
  panel_holes_2d?: [number, number][][][];
  silhouette_2d?: [number, number][][];
  creases?: Array<{
    line: [[number, number], [number, number]];
    direction?: string;
  }>;
  area_mm2?: number;
  bbox?: [number, number, number, number];
}

// Kernel-space Z lift so the drawing sits just above the grid plane.
const FLAT_LIFT = 0.05;

function flatLoop(
  ring: [number, number][],
  material: THREE.LineBasicMaterial,
): THREE.LineLoop {
  const points = ring.map(([x, y]) => new THREE.Vector3(x, y, FLAT_LIFT));
  const geom = new THREE.BufferGeometry().setFromPoints(points);
  return new THREE.LineLoop(geom, material);
}

function renderFlatPattern(fp: FlatPatternLike): void {
  clearModel();

  const group = new THREE.Group();
  const cutMaterial = new THREE.LineBasicMaterial({ color: 0xef4444 });

  // Prefer the merged fab silhouette; fall back to per-panel outlines.
  const rings = fp.silhouette_2d?.length
    ? fp.silhouette_2d
    : [
        ...(fp.panel_outlines_2d ?? []),
        ...(fp.panel_holes_2d ?? []).flat(),
      ];
  for (const ring of rings) {
    if (ring.length >= 2) group.add(flatLoop(ring, cutMaterial));
  }

  const [minX, minY, maxX, maxY] = fp.bbox ?? [0, 0, 0, 0];
  const span = Math.max(maxX - minX, maxY - minY, 1);

  for (const crease of fp.creases ?? []) {
    const up = crease.direction !== "Down";
    const mat = new THREE.LineDashedMaterial({
      color: up ? 0xef4444 : 0x3b82f6,
      dashSize: span / 50,
      gapSize: span / 80,
    });
    const [[x0, y0], [x1, y1]] = crease.line;
    const geom = new THREE.BufferGeometry().setFromPoints([
      new THREE.Vector3(x0, y0, FLAT_LIFT),
      new THREE.Vector3(x1, y1, FLAT_LIFT),
    ]);
    const line = new THREE.Line(geom, mat);
    line.computeLineDistances();
    group.add(line);
  }

  modelGroup.add(group);
  currentModel = group;
  hasModel = true;
  updateAxesVisibility();
  contactShadow.visible = false;

  // Near-top-down camera over the pattern (kernel XY → display X/-Z).
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  controls.target.set(cx, 0, -cy);
  camera.position.set(cx, span * 1.5, -cy + span * 0.3);
  camera.updateProjectionMatrix();
  controls.update();

  const panels = fp.panel_outlines_2d?.length ?? 0;
  const bends = fp.creases?.length ?? 0;
  statsEl.textContent =
    `${panels} ${panels === 1 ? "panel" : "panels"} · ` +
    `${bends} ${bends === 1 ? "bend" : "bends"} · ` +
    `${formatDim(maxX - minX)} × ${formatDim(maxY - minY)} mm flat`;
  loadingEl.classList.add("hidden");
  setTicker("flat pattern — red cut/bend-up, blue bend-down", "ready");
  renderer.render(scene, camera);
}

// ── Result handling ──────────────────────────────────────────
type ContentBlock = { type?: string; text?: string };
type ToolResultLike = {
  content?: ContentBlock[];
  structuredContent?: Record<string, unknown>;
  isError?: boolean;
  _meta?: Record<string, unknown>;
};

/** A ready-to-render preview the server rode along in the mount result's
 *  `_meta` — first paint without a get_preview_glb round trip. */
type InlineMetaPreview = {
  document_id?: string;
  glb?: string;
  version?: string;
  mode?: string;
};

function findMetaPreview(result: ToolResultLike): InlineMetaPreview | null {
  const p = result._meta?.["vcad.io/preview"];
  if (!p || typeof p !== "object") return null;
  const preview = p as InlineMetaPreview;
  return typeof preview.glb === "string" && preview.glb.length > 0 ? preview : null;
}

/** Find an inline `_vcad_glb` payload in result content blocks (legacy
 *  servers attached the GLB directly to the tool result). */
function findInlineGlb(result: ToolResultLike): string | null {
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as { _vcad_glb?: string };
      if (parsed._vcad_glb) return parsed._vcad_glb;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** Find a `flat_pattern` payload (sheet_metal_unfold) in the result. */
function findFlatPattern(result: ToolResultLike): FlatPatternLike | null {
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as { flat_pattern?: FlatPatternLike };
      if (parsed.flat_pattern) return parsed.flat_pattern;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** Find a session document_id in any JSON text block of the result. */
function findDocumentId(result: ToolResultLike): string | null {
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as { document_id?: string };
      if (typeof parsed.document_id === "string") return parsed.document_id;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** Pull the cheap `version` change token out of a preview result, if any. */
function findPreviewVersion(result: ToolResultLike): string | null {
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as { version?: unknown };
      if (typeof parsed.version === "string") return parsed.version;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** Generic payload finder: structuredContent first (the only carrier
 *  ChatGPT's widget bridge exposes), then any JSON text block that
 *  satisfies the predicate (Cursor / stdio fallback). */
function findPayload<T>(
  result: ToolResultLike,
  matches: (o: Record<string, unknown>) => boolean,
): T | null {
  const sc = result.structuredContent;
  if (sc && matches(sc)) return sc as unknown as T;
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as unknown;
      if (
        parsed &&
        typeof parsed === "object" &&
        matches(parsed as Record<string, unknown>)
      ) {
        return parsed as T;
      }
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** The GLB mode reported by get_preview_glb ("instances" when the scene
 *  carries one named node per assembly instance). */
function findPreviewMode(result: ToolResultLike): string | null {
  const hit = findPayload<{ mode?: string }>(
    result,
    (o) => typeof o.mode === "string",
  );
  return hit?.mode ?? null;
}

/**
 * Fetch a document's GLB via the app-only preview tool and render it in
 * place. Records the version token so the self-refresh poll knows the
 * current state and only re-fetches when it actually changes. Shared by the
 * tool-result handler (mount tools) and the poll loop (data-tool mutations).
 */
async function fetchAndRenderGlb(
  docId: string,
  changed: PartsChanged | null,
): Promise<void> {
  // In an instance-driven sim session, geometry refreshes must keep the
  // per-instance node layout or FK playback would lose its bind targets.
  const wantInstances = simEnvId != null && simUseInstances;
  const previewResult = (await app.callServerTool({
    name: "get_preview_glb",
    arguments: {
      document_id: docId,
      ...(wantInstances ? { instances: true } : {}),
    },
  })) as ToolResultLike;
  const glb = findInlineGlb(previewResult);
  if (!glb) {
    setStatus("no geometry to preview", "idle");
    return;
  }
  const ver = findPreviewVersion(previewResult);
  if (ver) lastPreviewVersion = ver;
  // FK stays enabled only when the server actually served instances mode.
  const isInstancesGlb =
    wantInstances && findPreviewMode(previewResult) === "instances";
  renderGlbForDoc(glb, docId, changed, isInstancesGlb);
}

/** Capture VCode IR text for the "Open in vcad.io" button. */
function captureVcode(result: ToolResultLike): void {
  for (const block of result.content ?? []) {
    if (block?.type === "text" && block.text?.startsWith("# vcad")) {
      vcodeDoc = block.text;
      openBtn.style.display = "inline-flex";
      return;
    }
  }
}

// ── Live re-render — geometry accretes in place ──────────────
// The viewport is idempotent on document_id: when a later tool result
// targets the document already on screen, we re-render WITHOUT snapping
// the camera, re-bind the selection to the same part, and flash the
// parts the edit touched — so a multi-turn session grows under the
// user's cursor instead of stacking stale snapshot cards.
let renderedDocId: string | null = null;

interface PartRef {
  part_id: string;
  name?: string;
}
interface PartsChanged {
  added?: PartRef[];
  removed?: PartRef[];
  modified?: PartRef[];
}

/** The mutation diff: structuredContent first (the only carrier ChatGPT's
 *  widget bridge exposes), then any JSON text block (Cursor fallback). */
function findChanged(result: ToolResultLike): PartsChanged | null {
  const sc = result.structuredContent?.changed;
  if (sc && typeof sc === "object") return sc as PartsChanged;
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as { changed?: PartsChanged };
      if (parsed.changed) return parsed.changed;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** Re-bind the selection to the same part after a re-render (sticky
 *  deixis). If the part is gone, selection stays cleared — clearModel
 *  already pushed a null context, so "this" never resolves to a ghost. */
function reselectById(partId: string): void {
  if (!currentModel) return;
  let found: SelectedPart | null = null;
  currentModel.traverse((o) => {
    if (found) return;
    const m = /^(\d+):(.*)$/.exec(o.name);
    if (m && m[1] === partId) {
      found = { partId: m[1], name: m[2] || `part ${m[1]}`, object: o };
    }
  });
  if (found) select(found);
}

/** Transient brand-pink flash on the parts a mutation just touched.
 *  Reuses the selection-highlight path; auto-clears after FLASH_MS. */
const FLASH_MS = 1200;
function flashChanged(changed: PartsChanged): void {
  if (!currentModel) return;
  const ids = new Set<string>();
  for (const e of changed.added ?? []) ids.add(e.part_id);
  for (const e of changed.modified ?? []) ids.add(e.part_id);
  if (ids.size === 0) return;

  const flashed: THREE.Object3D[] = [];
  currentModel.traverse((o) => {
    const m = /^(\d+):/.exec(o.name);
    if (m && ids.has(m[1]) && o !== selected?.object) {
      setHighlight(o, true);
      flashed.push(o);
    }
  });
  if (flashed.length === 0) return;

  const n = flashed.length;
  setTicker(`updated ${n} ${n === 1 ? "part" : "parts"}`, "ready");
  window.setTimeout(() => {
    // Don't un-highlight a part the user has since selected.
    for (const o of flashed) {
      if (o !== selected?.object) setHighlight(o, false);
    }
  }, FLASH_MS);
}

/** Render a freshly-fetched GLB for a document, holding the camera and
 *  re-binding the selection when it's the document already on screen.
 *  `simInstancesGlb` marks the load as the sim session's INSTANCES-mode GLB
 *  — only then are FK bind targets (re)built and pose application enabled;
 *  any other GLB (flat/inline/part-segmented) disables FK until the
 *  instances GLB reloads, so replay poses can't land on part-id nodes. */
function renderGlbForDoc(
  glb: string,
  docId: string | null,
  changed: PartsChanged | null,
  simInstancesGlb = false,
  versionToken: string | null = null,
): void {
  const preserveCamera = docId != null && docId === renderedDocId;
  const keepPartId = preserveCamera ? selected?.partId ?? null : null;
  loadGlb(glb, {
    preserveCamera,
    afterLoad: () => {
      // Advance the change token only once the GLB actually parsed and
      // mounted — a failed render must leave the token behind so the next
      // result (or poll tick) re-fetches instead of skipping past it.
      if (versionToken != null) lastPreviewVersion = versionToken;
      if (docId != null) renderedDocId = docId;
      if (keepPartId != null) reselectById(keepPartId);
      if (changed) flashChanged(changed);
      // Re-bind FK targets after a reload — node objects are new — but ONLY
      // for the instances-mode sim GLB; other GLBs of the same document use
      // "<partId>:<name>" node names that would collide with instance ids.
      if (simEnvId && simInstancesGlb) {
        buildSimInstanceIndex();
        simGlbActive = true;
      } else {
        simNodeIndex.clear();
        simGlbActive = false;
      }
    },
  });
}

// ── Verification receipt — the audit ledger (build_receipt) ──
// build_receipt's result carries a Receipt (board hash, DRC summary,
// per-part provenance). We render it as a docked, opaque ledger over the
// board, with a Re-run button that re-verifies the live board through
// verify_receipt → Holds / Stale / Violated. Verification is the moat;
// the geometry behind the ledger is supporting evidence.
interface ReceiptLike {
  board_hash?: string;
  design_rules_hash?: string;
  drc_backend?: string;
  drc?: {
    total?: number;
    by_rule?: Array<{ rule: string; count: number }>;
    violations?: string[];
  };
  parts?: Array<{ reference: string; footprint: string; value: string; mpn?: string }>;
  sourcing?: { lines?: unknown[] };
}

let lastReceipt: ReceiptLike | null = null;

/** Find a Receipt: structuredContent first (ChatGPT-visible), then a
 *  receipt-shaped JSON text block (Cursor fallback / agent-direct call). */
function findReceipt(result: ToolResultLike): ReceiptLike | null {
  const sc = result.structuredContent?.receipt;
  if (sc && typeof sc === "object") return sc as ReceiptLike;
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as ReceiptLike;
      if (parsed && parsed.board_hash && parsed.drc) return parsed;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

/** The re-verify verdict from verify_receipt. */
function findReceiptStatus(result: ToolResultLike): string | null {
  const sc = result.structuredContent?.verify_receipt as { status?: string } | undefined;
  if (sc?.status) return sc.status;
  for (const block of result.content ?? []) {
    if (block?.type !== "text" || !block.text) continue;
    try {
      const parsed = JSON.parse(block.text) as { status?: string };
      if (parsed.status) return parsed.status;
    } catch {
      // Not JSON — skip
    }
  }
  return null;
}

function setReceiptBadge(receipt: ReceiptLike, status?: string): void {
  let text: string;
  let cls: string;
  if (status) {
    text = status;
    cls = status === "Holds" ? "badge-hold" : status === "Stale" ? "badge-warn" : "badge-bad";
  } else {
    const total = receipt.drc?.total ?? 0;
    text = total === 0 ? "DRC clean" : `${total} ${total === 1 ? "violation" : "violations"}`;
    cls = total === 0 ? "badge-hold" : "badge-bad";
  }
  receiptBadgeEl.textContent = text;
  receiptBadgeEl.className = cls;
}

function rcptRow(
  k: string,
  v: string,
  opts?: { cls?: string; onClick?: () => void },
): HTMLElement {
  const r = document.createElement("div");
  r.className = `rcpt-row${opts?.cls ? " " + opts.cls : ""}`;
  const ke = document.createElement("span");
  ke.className = "k";
  ke.textContent = k;
  const ve = document.createElement("span");
  ve.className = "v";
  ve.textContent = v;
  r.append(ke, ve);
  if (opts?.onClick) r.addEventListener("click", opts.onClick);
  return r;
}

function rcptSection(title: string): HTMLElement {
  const s = document.createElement("div");
  s.className = "rcpt-sec";
  const h = document.createElement("h4");
  h.textContent = title;
  s.append(h);
  return s;
}

/** Recenter the orbit on a board-local XY point (kernel mm → Y-up world),
 *  so a violation row points the camera at the geometry it describes. */
function focusBoardXY(x: number, y: number): void {
  if (!hasModel) return;
  controls.target.set(x, 0, -y);
  controls.update();
  setTicker(`centered on ${x.toFixed(1)}, ${y.toFixed(1)} mm`, "ready");
}

function renderReceipt(receipt: ReceiptLike, status?: string): void {
  lastReceipt = receipt;
  setReceiptBadge(receipt, status);

  const backend = receipt.drc_backend ?? "drc";
  const bh = (receipt.board_hash ?? "").slice(0, 12) || "—";
  const rh = (receipt.design_rules_hash ?? "").slice(0, 12) || "—";
  receiptHashEl.textContent = `${backend} · board ${bh} · rules ${rh}`;

  receiptBodyEl.innerHTML = "";

  // DRC — counts, then the first violations (clickable to fly the camera).
  const drc = receipt.drc;
  const drcSec = rcptSection("DRC");
  drcSec.append(rcptRow("violations", String(drc?.total ?? 0)));
  for (const rc of drc?.by_rule ?? []) {
    drcSec.append(rcptRow(rc.rule, String(rc.count)));
  }
  for (const key of (drc?.violations ?? []).slice(0, 6)) {
    // Canonical key is `rule|message|x|y`.
    const segs = key.split("|");
    const rule = segs[0] ?? "violation";
    const x = Number(segs[segs.length - 2]);
    const y = Number(segs[segs.length - 1]);
    const hasPos = Number.isFinite(x) && Number.isFinite(y);
    drcSec.append(
      rcptRow(
        rule,
        hasPos ? `${x.toFixed(1)}, ${y.toFixed(1)}` : "—",
        hasPos ? { cls: "rcpt-viol", onClick: () => focusBoardXY(x, y) } : undefined,
      ),
    );
  }
  receiptBodyEl.append(drcSec);

  // Per-part provenance.
  if (receipt.parts?.length) {
    const pSec = rcptSection(`Parts (${receipt.parts.length})`);
    for (const p of receipt.parts.slice(0, 40)) {
      const v = [p.value, p.mpn].filter(Boolean).join(" · ");
      pSec.append(rcptRow(`${p.reference} · ${p.footprint}`, v || "—"));
    }
    receiptBodyEl.append(pSec);
  }

  // Sourcing — shown but muted: a price change must never read as an
  // electrical failure, so it never drives the verdict badge.
  const lines = receipt.sourcing?.lines;
  if (lines?.length) {
    const sSec = rcptSection("Sourcing");
    sSec.append(rcptRow("captured lines", String(lines.length), { cls: "rcpt-muted" }));
    receiptBodyEl.append(sSec);
  }

  receiptEl.classList.add("visible");
  receiptRerunBtn.style.display = "inline-flex";
}

/** Re-verify the shown receipt against the live board via verify_receipt. */
async function rerunReceipt(): Promise<void> {
  if (!lastReceipt) return;
  if (devMode) {
    console.log("[vcad-viewer:dev] verify_receipt:", lastDocumentId);
    setReceiptBadge(lastReceipt, "Holds");
    return;
  }
  if (!lastDocumentId) return;
  receiptRerunBtn.disabled = true;
  setTicker("re-verifying receipt…", "busy");
  try {
    const res = (await app.callServerTool({
      name: "verify_receipt",
      arguments: { document_id: lastDocumentId, receipt: lastReceipt },
    })) as ToolResultLike;
    const status = findReceiptStatus(res);
    if (status) {
      setReceiptBadge(lastReceipt, status);
      setTicker(`receipt ${status.toLowerCase()}`, status === "Holds" ? "ready" : "error");
    } else {
      setTicker("re-verify unavailable", "error");
    }
  } catch (e) {
    console.warn("[vcad-viewer] verify_receipt failed:", e);
    setTicker("re-verify failed", "error");
  } finally {
    receiptRerunBtn.disabled = false;
  }
}

receiptRerunBtn.addEventListener("click", () => void rerunReceipt());
receiptCloseBtn.addEventListener("click", () => receiptEl.classList.remove("visible"));

// ── Sim replay — the play button (create_robot_env) ─────────
// create_robot_env mounts this canvas; its result text carries
// { env_id, document_id }. We fetch the instance-segmented GLB (one named
// node per assembly instance), pull the recorded rollout via the app-only
// get_sim_replay tool, and play it back client-side: linear interpolation
// between per-step instance transforms (slerp for rotation), wall-clock
// step rate = dt × substeps / speed. The trajectory is data, so scrubbing
// is free; a live-follow badge pins the playhead to the newest step while
// the agent keeps stepping the env (get_sim_version is the change token).

interface SimTrsLike {
  translation?: [number, number, number];
  rotation?: [number, number, number]; // Euler XYZ, degrees
  scale?: [number, number, number];
}

interface SimReplayLike {
  env_id?: string;
  document_id?: string;
  dt?: number;
  substeps?: number;
  steps?: number;
  total_steps?: number;
  reset_epoch?: number;
  joint_trajectory?: number[][];
  rewards?: number[];
  dones?: boolean[];
  instance_transforms?: Array<Record<string, SimTrsLike | undefined>>;
  version?: string;
}

let simEnvId: string | null = null;
let simDocId: string | null = null;
let simUseInstances = false;
let simReplay: SimReplayLike | null = null;
let simPlayhead = 0; // fractional step position
let simPlaying = false;
let simSpeed = 1;
let simFollow = true; // pin the playhead to the newest step
let simKnownStepCount = -1; // step counter of the last ADOPTED replay
let simKnownVersion: string | null = null; // version token of the last ADOPTED replay
let simLastReadoutStep = -1;
let lastPushedSimContext = "";
const simNodeIndex = new Map<string, THREE.Object3D>();
// FK poses may ONLY be applied while the rendered GLB is the sim session's
// instances-mode GLB. A flat pattern or a plain/inline (part-segmented) GLB
// of the SAME document shares node-name shapes ("<id>:<name>") — applying
// instance transforms to those nodes would visibly mispose parts.
let simGlbActive = false;

// Scratch objects — reused every frame so playback never churns the GC.
const _simEuler = new THREE.Euler();
const _simQa = new THREE.Quaternion();
const _simQb = new THREE.Quaternion();
const DEG_TO_RAD = Math.PI / 180;

function simStepCount(): number {
  const r = simReplay;
  if (!r) return 0;
  return Math.max(
    r.joint_trajectory?.length ?? 0,
    r.rewards?.length ?? 0,
    r.instance_transforms?.length ?? 0,
  );
}

/** First ABSOLUTE step of the current replay window. The server keeps a
 *  ring buffer: once total_steps exceeds the window length, index k in the
 *  window is absolute step `base + k`. The scrub stays window-relative;
 *  only the readout and the model-context pushes speak absolute steps. */
function simStepBase(): number {
  const r = simReplay;
  if (!r) return 0;
  const total = r.total_steps ?? 0;
  return Math.max(total - simStepCount(), 0);
}

/** create_robot_env result → { env_id, document_id }. structuredContent
 *  first, then any JSON text block (the documented carrier). */
function findSimEnv(result: ToolResultLike): { envId: string; docId: string } | null {
  const hit = findPayload<{ env_id?: unknown; document_id?: unknown }>(
    result,
    (o) =>
      (typeof o.env_id === "string" || typeof o.env_id === "number") &&
      typeof o.document_id === "string",
  );
  if (!hit) return null;
  return { envId: String(hit.env_id), docId: String(hit.document_id) };
}

/** Index "<instanceId>:<name>" nodes of the loaded scene for FK binding.
 *  First match wins so a nested duplicate never shadows the root node. */
function buildSimInstanceIndex(): void {
  simNodeIndex.clear();
  if (!currentModel) return;
  currentModel.traverse((o) => {
    const idx = o.name.indexOf(":");
    if (idx <= 0) return;
    const id = o.name.slice(0, idx);
    if (!simNodeIndex.has(id)) simNodeIndex.set(id, o);
  });
}

function updateLiveBadge(): void {
  tpLiveEl.classList.toggle("on", simFollow);
  tpLiveEl.title = simFollow
    ? "following the newest step"
    : "click to jump to the newest step";
}

/** Brief pulse on the live badge when fresh steps arrive. */
function flashLiveBadge(): void {
  tpLiveEl.classList.remove("ping");
  // Force a reflow so re-adding restarts the animation.
  void tpLiveEl.offsetWidth;
  tpLiveEl.classList.add("ping");
  window.setTimeout(() => tpLiveEl.classList.remove("ping"), 700);
}

// Reward sparkline: polyline over rewards[], with a progress dot that
// tracks the playhead. Min/max are cached at rebuild for the dot math.
let sparkMin = 0;
let sparkMax = 0;

function rebuildSparkline(): void {
  const rewards = simReplay?.rewards ?? [];
  if (rewards.length < 2) {
    tpSparkLineEl.setAttribute("points", "");
    tpSparkDotEl.setAttribute("cx", "-10");
    return;
  }
  sparkMin = Infinity;
  sparkMax = -Infinity;
  for (const r of rewards) {
    if (r < sparkMin) sparkMin = r;
    if (r > sparkMax) sparkMax = r;
  }
  const span = sparkMax - sparkMin || 1;
  const pts: string[] = [];
  for (let i = 0; i < rewards.length; i++) {
    const x = (i / (rewards.length - 1)) * 100;
    const y = 14 - ((rewards[i] - sparkMin) / span) * 12;
    pts.push(`${x.toFixed(2)},${y.toFixed(2)}`);
  }
  tpSparkLineEl.setAttribute("points", pts.join(" "));
}

function updateSparkDot(step: number): void {
  const rewards = simReplay?.rewards ?? [];
  if (rewards.length === 0) {
    tpSparkDotEl.setAttribute("cx", "-10");
    return;
  }
  const i = Math.min(step, rewards.length - 1);
  const span = sparkMax - sparkMin || 1;
  const x = rewards.length > 1 ? (i / (rewards.length - 1)) * 100 : 0;
  const y = 14 - ((rewards[i] - sparkMin) / span) * 12;
  tpSparkDotEl.setAttribute("cx", x.toFixed(2));
  tpSparkDotEl.setAttribute("cy", y.toFixed(2));
}

/** Step counter, scrub position, joint readout, spark dot — everything
 *  keyed to the integer step under the playhead. */
function updateSimReadout(step: number, n: number): void {
  const last = Math.max(n - 1, 0);
  // Absolute step numbers in the readout: after ring-buffer wraparound the
  // window slides, so window index k is episode step base + k.
  const base = simStepBase();
  tpStepEl.textContent = `${base + step} / ${base + last}`;
  tpScrubEl.max = String(last);
  tpScrubEl.value = String(step);
  const rep = simReplay;
  const jt = rep?.joint_trajectory ?? [];
  const joints = jt.length > 0 ? jt[Math.min(step, jt.length - 1)] ?? [] : [];
  const bits: string[] = [];
  for (let i = 0; i < Math.min(joints.length, 6); i++) {
    bits.push(`j${i + 1} ${joints[i].toFixed(1)}°`);
  }
  if (joints.length > 6) bits.push(`+${joints.length - 6} joints`);
  const rewards = rep?.rewards ?? [];
  const reward = rewards.length > 0 ? rewards[Math.min(step, rewards.length - 1)] : undefined;
  if (reward !== undefined) bits.push(`reward ${reward.toFixed(2)}`);
  tpJointsEl.textContent = bits.join(" · ");
  if (rep?.dt != null) {
    tpStepEl.title = `dt ${(rep.dt * 1000).toFixed(0)} ms × ${rep.substeps ?? 1} substeps per step`;
  }
  updateSparkDot(step);
}

/** Apply the interpolated frame at the current playhead to the named
 *  instance nodes. Transforms are in kernel Z-up space — nodes live under
 *  modelGroup, which owns the display rotation, so they apply directly. */
function applySimFrame(forceReadout = false): void {
  const rep = simReplay;
  if (!rep) return;
  const n = simStepCount();
  if (n === 0) return;
  const p = Math.min(Math.max(simPlayhead, 0), n - 1);
  const k = Math.floor(p);
  const frac = p - k;

  // FK only while the rendered GLB is the instances-mode sim GLB — a flat
  // pattern or plain part-segmented GLB must never be re-posed (its nodes
  // are indexed by part id, not instance id).
  const tf = rep.instance_transforms ?? [];
  if (simGlbActive && tf.length > 0 && simNodeIndex.size > 0) {
    const a = tf[Math.min(k, tf.length - 1)];
    const b = tf[Math.min(k + 1, tf.length - 1)] ?? a;
    if (a) {
      for (const id of Object.keys(a)) {
        const node = simNodeIndex.get(id);
        const ta = a[id];
        if (!node || !ta) continue;
        const tb = b?.[id] ?? ta;
        const pa = ta.translation;
        const pb = tb.translation ?? pa;
        if (pa && pb) {
          node.position.set(
            pa[0] + (pb[0] - pa[0]) * frac,
            pa[1] + (pb[1] - pa[1]) * frac,
            pa[2] + (pb[2] - pa[2]) * frac,
          );
        }
        const sa = ta.scale;
        const sb = tb.scale ?? sa;
        if (sa && sb) {
          node.scale.set(
            sa[0] + (sb[0] - sa[0]) * frac,
            sa[1] + (sb[1] - sa[1]) * frac,
            sa[2] + (sb[2] - sa[2]) * frac,
          );
        }
        const ra = ta.rotation;
        const rb = tb.rotation ?? ra;
        if (ra && rb) {
          // Euler order "ZYX": the kernel's Transform3D convention is
          // R = Rz·Ry·Rx (rotate about world X, then Y, then Z — see
          // crates/vcad-eval/src/kinematics.rs euler_to_matrix and
          // packages/engine/src/evaluate.ts transformMesh, the authority),
          // which three.js spells "ZYX". Matches eulerXyzDegToQuat in
          // src/export/glb.ts, so replay poses agree with the GLB's own
          // node rotations.
          _simEuler.set(ra[0] * DEG_TO_RAD, ra[1] * DEG_TO_RAD, ra[2] * DEG_TO_RAD, "ZYX");
          _simQa.setFromEuler(_simEuler);
          _simEuler.set(rb[0] * DEG_TO_RAD, rb[1] * DEG_TO_RAD, rb[2] * DEG_TO_RAD, "ZYX");
          _simQb.setFromEuler(_simEuler);
          _simQa.slerp(_simQb, frac);
          node.quaternion.copy(_simQa);
        }
      }
    }
  }

  const step = Math.floor(p);
  if (forceReadout || step !== simLastReadoutStep) {
    simLastReadoutStep = step;
    updateSimReadout(step, n);
  }
}

function setSimPlaying(on: boolean): void {
  if (simPlaying === on) return;
  simPlaying = on;
  tpPlayBtn.textContent = on ? "⏸" : "▶";
  tpPlayBtn.title = on ? "Pause" : "Play";
}

/** Short context note on pause so "why did it stop there" typed in chat
 *  is grounded. Capability-guarded like the selection push; text-only. */
async function pushSimContext(): Promise<void> {
  if (!simEnvId || !simReplay) return;
  const n = simStepCount();
  if (n === 0) return;
  const t = Math.min(Math.floor(simPlayhead), n - 1);
  const rewards = simReplay.rewards ?? [];
  const r = rewards.length > 0 ? rewards[Math.min(t, rewards.length - 1)] : undefined;
  // Absolute step in the context note — after ring-buffer wraparound the
  // window index t is episode step base + t (the model correlates this with
  // its own action sequence, so window-relative numbers would mislead it).
  const text = `viewer_sim: paused at step t=${simStepBase() + t}, reward=${r != null ? r.toFixed(3) : "n/a"}`;
  if (text === lastPushedSimContext) return;
  lastPushedSimContext = text;
  if (devMode) {
    console.log("[vcad-viewer:dev] updateModelContext:", text);
    return;
  }
  const caps = app.getHostCapabilities();
  if (!caps?.updateModelContext) return;
  try {
    await app.updateModelContext({ content: [{ type: "text", text }] });
  } catch (e) {
    console.warn("[vcad-viewer] updateModelContext failed:", e);
  }
}

/** Install a replay (fresh fetch or dev-synthesized) and refresh the
 *  transport chrome. Following → snap the playhead to the newest step.
 *  No-op after exitSimMode: a replay landing from an in-flight fetch must
 *  never resurrect the transport bar over a different document. */
function adoptSimReplay(rep: SimReplayLike, jumpToNewest: boolean): void {
  if (!simEnvId) return; // sim mode exited while the fetch was in flight
  simReplay = rep;
  // Commit the change tokens HERE — on successful adoption — so a failed
  // replay fetch leaves them stale and the next poll retries (see
  // pollSimVersion).
  simKnownStepCount = rep.total_steps ?? rep.steps ?? simStepCount();
  simKnownVersion = typeof rep.version === "string" ? rep.version : null;
  rebuildSparkline();
  const n = simStepCount();
  const hasFk = Boolean(
    rep.instance_transforms?.some((row) => row && Object.keys(row).length > 0),
  );
  tpNoteEl.textContent = hasFk
    ? ""
    : "no articulated assembly — showing trajectory only";
  if (jumpToNewest && simFollow) simPlayhead = Math.max(n - 1, 0);
  else simPlayhead = Math.min(simPlayhead, Math.max(n - 1, 0));
  simLastReadoutStep = -1;
  applySimFrame(true);
  transportEl.classList.add("visible");
  updateLiveBadge();
}

/** Fetch + adopt the replay. Returns true only when a replay was ADOPTED —
 *  callers (pollSimVersion) treat false as "retry next poll". Guards both
 *  ends of the round trip against sim mode exiting / switching env while
 *  the call was in flight (the zombie-transport hazard). */
async function refreshSimReplay(jumpToNewest: boolean): Promise<boolean> {
  const envId = simEnvId;
  if (!envId) return false;
  const res = (await app.callServerTool({
    name: "get_sim_replay",
    arguments: { env_id: envId },
  })) as ToolResultLike;
  if (simEnvId !== envId) return false; // exited/switched during the fetch
  const rep = findPayload<SimReplayLike>(
    res,
    (o) => Array.isArray(o.joint_trajectory) || Array.isArray(o.instance_transforms),
  );
  if (!rep) return false;
  adoptSimReplay(rep, jumpToNewest);
  return true;
}

/** Fetch the instance-segmented GLB; fall back to the plain preview when
 *  the server returns nothing or doesn't speak instances mode. */
async function loadSimGlb(docId: string): Promise<void> {
  let res = (await app.callServerTool({
    name: "get_preview_glb",
    arguments: { document_id: docId, instances: true },
  })) as ToolResultLike;
  let glb = findInlineGlb(res);
  simUseInstances = Boolean(glb) && findPreviewMode(res) === "instances";
  if (!glb) {
    res = (await app.callServerTool({
      name: "get_preview_glb",
      arguments: { document_id: docId },
    })) as ToolResultLike;
    glb = findInlineGlb(res);
    simUseInstances = false;
  }
  if (!glb) {
    setStatus("no geometry to preview", "idle");
    return;
  }
  const ver = findPreviewVersion(res);
  if (ver) lastPreviewVersion = ver;
  // afterLoad rebuilds the FK node index (instances mode only).
  renderGlbForDoc(glb, docId, null, simUseInstances);
}

/** Enter (or re-enter) sim mode for a create_robot_env result. */
async function enterSimMode(envId: string, docId: string): Promise<void> {
  const isNewEnv = envId !== simEnvId;
  simEnvId = envId;
  simDocId = docId;
  lastDocumentId = docId;
  docLabelEl.textContent = docId;
  openBtn.style.display = "inline-flex";
  if (isNewEnv) {
    setSimPlaying(false);
    simReplay = null;
    simPlayhead = 0;
    simFollow = true;
    simKnownStepCount = -1;
    simKnownVersion = null;
    simLastReadoutStep = -1;
    lastPushedSimContext = "";
  }
  setStatus(docId === renderedDocId ? "updating…" : "loading simulation…");
  try {
    await loadSimGlb(docId);
  } catch (e) {
    console.error("[vcad-viewer] sim preview fetch failed:", e);
    setStatus("preview unavailable", "error");
    errEl.textContent = e instanceof Error ? e.message : String(e);
  }
  try {
    // If the replay isn't up yet, the sim-version poll below self-heals.
    await refreshSimReplay(true);
  } catch (e) {
    console.warn("[vcad-viewer] get_sim_replay failed:", e);
    setTicker("replay unavailable", "error");
  }
  updateLiveBadge();
}

/** A different document mounting over a sim session ends the replay. */
function exitSimMode(): void {
  simEnvId = null;
  simDocId = null;
  simReplay = null;
  simUseInstances = false;
  simGlbActive = false;
  simKnownVersion = null;
  setSimPlaying(false);
  simNodeIndex.clear();
  transportEl.classList.remove("visible");
}

/** Piggybacks on the adaptive preview poll: cheap step_count token, full
 *  replay re-fetch only on change. Returns true when new steps arrived so
 *  the shared loop stays on the fast cadence while the env is stepping. */
async function pollSimVersion(): Promise<boolean> {
  if (!simEnvId) return false;
  if (typeof document !== "undefined" && document.hidden) return false;
  let count: number | null = null;
  let version: string | null = null;
  try {
    const res = (await app.callServerTool({
      name: "get_sim_version",
      arguments: { env_id: simEnvId },
    })) as ToolResultLike;
    const v = findPayload<{ step_count?: number; version?: string }>(
      res,
      (o) => typeof o.step_count === "number",
    );
    count = v?.step_count ?? null;
    if (typeof v?.version === "string") version = v.version;
  } catch {
    return false; // transient failure — next tick retries
  }
  if (count == null) return false;
  // Prefer the version token when both sides have one — it also folds in the
  // server's reset epoch, so an equal-length rollout after gym_reset still
  // reads as changed. Fall back to the raw step counter for older servers.
  const unchanged =
    version != null && simKnownVersion != null
      ? version === simKnownVersion
      : count === simKnownStepCount;
  if (unchanged) return false;
  // The known tokens are committed by adoptSimReplay ONLY after the replay
  // re-fetch succeeds — a transient fetch failure leaves them stale so the
  // next poll retries instead of freezing playback at the old rollout.
  let refreshed = false;
  try {
    refreshed = await refreshSimReplay(true);
  } catch {
    return false;
  }
  if (!refreshed) return false;
  flashLiveBadge();
  return true;
}

// Playback engine: wall-clock rate = dt × substeps / speed seconds per
// step, linear interpolation between rows (slerp for rotation).
function simFrameTick(deltaSeconds: number): void {
  if (!simPlaying || !simReplay) return;
  const n = simStepCount();
  if (n <= 1) return;
  const stepSeconds =
    ((simReplay.dt ?? 0.01) * (simReplay.substeps ?? 1)) / simSpeed;
  if (stepSeconds <= 0) return;
  // Clamp delta spikes so a backgrounded tab doesn't teleport the playhead.
  simPlayhead += Math.min(deltaSeconds, 0.25) / stepSeconds;
  if (simPlayhead >= n - 1) {
    simPlayhead = n - 1;
    setSimPlaying(false);
    void pushSimContext();
  }
  applySimFrame();
}
simTick = simFrameTick;

tpPlayBtn.addEventListener("click", () => {
  if (!simReplay) return;
  if (simPlaying) {
    setSimPlaying(false);
    void pushSimContext();
    return;
  }
  const n = simStepCount();
  if (n > 1 && simPlayhead >= n - 1) {
    // Hitting play at the end re-arms live-follow and replays from the top.
    simFollow = true;
    updateLiveBadge();
    simPlayhead = 0;
  }
  setSimPlaying(true);
});

tpScrubEl.addEventListener("input", () => {
  if (!simReplay) return;
  setSimPlaying(false); // pause on scrub drag
  const n = simStepCount();
  const v = Math.min(Number(tpScrubEl.value) || 0, Math.max(n - 1, 0));
  simPlayhead = v;
  if (v < n - 1) {
    simFollow = false; // scrubbed back — stop chasing the newest step
    updateLiveBadge();
  }
  applySimFrame(true);
});
// Context push on drag end only, so scrubbing doesn't spam the host.
tpScrubEl.addEventListener("change", () => void pushSimContext());

tpSpeedEl.addEventListener("change", () => {
  simSpeed = Number(tpSpeedEl.value) || 1;
});

tpLiveEl.addEventListener("click", () => {
  if (!simReplay) return;
  simFollow = true;
  updateLiveBadge();
  simPlayhead = Math.max(simStepCount() - 1, 0);
  applySimFrame(true);
});

// ── Order dock — fused vcad+kerf order lifecycle ─────────────
// Read-only by contract: get_order_feed is the ONLY tool this dock calls;
// approval happens on vcad.io via openLink and decline goes through the
// agent — no money action ever originates in this iframe. Polls slow
// (10s) normally, fast (2.5s) while any order is transitional
// (approval/placing), and backs way off after repeated failures (the
// feed tool may not be deployed on every server yet).

interface OrderAuthorizationLike {
  status?: string;
  max_amount_usd?: number;
  cap_usd?: number; // spec-doc name — guarded alongside the wire name
  expires_at?: string;
  approve_url?: string;
}

interface OrderEventLike {
  state?: string;
  type?: string; // spec-doc name for the same field
  at?: string;
  note?: string;
}

interface OrderLike {
  order_id: string;
  state_chip?: string;
  raw_state?: string;
  process?: string;
  quantity?: number;
  total_amount_usd?: number;
  pricing_basis?: string;
  vendor?: string;
  vendor_display_name?: string; // spec-doc name — guarded
  lead_time_days?: number;
  quote_expires_at?: string;
  created_at?: string;
  events?: OrderEventLike[];
  authorization?: OrderAuthorizationLike | null;
  tracking?: unknown;
  receipt?: { status?: string } | null;
  kerf_intent_hash?: string;
}

interface OrderFeedLike {
  orders?: OrderLike[];
  wallet_balance_usd?: number | null;
  version?: string;
}

const ORDER_STOPS = [
  "quoted",
  "approval",
  "placing",
  "confirmed",
  "production",
  "delivered",
];

const ORDER_POLL_FAST_MS = 2500;
const ORDER_POLL_SLOW_MS = 10000;
const ORDER_POLL_BACKOFF_MS = 60000;
const ORDER_POLL_FAILURE_LIMIT = 4;

let orderPollStarted = false;
let orderPollHandle: ReturnType<typeof setTimeout> | undefined;
let orderPollFailures = 0;
let lastOrderFeed: OrderFeedLike | null = null;
let lastOrderFeedVersion: string | null = null;
let orderDockArmed = false; // first non-empty feed arms the dock for good
let ordersCollapsed = false;
const expandedOrders = new Set<string>();

function usd(n: number): string {
  return (
    "$" +
    n.toLocaleString("en-US", {
      minimumFractionDigits: 2,
      maximumFractionDigits: 2,
    })
  );
}

function fmtWhen(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function orderCard(o: OrderLike): HTMLElement {
  const card = document.createElement("div");
  card.className = "ord-card";

  // Name line: process · qty.
  const name = document.createElement("div");
  name.className = "ord-name";
  name.textContent = `${o.process ?? "order"}${o.quantity != null ? ` · ×${o.quantity}` : ""}`;
  card.append(name);

  // Vendor · lead · quote expiry.
  const subBits: string[] = [];
  const vendor = o.vendor ?? o.vendor_display_name;
  if (vendor) subBits.push(vendor);
  if (o.lead_time_days != null) subBits.push(`${o.lead_time_days}d lead`);
  if (o.quote_expires_at) subBits.push(`quote expires ${fmtWhen(o.quote_expires_at)}`);
  if (subBits.length > 0) {
    const sub = document.createElement("div");
    sub.className = "ord-sub";
    sub.textContent = subBits.join(" · ");
    card.append(sub);
  }

  // Total + pricing-basis pill (ACP-CM colors users learn to trust).
  const total = document.createElement("div");
  total.className = "ord-total";
  const amount = document.createElement("b");
  amount.textContent = o.total_amount_usd != null ? usd(o.total_amount_usd) : "—";
  total.append(amount);
  const basis = o.pricing_basis;
  if (basis === "estimate" || basis === "quoted" || basis === "binding") {
    const pill = document.createElement("span");
    pill.className = `pill pill-${basis}`;
    pill.textContent = basis;
    total.append(pill);
  }
  card.append(total);

  // Six-stop timeline; a failed order shows all stops muted + a red chip.
  const chips = document.createElement("div");
  chips.className = "ord-chips";
  const failed = o.state_chip === "failed";
  const cur = failed ? -1 : ORDER_STOPS.indexOf(o.state_chip ?? "");
  ORDER_STOPS.forEach((stop, i) => {
    const c = document.createElement("span");
    c.className =
      "ord-chip" +
      (cur >= 0 && i < cur ? " done" : "") +
      (i === cur ? " now" : "");
    c.textContent = stop;
    chips.append(c);
  });
  if (failed) {
    const c = document.createElement("span");
    c.className = "ord-chip fail";
    c.textContent = "failed";
    chips.append(c);
  }
  card.append(chips);

  // RECONCILING = an explained wait, never a retry affordance.
  if ((o.raw_state ?? "").toUpperCase() === "RECONCILING") {
    const wait = document.createElement("div");
    wait.className = "ord-wait";
    wait.textContent =
      "reconciling with the vendor — verifying order state, no action needed";
    card.append(wait);
  }

  // Approval banner: the human approves on vcad.io — the widget never
  // approves and offers no decline (that goes through the agent).
  const auth = o.authorization;
  if (auth?.status === "pending_human") {
    const banner = document.createElement("div");
    banner.className = "ord-banner";
    const text = document.createElement("div");
    text.className = "ord-banner-text";
    const cap = auth.max_amount_usd ?? auth.cap_usd;
    const bits = ["needs your approval"];
    if (cap != null) bits.push(`cap ${usd(cap)}`);
    if (auth.expires_at) bits.push(`expires ${fmtWhen(auth.expires_at)}`);
    text.textContent = bits.join(" · ");
    banner.append(text);
    if (auth.approve_url) {
      const url = auth.approve_url;
      const btn = document.createElement("button");
      btn.className = "btn btn-brand";
      btn.textContent = "Approve in vcad.io";
      btn.addEventListener("click", () => {
        app.openLink({ url }).catch(() => window.open(url, "_blank"));
      });
      banner.append(btn);
    }
    card.append(banner);
  }

  // Receipt chip — the design-half verification verdict.
  const rs = o.receipt?.status ?? "unverified";
  const rcptRowEl = document.createElement("div");
  rcptRowEl.className = "ord-rcpt";
  const rcptChip = document.createElement("span");
  rcptChip.className =
    "rcpt-chip " +
    (rs === "holds" ? "holds" : rs === "stale" || rs === "violated" ? "bad" : "unverified");
  rcptChip.textContent = `receipt ${rs}`;
  rcptRowEl.append(rcptChip);
  card.append(rcptRowEl);

  // Event log expander (collapsed by default) — raw vcad+kerf states.
  const events = o.events ?? [];
  if (events.length > 0) {
    const expanded = expandedOrders.has(o.order_id);
    const label = (open: boolean): string =>
      `${events.length} ${events.length === 1 ? "event" : "events"} ${open ? "▾" : "▸"}`;
    const toggle = document.createElement("button");
    toggle.className = "ord-evt-toggle";
    toggle.textContent = label(expanded);
    const list = document.createElement("div");
    list.className = "ord-events";
    list.style.display = expanded ? "block" : "none";
    for (const ev of events) {
      const row = document.createElement("div");
      row.className = "ord-evt";
      const t = document.createElement("span");
      t.className = "t";
      t.textContent = ev.at ? fmtWhen(ev.at) : "";
      const s = document.createElement("span");
      s.className = "s";
      s.textContent = ev.state ?? ev.type ?? "event";
      row.append(t, s);
      if (ev.note) {
        const noteEl = document.createElement("span");
        noteEl.className = "n";
        noteEl.textContent = ev.note;
        noteEl.title = ev.note;
        row.append(noteEl);
      }
      list.append(row);
    }
    toggle.addEventListener("click", () => {
      const open = list.style.display === "none";
      list.style.display = open ? "block" : "none";
      if (open) expandedOrders.add(o.order_id);
      else expandedOrders.delete(o.order_id);
      toggle.textContent = label(open);
    });
    card.append(toggle, list);
  }

  return card;
}

function renderOrderFeed(feed: OrderFeedLike): void {
  const orders = feed.orders ?? [];
  ordersCountEl.textContent = String(orders.length);
  ordersBodyEl.innerHTML = "";
  if (orders.length === 0) {
    const empty = document.createElement("div");
    empty.className = "ord-sub";
    empty.textContent = "no orders yet";
    ordersBodyEl.append(empty);
  } else {
    for (const o of orders) ordersBodyEl.append(orderCard(o));
  }
  if (feed.wallet_balance_usd != null) {
    ordersFootEl.textContent = `Wallet ${usd(feed.wallet_balance_usd)}`;
    ordersFootEl.style.display = "block";
  } else {
    ordersFootEl.textContent = "";
    ordersFootEl.style.display = "none";
  }
}

function applyOrderFeed(feed: OrderFeedLike): void {
  const version = typeof feed.version === "string" ? feed.version : null;
  const unchanged =
    version != null && version === lastOrderFeedVersion && lastOrderFeed != null;
  lastOrderFeed = feed;
  lastOrderFeedVersion = version;
  if ((feed.orders?.length ?? 0) > 0) orderDockArmed = true;
  if (!orderDockArmed) return; // stay hidden until the first non-empty feed
  if (!unchanged) renderOrderFeed(feed);
  ordersEl.classList.add("visible");
}

function orderFeedTransitional(): boolean {
  return Boolean(
    lastOrderFeed?.orders?.some(
      (o) => o.state_chip === "approval" || o.state_chip === "placing",
    ),
  );
}

async function pollOrderFeed(): Promise<void> {
  if (!lastDocumentId) return; // nothing mounted yet — stay lazy
  if (typeof document !== "undefined" && document.hidden) return;
  try {
    const res = (await app.callServerTool({
      name: "get_order_feed",
      arguments: { document_id: lastDocumentId },
    })) as ToolResultLike;
    orderPollFailures = 0;
    const feed = findPayload<OrderFeedLike>(res, (o) => Array.isArray(o.orders));
    if (feed) applyOrderFeed(feed);
  } catch {
    // Tool may not be deployed on this server — count and back off.
    orderPollFailures++;
  }
}

function scheduleNextOrderPoll(): void {
  const delay =
    orderPollFailures >= ORDER_POLL_FAILURE_LIMIT
      ? ORDER_POLL_BACKOFF_MS
      : orderFeedTransitional()
        ? ORDER_POLL_FAST_MS
        : ORDER_POLL_SLOW_MS;
  if (orderPollHandle) clearTimeout(orderPollHandle);
  orderPollHandle = setTimeout(() => void runOrderPoll(), delay);
}

// In-flight guard: visibilitychange fires runOrderPoll directly, and while a
// poll is awaiting the server its timer id is already consumed — without the
// guard each hide/show during an in-flight poll would fork a SECOND
// setTimeout chain that then polls forever in parallel.
let orderPollInFlight = false;

async function runOrderPoll(): Promise<void> {
  if (orderPollInFlight) return; // the live chain reschedules on completion
  orderPollInFlight = true;
  try {
    await pollOrderFeed();
  } finally {
    orderPollInFlight = false;
    scheduleNextOrderPoll();
  }
}

function startOrderPolling(): void {
  if (orderPollStarted) return;
  orderPollStarted = true;
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) return;
      if (orderPollHandle) clearTimeout(orderPollHandle);
      void runOrderPoll();
    });
  }
  scheduleNextOrderPoll();
}

ordersToggleBtn.addEventListener("click", () => {
  ordersCollapsed = !ordersCollapsed;
  ordersEl.classList.toggle("collapsed", ordersCollapsed);
  ordersToggleBtn.textContent = ordersCollapsed ? "▸" : "▾";
  ordersToggleBtn.title = ordersCollapsed ? "Expand" : "Collapse";
});

// ── Host protocol ────────────────────────────────────────────
// MCP Apps hosts (Claude, Cursor) speak the SEP-1865 postMessage
// protocol via the App class; ChatGPT injects `window.openai` instead —
// the shim adapts it to the same surface.
const app = isOpenAiHost()
  ? (createOpenAiShim() as unknown as App)
  : new App(
      { name: "vcad-viewer", version: "2.1.0" },
      // Declare pip so a host can DOCK the live canvas as a persistent side
      // panel that updates across the conversation, rather than scrolling
      // inline. (Capability only — the host/user chooses when to dock.)
      { availableDisplayModes: ["inline", "pip", "fullscreen"] },
    );

function applyHostContext(ctx: McpUiHostContext | undefined): void {
  if (!ctx) return;
  // Host only picks dark/light — the palette stays vcad's own.
  if (ctx.theme) {
    isDark = ctx.theme !== "light";
    applyTheme();
  }
  if (ctx.safeAreaInsets) {
    const { top, right, bottom, left } = ctx.safeAreaInsets;
    document.body.style.padding = `${top}px ${right}px ${bottom}px ${left}px`;
  }
  if (ctx.availableDisplayModes?.includes("fullscreen")) {
    fullscreenBtn.style.display = "inline-flex";
  }
  if (ctx.displayMode) currentDisplayMode = ctx.displayMode;
}

app.onhostcontextchanged = (params) => {
  applyHostContext(params.hostContext as McpUiHostContext | undefined);
  // Hosts may announce availableDisplayModes only after connect — retry the
  // one-shot dock when the context (finally) says pip is supported.
  void maybeAutoDock();
};

app.ontoolinput = () => {
  setStatus("building model…");
};

app.ontoolresult = (result) => {
  void handleToolResult(result as ToolResultLike);
};

async function handleToolResult(result: ToolResultLike): Promise<void> {
  captureVcode(result);

  if (result.isError) {
    setStatus("tool reported an error — nothing to preview", "error");
    return;
  }

  // Verification receipt → docked audit ledger. Render it, then fall
  // through so the board GLB loads behind it as supporting evidence. Any
  // other result dismisses a ledger left over from a prior call.
  const receipt = findReceipt(result);
  if (receipt) renderReceipt(receipt);
  else receiptEl.classList.remove("visible");

  // Flat pattern (sheet_metal_unfold): 2D drawing, no GLB fetch. Checked
  // before the document_id path — unfold results carry both.
  const flat = findFlatPattern(result);
  if (flat) {
    const docId = result.structuredContent?.document_id ?? findDocumentId(result);
    if (typeof docId === "string") {
      // Sim mode is keyed to the sim document — a different document's
      // flat pattern mounting over it ends the replay.
      if (simEnvId && docId !== simDocId) exitSimMode();
      lastDocumentId = docId;
      docLabelEl.textContent = docId;
      openBtn.style.display = "inline-flex";
    }
    // The 2D drawing replaces the 3D model — drop the FK bind targets so a
    // still-live replay (same document) can't re-pose flat geometry.
    simNodeIndex.clear();
    simGlbActive = false;
    renderFlatPattern(flat);
    return;
  }

  // Robot sim session (create_robot_env): env_id + document_id ride in the
  // result text. Mount the instance GLB + replay transport instead of the
  // plain preview path.
  const simEnv = findSimEnv(result);
  if (simEnv) {
    await enterSimMode(simEnv.envId, simEnv.docId);
    return;
  }

  // The parts this call changed (for the in-place flash).
  const changed = findChanged(result);

  // Legacy path: GLB inlined in the result
  const inline = findInlineGlb(result);
  if (inline) {
    const inlineDoc = result.structuredContent?.document_id ?? findDocumentId(result);
    // A different document's inline GLB mounting over a sim session ends the
    // replay; a same-document inline GLB keeps sim mode but disables FK
    // (renderGlbForDoc default) — inline GLBs are part-segmented, not the
    // instances-mode layout the replay binds to.
    if (typeof inlineDoc === "string" && simEnvId && inlineDoc !== simDocId) {
      exitSimMode();
    }
    renderGlbForDoc(inline, typeof inlineDoc === "string" ? inlineDoc : null, changed);
    return;
  }

  // Current path: fetch the GLB via the app-only preview tool. The id
  // comes from structuredContent, falling back to a document_id found in
  // any JSON text block — Cursor has known gaps forwarding
  // structuredContent to widgets.
  const docId = result.structuredContent?.document_id ?? findDocumentId(result);
  if (typeof docId !== "string") {
    setStatus("no geometry to preview", "idle");
    return;
  }
  // A different document mounting over a sim session ends the replay.
  if (simEnvId && docId !== simDocId) exitSimMode();
  const sameDoc = docId === renderedDocId;
  lastDocumentId = docId;
  docLabelEl.textContent = docId;
  // A session document exists, so the deep link can always lazily fetch
  // the doc on click even when no VCode rode along in the result.
  openBtn.style.display = "inline-flex";

  // Fast path 1: the server rode a ready-to-render GLB along in `_meta`
  // (mount tools) — paint it now, no round trip. Skipped in a live
  // instances-mode replay for this document: the inline GLB is
  // part-segmented and would drop the FK bind targets.
  const metaPreview = findMetaPreview(result);
  const wantSimInstances = simEnvId != null && docId === simDocId && simUseInstances;
  if (metaPreview && !wantSimInstances && metaPreview.mode !== "instances") {
    try {
      // The version token advances inside afterLoad — only after a
      // successful parse+mount — so a corrupt inline GLB can't strand
      // lastPreviewVersion on a version that never rendered (fast path 2
      // would then skip the fetch that recovers it).
      renderGlbForDoc(
        metaPreview.glb!,
        docId,
        changed,
        false,
        metaPreview.version ?? null,
      );
      return;
    } catch (e) {
      // Corrupt inline payload (e.g. bad base64) — fall through to the
      // fetch path, which rebuilds the GLB server-side.
      console.warn("[vcad-viewer] inline preview render failed:", e);
    }
  }

  // Fast path 2: the result's change token matches what is already on
  // screen — nothing to fetch, the render is current.
  const resultVersion = result.structuredContent?.document_version;
  if (
    sameDoc &&
    typeof resultVersion === "string" &&
    resultVersion === lastPreviewVersion
  ) {
    setTicker("ready", "ready");
    return;
  }

  // Same document already on screen → keep showing it under a subtle
  // ticker rather than the full loading overlay, so it accretes in place.
  if (sameDoc) setTicker("updating…", "busy");
  else setStatus("fetching geometry…");
  try {
    await fetchAndRenderGlb(docId, changed);
  } catch (e) {
    console.error("[vcad-viewer] preview fetch failed:", e);
    setStatus("preview unavailable", "error");
    errEl.textContent = e instanceof Error ? e.message : String(e);
  }
}

// ── "Open in vcad.io" deep link ──────────────────────────────
let vcodeDoc: string | null = null;
let lastDocumentId: string | null = null;
// Last geometry version this canvas has rendered — the self-refresh poll
// re-fetches only when the server reports a different token (see below).
let lastPreviewVersion: string | null = null;

openBtn.addEventListener("click", async () => {
  let doc = vcodeDoc;
  if (!doc && lastDocumentId) {
    try {
      const fetched = (await app.callServerTool({
        name: "get_document",
        arguments: { document_id: lastDocumentId },
      })) as ToolResultLike;
      const text = fetched.content?.find((c) => c.type === "text")?.text;
      if (text) doc = text;
    } catch (e) {
      console.warn("[vcad-viewer] get_document for open link failed:", e);
    }
  }
  if (!doc) return;
  const encoded = btoa(unescape(encodeURIComponent(doc)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  const url = "https://vcad.io/#/new?doc=" + encoded;
  app.openLink({ url }).catch(() => window.open(url, "_blank"));
});

// ── Fullscreen toggle ────────────────────────────────────────
let currentDisplayMode = "inline";

fullscreenBtn.addEventListener("click", async () => {
  const mode = currentDisplayMode === "fullscreen" ? "inline" : "fullscreen";
  try {
    const result = await app.requestDisplayMode({ mode });
    currentDisplayMode = result.mode;
    fullscreenBtn.textContent =
      currentDisplayMode === "fullscreen" ? "Exit fullscreen" : "Fullscreen";
  } catch (e) {
    console.warn("[vcad-viewer] display mode change failed:", e);
  }
});

// ── Auto-dock: pin the canvas as a persistent side panel ─────────────
// When the host supports pip, dock the freshly-mounted canvas once so it
// stays visible and updates live as the agent works — instead of scrolling
// away inline. Capability-guarded + best-effort + one-shot, so hosts without
// pip (and a user who later un-docks) just stay where they are.
let autoDocked = false;

async function maybeAutoDock(): Promise<void> {
  if (autoDocked) return;
  const modes = app.getHostContext()?.availableDisplayModes ?? [];
  if (!modes.includes("pip") || currentDisplayMode !== "inline") return;
  autoDocked = true;
  try {
    const result = await app.requestDisplayMode({ mode: "pip" });
    currentDisplayMode = result.mode;
  } catch (e) {
    console.warn("[vcad-viewer] auto-dock (pip) failed:", e);
  }
}

// ── Self-refresh: poll a cheap version token, re-fetch on change ─────
// Data tools (create/update/route/add_*/…) no longer carry a UI template,
// so the host doesn't push their results to this iframe. Instead the one
// mounted canvas polls get_preview_version — a geometry-free change token —
// and re-fetches the GLB only when the document actually changed. Net: one
// live surface across a long session instead of an iframe per mutation.
//
// Cadence is brisk while the document is changing and backs off when idle, so
// a quiet session isn't a steady drip of calls; it snaps back to brisk on any
// change or when the tab regains focus.
const POLL_FAST_MS = 2500;
const POLL_SLOW_MS = 10000;
const POLL_IDLE_THRESHOLD = 8; // ~20s with no change → back off
let pollIdleStreak = 0;
let pollHandle: ReturnType<typeof setTimeout> | undefined;

async function pollPreviewVersion(): Promise<void> {
  if (!lastDocumentId) return;
  if (typeof document !== "undefined" && document.hidden) return;
  let ver: string | null = null;
  try {
    const res = (await app.callServerTool({
      name: "get_preview_version",
      arguments: { document_id: lastDocumentId },
    })) as ToolResultLike;
    ver = findPreviewVersion(res);
  } catch {
    return; // transient poll/network failure — next tick retries
  }
  // Refresh on ANY change from what's on screen — including geometry first
  // appearing in a document that was empty when the canvas mounted (no
  // baseline guard, or that build would never show).
  if (!ver || ver === lastPreviewVersion) return;
  // Record first so a failed/empty render doesn't re-trigger every tick.
  lastPreviewVersion = ver;
  setTicker("updating…", "busy");
  try {
    await fetchAndRenderGlb(lastDocumentId, null);
  } catch {
    // Empty doc or transient fetch failure — version already advanced.
  }
}

function scheduleNextPoll(): void {
  const delay = pollIdleStreak >= POLL_IDLE_THRESHOLD ? POLL_SLOW_MS : POLL_FAST_MS;
  if (pollHandle) clearTimeout(pollHandle);
  pollHandle = setTimeout(() => void runPoll(), delay);
}

// Same in-flight guard as the order poll: a visibilitychange while a poll is
// awaiting the server must not fork a second parallel poll chain.
let previewPollInFlight = false;

async function runPoll(): Promise<void> {
  if (previewPollInFlight) return; // the live chain reschedules on completion
  previewPollInFlight = true;
  try {
    const before = lastPreviewVersion;
    await pollPreviewVersion();
    const geomChanged = lastPreviewVersion !== before;
    // Sim sessions piggyback the same adaptive cadence: fast while the env
    // is stepping (step_count advancing), slow once it goes quiet.
    const simChanged = await pollSimVersion();
    pollIdleStreak = geomChanged || simChanged ? 0 : pollIdleStreak + 1;
  } finally {
    previewPollInFlight = false;
    scheduleNextPoll();
  }
}

function startPreviewPolling(): void {
  // Returning to the tab → back to brisk and check immediately.
  if (typeof document !== "undefined") {
    document.addEventListener("visibilitychange", () => {
      if (document.hidden) return;
      pollIdleStreak = 0;
      if (pollHandle) clearTimeout(pollHandle);
      void runPoll();
    });
  }
  scheduleNextPoll();
}

// ── Dev harness ──────────────────────────────────────────────
// `#dev` renders sample geometry without an MCP host so the rig
// (chrome, grid, IBL, shadow, stats) can be eyeballed in a browser.
// `#dev-light` does the same in the light theme; `#dev-flat` renders a
// sample sheet-metal flat pattern (U-channel with two holes).
if (location.hash.startsWith("#dev")) {
  if (location.hash === "#dev-light") {
    isDark = false;
    applyTheme();
  }
  if (location.hash === "#dev-flat") {
    renderFlatPattern({
      silhouette_2d: [
        [[-60, -40], [60, -40], [60, 40], [-60, 40]],
        [[-45, -10], [-35, -10], [-35, 10], [-45, 10]],
        [[35, -10], [45, -10], [45, 10], [35, 10]],
      ],
      panel_outlines_2d: [
        [[-60, -40], [-20, -40], [-20, 40], [-60, 40]],
        [[-20, -40], [20, -40], [20, 40], [-20, 40]],
        [[20, -40], [60, -40], [60, 40], [20, 40]],
      ],
      creases: [
        { line: [[-20, -40], [-20, 40]], direction: "Up" },
        { line: [[20, -40], [20, 40]], direction: "Down" },
      ],
      bbox: [-60, -40, 60, 40],
    });
    docLabelEl.textContent = "dev-flat";
  } else if (location.hash === "#dev-glb") {
    // Load a GLB placed next to the page (dev.glb) through the real
    // loadGlb path — exercises animated GLBs (render_sequence output,
    // glTF animation tracks autoplay via glbMixer) without an MCP host.
    docLabelEl.textContent = "dev-glb";
    void fetch("./dev.glb")
      .then((r) => {
        if (!r.ok) throw new Error(`dev.glb: HTTP ${r.status}`);
        return r.arrayBuffer();
      })
      .then((buf) => {
        let bin = "";
        const bytes = new Uint8Array(buf);
        for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]!);
        loadGlb(btoa(bin));
      })
      .catch((e) => {
        console.error("dev-glb load failed:", e);
        setStatus("dev-glb load failed", "error");
      });
  } else if (location.hash === "#dev-sim") {
    // Articulated pendulum with instance-named nodes ("<instanceId>:<name>")
    // and a synthesized replay so the transport bar can be exercised
    // hostless: play/pause, scrub, speed, sparkline, joint readout.
    const sample = new THREE.Group();
    const steel = new THREE.MeshStandardMaterial({ color: 0x9da3ab, metalness: 0.9, roughness: 0.35 });
    const pink = new THREE.MeshStandardMaterial({ color: 0xf92672, metalness: 0.0, roughness: 0.55 });
    const base = new THREE.Mesh(new THREE.BoxGeometry(50, 50, 10), steel);
    base.name = "1:base";
    base.position.z = 5;
    const arm = new THREE.Mesh(new THREE.BoxGeometry(8, 8, 44), pink);
    arm.name = "2:arm";
    arm.position.z = 32;
    sample.add(base, arm);
    tameMaterials(sample);
    modelGroup.add(sample);
    currentModel = sample;
    hasModel = true;
    updateAxesVisibility();
    controls.target.set(0, 25, 0);
    camera.position.set(90, 80, 90);
    controls.update();
    updateStats(sample, new THREE.Box3().setFromObject(sample).getSize(new THREE.Vector3()));
    loadingEl.classList.add("hidden");
    setTicker("ready", "ready");
    docLabelEl.textContent = "dev-sim";

    simEnvId = "env_dev";
    simDocId = "doc_dev";
    simUseInstances = true;
    buildSimInstanceIndex();
    simGlbActive = true; // dev harness renders the instance-named scene directly
    const steps = 240;
    const joint: number[][] = [];
    const rewards: number[] = [];
    const dones: boolean[] = [];
    const transforms: Array<Record<string, SimTrsLike>> = [];
    for (let k = 0; k < steps; k++) {
      const a = 55 * Math.sin(k * 0.06) * Math.exp(-k / 400);
      joint.push([a]);
      rewards.push(-Math.abs(a) / 55 + 0.02 * Math.sin(k * 0.5));
      dones.push(false);
      transforms.push({
        "2": { translation: [0, 0, 32], rotation: [0, a, 0], scale: [1, 1, 1] },
      });
    }
    adoptSimReplay(
      {
        env_id: "env_dev",
        document_id: "doc_dev",
        dt: 0.01,
        substeps: 2,
        steps,
        total_steps: steps,
        joint_trajectory: joint,
        rewards,
        dones,
        instance_transforms: transforms,
        version: "dev",
      },
      false,
    );
    setSimPlaying(true);
  } else {
    const sample = new THREE.Group();
    const steel = new THREE.MeshStandardMaterial({ color: 0x9da3ab, metalness: 0.9, roughness: 0.35 });
    const plastic = new THREE.MeshStandardMaterial({ color: 0xf92672, metalness: 0.0, roughness: 0.55 });
    // Kernel space is Z-up: flange disc + boss along Z, pink cap on top.
    // Names follow the server's "<part_id>:<name>" GLB convention so
    // click-to-select can be exercised hostless.
    const flange = new THREE.Mesh(new THREE.CylinderGeometry(30, 30, 8, 64), steel);
    flange.name = "1:Flange";
    flange.rotation.x = Math.PI / 2;
    flange.position.z = 4;
    const boss = new THREE.Mesh(new THREE.CylinderGeometry(12, 12, 30, 48), steel);
    boss.name = "2:Boss";
    boss.rotation.x = Math.PI / 2;
    boss.position.z = 23;
    const cap = new THREE.Mesh(new THREE.CylinderGeometry(13, 13, 4, 48), plastic);
    cap.name = "3:Cap";
    cap.rotation.x = Math.PI / 2;
    cap.position.z = 40;
    sample.add(flange, boss, cap);
    tameMaterials(sample);
    modelGroup.add(sample);
    currentModel = sample;
    hasModel = true;
    updateAxesVisibility();

    const box = new THREE.Box3().setFromObject(modelGroup);
    const size = new THREE.Box3().setFromObject(sample).getSize(new THREE.Vector3());
    const worldSize = box.getSize(new THREE.Vector3());
    const worldCenter = box.getCenter(new THREE.Vector3());
    const footprint = Math.max(worldSize.x, worldSize.z) * 1.8;
    contactShadow.scale.set(footprint, footprint, 1);
    contactShadow.position.set(worldCenter.x, -0.01, worldCenter.z);
    contactShadow.visible = true;
    controls.target.set(0, 21, 0);
    camera.position.set(85, 75, 85);
    controls.update();
    docLabelEl.textContent = "dev-sample";
    updateStats(sample, size);
    loadingEl.classList.add("hidden");
    setTicker("ready", "ready");
  }
  openBtn.style.display = "inline-flex";
  fullscreenBtn.style.display = "inline-flex";
  lastDocumentId = "doc_dev";
  updateAffordances();
  if (location.hash === "#dev-receipt") {
    renderReceipt({
      board_hash: "a1b2c3d4e5f60718",
      design_rules_hash: "9f8e7d6c5b4a3210",
      drc_backend: "vcad-drc v0.9",
      drc: {
        total: 2,
        by_rule: [
          { rule: "Clearance", count: 1 },
          { rule: "CourtyardOverlap", count: 1 },
        ],
        violations: [
          "Clearance|trace within 0.15mm|12.4|8.1",
          "CourtyardOverlap|U1 over C3|20.0|15.5",
        ],
      },
      parts: [
        { reference: "U1", footprint: "QFN-32", value: "STM32G0", mpn: "STM32G031K8" },
        { reference: "R1", footprint: "0402", value: "10k" },
        { reference: "C3", footprint: "0402", value: "100n" },
      ],
      sourcing: { lines: [1, 2, 3] },
    });
  }
  if (location.hash === "#dev-orders") {
    const now = Date.now();
    const iso = (offsetMs: number): string => new Date(now + offsetMs).toISOString();
    applyOrderFeed({
      orders: [
        {
          order_id: "ord_dev1",
          state_chip: "approval",
          raw_state: "AWAITING_AUTHORIZATION",
          process: "sheet_metal",
          quantity: 5,
          total_amount_usd: 182.4,
          pricing_basis: "quoted",
          vendor: "SendCutSend",
          lead_time_days: 6,
          quote_expires_at: iso(2 * 86400e3),
          created_at: iso(-3600e3),
          events: [
            { state: "QUOTED", at: iso(-3600e3), note: "laser + bend, 5052-H32 2.0mm" },
            { state: "AWAITING_AUTHORIZATION", at: iso(-1800e3) },
          ],
          authorization: {
            status: "pending_human",
            max_amount_usd: 200,
            expires_at: iso(86400e3),
            approve_url: "https://vcad.io/authorize/auth_dev",
          },
          tracking: null,
          receipt: { status: "holds" },
        },
        {
          order_id: "ord_dev2",
          state_chip: "placing",
          raw_state: "RECONCILING",
          process: "cnc",
          quantity: 1,
          total_amount_usd: 512,
          pricing_basis: "binding",
          vendor: "Protolabs",
          lead_time_days: 9,
          created_at: iso(-7200e3),
          events: [
            { state: "PLACING", at: iso(-600e3) },
            { state: "RECONCILING", at: iso(-60e3), note: "vendor confirmation pending" },
          ],
          authorization: null,
          tracking: null,
          receipt: { status: "unverified" },
        },
        {
          order_id: "ord_dev3",
          state_chip: "failed",
          raw_state: "FAILED_VALIDATION",
          process: "3dp_sls",
          quantity: 12,
          total_amount_usd: 96.05,
          pricing_basis: "estimate",
          vendor: "JLC3DP",
          created_at: iso(-2 * 86400e3),
          events: [
            { state: "QUOTED", at: iso(-2 * 86400e3) },
            { state: "FAILED_VALIDATION", at: iso(-86400e3), note: "wall thickness below process minimum" },
          ],
          authorization: null,
          tracking: null,
          receipt: { status: "violated" },
        },
      ],
      wallet_balance_usd: 250,
      version: "dev",
    });
  }
  // Debug handle for poking the scene from the console. `loadGlb` lets a
  // harness swap in an arbitrary base64 GLB (e.g. a generated PCB preview).
  (window as unknown as Record<string, unknown>).__vcad = {
    scene, camera, controls, grid, gridUniforms, contactShadow, renderer,
    select, partInfoFor, modelGroup, loadGlb, renderGlbForDoc,
    adoptSimReplay, applySimFrame, applyOrderFeed, renderOrderFeed,
    get renderedDocId() { return renderedDocId; },
    get simState() {
      return {
        envId: simEnvId,
        playhead: simPlayhead,
        playing: simPlaying,
        follow: simFollow,
        steps: simStepCount(),
        instances: [...simNodeIndex.keys()],
      };
    },
  };
} else {
  // ── Connect (handlers are all registered above) ────────────
  setStatus("waiting for model…");
  await app.connect();
  applyHostContext(app.getHostContext());
  updateAffordances();
  console.log(
    "[vcad-viewer] connected to host; capabilities:",
    JSON.stringify(app.getHostCapabilities() ?? {}),
  );
  // Keep the one mounted canvas live as the agent mutates the document.
  startPreviewPolling();
  // Lazy order feed: ticks idle until a document mounts, then renders the
  // dock on the first non-empty feed.
  startOrderPolling();
  // Dock as a side panel when the host supports it, so it persists and
  // updates across the conversation rather than scrolling away.
  void maybeAutoDock();
}
