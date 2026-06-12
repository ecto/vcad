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
function animate(): void {
  requestAnimationFrame(animate);
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
  if (!currentModel) return;
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

function loadGlb(base64Data: string): void {
  const binary = atob(base64Data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }

  clearModel();

  loader.parse(
    bytes.buffer,
    "",
    (gltf) => {
      currentModel = gltf.scene;
      modelGroup.add(currentModel);
      hasModel = true;
      updateAxesVisibility();

      // Fit camera to model
      const box = new THREE.Box3().setFromObject(currentModel);
      const center = box.getCenter(new THREE.Vector3());
      const size = box.getSize(new THREE.Vector3());
      const maxDim = Math.max(size.x, size.y, size.z);
      const dist = maxDim * 2;

      // Center is in Z-up space, convert to Y-up for camera target
      controls.target.set(center.x, center.z, -center.y);
      camera.position.set(
        center.x + dist * 0.7,
        center.z + dist * 0.7,
        -center.y + dist * 0.7,
      );
      camera.updateProjectionMatrix();
      controls.update();

      // Contact shadow under the footprint (world Y-up space)
      const worldBox = new THREE.Box3().setFromObject(modelGroup);
      const worldSize = worldBox.getSize(new THREE.Vector3());
      const worldCenter = worldBox.getCenter(new THREE.Vector3());
      const footprint = Math.max(worldSize.x, worldSize.z) * 1.8;
      contactShadow.scale.set(footprint, footprint, 1);
      contactShadow.position.set(worldCenter.x, -0.01, worldCenter.z);
      contactShadow.visible = true;

      // Size is measured pre-rotation in kernel Z-up mm
      updateStats(currentModel, size);
      loadingEl.classList.add("hidden");
      setTicker("ready", "ready");
      renderer.render(scene, camera);
    },
    (error) => {
      console.error("GLB parse error:", error);
      setStatus("error loading model", "error");
    },
  );
}

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
};

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

// ── MCP Apps protocol ────────────────────────────────────────
const app = new App(
  { name: "vcad-viewer", version: "2.1.0" },
  { availableDisplayModes: ["inline", "fullscreen"] },
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

app.onhostcontextchanged = (params) => applyHostContext(params.hostContext);

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

  // Flat pattern (sheet_metal_unfold): 2D drawing, no GLB fetch. Checked
  // before the document_id path — unfold results carry both.
  const flat = findFlatPattern(result);
  if (flat) {
    const docId = result.structuredContent?.document_id ?? findDocumentId(result);
    if (typeof docId === "string") {
      lastDocumentId = docId;
      docLabelEl.textContent = docId;
      openBtn.style.display = "inline-flex";
    }
    renderFlatPattern(flat);
    return;
  }

  // Legacy path: GLB inlined in the result
  const inline = findInlineGlb(result);
  if (inline) {
    loadGlb(inline);
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
  lastDocumentId = docId;
  docLabelEl.textContent = docId;
  // A session document exists, so the deep link can always lazily fetch
  // the doc on click even when no VCode rode along in the result.
  openBtn.style.display = "inline-flex";
  setStatus("fetching geometry…");
  try {
    const previewResult = (await app.callServerTool({
      name: "get_preview_glb",
      arguments: { document_id: docId },
    })) as ToolResultLike;
    const glb = findInlineGlb(previewResult);
    if (glb) {
      loadGlb(glb);
    } else {
      setStatus("no geometry to preview", "idle");
    }
  } catch (e) {
    console.error("[vcad-viewer] preview fetch failed:", e);
    setStatus("preview unavailable", "error");
    errEl.textContent = e instanceof Error ? e.message : String(e);
  }
}

// ── "Open in vcad.io" deep link ──────────────────────────────
let vcodeDoc: string | null = null;
let lastDocumentId: string | null = null;

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
  } else {
    const sample = new THREE.Group();
    const steel = new THREE.MeshStandardMaterial({ color: 0x9da3ab, metalness: 0.9, roughness: 0.35 });
    const plastic = new THREE.MeshStandardMaterial({ color: 0xf92672, metalness: 0.0, roughness: 0.55 });
    // Kernel space is Z-up: flange disc + boss along Z, pink cap on top.
    const flange = new THREE.Mesh(new THREE.CylinderGeometry(30, 30, 8, 64), steel);
    flange.rotation.x = Math.PI / 2;
    flange.position.z = 4;
    const boss = new THREE.Mesh(new THREE.CylinderGeometry(12, 12, 30, 48), steel);
    boss.rotation.x = Math.PI / 2;
    boss.position.z = 23;
    const cap = new THREE.Mesh(new THREE.CylinderGeometry(13, 13, 4, 48), plastic);
    cap.rotation.x = Math.PI / 2;
    cap.position.z = 40;
    sample.add(flange, boss, cap);
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
  // Debug handle for poking the scene from the console.
  (window as unknown as Record<string, unknown>).__vcad = {
    scene, camera, controls, grid, gridUniforms, contactShadow, renderer,
  };
} else {
  // ── Connect (handlers are all registered above) ────────────
  setStatus("waiting for model…");
  await app.connect();
  applyHostContext(app.getHostContext());
  console.log("[vcad-viewer] connected to host");
}
