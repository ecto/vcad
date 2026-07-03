/**
 * A small rotating Buckminsterfullerene (C₆₀) for the boot splash — a self-
 * contained Canvas 2D renderer (no R3F / WASM, so it's safe to run before the
 * engine boots). The cage is generated from the golden ratio, bonds are
 * perceived by distance, and atoms are drawn as depth-sorted shaded spheres.
 */

import { useEffect, useRef } from "react";

function buildC60(): [number, number, number][] {
  const phi = (1 + Math.sqrt(5)) / 2;
  const base: [number, number, number][] = [
    [0, 1, 3 * phi],
    [2, 1 + 2 * phi, phi],
    [1, 2 + phi, 2 * phi],
  ];
  const verts: [number, number, number][] = [];
  for (const [p, q, r] of base) {
    for (const sp of [p, -p]) {
      for (const sq of [q, -q]) {
        for (const sr of [r, -r]) {
          if (p === 0 && sp < 0) continue;
          verts.push([sp, sq, sr], [sr, sp, sq], [sq, sr, sp]);
        }
      }
    }
  }
  const uniq: [number, number, number][] = [];
  for (const v of verts) {
    if (!uniq.some((u) => Math.hypot(u[0] - v[0], u[1] - v[1], u[2] - v[2]) < 1e-6)) uniq.push(v);
  }
  // normalize to unit-ish radius
  const s = 1 / (3 * phi);
  return uniq.map(([x, y, z]) => [x * s, y * s, z * s]);
}

const C60 = buildC60();
const BONDS: [number, number][] = (() => {
  const b: [number, number][] = [];
  // nearest-neighbor edges (edge length ≈ 2*s)
  const edge = (2 / (3 * (1 + Math.sqrt(5)) / 2)) * 1.15;
  for (let i = 0; i < C60.length; i++)
    for (let j = i + 1; j < C60.length; j++) {
      const d = Math.hypot(
        C60[i]![0] - C60[j]![0],
        C60[i]![1] - C60[j]![1],
        C60[i]![2] - C60[j]![2],
      );
      if (d < edge) b.push([i, j]);
    }
  return b;
})();

export function SplashMolecule({ size = 132 }: { size?: number }) {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = size * dpr;
    canvas.height = size * dpr;
    ctx.scale(dpr, dpr);
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

    // brand accent for a subtle molecular tint
    const cs = getComputedStyle(document.documentElement);
    const brand = cs.getPropertyValue("--brand").trim() || "#e0218a";

    let raf = 0;
    let t = reduce ? 0.6 : 0;
    const cx = size / 2;
    const cy = size / 2;
    const R = size * 0.36;

    const draw = () => {
      ctx.clearRect(0, 0, size, size);
      const ay = t;
      const ax = 0.42;
      const cyaw = Math.cos(ay), syaw = Math.sin(ay);
      const cpit = Math.cos(ax), spit = Math.sin(ax);
      const pts = C60.map(([x, y, z]) => {
        let X = x * cyaw - z * syaw;
        let Z = x * syaw + z * cyaw;
        const Y = y * cpit - Z * spit;
        Z = y * spit + Z * cpit;
        return { x: cx + X * R, y: cy - Y * R, z: Z };
      });

      // bonds (behind), depth-cued
      for (const [i, j] of BONDS) {
        const a = pts[i]!, b = pts[j]!;
        const depth = (a.z + b.z) * 0.5;
        const alpha = 0.18 + Math.max(0, depth + 1) * 0.16;
        ctx.strokeStyle = `rgba(120,130,150,${alpha.toFixed(3)})`;
        ctx.lineWidth = 1.4;
        ctx.beginPath();
        ctx.moveTo(a.x, a.y);
        ctx.lineTo(b.x, b.y);
        ctx.stroke();
      }

      // atoms, far → near, shaded
      const order = pts.map((_, i) => i).sort((u, v) => pts[u]!.z - pts[v]!.z);
      for (const i of order) {
        const p = pts[i]!;
        const rr = size * 0.05;
        const fog = 0.55 + Math.max(0, p.z + 1) * 0.22;
        const g = ctx.createRadialGradient(p.x - rr * 0.4, p.y - rr * 0.4, rr * 0.1, p.x, p.y, rr);
        g.addColorStop(0, `rgba(235,238,245,${fog})`);
        g.addColorStop(0.6, `rgba(150,160,180,${fog})`);
        g.addColorStop(1, `rgba(60,66,82,${fog})`);
        ctx.fillStyle = g;
        ctx.beginPath();
        ctx.arc(p.x, p.y, rr, 0, Math.PI * 2);
        ctx.fill();
      }

      // faint brand rim glow
      ctx.strokeStyle = brand + "22";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(cx, cy, R * 1.12, 0, Math.PI * 2);
      ctx.stroke();

      if (!reduce) {
        t += 0.006;
        raf = requestAnimationFrame(draw);
      }
    };
    draw();
    return () => cancelAnimationFrame(raf);
  }, [size]);

  return (
    <canvas
      ref={canvasRef}
      width={size}
      height={size}
      style={{ width: size, height: size }}
      aria-hidden="true"
    />
  );
}
