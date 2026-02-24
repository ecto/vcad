import { useState, useMemo } from "react";
import { ScrubInput } from "@/components/ui/scrub-input";
import { useDocumentStore, getNodeEmbroideryDesign } from "@vcad/core";
import type { EmbroideryPatternPartInfo, StitchPartInfo } from "@vcad/core";
import type { StitchFillType } from "@vcad/ir";
import { DEFAULT_FILL_PARAMS } from "@vcad/ir";

function SectionHeader({ children }: { children: string }) {
  return (
    <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted pt-2 pb-1">
      {children}
    </div>
  );
}

function Divider() {
  return <div className="border-t border-border my-2" />;
}

interface EmbroideryPropertiesProps {
  part: EmbroideryPatternPartInfo | StitchPartInfo;
}

export function EmbroideryProperties({ part }: EmbroideryPropertiesProps) {
  const document = useDocumentStore((s) => s.document);
  const setThreadColor = useDocumentStore((s) => s.setThreadColor);
  const setThreadName = useDocumentStore((s) => s.setThreadName);
  const setStitchGroupFillParams = useDocumentStore((s) => s.setStitchGroupFillParams);
  const optimizeJumpStitches = useDocumentStore((s) => s.optimizeJumpStitches);

  const [expandedGroup, setExpandedGroup] = useState<number | null>(null);

  const design = useMemo(
    () => getNodeEmbroideryDesign(document, part.patternNodeId),
    [document, part.patternNodeId],
  );

  if (!design) {
    return (
      <div className="text-xs text-text-muted py-1">
        No embroidery design found on node {part.patternNodeId}
      </div>
    );
  }

  const nodeId = part.patternNodeId;

  // --- Compute stats ---
  const totalStitches = design.stitch_groups.reduce((s, g) => s + g.stitches.length, 0);
  const jumpCount = design.stitch_groups.length > 0 ? design.stitch_groups.length - 1 : 0;
  const groupCount = design.stitch_groups.length;
  // ~0.06s per 1000 stitches at typical machine speed
  const estimatedMinutes = Math.round((totalStitches * 0.06) / 60);

  // Per-thread stitch counts
  const threadStitchCounts = useMemo(() => {
    const counts: number[] = new Array(design.threads.length).fill(0);
    for (const g of design.stitch_groups) {
      if (g.thread_index < counts.length) {
        counts[g.thread_index]! += g.stitches.length;
      }
    }
    return counts;
  }, [design]);

  return (
    <div className="space-y-1">
      {/* Stats */}
      <SectionHeader>Statistics</SectionHeader>
      <div className="grid grid-cols-2 gap-1">
        <StatCell label="Stitches" value={totalStitches.toLocaleString()} />
        <StatCell label="Jumps" value={String(jumpCount)} />
        <StatCell label="Groups" value={String(groupCount)} />
        <StatCell label="Est. time" value={estimatedMinutes > 0 ? `${estimatedMinutes} min` : "<1 min"} />
      </div>

      <Divider />

      {/* Thread palette */}
      <SectionHeader>Threads</SectionHeader>
      <div className="space-y-1">
        {design.threads.map((thread, idx) => (
          <div key={idx} className="flex items-center gap-1.5">
            <input
              type="color"
              value={rgbToHex(thread.color)}
              onChange={(e) => {
                const c = hexToRgb(e.target.value);
                setThreadColor(nodeId, idx, c);
              }}
              className="w-5 h-5 p-0 border border-border rounded cursor-pointer bg-transparent"
              title={`Thread ${idx + 1} color`}
            />
            <input
              type="text"
              value={thread.name}
              onChange={(e) => setThreadName(nodeId, idx, e.target.value)}
              className="flex-1 min-w-0 text-xs bg-transparent border-b border-transparent hover:border-border focus:border-accent focus:outline-none text-text px-0.5 py-0.5"
            />
            <span className="text-[10px] text-text-muted tabular-nums shrink-0">
              {threadStitchCounts[idx]?.toLocaleString()}
            </span>
          </div>
        ))}
      </div>

      <Divider />

      {/* Stitch groups */}
      <SectionHeader>Stitch Groups</SectionHeader>
      <div className="space-y-0.5">
        {design.stitch_groups.map((group, gIdx) => {
          const thread = design.threads[group.thread_index];
          const isExpanded = expandedGroup === gIdx;
          const fp = group.fill_params ?? DEFAULT_FILL_PARAMS;

          return (
            <div key={gIdx} className="border border-border rounded">
              {/* Group header */}
              <button
                onClick={() => setExpandedGroup(isExpanded ? null : gIdx)}
                className="w-full flex items-center gap-1.5 px-2 py-1 hover:bg-hover/50 text-left"
              >
                <div
                  className="w-3 h-3 rounded-full shrink-0 border border-border/50"
                  style={{
                    backgroundColor: thread
                      ? `rgb(${thread.color[0]}, ${thread.color[1]}, ${thread.color[2]})`
                      : "#888",
                  }}
                />
                <span className="text-xs text-text truncate">
                  Group {gIdx + 1}
                </span>
                <span className="text-[10px] text-text-muted ml-auto tabular-nums shrink-0">
                  {group.stitches.length}
                </span>
                <svg
                  className={`w-3 h-3 text-text-muted transition-transform shrink-0 ${isExpanded ? "rotate-180" : ""}`}
                  fill="none"
                  viewBox="0 0 24 24"
                  stroke="currentColor"
                  strokeWidth={2}
                >
                  <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
                </svg>
              </button>

              {/* Expanded fill params */}
              {isExpanded && (
                <div className="px-2 pb-2 space-y-1 border-t border-border">
                  {/* Fill type */}
                  <div className="flex items-center gap-2 pt-1">
                    <span className="text-[10px] text-text-muted w-10">Type</span>
                    <select
                      value={fp.fill_type}
                      onChange={(e) =>
                        setStitchGroupFillParams(nodeId, gIdx, {
                          fill_type: e.target.value as StitchFillType,
                        })
                      }
                      className="flex-1 text-xs bg-hover border border-border rounded px-1.5 py-0.5 text-text"
                    >
                      <option value="manual">Manual</option>
                      <option value="fill">Fill</option>
                      <option value="satin">Satin</option>
                      <option value="running">Running</option>
                    </select>
                  </div>

                  <ScrubInput
                    label="Angle"
                    value={fp.angle_deg}
                    min={0}
                    max={360}
                    step={5}
                    onChange={(v) => setStitchGroupFillParams(nodeId, gIdx, { angle_deg: v })}
                    unit="°"
                  />
                  <ScrubInput
                    label="Density"
                    value={fp.density_mm}
                    min={0.1}
                    max={2.0}
                    step={0.05}
                    onChange={(v) => setStitchGroupFillParams(nodeId, gIdx, { density_mm: v })}
                    unit="mm"
                  />
                  <ScrubInput
                    label="Max length"
                    value={fp.max_stitch_length_mm}
                    min={1}
                    max={20}
                    step={0.5}
                    onChange={(v) => setStitchGroupFillParams(nodeId, gIdx, { max_stitch_length_mm: v })}
                    unit="mm"
                  />

                  {/* Underlay checkbox */}
                  <label className="flex items-center gap-1.5 pt-0.5 cursor-pointer">
                    <input
                      type="checkbox"
                      checked={fp.underlay}
                      onChange={(e) =>
                        setStitchGroupFillParams(nodeId, gIdx, { underlay: e.target.checked })
                      }
                      className="w-3 h-3 accent-accent"
                    />
                    <span className="text-xs text-text">Underlay</span>
                  </label>
                </div>
              )}
            </div>
          );
        })}
      </div>

      <Divider />

      {/* Optimize button */}
      <button
        onClick={() => optimizeJumpStitches(nodeId)}
        className="w-full rounded bg-hover px-3 py-1.5 text-xs font-medium text-text hover:bg-hover/80 transition-colors"
      >
        Optimize Jump Stitches
      </button>
    </div>
  );
}

function StatCell({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded bg-hover/50 px-2 py-1">
      <div className="text-[10px] text-text-muted">{label}</div>
      <div className="text-xs font-medium text-text tabular-nums">{value}</div>
    </div>
  );
}

function rgbToHex(c: [number, number, number]): string {
  return "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
}

function hexToRgb(hex: string): [number, number, number] {
  const h = hex.replace("#", "");
  return [
    parseInt(h.slice(0, 2), 16),
    parseInt(h.slice(2, 4), 16),
    parseInt(h.slice(4, 6), 16),
  ];
}
