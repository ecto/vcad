/**
 * Viewport + HUD for a decoded compressed field (see field-overlay-store).
 *
 * `CompressedFieldSlice` lives inside the viewport's Z-up kernel group and
 * draws one z-slice as a colormapped plane at its true mm position, plus
 * the field's domain box. `CompressedFieldHud` is a DOM panel with the
 * slice slider, colormap legend, the stream's max-error claim, and the
 * decode verification (hash vs native, measured error, timings).
 */
import { useEffect, useMemo } from "react";
import * as THREE from "three";
import { useFieldOverlayStore } from "@/stores/field-overlay-store";
import { turbo } from "@/lib/compressed-field";

export function CompressedFieldSlice() {
  const field = useFieldOverlayStore((s) => s.field);
  const slice = useFieldOverlayStore((s) => s.slice);

  const texture = useMemo(() => {
    if (!field) return null;
    const [, ny, nx] = field.meta.dims_zyx;
    const { min, max } = field.meta;
    const rgba = new Uint8Array(nx * ny * 4);
    const off = slice * nx * ny;
    for (let i = 0; i < nx * ny; i++) {
      const [r, g, b] = turbo((field.data[off + i]! - min) / (max - min || 1));
      rgba.set([r, g, b, 255], i * 4);
    }
    const t = new THREE.DataTexture(rgba, nx, ny, THREE.RGBAFormat);
    t.magFilter = THREE.NearestFilter;
    t.colorSpace = THREE.SRGBColorSpace;
    t.needsUpdate = true;
    return t;
  }, [field, slice]);

  useEffect(() => () => texture?.dispose(), [texture]);

  if (!field || !texture) return null;
  const [nz, ny, nx] = field.meta.dims_zyx;
  const [ox, oy, oz] = field.meta.origin_mm;
  const [vx, vy, vz] = field.meta.voxel_mm;
  const sx = nx * vx;
  const sy = ny * vy;
  const sz = nz * vz;
  const z = oz + (slice + 0.5) * vz;
  return (
    <group>
      {/* PlaneGeometry lies in XY; texture row 0 (y = 0) is at the bottom. */}
      <mesh position={[ox + sx / 2, oy + sy / 2, z]} renderOrder={10}>
        <planeGeometry args={[sx, sy]} />
        <meshBasicMaterial map={texture} side={THREE.DoubleSide} toneMapped={false} />
      </mesh>
      <lineSegments position={[ox + sx / 2, oy + sy / 2, oz + sz / 2]}>
        <edgesGeometry args={[new THREE.BoxGeometry(sx, sy, sz)]} />
        <lineBasicMaterial color="#888" />
      </lineSegments>
    </group>
  );
}

const fmt = (ms: number) => `${ms.toFixed(1)} ms`;

export function CompressedFieldHud() {
  const { field, slice, loading, error, setSlice, clear } = useFieldOverlayStore();
  if (!field && !loading && !error) return null;
  if (!field) {
    return (
      <div className="fixed bottom-10 left-4 z-50 bg-surface/95 border border-border px-3 py-2 text-xs font-mono">
        {loading ? "decoding field…" : `field error: ${error}`}
      </div>
    );
  }
  const { meta, timings } = field;
  const [nz, ny, nx] = meta.dims_zyx;
  const rawBytes = nx * ny * nz * 4;
  const gradient = Array.from({ length: 11 }, (_, i) => `rgb(${turbo(i / 10).join(",")})`).join(",");
  const zmm = meta.origin_mm[2] + (slice + 0.5) * meta.voxel_mm[2];
  return (
    <div
      className="fixed bottom-10 left-4 z-50 w-80 bg-surface/95 border border-border px-3 py-2 text-xs font-mono space-y-1"
      data-testid="compressed-field-hud"
    >
      <div className="flex justify-between">
        <span className="font-semibold">
          {meta.field} · {field.name}
        </span>
        <button onClick={clear} className="opacity-60 hover:opacity-100">
          ×
        </button>
      </div>
      <div className="h-3 w-full" style={{ background: `linear-gradient(to right, ${gradient})` }} />
      <div className="flex justify-between">
        <span>{meta.min.toFixed(2)}</span>
        <span>{meta.max.toFixed(2)} {meta.bound.units}</span>
      </div>
      <label className="flex items-center gap-2">
        z
        <input
          type="range"
          min={0}
          max={nz - 1}
          value={slice}
          onChange={(e) => setSlice(Number(e.target.value))}
          className="flex-1"
        />
        <span>{zmm.toFixed(2)} mm</span>
      </label>
      <div>
        error bound: |Δ| ≤ {meta.bound.value.toExponential(3)} {meta.bound.units}
        {field.measuredMaxAbs !== undefined && (
          <> · measured {field.measuredMaxAbs.toExponential(3)}</>
        )}
      </div>
      <div>
        {(field.streamBytes / 1024).toFixed(1)} KB of {(rawBytes / 1024).toFixed(0)} KB (
        {(rawBytes / field.streamBytes).toFixed(1)}×) · {meta.format ?? "tc"} · {field.backend}
      </div>
      <div>
        hash {field.hash}{" "}
        {field.hashMatch === null ? "(no native hash)" : field.hashMatch ? "= native ✓" : `≠ native ${meta.native_fnv1a} ✗`}
      </div>
      <div className="opacity-70">
        fetch {fmt(timings.fetchMs)} · init {fmt(timings.initMs)} · decode {fmt(timings.decodeMs)} · total{" "}
        {fmt(timings.totalMs)}
      </div>
    </div>
  );
}
