/**
 * vcad MCP Apps viewer (SEP-1865 View).
 *
 * Bundled by Vite into a single self-contained HTML resource — no CDN
 * imports at render time. Receives tool results from the host via the
 * official App class, then fetches the GLB preview through the app-only
 * `get_preview_glb` tool so geometry never rides in model-visible results.
 */

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import {
  App,
  applyDocumentTheme,
  applyHostStyleVariables,
  applyHostFonts,
  type McpUiHostContext,
} from "@modelcontextprotocol/ext-apps";

const statusEl = document.getElementById("status")!;
const errEl = document.getElementById("error")!;
const loadingEl = document.getElementById("loading")!;
const openBtn = document.getElementById("open-btn") as HTMLButtonElement;
const fullscreenBtn = document.getElementById("fullscreen-btn") as HTMLButtonElement;

function setStatus(text: string): void {
  loadingEl.classList.remove("hidden");
  statusEl.textContent = text;
}

// ── Scene setup ──────────────────────────────────────────────
// Alpha canvas: the page background comes from host CSS variables, so the
// viewport follows the host theme without re-creating the scene.
const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.2;
document.body.appendChild(renderer.domElement);

const scene = new THREE.Scene();

const camera = new THREE.PerspectiveCamera(
  50,
  window.innerWidth / window.innerHeight,
  0.1,
  10000,
);
camera.position.set(80, 80, 80);

const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;
controls.dampingFactor = 0.1;
controls.addEventListener("change", () => renderer.render(scene, camera));

// ── Lighting ─────────────────────────────────────────────────
scene.add(new THREE.AmbientLight(0xffffff, 0.4));

const key = new THREE.DirectionalLight(0xffffff, 1.2);
key.position.set(50, 80, 40);
scene.add(key);

const fill = new THREE.DirectionalLight(0xffffff, 0.4);
fill.position.set(-30, 40, -20);
scene.add(fill);

const rim = new THREE.DirectionalLight(0xffffff, 0.2);
rim.position.set(-50, -20, 50);
scene.add(rim);

// ── Grid ─────────────────────────────────────────────────────
scene.add(new THREE.GridHelper(200, 20, 0x555577, 0x444466));

// ── Resize handler ───────────────────────────────────────────
window.addEventListener("resize", () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
  renderer.render(scene, camera);
});

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

function loadGlb(base64Data: string): void {
  const binary = atob(base64Data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }

  if (currentModel) {
    modelGroup.remove(currentModel);
    currentModel.traverse((child) => {
      const mesh = child as THREE.Mesh;
      if (mesh.geometry) mesh.geometry.dispose();
      if (mesh.material) {
        if (Array.isArray(mesh.material)) mesh.material.forEach((m) => m.dispose());
        else mesh.material.dispose();
      }
    });
  }

  loader.parse(
    bytes.buffer,
    "",
    (gltf) => {
      currentModel = gltf.scene;
      modelGroup.add(currentModel);

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

      loadingEl.classList.add("hidden");
      renderer.render(scene, camera);
    },
    (error) => {
      console.error("GLB parse error:", error);
      setStatus("Error loading model");
    },
  );
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

/** Capture VCode IR text for the "Open in vcad.io" button. */
function captureVcode(result: ToolResultLike): void {
  for (const block of result.content ?? []) {
    if (block?.type === "text" && block.text?.startsWith("# vcad")) {
      vcodeDoc = block.text;
      openBtn.style.display = "block";
      return;
    }
  }
}

// ── MCP Apps protocol ────────────────────────────────────────
const app = new App(
  { name: "vcad-viewer", version: "2.0.0" },
  { availableDisplayModes: ["inline", "fullscreen"] },
);

function applyHostContext(ctx: McpUiHostContext | undefined): void {
  if (!ctx) return;
  if (ctx.theme) applyDocumentTheme(ctx.theme);
  if (ctx.styles?.variables) applyHostStyleVariables(ctx.styles.variables);
  if (ctx.styles?.css?.fonts) applyHostFonts(ctx.styles.css.fonts);
  if (ctx.safeAreaInsets) {
    const { top, right, bottom, left } = ctx.safeAreaInsets;
    document.body.style.padding = `${top}px ${right}px ${bottom}px ${left}px`;
  }
  if (ctx.availableDisplayModes?.includes("fullscreen")) {
    fullscreenBtn.style.display = "block";
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
    setStatus("tool reported an error — nothing to preview");
    return;
  }

  // Legacy path: GLB inlined in the result
  const inline = findInlineGlb(result);
  if (inline) {
    loadGlb(inline);
    return;
  }

  // Current path: fetch the GLB via the app-only preview tool
  const docId = result.structuredContent?.document_id;
  if (typeof docId !== "string") {
    setStatus("no geometry to preview");
    return;
  }
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
      setStatus("no geometry to preview");
    }
  } catch (e) {
    console.error("[vcad-viewer] preview fetch failed:", e);
    setStatus("preview unavailable");
    errEl.textContent = e instanceof Error ? e.message : String(e);
  }
}

// ── "Open in vcad.io" deep link ──────────────────────────────
let vcodeDoc: string | null = null;

openBtn.addEventListener("click", () => {
  if (!vcodeDoc) return;
  const encoded = btoa(unescape(encodeURIComponent(vcodeDoc)))
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

// ── Connect (handlers are all registered above) ──────────────
setStatus("vcad 3D Viewer — waiting for model…");
await app.connect();
applyHostContext(app.getHostContext());
console.log("[vcad-viewer] connected to host");
