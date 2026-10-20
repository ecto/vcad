/**
 * Viewport + HUD for decoded compressed fields (see field-overlay-store).
 *
 * `CompressedFieldSlice` lives inside the viewport's Z-up kernel group:
 * - volume layer, "skin" mode: the part's exposed voxel faces colored by
 *   the active field (the overlay *on* the part);
 * - volume layer, "slice" mode: one colormapped cut plane along x/y/z
 *   inside a ghosted part;
 * - plane layer (2D, e.g. a busbar cross-section): the image placed on its
 *   world plane inside a ghosted part.
 * Hovering reads the field under the cursor: value ± the stream's bound.
 *
 * `CompressedFieldHud` is the DOM panel: layer tabs, legend, mode/axis/
 * slice controls, the probe readout, and per-layer verification (hash vs
 * native, measured error vs bound, decode times).
 */
import { useEffect, useMemo } from "react";
import * as THREE from "three";
import { useThree, type ThreeEvent } from "@react-three/fiber";
import { activeLayer, useFieldOverlayStore, type Axis } from "@/stores/field-overlay-store";
import { buildSkin, isPlane, solidMask, turbo, type DecodedField } from "@/lib/compressed-field";

type V3 = [number, number, number];

/** An image of the field on a world plane: pixel (iu, iv) → cell, and its corners. */
interface PlaneImage {
  w: number;
  h: number;
  cell: (iu: number, iv: number) => number;
  corners: [V3, V3, V3, V3]; // (u0,v0) (u1,v0) (u1,v1) (u0,v1)
}

function planeImage(f: DecodedField, axis: Axis, k: number): PlaneImage {
  const [nz, ny, nx] = f.meta.dims_zyx;
  const [ox, oy, oz] = f.meta.origin_mm;
  const [vx, vy, vz] = f.meta.voxel_mm;
  const [ex, ey, ez] = [ox + nx * vx, oy + ny * vy, oz + nz * vz];
  if (f.meta.plane) {
    // 2D field: columns → first plane axis, rows → second; origin_mm/voxel_mm are world xyz.
    const at = f.meta.plane === "yz" ? ox : f.meta.plane === "xz" ? oy : oz;
    const [u0, v0, du, dv] =
      f.meta.plane === "yz" ? [oy, oz, vy, vz] : f.meta.plane === "xz" ? [ox, oz, vx, vz] : [ox, oy, vx, vy];
    const [u1, v1] = [u0 + nx * du, v0 + ny * dv];
    const P = (u: number, v: number): V3 =>
      f.meta.plane === "yz" ? [at, u, v] : f.meta.plane === "xz" ? [u, at, v] : [u, v, at];
    return { w: nx, h: ny, cell: (iu, iv) => iv * nx + iu, corners: [P(u0, v0), P(u1, v0), P(u1, v1), P(u0, v1)] };
  }
  if (axis === "z") {
    const z = oz + (k + 0.5) * vz;
    return { w: nx, h: ny, cell: (iu, iv) => (k * ny + iv) * nx + iu, corners: [[ox, oy, z], [ex, oy, z], [ex, ey, z], [ox, ey, z]] };
  }
  if (axis === "y") {
    const y = oy + (k + 0.5) * vy;
    return { w: nx, h: nz, cell: (iu, iv) => (iv * ny + k) * nx + iu, corners: [[ox, y, oz], [ex, y, oz], [ex, y, ez], [ox, y, ez]] };
  }
  const x = ox + (k + 0.5) * vx;
  return { w: ny, h: nz, cell: (iu, iv) => (iv * ny + iu) * nx + k, corners: [[x, oy, oz], [x, ey, oz], [x, ey, ez], [x, oy, ez]] };
}

const norm = (r: [number, number], v: number) => (v - r[0]) / (r[1] - r[0] || 1);

function cellPos(f: DecodedField, c: number): V3 {
  const [, ny, nx] = f.meta.dims_zyx;
  const [i, j, k] = [c % nx, Math.floor(c / nx) % ny, Math.floor(c / (nx * ny))];
  const o = f.meta.origin_mm;
  const v = f.meta.voxel_mm;
  if (f.meta.plane === "yz") return [o[0], o[1] + (i + 0.5) * v[1], o[2] + (j + 0.5) * v[2]];
  return [o[0] + (i + 0.5) * v[0], o[1] + (j + 0.5) * v[1], o[2] + (k + 0.5) * v[2]];
}

function FieldPlane({
  field,
  range,
  img,
  layerId,
  mask,
}: {
  field: DecodedField;
  range: [number, number];
  img: PlaneImage;
  layerId: string;
  /** Solid mask: void cells are drawn transparent and don't probe. */
  mask?: Uint8Array;
}) {
  const setProbe = useFieldOverlayStore((s) => s.setProbe);
  const { texture, geometry } = useMemo(() => {
    const rgba = new Uint8Array(img.w * img.h * 4);
    for (let iv = 0; iv < img.h; iv++)
      for (let iu = 0; iu < img.w; iu++) {
        const cell = img.cell(iu, iv);
        const [r, g, b] = turbo(norm(range, field.data[cell]!));
        rgba.set([r, g, b, !mask || mask[cell] ? 255 : 0], (iv * img.w + iu) * 4);
      }
    const t = new THREE.DataTexture(rgba, img.w, img.h, THREE.RGBAFormat);
    t.magFilter = THREE.NearestFilter;
    t.colorSpace = THREE.SRGBColorSpace;
    t.needsUpdate = true;
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.Float32BufferAttribute(img.corners.flat(), 3));
    g.setAttribute("uv", new THREE.Float32BufferAttribute([0, 0, 1, 0, 1, 1, 0, 1], 2));
    g.setIndex([0, 1, 2, 0, 2, 3]);
    return { texture: t, geometry: g };
  }, [field, range, img, mask]);
  useEffect(() => () => (texture.dispose(), geometry.dispose()), [texture, geometry]);
  const onMove = (e: ThreeEvent<PointerEvent>) => {
    if (!e.uv) return;
    e.stopPropagation();
    const iu = Math.min(img.w - 1, Math.floor(e.uv.x * img.w));
    const iv = Math.min(img.h - 1, Math.floor(e.uv.y * img.h));
    const cell = img.cell(iu, iv);
    if (mask && !mask[cell]) return setProbe(null);
    setProbe({ layerId, cell, value: field.data[cell]!, pos: cellPos(field, cell) });
  };
  return (
    <mesh geometry={geometry} renderOrder={10} onPointerMove={onMove} onPointerOut={() => setProbe(null)}>
      <meshBasicMaterial map={texture} side={THREE.DoubleSide} toneMapped={false} alphaTest={0.5} />
    </mesh>
  );
}

function FieldSkin({ field, range, ghost, layerId }: { field: DecodedField; range: [number, number]; ghost: boolean; layerId: string }) {
  const boxes = useFieldOverlayStore((s) => s.stack?.geometry?.boxes);
  const setProbe = useFieldOverlayStore((s) => s.setProbe);
  const skin = useMemo(() => (boxes ? buildSkin(field.meta, solidMask(field.meta, boxes)) : null), [field.meta, boxes]);
  const geometry = useMemo(() => {
    if (!skin) return null;
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(skin.positions, 3));
    g.setIndex(new THREE.BufferAttribute(skin.index, 1));
    g.computeVertexNormals();
    const colors = new Float32Array(skin.positions.length);
    const c = new THREE.Color();
    skin.cell.forEach((cell, q) => {
      const [r, gg, b] = turbo(norm(range, field.data[cell]!));
      c.setRGB(r / 255, gg / 255, b / 255, THREE.SRGBColorSpace);
      for (let v = 0; v < 4; v++) colors.set([c.r, c.g, c.b], (4 * q + v) * 3);
    });
    g.setAttribute("color", new THREE.BufferAttribute(colors, 3));
    return g;
  }, [skin, field, range]);
  useEffect(() => () => geometry?.dispose(), [geometry]);
  if (!skin || !geometry) return null;
  if (ghost) {
    return (
      <mesh geometry={geometry} renderOrder={5}>
        <meshBasicMaterial color="#9aa4b2" transparent opacity={0.12} depthWrite={false} side={THREE.DoubleSide} />
      </mesh>
    );
  }
  const onMove = (e: ThreeEvent<PointerEvent>) => {
    if (e.faceIndex == null) return;
    e.stopPropagation();
    const cell = skin.cell[Math.floor(e.faceIndex / 2)]!;
    setProbe({ layerId, cell, value: field.data[cell]!, pos: cellPos(field, cell) });
  };
  return (
    <mesh geometry={geometry} onPointerMove={onMove} onPointerOut={() => setProbe(null)}>
      <meshLambertMaterial vertexColors />
    </mesh>
  );
}

/** Frame the camera on a newly loaded stack (once per stack). */
function useFrameStack() {
  const stack = useFieldOverlayStore((s) => s.stack);
  const first = useFieldOverlayStore((s) => s.layers[0]);
  const camera = useThree((s) => s.camera);
  const controls = useThree((s) => s.controls) as unknown as { target: THREE.Vector3; update: () => void } | null;
  useEffect(() => {
    if (!stack || !first || !controls) return;
    const m = first.field.meta;
    const [nz, ny, nx] = m.dims_zyx;
    const lo = m.origin_mm;
    const hi = [lo[0] + nx * m.voxel_mm[0], lo[1] + ny * m.voxel_mm[1], lo[2] + nz * m.voxel_mm[2]];
    const boxes = stack.geometry?.boxes;
    const bmin = boxes ? [0, 1, 2].map((a) => Math.min(...boxes.map((b) => b.min[a]!))) : lo;
    const bmax = boxes ? [0, 1, 2].map((a) => Math.max(...boxes.map((b) => b.max[a]!))) : hi;
    const c = [0, 1, 2].map((a) => (bmin[a]! + bmax[a]!) / 2);
    const r = Math.hypot(...[0, 1, 2].map((a) => bmax[a]! - bmin[a]!)) / 2;
    // Kernel Z-up → display Y-up: (x, y, z) → (x, z, -y).
    const target = new THREE.Vector3(c[0], c[2], -c[1]!);
    const dir = new THREE.Vector3(0.9, 0.75, 1.1).normalize();
    camera.position.copy(target).addScaledVector(dir, r * 2.6);
    controls.target.copy(target);
    camera.lookAt(target);
    controls.update();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stack, controls]);
}

export function CompressedFieldSlice() {
  useFrameStack();
  const layer = useFieldOverlayStore(activeLayer);
  const hasGeometry = useFieldOverlayStore((s) => !!s.stack?.geometry);
  const mode = useFieldOverlayStore((s) => s.mode);
  const axis = useFieldOverlayStore((s) => s.axis);
  const slice = useFieldOverlayStore((s) => s.slice);
  // A plane layer's skin uses any volume layer's grid (they are co-registered).
  const volume = useFieldOverlayStore((s) => s.layers.find((l) => !isPlane(l.field.meta)));
  const boxes = useFieldOverlayStore((s) => s.stack?.geometry?.boxes);
  const img = useMemo(() => (layer ? planeImage(layer.field, axis, slice) : null), [layer, axis, slice]);
  const meta = layer?.field.meta;
  const mask = useMemo(() => (meta && boxes && !meta.plane ? solidMask(meta, boxes) : undefined), [meta, boxes]);
  if (!layer || !img) return null;
  const plane = isPlane(layer.field.meta);
  const showSkin = hasGeometry && !plane && mode === "skin";
  return (
    <group>
      {hasGeometry && volume && <FieldSkin field={showSkin ? layer.field : volume.field} range={showSkin ? layer.range : volume.range} ghost={!showSkin} layerId={layer.id} />}
      {!showSkin && <FieldPlane field={layer.field} range={layer.range} img={img} layerId={layer.id} mask={mask} />}
    </group>
  );
}

const fmt = (ms: number) => `${ms.toFixed(1)} ms`;
const num = (v: number) => (Math.abs(v) >= 100 ? v.toFixed(1) : Math.abs(v) >= 1 ? v.toFixed(3) : v.toPrecision(4));

export function CompressedFieldHud() {
  const s = useFieldOverlayStore();
  const layer = activeLayer(s);
  if (!layer && !s.loading && !s.error) return null;
  if (!layer) {
    return (
      <div className="fixed bottom-10 left-4 z-50 bg-surface/95 border border-border px-3 py-2 text-xs font-mono">
        {s.loading ? "decoding fields…" : `field error: ${s.error}`}
      </div>
    );
  }
  const { meta } = layer.field;
  const units = meta.units ?? meta.bound.units;
  const plane = isPlane(meta);
  const [nz, ny, nx] = meta.dims_zyx;
  const axisN = s.axis === "x" ? nx : s.axis === "y" ? ny : nz;
  const gradient = Array.from({ length: 11 }, (_, i) => `rgb(${turbo(i / 10).join(",")})`).join(",");
  const streamBytes = s.layers.reduce((a, l) => a + l.field.streamBytes, 0);
  const rawBytes = s.layers.reduce((a, l) => a + l.field.data.length * 4, 0);
  const btn = (on: boolean) =>
    `px-1.5 py-0.5 border ${on ? "bg-text/15 border-text/50 text-text" : "border-border opacity-70 hover:opacity-100"}`;
  return (
    <div
      className="fixed bottom-10 left-4 z-50 w-96 bg-surface/95 border border-border px-3 py-2 text-xs font-mono space-y-1.5"
      data-testid="compressed-field-hud"
    >
      <div className="flex justify-between">
        <span className="font-semibold">{s.stack?.title}</span>
        <button onClick={s.clear} className="opacity-60 hover:opacity-100">
          ×
        </button>
      </div>
      {s.stack?.notes && <div className="opacity-60">{s.stack.notes}</div>}
      {s.layers.length > 1 && (
        <div className="flex flex-wrap gap-1">
          {s.layers.map((l) => (
            <button key={l.id} className={btn(l.id === s.active)} onClick={() => s.setActive(l.id)}>
              {l.label}
            </button>
          ))}
        </div>
      )}
      <div className="h-3 w-full" style={{ background: `linear-gradient(to right, ${gradient})` }} />
      <div className="flex justify-between">
        <span>{num(layer.range[0])}</span>
        <span>
          {num(layer.range[1])} {units}
        </span>
      </div>
      {!plane && (
        <div className="flex items-center gap-1">
          {s.stack?.geometry && (
            <>
              <button className={btn(s.mode === "skin")} onClick={() => s.setMode("skin")}>
                surface
              </button>
              <button className={btn(s.mode === "slice")} onClick={() => s.setMode("slice")}>
                slice
              </button>
            </>
          )}
          {s.mode === "slice" && (
            <>
              {(["x", "y", "z"] as const).map((a) => (
                <button key={a} className={btn(s.axis === a)} onClick={() => s.setAxis(a)}>
                  {a}
                </button>
              ))}
              <input
                type="range"
                min={0}
                max={axisN - 1}
                value={s.slice}
                onChange={(e) => s.setSlice(Number(e.target.value))}
                className="flex-1"
              />
            </>
          )}
        </div>
      )}
      {plane && <div className="opacity-70">2D field on the {meta.plane} plane</div>}
      <div className="min-h-4">
        {s.probe && s.probe.layerId === layer.id ? (
          <>
            {num(s.probe.value)} ± {meta.bound.value.toExponential(1)} {units} @ (
            {s.probe.pos.map((p) => p.toFixed(1)).join(", ")}) mm
          </>
        ) : (
          <span className="opacity-50">hover the part to probe</span>
        )}
      </div>
      <table className="w-full">
        <tbody>
          {s.layers.map((l) => (
            <tr key={l.id} className={l.id === s.active ? "" : "opacity-60"}>
              <td>{l.id}</td>
              <td>{(l.field.streamBytes / 1024).toFixed(1)}K</td>
              <td title={`hash ${l.field.hash}`}>{l.field.hashMatch ? "hash ✓" : l.field.hashMatch === null ? "hash ?" : "hash ✗"}</td>
              <td>
                |Δ|≤{l.field.meta.bound.value.toExponential(1)}
                {l.field.measuredMaxAbs !== undefined && (l.field.measuredMaxAbs <= l.field.meta.bound.value ? " ✓" : " ✗")}
              </td>
              <td>{fmt(l.field.timings.decodeMs)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <div className="opacity-70">
        {(streamBytes / 1024).toFixed(1)} KB of {(rawBytes / 1024).toFixed(0)} KB ({(rawBytes / streamBytes).toFixed(0)}×) ·{" "}
        {layer.field.backend} · {meta.format ?? "tc"} · load {fmt(s.totalMs)}
      </div>
    </div>
  );
}
