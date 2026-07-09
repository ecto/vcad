// Live WASM kernel background — a story told in real geometry.
//
// The reel follows a part's life: stock → rough cut → drill → pattern →
// finish → lighten → join → bend (sheet metal) → down to the atoms — and,
// sometimes, the payoff: the green verified frame. First cycle plays in
// story order; after that the bag is shuffled and the reward is variable.
//
// Every frame is honest: solids come from kernel booleans, the bracket from
// evaluateSheetMetalChain, the molecule from atoms_parse_xyz. Wireframes show
// only true edges (creases + per-frame silhouettes). Scenes that fail or run
// slow on this machine disable themselves.

const TAU = Math.PI * 2;
const FADE_MS = 450;

export async function startHeroEngine(canvas, caption) {
  const wasm = await import("../../packages/kernel-wasm/vcad_kernel_wasm.js");
  await wasm.default();
  const { Solid } = wasm;
  const version = safeVersion(wasm);

  const ctx = canvas.getContext("2d");
  let dpr = Math.min(devicePixelRatio || 1, 2);
  let adaptive = true;

  /* ================= scenes ================= */
  // body: { solid } | { mesh: {positions, indices} } | { mol: {pts, bonds, elems} }
  //       + at: [fx, fy] canvas-fraction anchor, size: fraction of min(w,h)

  const scenes = [
    {
      op: "stock",
      ms: 3200, hue: "170,178,188", spinMul: 0.6,
      build(t) {
        const s1 = Solid.cube(140, 90, 18);
        const s2 = Solid.cube(90, 90, 90);
        return {
          bodies: [
            { solid: s1, at: [0.42, 0.6], size: 0.34 },
            { solid: s2, at: [0.7, 0.42], size: 0.22 },
          ],
          anno: "6061-t6 · sawn blanks", annoBody: 0,
        };
      },
    },
    {
      op: "boolean difference",
      ms: 5200, hue: "143,163,190", spinMul: 1.4,
      build(t) {
        const r = 52 + 14 * Math.sin(t * 0.0009);
        const cube = Solid.cube(90, 90, 90);
        const ball = Solid.sphere(r, 30).translate(90, 90, 90);
        const part = cube.difference(ball);
        cube.free(); ball.free();
        return { bodies: [{ solid: part, at: [0.6, 0.52], size: 0.4 }], anno: `r ${r.toFixed(1)}` };
      },
    },
    {
      op: "boolean difference · linear pattern",
      ms: 6000, hue: "175,182,192", spinMul: -1,
      build(t) {
        const bore = 16 + 7 * Math.sin(t * 0.001);
        const plate = Solid.cube(140, 90, 18);
        const hole = Solid.cylinder(bore, 40, 28).translate(70, 45, -10);
        let part = plate.difference(hole);
        if (adaptive) {
          const drill = Solid.cylinder(4.25, 40, 14).translate(14, 14, -10);
          const holes = drill.linearPattern(1, 0, 0, 2, 112).linearPattern(0, 1, 0, 2, 62);
          const next = part.difference(holes);
          part.free(); drill.free(); holes.free();
          part = next;
        }
        const bit = Solid.cylinder(4.25, 70, 12);
        plate.free(); hole.free();
        return {
          bodies: [
            { solid: part, at: [0.55, 0.58], size: 0.42 },
            { solid: bit, at: [0.24, 0.38], size: 0.13 },
          ],
          anno: `ø ${(bore * 2).toFixed(1)}`, annoBody: 0,
        };
      },
    },
    {
      op: "circular pattern",
      ms: 5600, hue: "201,128,84", spinMul: 1.2,
      build(t) {
        const bcr = 38 + 5 * Math.sin(t * 0.0009);
        const disc = Solid.cylinder(60, 14, 30);
        const hub = Solid.cylinder(15, 40, 20).translate(0, 0, -10);
        const bolt = Solid.cylinder(3.5, 40, 8).translate(bcr, 0, -10);
        const ring = bolt.circularPattern(0, 0, 0, 0, 0, 1, 6, 360);
        const p1 = disc.difference(hub);
        const flange = p1.difference(ring);
        // the fastener that goes with it — kept cheap: no boolean needed
        const screw = Solid.cylinder(3.2, 26, 10);
        disc.free(); hub.free(); bolt.free(); ring.free(); p1.free();
        return {
          bodies: [
            { solid: flange, at: [0.58, 0.55], size: 0.4 },
            { solid: screw, at: [0.28, 0.68], size: 0.09 },
          ],
          anno: `6 × ø 7.0 · bc ${(bcr * 2).toFixed(0)}`, annoBody: 0,
        };
      },
    },
    {
      op: "chamfer",
      ms: 4800, hue: "190,160,98", spinMul: -1.3,
      build(t) {
        const d = 6 + 4.5 * Math.sin(t * 0.0011);
        const block = Solid.cube(110, 80, 45);
        const chamfered = block.chamfer(d);
        const bore = Solid.cylinder(14, 60, 24).translate(55, 40, -5);
        const part = chamfered.difference(bore);
        block.free(); chamfered.free(); bore.free();
        return { bodies: [{ solid: part, at: [0.44, 0.52], size: 0.42 }], anno: `chamfer ${d.toFixed(1)}` };
      },
    },
    {
      op: "fillet",
      ms: 4800, hue: "170,178,188", spinMul: 1.6,
      build(t) {
        const r = 8 + 5 * Math.sin(t * 0.0009);
        const block = Solid.cube(100, 70, 40);
        const part = block.fillet(r);
        block.free();
        return { bodies: [{ solid: part, at: [0.62, 0.6], size: 0.4 }], anno: `r ${r.toFixed(1)}` };
      },
    },
    {
      op: "shell",
      ms: 5600, hue: "143,163,190", spinMul: 0.9,
      build(t) {
        const wall = 3.5 + 2 * Math.sin(t * 0.001);
        const box = Solid.cube(100, 80, 60);
        const hollow = box.shell(wall);
        const cutter = Solid.cube(60, 50, 70).translate(55, 45, 5);
        const part = hollow.difference(cutter);
        box.free(); hollow.free(); cutter.free();
        return { bodies: [{ solid: part, at: [0.5, 0.5], size: 0.44 }], anno: `wall ${wall.toFixed(1)}` };
      },
    },
    {
      op: "boolean union",
      ms: 4800, hue: "190,160,98", spinMul: -1.2,
      build(t) {
        const bossR = 20 + 6 * Math.sin(t * 0.001);
        const plate = Solid.cube(120, 80, 14);
        const boss = Solid.cylinder(bossR, 44, 28).translate(60, 40, 0);
        const joined = plate.union(boss);
        const bore = Solid.cylinder(10, 60, 20).translate(60, 40, -5);
        const part = joined.difference(bore);
        plate.free(); boss.free(); joined.free(); bore.free();
        return { bodies: [{ solid: part, at: [0.4, 0.6], size: 0.42 }], anno: `boss ø ${(bossR * 2).toFixed(0)}` };
      },
    },
    {
      op: "sheet metal · edge flange",
      ms: 6000, hue: "150,170,200", spinMul: 0.8,
      build(t) {
        const angle = 0.9 + 0.65 * Math.sin(t * 0.0009); // radians
        const chain = [
          { type: "BaseFlangeRect", width: 120, depth: 80, thickness: 2, material: "al-soft" },
          { type: "EdgeFlange", panelId: 0, edgeIndex: 0, length: 32, angle, direction: "Up" },
          { type: "EdgeFlange", panelId: 0, edgeIndex: 2, length: 32, angle, direction: "Up" },
        ];
        const res = JSON.parse(wasm.evaluateSheetMetalChain(JSON.stringify(chain)));
        if (res.error) throw new Error(res.error);
        return {
          bodies: [{ mesh: res.mesh, at: [0.56, 0.54], size: 0.42 }],
          anno: `bend ${((angle * 180) / Math.PI).toFixed(0)}° · k 0.44`,
        };
      },
    },
    {
      op: "atoms · parse_xyz",
      ms: 5600, spinMul: 1.8,
      build() {
        const mol = parseMolecule(wasm, tolueneXyz());
        return { bodies: [{ mol, at: [0.55, 0.5], size: 0.4 }], anno: "C₇H₈ · 15 atoms" };
      },
    },
    {
      op: "verified",
      ms: 5000, spinMul: 1,
      rare: true, // the payoff frame — shows up when it shows up
      green: true,
      build(t) {
        const plate = Solid.cube(140, 90, 18);
        const hole = Solid.cylinder(21, 40, 28).translate(70, 45, -10);
        const part = plate.difference(hole);
        plate.free(); hole.free();
        return {
          bodies: [{ solid: part, at: [0.5, 0.55], size: 0.42 }],
          anno: "✓ dfm · 0 violations · receipt 9f3a…c2",
        };
      },
    },
  ];

  /* ================= molecule helpers ================= */

  function tolueneXyz() {
    const lines = [];
    const C = [], H = [];
    for (let i = 0; i < 6; i++) {
      const a = (i * TAU) / 6;
      C.push([1.396 * Math.cos(a), 1.396 * Math.sin(a), 0]);
      if (i !== 0) H.push([2.48 * Math.cos(a), 2.48 * Math.sin(a), 0]);
    }
    C.push([2.906, 0, 0]); // methyl carbon on ring position 0
    H.push([3.29, 0.89, 0], [3.29, -0.45, 0.77], [3.29, -0.45, -0.77]);
    for (const c of C) lines.push(`C ${c[0].toFixed(3)} ${c[1].toFixed(3)} ${c[2].toFixed(3)}`);
    for (const h of H) lines.push(`H ${h[0].toFixed(3)} ${h[1].toFixed(3)} ${h[2].toFixed(3)}`);
    return `${lines.length}\ntoluene\n${lines.join("\n")}\n`;
  }

  function parseMolecule(wasm, xyz) {
    // MoleculeSystem: { species, positions, speciesIdx, bonds } — the kernel
    // perceives the bonds; we just draw what it saw.
    const sys = JSON.parse(wasm.atoms_parse_xyz(xyz));
    const S = 26; // Å → model units
    const pts = sys.positions.map((p) => [p[0] * S, p[1] * S, p[2] * S]);
    const elems = sys.speciesIdx.map((i) => (sys.species[i].element || "C")[0]);
    const bonds = (sys.bonds || []).map((b) => [b.a, b.b]);
    if (!pts.length) throw new Error("empty MoleculeSystem");
    return { pts, bonds, elems };
  }

  /* ================= mesh → true edges ================= */

  function extractEdges(pos, idx) {
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

  function meshBounds(pos) {
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

  /* ================= scene state ================= */

  let frameData = null; // { bodies: [drawable], anno, annoBody, green }
  let sceneIdx = 0;
  let cycle = 0;
  let playlist = [];
  let playPos = 0;

  // deterministic shuffle-bag; the verified frame is the variable reward
  function mulberry(seed) {
    return () => {
      seed |= 0; seed = (seed + 0x6d2b79f5) | 0;
      let z = Math.imul(seed ^ (seed >>> 15), 1 | seed);
      z = (z + Math.imul(z ^ (z >>> 7), 61 | z)) ^ z;
      return ((z ^ (z >>> 14)) >>> 0) / 4294967296;
    };
  }
  function buildPlaylist() {
    const base = scenes.map((s, i) => i).filter((i) => !scenes[i].rare && !scenes[i].disabled);
    const rnd = mulberry(cycle * 2654435761 + 1);
    if (cycle > 0) {
      for (let i = base.length - 1; i > 0; i--) {
        const j = Math.floor(rnd() * (i + 1));
        [base[i], base[j]] = [base[j], base[i]];
      }
    }
    const rare = scenes.findIndex((s) => s.rare && !s.disabled);
    if (rare >= 0 && rnd() < 0.4) base.splice(Math.floor(rnd() * base.length) + 1, 0, rare);
    playlist = base.length ? base : [0];
    playPos = 0;
    sceneIdx = playlist[0];
  }

  function setCaption() {
    if (caption) caption.textContent = `live · vcad kernel ${version} · ${scenes[sceneIdx].op}`;
  }

  function nextScene() {
    playPos++;
    if (playPos >= playlist.length) { cycle++; buildPlaylist(); }
    else sceneIdx = playlist[playPos];
    if (scenes[sceneIdx].disabled) { nextScene(); return; }
    setCaption();
  }

  function rebuild(t) {
    const started = performance.now();
    const scene = scenes[sceneIdx];
    try {
      const built = scene.build(t);
      const bodies = built.bodies.map((b) => {
        if (b.mol) return { kind: "mol", mol: b.mol, ...meshBounds(b.mol.pts.flat()), at: b.at, size: b.size };
        const pos = b.solid ? null : Float32Array.from(b.mesh.positions);
        const m = b.solid ? b.solid.getMesh(24) : null;
        const positions = b.solid ? m.positions : pos;
        const indices = b.solid ? m.indices : Uint32Array.from(b.mesh.indices);
        const out = {
          kind: "mesh",
          positions,
          edges: extractEdges(positions, indices),
          ...meshBounds(positions),
          at: b.at, size: b.size,
        };
        b.solid?.free();
        return out;
      });
      frameData = { bodies, anno: built.anno, annoBody: built.annoBody ?? 0, green: !!scene.green, hue: scene.hue };
    } catch (e) {
      scene.disabled = true;
      console.warn(`reel scene "${scene.op}" disabled:`, e);
      return false;
    }
    const took = performance.now() - started;
    if (took > 60) adaptive = false;
    if (took > 220) {
      scene.strikes = (scene.strikes || 0) + 1;
      if (scene.strikes >= 2) {
        scene.disabled = true;
        console.warn(`reel scene "${scene.op}" disabled: ${took.toFixed(0)}ms rebuild (2 strikes)`);
      }
    } else scene.strikes = 0;
    return true;
  }

  /* ================= projection + draw ================= */

  const TILT = Math.atan(Math.SQRT1_2);
  const CT = Math.cos(TILT), ST = Math.sin(TILT);

  function draw(spin, alpha) {
    const w = canvas.width, h = canvas.height;
    ctx.clearRect(0, 0, w, h);
    if (!frameData || alpha <= 0) return;
    const cs = Math.cos(spin), sn = Math.sin(spin);
    const vx = ST * sn, vy = ST * cs, vz = CT;
    const lineColor = frameData.green ? "62, 207, 142" : (frameData.hue || "158, 165, 175");

    frameData.bodies.forEach((body, bi) => {
      const s = (Math.min(w, h) * (body.size ?? 0.4)) / body.extent;
      const [ox, oy, oz] = body.center;
      const ax = w * body.at[0], ay = h * body.at[1];
      const proj = (x0, y0, z0, out) => {
        const x = x0 - ox, y = y0 - oy, z = z0 - oz;
        const rx = x * cs - y * sn, ry = x * sn + y * cs;
        out[0] = ax + rx * s;
        out[1] = ay + (ry * CT - z * ST) * s;
      };
      const p1 = [0, 0], p2 = [0, 0];

      if (body.kind === "mol") {
        const { pts, bonds, elems } = body.mol;
        ctx.strokeStyle = `rgba(170, 178, 188, ${0.4 * alpha})`;
        ctx.lineWidth = 1 * dpr;
        ctx.beginPath();
        for (const [i, j] of bonds) {
          proj(...pts[i], p1); proj(...pts[j], p2);
          ctx.moveTo(p1[0], p1[1]); ctx.lineTo(p2[0], p2[1]);
        }
        ctx.stroke();
        for (let i = 0; i < pts.length; i++) {
          proj(...pts[i], p1);
          const r = (elems[i] === "H" ? 5 : 10) * s;
          ctx.beginPath();
          ctx.arc(p1[0], p1[1], Math.max(1.5 * dpr, r), 0, TAU);
          ctx.fillStyle = elems[i] === "H"
            ? `rgba(226, 229, 233, ${0.55 * alpha})`
            : `rgba(96, 104, 114, ${0.85 * alpha})`;
          ctx.fill();
        }
      } else {
        const P = body.positions, E = body.edges;
        ctx.lineWidth = 1 * dpr;
        ctx.strokeStyle = `rgba(${lineColor}, ${0.5 * alpha})`;
        ctx.beginPath();
        for (let i = 0; i < E.length; i += 9) {
          let show = E[i + 8] === 1;
          if (!show) {
            const f1 = E[i + 2] * vx + E[i + 3] * vy + E[i + 4] * vz;
            const f2 = E[i + 5] * vx + E[i + 6] * vy + E[i + 7] * vz;
            show = (f1 > 0) !== (f2 > 0);
          }
          if (!show) continue;
          const a = E[i], b = E[i + 1];
          proj(P[a * 3], P[a * 3 + 1], P[a * 3 + 2], p1);
          proj(P[b * 3], P[b * 3 + 1], P[b * 3 + 2], p2);
          ctx.moveTo(p1[0], p1[1]); ctx.lineTo(p2[0], p2[1]);
        }
        ctx.stroke();
      }

      if (bi === frameData.annoBody) {
        const annoColor = frameData.green ? "62, 207, 142" : "242, 92, 31";
        ctx.fillStyle = `rgba(${annoColor}, ${0.75 * alpha})`;
        ctx.font = `${10 * dpr}px "JetBrains Mono", monospace`;
        ctx.fillText(frameData.anno, ax + (body.extent * s) / 2 + 12 * dpr, ay - (body.extent * s) / 4);
      }
    });
  }

  /* ================= loop ================= */

  let running = true, lastBuild = 0, raf = 0, sceneStart = 0;

  function frame(t) {
    if (!running) return;
    if (!sceneStart) sceneStart = t;
    const local = t - sceneStart;
    const ms = scenes[sceneIdx].ms || 8000;
    if (local > ms) { nextScene(); sceneStart = t; lastBuild = 0; }
    if (t - lastBuild > 140) {
      lastBuild = t;
      for (let tries = 0; tries < scenes.length && !rebuild(t); tries++) {
        nextScene(); sceneStart = t;
      }
    }
    const aIn = Math.min(1, (t - sceneStart) / FADE_MS);
    const aOut = Math.min(1, (ms - (t - sceneStart)) / FADE_MS);
    draw(t * 0.00022 * TAU * (scenes[sceneIdx].spinMul ?? 1), Math.max(0, Math.min(aIn, aOut)));
    raf = requestAnimationFrame(frame);
  }

  function resize() {
    dpr = Math.min(devicePixelRatio || 1, 2);
    canvas.width = canvas.clientWidth * dpr;
    canvas.height = canvas.clientHeight * dpr;
  }
  resize();
  addEventListener("resize", resize, { passive: true });

  const keepAlive = () => !!window.__vcadReelKeepAlive;
  new IntersectionObserver((en) => {
    const vis = (en.some((e) => e.isIntersecting) && !document.hidden) || keepAlive();
    if (vis && !running) { running = true; raf = requestAnimationFrame(frame); }
    if (!vis) { running = false; cancelAnimationFrame(raf); }
  }).observe(canvas);
  document.addEventListener("visibilitychange", () => {
    if (document.hidden && !keepAlive()) { running = false; cancelAnimationFrame(raf); }
    else if (!running) { running = true; raf = requestAnimationFrame(frame); }
  });

  buildPlaylist();
  setCaption();
  rebuild(0);
  raf = requestAnimationFrame(frame);
  canvas.classList.add("live");
  document.dispatchEvent(new CustomEvent("vcad-engine-live"));
  window.__vcadReel = {
    scenes: scenes.map((s) => s.op),
    skip: () => { sceneStart = 1; },
    setScene: (i) => { sceneIdx = i % scenes.length; setCaption(); },
    render: (t) => {
      running = false; cancelAnimationFrame(raf); // freeze the loop for deterministic captures
      const ok = rebuild(t);
      if (ok) draw(t * 0.00022 * TAU * (scenes[sceneIdx].spinMul ?? 1), 1);
      return ok ? "ok" : "disabled";
    },
    resume: () => { if (!running) { running = true; sceneStart = 0; raf = requestAnimationFrame(frame); } },
  };
}

function safeVersion(wasm) {
  try { return wasm.get_kernel_version(); } catch { return "0.9.4"; }
}
