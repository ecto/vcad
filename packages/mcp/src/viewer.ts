/**
 * Embedded MCP Apps viewer HTML for inline 3D CAD preview.
 *
 * This module exports a self-contained HTML string that renders GLB models
 * using Three.js in a sandboxed iframe. Host communication uses the official
 * `@modelcontextprotocol/ext-apps` App class (SEP-1865), which owns the
 * `ui/initialize` → `ui/notifications/initialized` handshake — hosts MUST
 * NOT deliver tool results before that handshake completes, so a hand-rolled
 * protocol that gets it wrong renders nothing in spec-compliant hosts
 * (Cursor, Claude Desktop).
 */

/** CSP resource domains needed by the viewer (Three.js + ext-apps CDN). */
export const VIEWER_CSP = {
  resourceDomains: ["https://cdn.jsdelivr.net"],
};

/** The ui:// URI for the viewer resource. */
export const VIEWER_RESOURCE_URI = "ui://vcad/viewer";

/** MIME type for MCP App HTML resources. */
export const MCP_APP_MIME_TYPE = "text/html;profile=mcp-app";

/**
 * Generate the viewer HTML string.
 *
 * The viewer:
 * - Imports Three.js + GLTFLoader + OrbitControls from jsDelivr CDN
 * - Implements MCP Apps postMessage protocol (ui/initialize, tool-result)
 * - Parses base64-encoded GLB from tool result content
 * - Renders with orbit controls, lighting, and Z-up to Y-up conversion
 */
export function getViewerHtml(): string {
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>vcad 3D Viewer</title>
<script type="importmap">
{
  "imports": {
    "three": "https://cdn.jsdelivr.net/npm/three@0.170.0/build/three.module.js",
    "three/addons/": "https://cdn.jsdelivr.net/npm/three@0.170.0/examples/jsm/"
  }
}
</script>
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: 100%; height: 100%; min-height: 380px; overflow: hidden; background: #1a1a2e; }
  canvas { display: block; width: 100%; height: 100%; }
  #loading {
    position: absolute; inset: 0;
    display: flex; align-items: center; justify-content: center;
    color: #ccccdd; font-family: system-ui, sans-serif; font-size: 16px;
    flex-direction: column; gap: 8px;
  }
  #loading.hidden { display: none; }
  #error { color: #ff6666; font-size: 12px; white-space: pre-wrap; max-width: 80%; }
  #open-btn {
    position: absolute; bottom: 12px; right: 12px;
    background: rgba(255,255,255,0.12); color: #ddd;
    border: 1px solid rgba(255,255,255,0.2); border-radius: 6px;
    padding: 6px 14px; font: 13px system-ui, sans-serif;
    cursor: pointer; display: none; backdrop-filter: blur(4px);
    transition: background 0.15s;
  }
  #open-btn:hover { background: rgba(255,255,255,0.22); color: #fff; }
</style>
</head>
<body>
<div id="loading">
  <div>vcad 3D Viewer — loading Three.js...</div>
  <div id="error"></div>
</div>
<button id="open-btn">Open in vcad.io</button>
<script type="module">
const CDN = 'https://cdn.jsdelivr.net/npm/three@0.170.0';
const errEl = document.getElementById('error');

let THREE, OrbitControls, GLTFLoader, App;
try {
  THREE = await import(CDN + '/build/three.module.js');
  const oc = await import(CDN + '/examples/jsm/controls/OrbitControls.js');
  OrbitControls = oc.OrbitControls;
  const gl = await import(CDN + '/examples/jsm/loaders/GLTFLoader.js');
  GLTFLoader = gl.GLTFLoader;
  const ea = await import('https://cdn.jsdelivr.net/npm/@modelcontextprotocol/ext-apps@1.7.4/+esm');
  App = ea.App;
  document.querySelector('#loading div').textContent = 'vcad 3D Viewer — waiting for model...';
} catch (e) {
  errEl.textContent = 'Viewer dependencies failed to load: ' + e.message;
  throw e;
}

// ── Scene setup ──────────────────────────────────────────────
const renderer = new THREE.WebGLRenderer({ antialias: true });
renderer.setPixelRatio(window.devicePixelRatio);
renderer.setSize(window.innerWidth, window.innerHeight);
renderer.toneMapping = THREE.ACESFilmicToneMapping;
renderer.toneMappingExposure = 1.2;
document.body.appendChild(renderer.domElement);

const scene = new THREE.Scene();
scene.background = new THREE.Color(0x1a1a2e);

const camera = new THREE.PerspectiveCamera(50, window.innerWidth / window.innerHeight, 0.1, 10000);
camera.position.set(80, 80, 80);

const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;
controls.dampingFactor = 0.1;
controls.addEventListener('change', () => renderer.render(scene, camera));

// ── Lighting ─────────────────────────────────────────────────
const ambient = new THREE.AmbientLight(0xffffff, 0.4);
scene.add(ambient);

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
const grid = new THREE.GridHelper(200, 20, 0x333355, 0x222244);
scene.add(grid);

// ── Resize handler ───────────────────────────────────────────
window.addEventListener('resize', () => {
  camera.aspect = window.innerWidth / window.innerHeight;
  camera.updateProjectionMatrix();
  renderer.setSize(window.innerWidth, window.innerHeight);
  renderer.render(scene, camera);
});

// ── Animation loop ───────────────────────────────────────────
function animate() {
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
let currentModel = null;

function loadGlb(base64Data) {
  const binary = atob(base64Data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }

  // Remove old model
  if (currentModel) {
    modelGroup.remove(currentModel);
    currentModel.traverse(child => {
      if (child.geometry) child.geometry.dispose();
      if (child.material) {
        if (Array.isArray(child.material)) child.material.forEach(m => m.dispose());
        else child.material.dispose();
      }
    });
  }

  loader.parse(bytes.buffer, '', (gltf) => {
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
      -center.y + dist * 0.7
    );
    camera.updateProjectionMatrix();
    controls.update();

    document.getElementById('loading').classList.add('hidden');
    renderer.render(scene, camera);
  }, (error) => {
    console.error('GLB parse error:', error);
    document.getElementById('loading').textContent = 'Error loading model';
  });
}

// ── MCP Apps protocol (official ext-apps App class) ──────────
// The App owns the SEP-1865 handshake: it sends ui/initialize with
// appCapabilities, emits ui/notifications/initialized, and dispatches
// ui/notifications/tool-result to the handler below. It also auto-reports
// size changes to the host via a ResizeObserver.
const app = new App({ name: 'vcad-viewer', version: '1.0.0' });

// Set before connect() so the initial tool result is not missed.
app.ontoolresult = (result) => {
  console.log('[vcad-viewer] tool result received');
  extractGlb(result);
};

// ── "Open in vcad.io" deep link ──────────────────────────────
let vcodeDoc = null;
const openBtn = document.getElementById('open-btn');

openBtn.addEventListener('click', () => {
  if (!vcodeDoc) return;
  const encoded = btoa(unescape(encodeURIComponent(vcodeDoc)))
    .replace(/\\+/g, '-').replace(/\\//g, '_').replace(/=+$/, '');
  const url = 'https://vcad.io/#/new?doc=' + encoded;
  app.openLink({ url }).catch(() => window.open(url, '_blank'));
});

function extractGlb(obj) {
  if (!obj) return;

  // Direct _vcad_glb on the object
  if (obj._vcad_glb) {
    loadGlb(obj._vcad_glb);
    return;
  }

  // Check content array for GLB blocks and VCode text
  const content = obj.content || obj;
  const blocks = Array.isArray(content) ? content : [];
  for (const block of blocks) {
    if (block?.type === 'text' && block.text) {
      // Capture VCode IR for the "Open in vcad.io" button
      if (block.text.startsWith('# vcad')) {
        vcodeDoc = block.text;
        openBtn.style.display = 'block';
      }
      try {
        const parsed = JSON.parse(block.text);
        if (parsed._vcad_glb) {
          loadGlb(parsed._vcad_glb);
          return;
        }
      } catch {
        if (block.text.startsWith('VCAD_GLB:')) {
          loadGlb(block.text.slice(9));
          return;
        }
      }
    }
  }
}

await app.connect();
console.log('[vcad-viewer] connected to host');
</script>
</body>
</html>`;
}
