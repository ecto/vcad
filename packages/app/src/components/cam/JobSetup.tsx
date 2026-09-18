/**
 * Setup: which part, out of what blank, with which cutter, at what feeds.
 *
 * The contour comes from the document itself rather than from a DXF the
 * operator has to keep in step — `camOutlineFromDocument` re-evaluates the
 * document *inside* the kernel so it can section the B-rep's raw tessellation.
 * The alternative is the export mesh, whose repair pass can tear a tangent
 * fillet by 0.4 mm: a quarter of a slot mouth, and a wall the cutter would
 * follow.
 */
import { useCallback } from "react";
import { Scissors } from "@phosphor-icons/react/dist/ssr/Scissors";
import { Lightning } from "@phosphor-icons/react/dist/ssr/Lightning";
import { Warning } from "@phosphor-icons/react/dist/ssr/Warning";
import { useDocumentStore, useEngineStore } from "@vcad/core";
import { useCamJobStore } from "@/stores/cam-job-store";
import { NumberField } from "./NumberField";

/** The materials the panel offers, by the kernel's own stable ids. */
const MATERIALS: Array<{ id: string; name: string }> = [
  { id: "aluminium-6061-t6", name: "Aluminium 6061-T6" },
  { id: "aluminium-7075-t6", name: "Aluminium 7075-T6" },
  { id: "brass-c360", name: "Brass C360" },
  { id: "copper-c110", name: "Copper C110" },
  { id: "steel-mild-1018", name: "Mild steel 1018" },
  { id: "acrylic-pmma", name: "Acrylic (PMMA)" },
  { id: "pom-acetal", name: "POM / acetal" },
  { id: "fr4", name: "FR4" },
  { id: "mdf", name: "MDF" },
  { id: "plywood", name: "Plywood" },
  { id: "hardwood", name: "Hardwood" },
];

export function JobSetup() {
  const engine = useEngineStore((s) => s.engine);
  const document = useDocumentStore((s) => s.document);

  const outline = useCamJobStore((s) => s.outline);
  const torn = useCamJobStore((s) => s.torn);
  const outlineError = useCamJobStore((s) => s.outlineError);
  const sectioning = useCamJobStore((s) => s.sectioning);
  const partIndex = useCamJobStore((s) => s.partIndex);
  const stock = useCamJobStore((s) => s.stock);
  const tool = useCamJobStore((s) => s.tool);
  const values = useCamJobStore((s) => s.values);
  const material = useCamJobStore((s) => s.material);
  const recommendation = useCamJobStore((s) => s.recommendation);
  const recommendError = useCamJobStore((s) => s.recommendError);
  const unmachinable = useCamJobStore((s) => s.unmachinable);
  const derivationRefusal = useCamJobStore((s) => s.derivationRefusal);

  const setStock = useCamJobStore((s) => s.setStock);
  const setTool = useCamJobStore((s) => s.setTool);
  const setValues = useCamJobStore((s) => s.setValues);
  const setMaterial = useCamJobStore((s) => s.setMaterial);
  const setPartIndex = useCamJobStore((s) => s.setPartIndex);
  const sectionPart = useCamJobStore((s) => s.sectionPart);
  const recommendFeeds = useCamJobStore((s) => s.recommendFeeds);

  const parts = document.roots.filter((r) => r.visible !== false);

  const onSection = useCallback(() => {
    if (!engine) return;
    sectionPart(engine, document);
  }, [engine, document, sectionPart]);

  const onRecommend = useCallback(() => {
    if (!engine) return;
    recommendFeeds(engine);
  }, [engine, recommendFeeds]);

  const dial = recommendation?.dial_spindle ? recommendation.recommendation.dial : null;

  return (
    <div className="space-y-4 text-sm">
      {/* ---- the part ---- */}
      <section className="space-y-2">
        <h3 className="text-xs uppercase tracking-wide text-text-muted">Part</h3>
        {parts.length > 1 && (
          <label className="block">
            <span className="text-xs text-text-muted">Which part</span>
            <select
              className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
              value={partIndex}
              onChange={(e) => setPartIndex(Number(e.target.value))}
            >
              {parts.map((root, i) => (
                <option key={String(root.root)} value={i}>
                  {document.nodes[String(root.root)]?.name ?? `Part ${i + 1}`}
                </option>
              ))}
            </select>
          </label>
        )}
        <button
          className="w-full flex items-center justify-center gap-2 py-2 border border-border rounded hover:bg-hover disabled:opacity-50"
          onClick={onSection}
          disabled={!engine || sectioning || parts.length === 0}
        >
          <Scissors size={14} />
          {sectioning ? "Sectioning…" : outline ? "Re-section part" : "Take contour from part"}
        </button>

        {torn && (
          <div className="px-2 py-2 bg-error/10 text-error text-xs rounded space-y-1">
            <div className="flex items-start gap-1">
              <Warning size={14} className="mt-0.5 shrink-0" />
              <span>{torn.error}</span>
            </div>
            <div className="text-text-muted">
              {torn.gaps.length} gap(s), widest{" "}
              {Math.max(...torn.gaps.map((g) => g.distance)).toFixed(4)} mm. The outline is
              shown as it is: closing it here would hand the cutter a wall the part does
              not have.
            </div>
          </div>
        )}
        {outlineError && (
          <div className="px-2 py-2 bg-error/10 text-error text-xs rounded">
            {outlineError}
          </div>
        )}
        {outline && (
          <dl className="grid grid-cols-2 gap-x-2 gap-y-1 text-xs text-text-muted">
            <dt>Sectioned at</dt>
            <dd className="text-text">Z {outline.z.toFixed(3)} (auto)</dd>
            <dt>Openings</dt>
            <dd className="text-text">{outline.regions[0]?.holes.length ?? 0}</dd>
            <dt>Part height</dt>
            <dd className="text-text">
              {outline.suggested_stock_thickness.toFixed(2)} mm
            </dd>
            <dt>Mesh</dt>
            <dd
              className="text-text"
              title={
                outline.mesh_source === "raw_tessellation"
                  ? "Sectioned from the B-rep's raw tessellation — the exact one."
                  : "Sectioned from the repaired export mesh, which can tear by a few tenths where tangent faces meet."
              }
            >
              {outline.mesh_source === "raw_tessellation" ? "B-rep (exact)" : outline.mesh_source}
            </dd>
          </dl>
        )}
        {derivationRefusal && (
          <div className="px-2 py-2 bg-error/10 text-error text-xs rounded">
            {derivationRefusal}
            {unmachinable.length > 0 && (
              <ul className="mt-1 text-text-muted">
                {unmachinable.map((h, i) => (
                  <li key={i}>
                    Ø{h.diameter.toFixed(2)} at X {h.center[0].toFixed(1)}, Y{" "}
                    {h.center[1].toFixed(1)}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </section>

      {/* ---- stock ---- */}
      <section className="space-y-2">
        <h3 className="text-xs uppercase tracking-wide text-text-muted">Stock</h3>
        <div className="grid grid-cols-2 gap-2">
          <NumberField
            label="Thickness"
            unit="mm"
            value={stock.thickness}
            min={0}
            onCommit={(thickness) => setStock({ thickness })}
          />
          <NumberField
            label="Margin"
            unit="mm"
            value={stock.margin}
            min={0}
            onCommit={(margin) => setStock({ margin })}
          />
        </div>
        <label className="flex items-center gap-2 text-xs">
          <input
            type="checkbox"
            checked={stock.spoilboard !== null}
            onChange={(e) => setStock({ spoilboard: e.target.checked ? 12 : null })}
          />
          <span>Sacrificial board under the stock</span>
        </label>
        {stock.spoilboard !== null && (
          <NumberField
            label="Spoilboard"
            unit="mm"
            value={stock.spoilboard}
            min={0}
            onCommit={(spoilboard) => setStock({ spoilboard })}
            title="Any cut that goes past the underside of the stock is refused unless a board at least that thick is declared."
          />
        )}
      </section>

      {/* ---- tool ---- */}
      <section className="space-y-2">
        <h3 className="text-xs uppercase tracking-wide text-text-muted">Tool</h3>
        <div className="grid grid-cols-2 gap-2">
          <NumberField
            label="Diameter"
            unit="mm"
            value={tool.diameter}
            min={0}
            step={0.1}
            onCommit={(diameter) => setTool({ diameter })}
          />
          <NumberField
            label="Flutes"
            value={tool.flutes}
            step={1}
            min={1}
            onCommit={(flutes) => setTool({ flutes: Math.round(flutes) })}
          />
          <NumberField
            label="Flute length"
            unit="mm"
            value={tool.flute_length}
            min={0}
            onCommit={(flute_length) => setTool({ flute_length })}
            title="A cut deeper than the flutes are long rubs the shank against the wall. The job checks this."
          />
          <NumberField
            label="Stickout"
            unit="mm"
            value={tool.stickout ?? 20}
            min={0}
            onCommit={(stickout) => setTool({ stickout })}
          />
        </div>
      </section>

      {/* ---- material and feeds ---- */}
      <section className="space-y-2">
        <h3 className="text-xs uppercase tracking-wide text-text-muted">Material and feeds</h3>
        <label className="block">
          <span className="text-xs text-text-muted">Material</span>
          <select
            className="w-full bg-surface border border-border rounded px-2 py-1 text-sm"
            value={material}
            onChange={(e) => setMaterial(e.target.value)}
          >
            {MATERIALS.map((m) => (
              <option key={m.id} value={m.id}>
                {m.name}
              </option>
            ))}
          </select>
        </label>
        <button
          className="w-full flex items-center justify-center gap-2 py-1.5 border border-border rounded hover:bg-hover disabled:opacity-50 text-sm"
          onClick={onRecommend}
          disabled={!engine}
        >
          <Lightning size={14} />
          Recommend feeds
        </button>
        {recommendError && (
          <div className="px-2 py-2 bg-error/10 text-error text-xs rounded">
            {recommendError}
          </div>
        )}
        {dial && (
          <div className="px-2 py-2 bg-surface-secondary text-xs rounded">
            <div className="font-medium">Set the router dial to {dial}.</div>
            <div className="text-text-muted">
              This spindle has a manual speed dial, so the S word in the program does
              nothing — {recommendation?.recommendation.rpm.toFixed(0)} rpm only happens if
              you set it by hand.
            </div>
          </div>
        )}
        {recommendation?.recommendation.notes
          .filter((n) => n.level !== "Info")
          .map((n, i) => (
            <div key={i} className="px-2 py-1.5 bg-warning/10 text-xs rounded">
              <span className="text-text-muted">{String(n.level).toLowerCase()}: </span>
              {n.text}
            </div>
          ))}
        <div className="grid grid-cols-2 gap-2">
          <NumberField
            label="Feed"
            unit="mm/min"
            step={10}
            value={values.feed}
            min={0}
            onCommit={(feed) => setValues({ feed })}
          />
          <NumberField
            label="Plunge"
            unit="mm/min"
            step={10}
            value={values.plunge}
            min={0}
            onCommit={(plunge) => setValues({ plunge })}
          />
          <NumberField
            label="Spindle"
            unit="rpm"
            step={500}
            value={values.rpm}
            min={0}
            onCommit={(rpm) => setValues({ rpm })}
          />
          <NumberField
            label="Stepdown"
            unit="mm"
            value={values.stepdown}
            min={0}
            onCommit={(stepdown) => setValues({ stepdown })}
          />
          <NumberField
            label="Stepover"
            unit="mm"
            value={values.stepover}
            min={0}
            onCommit={(stepover) => setValues({ stepover })}
          />
          <NumberField
            label="Bottom allowance"
            unit="mm"
            value={values.bottom_allowance}
            step={0.05}
            onCommit={(bottom_allowance) => setValues({ bottom_allowance })}
            title="Onion skin. Negative breaks through into a declared spoilboard; without one the job is refused."
          />
        </div>
      </section>
    </div>
  );
}
