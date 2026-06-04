/**
 * Per-item ECAD inspector, rendered inline inside the main PropertyPanel.
 *
 * Task 3 (modeless PCB editing): ECAD selections — component / footprint /
 * net / trace / via / pad — flow into the SAME contextual inspector as
 * mechanical parts instead of a separate floating panel. This component is
 * the shared body; `ElectronicsPropertyPanel` keeps the board-overview HUD.
 *
 * Reads the current selection from the electronics store and adapts. Returns
 * `null` for the `none` selection (the board HUD owns that state), so callers
 * can mount it unconditionally.
 */

import { useState } from "react";
import { useDocumentStore, useCoreElectronicsStore, getNodePcb } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";

export function EcadFeatureInspector() {
  const selection = useElectronicsStore((s) => s.selection);
  const netlist = useElectronicsStore((s) => s.netlist);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;

  if (selection.type === "component" || selection.type === "footprint") {
    return <ComponentFootprintPanel key={selection.ref} compRef={selection.ref} />;
  }

  if (selection.type === "net") {
    const netName = selection.netId;
    const netInfo = netlist?.nets.find((n) => n.name === netName);
    const traceCount = pcb?.traces.filter((t) => t.net === netName).length ?? 0;
    const totalLength = (pcb?.traces ?? [])
      .filter((t) => t.net === netName)
      .reduce((sum, t) => {
        const dx = t.end.x - t.start.x;
        const dy = t.end.y - t.start.y;
        return sum + Math.hypot(dx, dy);
      }, 0);

    return (
      <Section>
        <div className="font-medium text-brand mb-2">{netName}</div>
        <div className="space-y-1 text-text-muted">
          <Row label="Pads" value={String(netInfo?.connections.length ?? 0)} />
          <Row label="Traces" value={String(traceCount)} />
          <Row label="Total Length" value={`${totalLength.toFixed(2)}mm`} />
          {netInfo && netInfo.connections.length > 0 && (
            <div className="pt-1 border-t border-border mt-1">
              <div className="text-[10px] text-text-muted mb-0.5">Connections:</div>
              {netInfo.connections.slice(0, 8).map((c, i) => (
                <div key={i} className="text-[10px]">
                  {c.component_ref}.{c.pin_number}
                </div>
              ))}
              {netInfo.connections.length > 8 && (
                <div className="text-[9px] text-text-muted">
                  +{netInfo.connections.length - 8} more
                </div>
              )}
            </div>
          )}
        </div>
      </Section>
    );
  }

  if (selection.type === "trace") {
    const trace = pcb?.traces[selection.idx];
    if (!trace) return null;
    const dx = trace.end.x - trace.start.x;
    const dy = trace.end.y - trace.start.y;
    const len = Math.hypot(dx, dy);

    return (
      <Section>
        <div className="flex justify-between items-center mb-2">
          <span className="font-medium text-text">Trace</span>
          <button
            className="px-1.5 py-0.5 text-[10px] text-danger hover:bg-danger/10 rounded transition-colors"
            onClick={() => {
              const bId = useCoreElectronicsStore.getState().activeBoardNodeId;
              if (bId != null) useDocumentStore.getState().removeTrace(bId, selection.idx);
              useElectronicsStore.getState().select({ type: "none" });
            }}
          >
            Delete
          </button>
        </div>
        <div className="space-y-1 text-text-muted">
          <Row label="Net" value={trace.net} />
          <Row label="Layer" value={trace.layer} />
          <Row label="Width" value={`${trace.width}mm`} />
          <Row label="Length" value={`${len.toFixed(2)}mm`} />
        </div>
      </Section>
    );
  }

  if (selection.type === "via") {
    const via = pcb?.vias[selection.idx];
    if (!via) return null;

    return (
      <Section>
        <div className="flex justify-between items-center mb-2">
          <span className="font-medium text-text">Via</span>
          <button
            className="px-1.5 py-0.5 text-[10px] text-danger hover:bg-danger/10 rounded transition-colors"
            onClick={() => {
              const bId = useCoreElectronicsStore.getState().activeBoardNodeId;
              if (bId != null) useDocumentStore.getState().removeVia(bId, selection.idx);
              useElectronicsStore.getState().select({ type: "none" });
            }}
          >
            Delete
          </button>
        </div>
        <div className="space-y-1 text-text-muted">
          <Row label="Net" value={via.net} />
          <Row label="Diameter" value={`${via.diameter}mm`} />
          <Row label="Drill" value={`${via.drill}mm`} />
          <Row label="Layers" value={`${via.startLayer} - ${via.endLayer}`} />
          <Row
            label="Position"
            value={`${via.position.x.toFixed(2)}, ${via.position.y.toFixed(2)}`}
          />
        </div>
      </Section>
    );
  }

  if (selection.type === "pad") {
    return (
      <Section>
        <div className="font-medium text-text mb-2">
          {selection.fpRef}.{selection.padNum}
        </div>
        <div className="space-y-1 text-text-muted">
          <Row label="Net" value={selection.net} />
        </div>
      </Section>
    );
  }

  return null;
}

// ---------------------------------------------------------------------------
// Editable component/footprint panel
// ---------------------------------------------------------------------------

function ComponentFootprintPanel({ compRef }: { compRef: string }) {
  const netlist = useElectronicsStore((s) => s.netlist);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const cpDocument = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(cpDocument, activeBoardNodeId) : null;
  const schematic = cpDocument.schematic;

  const schComp = schematic?.components.find((c) => c.ref === compRef);
  const schIdx = schematic?.components.findIndex((c) => c.ref === compRef) ?? -1;
  const fp = pcb?.footprints.find((f) => f.ref === compRef);
  const fpIdx = pcb?.footprints.findIndex((f) => f.ref === compRef) ?? -1;

  const [editValue, setEditValue] = useState(schComp?.value ?? "");
  const [editFpX, setEditFpX] = useState(String(fp?.position.x ?? 0));
  const [editFpY, setEditFpY] = useState(String(fp?.position.y ?? 0));
  const [editFpRot, setEditFpRot] = useState(String(fp?.rotation ?? 0));

  const nets = new Set<string>();
  if (netlist) {
    for (const net of netlist.nets) {
      for (const conn of net.connections) {
        if (conn.component_ref === compRef) {
          nets.add(net.name);
          break;
        }
      }
    }
  }

  const commitValue = () => {
    if (schIdx >= 0 && editValue !== schComp?.value) {
      useDocumentStore.getState().updateSchematicComponent(schIdx, { value: editValue }, activeBoardNodeId ?? undefined);
    }
  };

  const commitFpPosition = () => {
    const x = parseFloat(editFpX);
    const y = parseFloat(editFpY);
    if (fpIdx >= 0 && !isNaN(x) && !isNaN(y)) {
      if (activeBoardNodeId != null) useDocumentStore.getState().moveFootprint(activeBoardNodeId, fpIdx, { x, y, z: 0 });
    }
  };

  const commitFpRotation = () => {
    const rot = parseFloat(editFpRot);
    if (fpIdx >= 0 && !isNaN(rot) && rot !== (fp?.rotation ?? 0)) {
      // rotateFootprint adds to current, so we compute delta
      const delta = rot - (fp?.rotation ?? 0);
      if (activeBoardNodeId != null) useDocumentStore.getState().rotateFootprint(activeBoardNodeId, fpIdx, delta);
    }
  };

  return (
    <Section>
      <div className="font-medium text-text mb-2">{compRef}</div>
      <div className="space-y-1.5 text-text-muted">
        {/* Editable value */}
        {schComp && (
          <div className="flex justify-between items-center">
            <span>Value</span>
            <input
              type="text"
              value={editValue}
              onChange={(e) => setEditValue(e.target.value)}
              onBlur={commitValue}
              onKeyDown={(e) => e.key === "Enter" && commitValue()}
              className="w-20 text-[11px] text-text bg-transparent border border-border rounded px-1 py-0.5 text-right"
            />
          </div>
        )}
        {fp && (
          <>
            <Row label="Footprint" value={fp.footprintName} />
            {/* Editable position */}
            <div className="flex justify-between items-center">
              <span>X</span>
              <input
                type="number"
                value={editFpX}
                onChange={(e) => setEditFpX(e.target.value)}
                onBlur={commitFpPosition}
                onKeyDown={(e) => e.key === "Enter" && commitFpPosition()}
                className="w-16 text-[11px] text-text bg-transparent border border-border rounded px-1 py-0.5 text-right"
                step={0.1}
              />
            </div>
            <div className="flex justify-between items-center">
              <span>Y</span>
              <input
                type="number"
                value={editFpY}
                onChange={(e) => setEditFpY(e.target.value)}
                onBlur={commitFpPosition}
                onKeyDown={(e) => e.key === "Enter" && commitFpPosition()}
                className="w-16 text-[11px] text-text bg-transparent border border-border rounded px-1 py-0.5 text-right"
                step={0.1}
              />
            </div>
            {/* Editable rotation */}
            <div className="flex justify-between items-center">
              <span>Rotation</span>
              <input
                type="number"
                value={editFpRot}
                onChange={(e) => setEditFpRot(e.target.value)}
                onBlur={commitFpRotation}
                onKeyDown={(e) => e.key === "Enter" && commitFpRotation()}
                className="w-16 text-[11px] text-text bg-transparent border border-border rounded px-1 py-0.5 text-right"
                step={90}
              />
            </div>
            <Row label="Side" value={fp.front !== false ? "Front" : "Back"} />
            <Row label="Pads" value={String(fp.pads.length)} />
          </>
        )}
        {nets.size > 0 && (
          <div className="pt-1 border-t border-border mt-1">
            <div className="text-[10px] text-text-muted mb-0.5">Nets:</div>
            <div className="flex flex-wrap gap-1">
              {[...nets].map((n) => (
                <span
                  key={n}
                  className="px-1 py-0.5 bg-brand/10 text-brand rounded text-[9px] cursor-pointer hover:bg-brand/20"
                  onClick={() =>
                    useElectronicsStore.getState().select({ type: "net", netId: n })
                  }
                >
                  {n}
                </span>
              ))}
            </div>
          </div>
        )}
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------

/** Inline panel section — matches the contextual-inspector padding. */
function Section({ children }: { children: React.ReactNode }) {
  return <div className="p-3 text-[11px]">{children}</div>;
}

export function Row({
  label,
  value,
  danger,
}: {
  label: string;
  value: string;
  danger?: boolean;
}) {
  return (
    <div className="flex justify-between">
      <span>{label}</span>
      <span className={danger ? "text-danger font-medium" : "text-text"}>
        {value}
      </span>
    </div>
  );
}
