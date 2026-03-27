/**
 * Embedded MCP Apps viewer HTML for inline 3D CAD preview.
 *
 * This module exports a self-contained HTML string that renders GLB models
 * using Three.js in a sandboxed iframe. It communicates with the MCP Apps
 * host via the postMessage JSON-RPC protocol to receive tool results.
 */

/** CSP resource domains needed by the viewer (Three.js CDN). */
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
<style>
  * { margin: 0; padding: 0; box-sizing: border-box; }
  html, body { width: 100%; height: 100%; overflow: hidden; background: #1a1a2e; }
  canvas { display: block; width: 100%; height: 100%; }
  #loading {
    position: absolute; inset: 0;
    display: flex; align-items: center; justify-content: center;
    color: #8888aa; font-family: system-ui, sans-serif; font-size: 14px;
  }
  #loading.hidden { display: none; }
</style>
</head>
<body>
<div id="loading">Loading model...</div>
<script type="importmap">
{
  "imports": {
    "three": "https://cdn.jsdelivr.net/npm/three@0.170.0/build/three.module.js",
    "three/addons/": "https://cdn.jsdelivr.net/npm/three@0.170.0/examples/jsm/"
  }
}
</script>
<script type="module">
import * as THREE from 'three';
import { OrbitControls } from 'three/addons/controls/OrbitControls.js';
import { GLTFLoader } from 'three/addons/loaders/GLTFLoader.js';

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

// ── MCP Apps protocol ────────────────────────────────────────
let messageId = 1;

function sendToHost(message) {
  window.parent.postMessage(message, '*');
}

// Send ui/initialize handshake
sendToHost({
  jsonrpc: '2.0',
  id: messageId++,
  method: 'ui/initialize',
  params: {
    capabilities: {},
    clientInfo: { name: 'vcad-viewer', version: '1.0.0' },
    protocolVersion: '2026-01-26',
  },
});

// Listen for host messages
window.addEventListener('message', (event) => {
  const data = event.data;
  if (!data || !data.jsonrpc) return;

  // Handle tool result notification
  if (data.method === 'ui/notifications/tool-result') {
    handleToolResult(data.params);
  }

  // Handle tool input notification (we can also extract from input)
  if (data.method === 'ui/notifications/tool-input') {
    // Input notifications arrive before results; nothing to render yet
  }
});

function handleToolResult(params) {
  // The result content is an array of content blocks
  const content = params?.result;
  if (!content) return;

  // Content may be a string or an array of content blocks
  const blocks = Array.isArray(content) ? content : [];

  // Look for our GLB preview marker in text content blocks
  for (const block of blocks) {
    if (block.type === 'text' && block.text) {
      try {
        // Try to parse as JSON to find _vcad_glb marker
        const parsed = JSON.parse(block.text);
        if (parsed._vcad_glb) {
          loadGlb(parsed._vcad_glb);
          return;
        }
      } catch {
        // Not JSON or no marker, check for raw base64 prefix
        if (block.text.startsWith('VCAD_GLB:')) {
          loadGlb(block.text.slice(9));
          return;
        }
      }
    }
  }
}
</script>
</body>
</html>`;
}
