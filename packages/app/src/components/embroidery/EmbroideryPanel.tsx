import { useState, useMemo, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { Scissors } from "@phosphor-icons/react/dist/ssr/Scissors";
import { Palette } from "@phosphor-icons/react/dist/ssr/Palette";
import { Eye } from "@phosphor-icons/react/dist/ssr/Eye";
import { Export } from "@phosphor-icons/react/dist/ssr/Export";
import { TextT } from "@phosphor-icons/react/dist/ssr/TextT";
import {
  useEmbroideryStore,
  formatEmbroideryDuration,
} from "@/stores/embroidery-store";
import { useDocumentStore } from "@vcad/core";
import { downloadBlob } from "@/lib/download";

type Tab = "create" | "design" | "preview" | "export";

type StitchType = "running" | "satin" | "fill";

export function EmbroideryPanel() {
  const [activeTab, setActiveTab] = useState<Tab>("create");

  const closePanel = useEmbroideryStore((s) => s.closePanel);
  const pattern = useEmbroideryStore((s) => s.pattern);
  const stats = useEmbroideryStore((s) => s.stats);
  const error = useEmbroideryStore((s) => s.error);
  const selectedFormat = useEmbroideryStore((s) => s.selectedFormat);
  const setSelectedFormat = useEmbroideryStore((s) => s.setSelectedFormat);
  const fileName = useEmbroideryStore((s) => s.fileName);
  const patternJson = useEmbroideryStore((s) => s.patternJson);

  // Create tab state
  const [createText, setCreateText] = useState("Hello");
  const [createHeight, setCreateHeight] = useState(10);
  const [createColor, setCreateColor] = useState("#000000");
  const [createStitchType, setCreateStitchType] = useState<StitchType>("running");
  const [generating, setGenerating] = useState(false);

  const viewBox = useMemo(() => {
    if (!pattern?.stitchPaths?.length) return "0 0 100 100";
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const path of pattern.stitchPaths) {
      for (const [x, y] of path.points) {
        if (x < minX) minX = x;
        if (y < minY) minY = y;
        if (x > maxX) maxX = x;
        if (y > maxY) maxY = y;
      }
    }
    const pad = 2;
    return `${minX - pad} ${minY - pad} ${maxX - minX + pad * 2} ${maxY - minY + pad * 2}`;
  }, [pattern]);

  const hexToRgb = (hex: string): [number, number, number] => {
    const r = parseInt(hex.slice(1, 3), 16);
    const g = parseInt(hex.slice(3, 5), 16);
    const b = parseInt(hex.slice(5, 7), 16);
    return [r, g, b];
  };

  const handleGenerate = useCallback(async () => {
    if (!createText.trim()) return;
    setGenerating(true);
    const store = useEmbroideryStore.getState();
    store.setError(null);

    const res = await useDocumentStore.getState().addTextEmbroidery({
      text: createText,
      height: createHeight,
      color: hexToRgb(createColor),
      stitchType: createStitchType,
    });

    if (res) {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const result = res.result as any;
      store.setFileName("Text Embroidery");
      store.setPattern({
        stitchCount: result.stats.stitchCount,
        colorCount: result.stats.colorCount,
        width: result.stats.width,
        height: result.stats.height,
        threads: result.threads,
        stitchPaths: result.stitchPaths,
      });
      store.setStats(result.stats);
      store.setPatternJson(result.patternJson);
      setActiveTab("preview");
    } else {
      store.setError("Failed to generate stitches from text");
    }
    setGenerating(false);
  }, [createText, createHeight, createColor, createStitchType]);

  const handleExport = useCallback(async (format: "pes" | "dst") => {
    if (!patternJson) return;
    try {
      const wasm = await import("@vcad/kernel-wasm");
      const bytes = format === "pes"
        ? wasm.writeEmbroideryPes(patternJson)
        : wasm.writeEmbroideryDst(patternJson);
      downloadBlob(new Blob([bytes as unknown as BlobPart]), `pattern.${format}`);
    } catch (err) {
      console.error(`Failed to export ${format}:`, err);
      useEmbroideryStore.getState().setError(`Export failed: ${err}`);
    }
  }, [patternJson]);

  const tabs: { id: Tab; label: string; icon: typeof Palette }[] = [
    { id: "create", label: "Create", icon: TextT },
    { id: "design", label: "Design", icon: Palette },
    { id: "preview", label: "Preview", icon: Eye },
    { id: "export", label: "Export", icon: Export },
  ];

  return (
    <div className="fixed right-0 top-0 z-40 flex h-full w-80 flex-col border-l border-border bg-surface shadow-xl">
      {/* Header */}
      <div className="flex items-center justify-between border-b border-border px-4 py-3">
        <div className="flex items-center gap-2">
          <Scissors size={18} className="text-accent" />
          <span className="text-sm font-medium text-text">Embroidery</span>
          {fileName && (
            <span className="text-xs text-text-muted truncate max-w-[120px]">{fileName}</span>
          )}
        </div>
        <button
          onClick={closePanel}
          className="rounded p-1 hover:bg-hover transition-colors"
        >
          <X size={16} className="text-text-muted" />
        </button>
      </div>

      {/* Tabs */}
      <div className="flex border-b border-border">
        {tabs.map(({ id, label, icon: Icon }) => (
          <button
            key={id}
            onClick={() => setActiveTab(id)}
            className={`flex-1 flex items-center justify-center gap-1.5 px-3 py-2 text-xs transition-colors ${
              activeTab === id
                ? "border-b-2 border-accent text-accent"
                : "text-text-muted hover:text-text"
            }`}
          >
            <Icon size={14} />
            {label}
          </button>
        ))}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {error && (
          <div className="mb-3 rounded bg-danger/10 px-3 py-2 text-xs text-danger">
            {error}
          </div>
        )}

        {activeTab === "create" && (
          <div className="space-y-4">
            <div>
              <label className="text-xs font-medium text-text-muted mb-1 block">Text</label>
              <textarea
                value={createText}
                onChange={(e) => setCreateText(e.target.value)}
                className="w-full rounded border border-border bg-hover/50 px-3 py-2 text-sm text-text placeholder:text-text-muted focus:border-accent focus:outline-none resize-none"
                rows={3}
                placeholder="Enter text..."
              />
            </div>

            <div>
              <label className="text-xs font-medium text-text-muted mb-1 block">
                Height: {createHeight} mm
              </label>
              <input
                type="range"
                min={3}
                max={50}
                step={0.5}
                value={createHeight}
                onChange={(e) => setCreateHeight(Number(e.target.value))}
                className="w-full accent-accent"
              />
            </div>

            <div>
              <label className="text-xs font-medium text-text-muted mb-1 block">Thread Color</label>
              <div className="flex items-center gap-2">
                <input
                  type="color"
                  value={createColor}
                  onChange={(e) => setCreateColor(e.target.value)}
                  className="h-8 w-8 cursor-pointer rounded border border-border"
                />
                <span className="text-xs text-text-muted">{createColor}</span>
              </div>
            </div>

            <div>
              <label className="text-xs font-medium text-text-muted mb-1 block">Stitch Type</label>
              <div className="flex gap-2">
                {(["running", "satin", "fill"] as const).map((st) => (
                  <button
                    key={st}
                    onClick={() => setCreateStitchType(st)}
                    className={`flex-1 rounded px-3 py-2 text-xs font-medium transition-colors ${
                      createStitchType === st
                        ? "bg-accent text-white"
                        : "bg-hover text-text-muted hover:text-text"
                    }`}
                  >
                    {st.charAt(0).toUpperCase() + st.slice(1)}
                  </button>
                ))}
              </div>
            </div>

            <button
              onClick={handleGenerate}
              disabled={generating || !createText.trim()}
              className="w-full rounded bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {generating ? "Generating..." : "Generate Stitches"}
            </button>
          </div>
        )}

        {activeTab === "design" && !pattern && !error && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <Scissors size={32} className="text-text-muted mb-2" />
            <p className="text-sm text-text-muted">No pattern loaded</p>
            <p className="text-xs text-text-muted mt-1">
              Use the Create tab or drop a .pes/.dst file
            </p>
          </div>
        )}

        {activeTab === "design" && pattern && (
          <div className="space-y-4">
            <div>
              <h3 className="text-xs font-medium text-text-muted mb-2">Thread Palette</h3>
              <div className="space-y-1">
                {pattern.threads.map((thread, i) => (
                  <div key={i} className="flex items-center gap-2 px-2 py-1 rounded hover:bg-hover/50">
                    <div
                      className="w-4 h-4 rounded-full border border-border"
                      style={{
                        backgroundColor: `rgb(${thread.color[0]}, ${thread.color[1]}, ${thread.color[2]})`,
                      }}
                    />
                    <span className="text-xs text-text">{thread.name}</span>
                    <span className="text-xs text-text-muted ml-auto">#{i + 1}</span>
                  </div>
                ))}
              </div>
            </div>

            {stats && (
              <div>
                <h3 className="text-xs font-medium text-text-muted mb-2">Statistics</h3>
                <div className="grid grid-cols-2 gap-2">
                  <StatItem label="Stitches" value={stats.stitchCount.toLocaleString()} />
                  <StatItem label="Colors" value={String(stats.colorCount)} />
                  <StatItem label="Width" value={`${stats.width.toFixed(1)} mm`} />
                  <StatItem label="Height" value={`${stats.height.toFixed(1)} mm`} />
                  <StatItem label="Thread" value={`${stats.threadLength.toFixed(0)} mm`} />
                  <StatItem label="Time" value={formatEmbroideryDuration(stats.estimatedTimeSeconds)} />
                </div>
              </div>
            )}
          </div>
        )}

        {activeTab === "preview" && pattern && (
          <div className="flex flex-col items-center">
            <svg
              viewBox={viewBox}
              className="w-full aspect-square bg-white rounded border border-border"
              style={{ maxHeight: "400px" }}
            >
              {pattern.stitchPaths.map((path, i) => (
                <polyline
                  key={i}
                  points={path.points.map(([x, y]) => `${x},${y}`).join(" ")}
                  fill="none"
                  stroke={`rgb(${path.color[0]}, ${path.color[1]}, ${path.color[2]})`}
                  strokeWidth={0.3}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              ))}
            </svg>
            <p className="text-xs text-text-muted mt-2">
              {pattern.stitchPaths.length} stitch path{pattern.stitchPaths.length !== 1 ? "s" : ""}
            </p>
          </div>
        )}

        {activeTab === "preview" && !pattern && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <Eye size={32} className="text-text-muted mb-2" />
            <p className="text-sm text-text-muted">No pattern to preview</p>
          </div>
        )}

        {activeTab === "export" && pattern && (
          <div className="space-y-4">
            <div>
              <h3 className="text-xs font-medium text-text-muted mb-2">Format</h3>
              <div className="flex gap-2">
                {(["pes", "dst"] as const).map((fmt) => (
                  <button
                    key={fmt}
                    onClick={() => setSelectedFormat(fmt)}
                    className={`flex-1 rounded px-3 py-2 text-xs font-medium transition-colors ${
                      selectedFormat === fmt
                        ? "bg-accent text-white"
                        : "bg-hover text-text-muted hover:text-text"
                    }`}
                  >
                    {fmt.toUpperCase()}
                  </button>
                ))}
              </div>
            </div>

            <div>
              <h3 className="text-xs font-medium text-text-muted mb-2">Hoop Size</h3>
              <div className="rounded bg-hover/50 px-3 py-2 text-xs text-text">
                {stats ? `${stats.width.toFixed(0)} x ${stats.height.toFixed(0)} mm` : "\u2014"}
              </div>
            </div>

            <button
              className="w-full rounded bg-accent px-4 py-2 text-sm font-medium text-white hover:bg-accent/90 transition-colors disabled:opacity-50"
              disabled={!patternJson}
              onClick={() => handleExport(selectedFormat)}
            >
              Export {selectedFormat.toUpperCase()}
            </button>
          </div>
        )}

        {activeTab === "export" && !pattern && (
          <div className="flex flex-col items-center justify-center py-12 text-center">
            <Export size={32} className="text-text-muted mb-2" />
            <p className="text-sm text-text-muted">No pattern to export</p>
          </div>
        )}
      </div>

      {pattern && (
        <div className="border-t border-border px-4 py-3 flex gap-2">
          <button
            className="flex-1 rounded bg-accent px-3 py-1.5 text-xs font-medium text-white hover:bg-accent/90 transition-colors disabled:opacity-50"
            disabled={!patternJson}
            onClick={() => handleExport("pes")}
          >
            Export PES
          </button>
          <button
            className="flex-1 rounded bg-hover px-3 py-1.5 text-xs font-medium text-text hover:bg-hover/80 transition-colors disabled:opacity-50"
            disabled={!patternJson}
            onClick={() => handleExport("dst")}
          >
            Export DST
          </button>
        </div>
      )}
    </div>
  );
}

function StatItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded bg-hover/50 px-2 py-1.5">
      <div className="text-[10px] text-text-muted">{label}</div>
      <div className="text-xs font-medium text-text">{value}</div>
    </div>
  );
}
