// Shared CAD-honest wireframe rendering: welded edge extraction (creases +
// per-frame silhouettes), brand camera projection, and molecule drawing.
// Consumers: engine-card.js, prove-demo.js. (hero-engine.js predates this and
// carries its own copy — TODO: migrate it here.)

export const TILT = Math.atan(Math.SQRT1_2); // the locked 35.26° brand camera

export function extractEdges(pos, idx) {
  const weld = new Map();
  const canon = new Uint32Array(pos.length / 3);
  for (let i = 0; i < pos.length / 3; i++) {
    const k = `${Math.round(pos[i * 3] * 512)},${Math.round(pos[i * 3 + 1] * 512)},${Math.round(pos[i * 3 + 2] * 512)}`;
    const c = weld.get(k);
    if (c === undefined) { weld.set(k, i); canon[i] = i; } else canon[i] = c;
  }
  const tri = idx.length / 3;
  const edges = new Map();
  for (let i = 0; i < tri; i++) {
    const a0 = canon[idx[i * 3]], b0 = canon[idx[i * 3 + 1]], c0 = canon[idx[i * 3 + 2]];
    const ax = pos[a0 * 3], ay = pos[a0 * 3 + 1], az = pos[a0 * 3 + 2];
    const ux = pos[b0 * 3] - ax, uy = pos[b0 * 3 + 1] - ay, uz = pos[b0 * 3 + 2] - az;
    const vx = pos[c0 * 3] - ax, vy = pos[c0 * 3 + 1] - ay, vz = pos[c0 * 3 + 2] - az;
    let nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx;
    const l = Math.hypot(nx, ny, nz) || 1;
    nx /= l; ny /= l; nz /= l;
    const verts = [a0, b0, c0];
    for (let e = 0; e < 3; e++) {
      const a = verts[e], b = verts[(e + 1) % 3];
      if (a === b) continue;
      const k = a < b ? a * 4194304 + b : b * 4194304 + a;
      const cur = edges.get(k);
      if (cur) { cur[5] = nx; cur[6] = ny; cur[7] = nz; cur[8] = 2; }
      else edges.set(k, [a, b, nx, ny, nz, nx, ny, nz, 1]);
    }
  }
  const out = new Float32Array(edges.size * 9);
  let j = 0;
  for (const [, v] of edges) {
    const dot = v[2] * v[5] + v[3] * v[6] + v[4] * v[7];
    out[j++] = v[0]; out[j++] = v[1];
    out[j++] = v[2]; out[j++] = v[3]; out[j++] = v[4];
    out[j++] = v[5]; out[j++] = v[6]; out[j++] = v[7];
    out[j++] = v[8] === 1 || dot < 0.86 ? 1 : 0;
  }
  return out;
}

export function meshBounds(pos) {
  let minX = 1e30, minY = 1e30, minZ = 1e30, maxX = -1e30, maxY = -1e30, maxZ = -1e30;
  for (let i = 0; i < pos.length; i += 3) {
    if (pos[i] < minX) minX = pos[i]; if (pos[i] > maxX) maxX = pos[i];
    if (pos[i + 1] < minY) minY = pos[i + 1]; if (pos[i + 1] > maxY) maxY = pos[i + 1];
    if (pos[i + 2] < minZ) minZ = pos[i + 2]; if (pos[i + 2] > maxZ) maxZ = pos[i + 2];
  }
  return {
    center: [(minX + maxX) / 2, (minY + maxY) / 2, (minZ + maxZ) / 2],
    extent: Math.max(maxX - minX, maxY - minY, maxZ - minZ) || 1,
  };
}

/** Draw a welded-edge wireframe. opts: {spin, tilt=TILT, ax, ay, s, color="158,165,175", alpha=0.5, lw=1, dpr} */
export function drawWireframe(ctx, body, opts) {
  const { positions: P, edges: E, center } = body;
  const tilt = opts.tilt ?? TILT;
  const ct = Math.cos(tilt), st = Math.sin(tilt);
  const cs = Math.cos(opts.spin), sn = Math.sin(opts.spin);
  const vx = st * sn, vy = st * cs, vz = ct;
  const [ox, oy, oz] = center;
  const proj = (i, out) => {
    const x = P[i * 3] - ox, y = P[i * 3 + 1] - oy, z = P[i * 3 + 2] - oz;
    const rx = x * cs - y * sn, ry = x * sn + y * cs;
    out[0] = opts.ax + rx * opts.s;
    out[1] = opts.ay + (ry * ct - z * st) * opts.s;
  };
  const p1 = [0, 0], p2 = [0, 0];
  ctx.lineWidth = (opts.lw ?? 1) * opts.dpr;
  ctx.strokeStyle = `rgba(${opts.color ?? "158,165,175"}, ${opts.alpha ?? 0.5})`;
  ctx.beginPath();
  for (let i = 0; i < E.length; i += 9) {
    let show = E[i + 8] === 1;
    if (!show) {
      const f1 = E[i + 2] * vx + E[i + 3] * vy + E[i + 4] * vz;
      const f2 = E[i + 5] * vx + E[i + 6] * vy + E[i + 7] * vz;
      show = (f1 > 0) !== (f2 > 0);
    }
    if (!show) continue;
    proj(E[i], p1); proj(E[i + 1], p2);
    ctx.moveTo(p1[0], p1[1]); ctx.lineTo(p2[0], p2[1]);
  }
  ctx.stroke();
}

/** Draw a molecule (kernel-perceived bonds). body: {pts, bonds, elems, center, extent} */
export function drawMolecule(ctx, body, opts) {
  const tilt = opts.tilt ?? TILT;
  const ct = Math.cos(tilt), st = Math.sin(tilt);
  const cs = Math.cos(opts.spin), sn = Math.sin(opts.spin);
  const [ox, oy, oz] = body.center;
  const proj = (p, out) => {
    const x = p[0] - ox, y = p[1] - oy, z = p[2] - oz;
    const rx = x * cs - y * sn, ry = x * sn + y * cs;
    out[0] = opts.ax + rx * opts.s;
    out[1] = opts.ay + (ry * ct - z * st) * opts.s;
  };
  const p1 = [0, 0], p2 = [0, 0];
  ctx.strokeStyle = `rgba(170, 178, 188, ${0.4 * (opts.alpha ?? 1)})`;
  ctx.lineWidth = 1 * opts.dpr;
  ctx.beginPath();
  for (const [i, j] of body.bonds) {
    proj(body.pts[i], p1); proj(body.pts[j], p2);
    ctx.moveTo(p1[0], p1[1]); ctx.lineTo(p2[0], p2[1]);
  }
  ctx.stroke();
  for (let i = 0; i < body.pts.length; i++) {
    proj(body.pts[i], p1);
    const r = (body.elems[i] === "H" ? 5 : 10) * opts.s;
    ctx.beginPath();
    ctx.arc(p1[0], p1[1], Math.max(1.5 * opts.dpr, r), 0, Math.PI * 2);
    ctx.fillStyle = body.elems[i] === "H"
      ? `rgba(226, 229, 233, ${0.55 * (opts.alpha ?? 1)})`
      : `rgba(96, 104, 114, ${0.85 * (opts.alpha ?? 1)})`;
    ctx.fill();
  }
}

let kernelPromise = null;
/** Shared, memoized kernel load — every consumer on a page shares one instance. */
export function loadKernel() {
  if (!kernelPromise) {
    kernelPromise = import("../../packages/kernel-wasm/vcad_kernel_wasm.js").then(async (wasm) => {
      await wasm.default();
      return wasm;
    });
  }
  return kernelPromise;
}
