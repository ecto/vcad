/**
 * Floating property panel for electronics workspace.
 * Adapts content to current selection type.
 */

import { useDocumentStore } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";

export function ElectronicsPropertyPanel() {
  const selection = useElectronicsStore((s) => s.selection);
  const netlist = useElectronicsStore((s) => s.netlist);
  const drcViolations = useElectronicsStore((s) => s.drcViolations);
  const ercViolations = useElectronicsStore((s) => s.ercViolations);
  const pcb = useDocumentStore((s) => s.document.pcb);
  const schematic = useDocumentStore((s) => s.document.schematic);

  if (selection.type === "none") {
    // Board overview
    return (
      <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
        <div className="font-medium text-text mb-2">Board Overview</div>
        <div className="space-y-1 text-text-muted">
          {pcb && (
            <>
              <Row label="Footprints" value={String(pcb.footprints.length)} />
              <Row label="Traces" value={String(pcb.traces.length)} />
              <Row label="Vias" value={String(pcb.vias.length)} />
              <Row label="Nets" value={String(pcb.nets.length)} />
              <Row
                label="Layers"
                value={String(pcb.stackup.layers.filter((l) => l.copperThickness).length)}
              />
            </>
          )}
          {schematic && (
            <Row label="Components" value={String(schematic.components.length)} />
          )}
          <div className="pt-1 border-t border-border mt-1">
            <Row
              label="DRC Errors"
              value={String(drcViolations.filter((v) => v.severity === "Error").length)}
              danger={drcViolations.some((v) => v.severity === "Error")}
            />
            <Row
              label="DRC Warnings"
              value={String(drcViolations.filter((v) => v.severity === "Warning").length)}
            />
            <Row
              label="ERC Issues"
              value={String(ercViolations.length)}
              danger={ercViolations.length > 0}
            />
          </div>
        </div>
      </div>
    );
  }

  if (selection.type === "component" || selection.type === "footprint") {
    const ref = selection.ref;
    const schComp = schematic?.components.find((c) => c.ref === ref);
    const fp = pcb?.footprints.find((f) => f.ref === ref);
    const nets = new Set<string>();
    if (netlist) {
      for (const net of netlist.nets) {
        for (const conn of net.connections) {
          if (conn.component_ref === ref) {
            nets.add(net.name);
            break;
          }
        }
      }
    }

    return (
      <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
        <div className="font-medium text-text mb-2">{ref}</div>
        <div className="space-y-1 text-text-muted">
          {schComp && <Row label="Value" value={schComp.value} />}
          {fp && (
            <>
              <Row label="Footprint" value={fp.footprintName} />
              <Row label="Position" value={`${fp.position.x.toFixed(2)}, ${fp.position.y.toFixed(2)}`} />
              <Row label="Rotation" value={`${fp.rotation ?? 0}deg`} />
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
                    className="px-1 py-0.5 bg-accent/10 text-accent rounded text-[9px] cursor-pointer hover:bg-accent/20"
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
      </div>
    );
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
      <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
        <div className="font-medium text-accent mb-2">{netName}</div>
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
      </div>
    );
  }

  if (selection.type === "trace") {
    const trace = pcb?.traces[selection.idx];
    if (!trace) return null;
    const dx = trace.end.x - trace.start.x;
    const dy = trace.end.y - trace.start.y;
    const len = Math.hypot(dx, dy);

    return (
      <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
        <div className="font-medium text-text mb-2">Trace</div>
        <div className="space-y-1 text-text-muted">
          <Row label="Net" value={trace.net} />
          <Row label="Layer" value={trace.layer} />
          <Row label="Width" value={`${trace.width}mm`} />
          <Row label="Length" value={`${len.toFixed(2)}mm`} />
        </div>
      </div>
    );
  }

  if (selection.type === "via") {
    const via = pcb?.vias[selection.idx];
    if (!via) return null;

    return (
      <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
        <div className="font-medium text-text mb-2">Via</div>
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
      </div>
    );
  }

  if (selection.type === "pad") {
    return (
      <div className="absolute top-3 right-3 w-56 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-3 text-[11px] pointer-events-auto">
        <div className="font-medium text-text mb-2">
          {selection.fpRef}.{selection.padNum}
        </div>
        <div className="space-y-1 text-text-muted">
          <Row label="Net" value={selection.net} />
        </div>
      </div>
    );
  }

  return null;
}

function Row({
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
