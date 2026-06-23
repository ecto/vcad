/**
 * vcad live review window — the hostless browser viewer.
 *
 * Opened from a shared link (mcp.vcad.io/live/<id>). Subscribes to the session
 * spine over Supabase Realtime (topic session:<id>), folds the event stream:
 * kernel mutations → refetch + re-render the GLB, overlay annotations → pins,
 * control events → a banner. Read-only for geometry; viewers can drop pins,
 * which POST back to /annotate and ride the same broadcast. Presence shows who
 * else is watching.
 *
 * NOTE: this is a self-contained scene rig (not yet the shared viewer-app one)
 * — extracting a common scene.ts is a deliberate follow-up.
 */

import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { GLTFLoader } from "three/examples/jsm/loaders/GLTFLoader.js";
import { createClient } from "@supabase/supabase-js";

// ── Session + API base (parsed from the path; no MCP host injects it) ────────
const segs = location.pathname.split("/").filter(Boolean); // ["live", "<id>"]
const sessionId = decodeURIComponent(segs[1] ?? "");
const api = `/live/${encodeURIComponent(sessionId)}`;

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const sidEl = $("sid");
const rosterEl = $("roster");
const connEl = $("conn");
const tickerEl = $("ticker");
const bannerEl = $("banner");
const hintEl = $("hint");
const pinInput = $("pinInput") as HTMLDivElement;
const pinText = $("pinText") as HTMLInputElement;
const pinSave = $("pinSave") as HTMLButtonElement;
sidEl.textContent = sessionId ? sessionId.slice(0, 24) : "(no session)";

// ── Viewer identity (anonymous; the server namespaces it as viewer:<name>) ───
const viewerId = Math.random().toString(36).slice(2, 10);
const viewerName = `guest-${viewerId.slice(0, 4)}`;
const colorFor = (s: string) => `hsl(${[...s].reduce((a, c) => a + c.charCodeAt(0), 0) % 360} 70% 60%)`;

// ── Three.js scene ───────────────────────────────────────────────────────────
const stage = $("stage");
const renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true });
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
stage.appendChild(renderer.domElement);

const scene = new THREE.Scene();
const camera = new THREE.PerspectiveCamera(50, 1, 0.1, 100000);
camera.position.set(120, 90, 120);
const controls = new OrbitControls(camera, renderer.domElement);
controls.enableDamping = true;

scene.add(new THREE.HemisphereLight(0xffffff, 0x223044, 0.9));
const key = new THREE.DirectionalLight(0xfffaf0, 1.3);
key.position.set(1, 1.5, 1);
scene.add(key);
const fill = new THREE.DirectionalLight(0xeaf2ff, 0.5);
fill.position.set(-1, 0.5, -0.5);
scene.add(fill);

const grid = new THREE.GridHelper(400, 40, 0x2a313c, 0x1b2027);
scene.add(grid);

// Z-up (kernel) → Y-up (three): geometry AND pins live under this group so a
// pin stored in kernel coords lines up with the model.
const modelGroup = new THREE.Group();
modelGroup.rotation.x = -Math.PI / 2;
scene.add(modelGroup);
const meshRoot = new THREE.Group(); // geometry — cleared on each GLB reload
const pinRoot = new THREE.Group(); // overlays — persist across reloads
modelGroup.add(meshRoot, pinRoot);

const loader = new GLTFLoader();

function resize() {
  const w = stage.clientWidth, h = stage.clientHeight;
  renderer.setSize(w, h, false);
  camera.aspect = w / Math.max(1, h);
  camera.updateProjectionMatrix();
}
window.addEventListener("resize", resize);
resize();

function fitCamera() {
  const box = new THREE.Box3().setFromObject(meshRoot);
  if (box.isEmpty()) return;
  const size = box.getSize(new THREE.Vector3());
  const center = box.getCenter(new THREE.Vector3());
  const radius = Math.max(size.x, size.y, size.z) * 0.5 || 50;
  const dist = radius / Math.sin((camera.fov * Math.PI) / 360) + radius;
  controls.target.copy(center);
  camera.position.copy(center).add(new THREE.Vector3(1, 0.8, 1).normalize().multiplyScalar(dist));
  camera.near = Math.max(0.1, dist / 1000);
  camera.far = dist * 1000;
  camera.updateProjectionMatrix();
}

let glbToken = 0;
async function reloadGlb() {
  const my = ++glbToken;
  try {
    const r = await fetch(`${api}/glb`, { cache: "no-store" });
    if (!r.ok || my !== glbToken) return;
    const buf = await r.arrayBuffer();
    if (my !== glbToken) return;
    loader.parse(
      buf,
      "",
      (gltf) => {
        if (my !== glbToken) return;
        meshRoot.clear();
        meshRoot.add(gltf.scene);
        fitCamera();
      },
      () => {/* malformed GLB — keep the previous model */},
    );
  } catch {/* network — keep previous */}
}

// Debounce kernel-driven reloads so a burst of mutations coalesces.
let reloadTimer: ReturnType<typeof setTimeout> | null = null;
function scheduleReload() {
  if (reloadTimer) clearTimeout(reloadTimer);
  reloadTimer = setTimeout(reloadGlb, 250);
}

// ── Overlay pins ─────────────────────────────────────────────────────────────
function labelSprite(textStr: string, color: string): THREE.Sprite {
  const pad = 8, font = 22;
  const c = document.createElement("canvas");
  const ctx = c.getContext("2d")!;
  ctx.font = `${font}px ui-sans-serif, system-ui, sans-serif`;
  const label = textStr.slice(0, 40);
  const w = Math.ceil(ctx.measureText(label).width) + pad * 2 + 22;
  c.width = w; c.height = font + pad * 2;
  ctx.font = `${font}px ui-sans-serif, system-ui, sans-serif`;
  ctx.fillStyle = "rgba(22,27,34,0.92)";
  ctx.strokeStyle = color; ctx.lineWidth = 2;
  roundRect(ctx, 1, 1, c.width - 2, c.height - 2, 8); ctx.fill(); ctx.stroke();
  ctx.fillStyle = color; ctx.beginPath(); ctx.arc(pad + 6, c.height / 2, 5, 0, Math.PI * 2); ctx.fill();
  ctx.fillStyle = "#e6edf3"; ctx.textBaseline = "middle";
  ctx.fillText(label, pad + 18, c.height / 2 + 1);
  const tex = new THREE.CanvasTexture(c);
  tex.anisotropy = 4;
  const sprite = new THREE.Sprite(new THREE.SpriteMaterial({ map: tex, depthTest: false, transparent: true }));
  const scale = 0.35;
  sprite.scale.set((c.width / c.height) * 10 * scale, 10 * scale, 1);
  sprite.renderOrder = 999;
  return sprite;
}
function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y); ctx.arcTo(x + w, y, x + w, y + h, r); ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r); ctx.arcTo(x, y, x + w, y, r); ctx.closePath();
}

const seenPins = new Set<number>();
/** Place a pin from an overlay event. v1 anchor: {point:[x,y,z]} in kernel
 *  coords (added under modelGroup); falls back to the model center. */
function addPin(e: SpineEvent) {
  if (seenPins.has(e.id)) return;
  seenPins.add(e.id);
  const p = (e.payload?.anchor as { point?: number[] } | undefined)?.point;
  const note = String(e.payload?.text ?? e.type ?? "pin");
  const color = colorFor(e.author);
  const sprite = labelSprite(`${note} · ${e.author}`, color);
  if (Array.isArray(p) && p.length === 3) {
    sprite.position.set(p[0], p[1], p[2]);
  } else {
    const c = new THREE.Box3().setFromObject(meshRoot).getCenter(new THREE.Vector3());
    modelGroup.worldToLocal(c);
    sprite.position.copy(c);
  }
  pinRoot.add(sprite);
}

// ── Fold the event stream ────────────────────────────────────────────────────
interface SpineEvent {
  id: number; seq: number; author: string; kind: string; type: string;
  payload: Record<string, unknown>; created_at?: string;
}
let lastSeq = 0;
const seenSeq = new Set<number>();

function tick(msg: string) {
  tickerEl.textContent = msg;
}
function banner(msg: string) {
  bannerEl.textContent = msg; bannerEl.style.display = "block";
  setTimeout(() => (bannerEl.style.display = "none"), 6000);
}

function applyEvent(e: SpineEvent) {
  if (!e || typeof e.seq !== "number" || seenSeq.has(e.seq)) return;
  seenSeq.add(e.seq);
  lastSeq = Math.max(lastSeq, e.seq);
  if (e.kind === "kernel") {
    const changed = e.payload?.changed;
    if (changed) scheduleReload();
    tick(`#${e.seq} ${e.type} · ${e.author}`);
  } else if (e.kind === "overlay") {
    addPin(e);
    tick(`#${e.seq} ${e.type} "${String(e.payload?.text ?? "")}" · ${e.author}`);
  } else if (e.kind === "control") {
    banner(`${e.type.replace(/_/g, " ")} · ${e.author}`);
    tick(`#${e.seq} ${e.type} · ${e.author}`);
  }
}

async function replay(since?: number) {
  try {
    const u = since != null ? `${api}/events?since=${since}` : `${api}/events`;
    const r = await fetch(u, { cache: "no-store" });
    if (!r.ok) return;
    const { events } = (await r.json()) as { events: SpineEvent[] };
    for (const e of events) applyEvent(e);
  } catch {/* ignore */}
}

// ── Presence roster ──────────────────────────────────────────────────────────
function renderRoster(state: Record<string, Array<{ name?: string }>>) {
  const names: string[] = [];
  for (const k of Object.keys(state)) for (const m of state[k]) names.push(m.name ?? "guest");
  rosterEl.innerHTML = "";
  for (const n of names.slice(0, 8)) {
    const a = document.createElement("div");
    a.className = "avatar"; a.style.background = colorFor(n); a.title = n;
    a.textContent = n.replace(/^viewer:/, "").slice(0, 2).toUpperCase();
    rosterEl.appendChild(a);
  }
}

// ── Click to drop a pin ──────────────────────────────────────────────────────
const ray = new THREE.Raycaster();
let pendingPoint: THREE.Vector3 | null = null;
renderer.domElement.addEventListener("pointerdown", (ev) => {
  if (ev.button !== 0) return;
  const rect = renderer.domElement.getBoundingClientRect();
  const ndc = new THREE.Vector2(
    ((ev.clientX - rect.left) / rect.width) * 2 - 1,
    -((ev.clientY - rect.top) / rect.height) * 2 + 1,
  );
  ray.setFromCamera(ndc, camera);
  const hit = ray.intersectObject(meshRoot, true)[0];
  if (!hit) { pinInput.style.display = "none"; return; }
  pendingPoint = modelGroup.worldToLocal(hit.point.clone()); // → kernel coords
  pinInput.style.left = `${ev.clientX - rect.left}px`;
  pinInput.style.top = `${ev.clientY - rect.top}px`;
  pinInput.style.display = "block";
  pinText.value = ""; pinText.focus();
});
async function savePin() {
  if (!pendingPoint) return;
  const text = pinText.value.trim();
  pinInput.style.display = "none";
  try {
    await fetch(`${api}/annotate`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        type: "pin",
        author: viewerName,
        payload: { anchor: { point: [pendingPoint.x, pendingPoint.y, pendingPoint.z] }, text },
      }),
    });
    // The pin rides the broadcast back to us; no optimistic render needed.
  } catch {/* ignore */}
  pendingPoint = null;
}
pinSave.addEventListener("click", savePin);
pinText.addEventListener("keydown", (e) => { if (e.key === "Enter") savePin(); });

// ── Boot ─────────────────────────────────────────────────────────────────────
function animate() {
  requestAnimationFrame(animate);
  controls.update();
  renderer.render(scene, camera);
}
animate();

async function boot() {
  if (!sessionId) { tick("no session id in URL"); return; }
  await reloadGlb();
  await replay();

  let cfg: { supabaseUrl?: string; anonKey?: string };
  try {
    cfg = await (await fetch(`${api}/config`, { cache: "no-store" })).json();
  } catch {
    tick("could not load realtime config"); return;
  }
  if (!cfg.supabaseUrl || !cfg.anonKey) { tick("realtime not configured"); return; }

  const supabase = createClient(cfg.supabaseUrl, cfg.anonKey, {
    realtime: { params: { eventsPerSecond: 20 } },
  });
  supabase.realtime.setAuth(cfg.anonKey);

  supabase
    .channel(`session:${sessionId}`)
    .on("broadcast", { event: "session_event" }, (m: { payload: SpineEvent }) => applyEvent(m.payload))
    .subscribe((status: string) => {
      if (status === "SUBSCRIBED") {
        connEl.textContent = "live"; connEl.className = "ok";
        void replay(lastSeq); // backfill anything missed before subscribe
      } else if (status === "CHANNEL_ERROR" || status === "TIMED_OUT") {
        connEl.textContent = "reconnecting…"; connEl.className = "";
      }
    });

  const presence = supabase.channel(`session:${sessionId}:presence`, {
    config: { presence: { key: viewerId } },
  });
  presence
    .on("presence", { event: "sync" }, () =>
      renderRoster(presence.presenceState() as Record<string, Array<{ name?: string }>>),
    )
    .subscribe((status: string) => {
      if (status === "SUBSCRIBED") void presence.track({ name: viewerName });
    });

  hintEl.textContent = "click the model to drop a pin · geometry is read-only";
}
void boot();
