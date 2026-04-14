import { Image } from "@phosphor-icons/react/dist/ssr/Image";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { CircleHalf } from "@phosphor-icons/react/dist/ssr/CircleHalf";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { CaretLeft } from "@phosphor-icons/react/dist/ssr/CaretLeft";
import { Globe } from "@phosphor-icons/react/dist/ssr/Globe";
import { useDocumentStore, useUiStore } from "@vcad/core";
import type { EnvironmentPreset, Background } from "@vcad/ir";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

const PRESETS: EnvironmentPreset[] = [
  "studio", "dawn", "sunset", "night", "warehouse", "park", "city", "forest", "apartment", "neutral",
];

function rgbToHex(color: [number, number, number]): string {
  return `#${color.map((c) => Math.round(c * 255).toString(16).padStart(2, "0")).join("")}`;
}

function hexToRgb(hex: string): [number, number, number] {
  return [
    parseInt(hex.slice(1, 3), 16) / 255,
    parseInt(hex.slice(3, 5), 16) / 255,
    parseInt(hex.slice(5, 7), 16) / 255,
  ];
}

function SectionHeader({ children }: { children: string }) {
  return (
    <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted pt-2 pb-1">
      {children}
    </div>
  );
}

/**
 * Inspector pane for the scene itself — environment, background, and (later)
 * lights, units, grid, etc. Mirrors the structure of PropertyPanel's part
 * variants so it slots cleanly into the same drill-down flow.
 */
export function SceneInspector() {
  const document = useDocumentStore((s) => s.document);
  const updateEnvironment = useDocumentStore((s) => s.updateEnvironment);
  const updateBackground = useDocumentStore((s) => s.updateBackground);
  const setSidebarPane = useUiStore((s) => s.setSidebarPane);
  const setInspectorTarget = useUiStore((s) => s.setInspectorTarget);

  const env = document.scene?.environment;
  const envValue = env?.type === "None" ? "none" : env?.type === "Preset" ? env.preset : "none";
  const bg: Background = document.scene?.background ?? { type: "Environment" };

  const bgTypes: { type: Background["type"]; icon: typeof Image; label: string }[] = [
    { type: "Environment", icon: Image, label: "Use environment" },
    { type: "Solid", icon: Cube, label: "Solid color" },
    { type: "Gradient", icon: CircleHalf, label: "Gradient" },
    { type: "Transparent", icon: X, label: "Transparent" },
  ];

  function setBgType(type: Background["type"]) {
    switch (type) {
      case "Environment":
        updateBackground({ type: "Environment" });
        break;
      case "Solid":
        updateBackground({ type: "Solid", color: [0.9, 0.9, 0.9] });
        break;
      case "Gradient":
        updateBackground({ type: "Gradient", top: [0.15, 0.15, 0.18], bottom: [0.05, 0.05, 0.06] });
        break;
      case "Transparent":
        updateBackground({ type: "Transparent" });
        break;
    }
  }

  function back() {
    setInspectorTarget(null);
    setSidebarPane("tree");
  }

  return (
    <div className="w-full h-full flex flex-col bg-surface min-h-0">
      <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-border px-3">
        <div className="flex items-center gap-2 min-w-0">
          <button
            onClick={back}
            className="flex h-6 w-6 -ml-1 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
            aria-label="Back to tree"
            title="Back to tree"
          >
            <CaretLeft size={14} />
          </button>
          <Globe size={13} className="text-brand shrink-0" />
          <span className="text-xs font-medium text-text truncate">Scene</span>
        </div>
        <button
          onClick={() => setInspectorTarget(null)}
          className="flex h-6 w-6 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          aria-label="Close"
        >
          <X size={14} />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-3 scrollbar-thin">
        <SectionHeader>Environment</SectionHeader>
        <select
          value={envValue}
          onChange={(e) => {
            const val = e.target.value;
            if (val === "none") {
              updateEnvironment({ type: "None" });
            } else {
              updateEnvironment({
                type: "Preset",
                preset: val as EnvironmentPreset,
                intensity: env?.type === "Preset" ? env.intensity ?? 0.4 : 0.4,
              });
            }
          }}
          className="w-full px-2 py-1 text-xs bg-card border border-border text-text outline-none focus:border-brand capitalize"
        >
          <option value="none" className="bg-surface">none</option>
          {PRESETS.map((p) => (
            <option key={p} value={p} className="bg-surface">
              {p}
            </option>
          ))}
        </select>

        <SectionHeader>Background</SectionHeader>
        <div className="flex items-center gap-1">
          {bgTypes.map(({ type, icon: TypeIcon, label }) => (
            <Tooltip key={type} content={label}>
              <button
                onClick={() => setBgType(type)}
                className={cn(
                  "flex h-7 w-7 items-center justify-center border",
                  bg.type === type
                    ? "bg-brand/15 border-brand/40 text-brand"
                    : "bg-card border-border text-text-muted hover:text-text",
                )}
              >
                <TypeIcon size={14} />
              </button>
            </Tooltip>
          ))}
        </div>
        {bg.type === "Solid" && (
          <div className="flex items-center gap-2 mt-2">
            <Tooltip content="Background color">
              <input
                type="color"
                value={rgbToHex(bg.color)}
                onChange={(e) => updateBackground({ type: "Solid", color: hexToRgb(e.target.value) })}
                className="w-8 h-8 border border-border cursor-pointer shrink-0"
              />
            </Tooltip>
            <span className="text-[10px] text-text-muted font-mono">{rgbToHex(bg.color)}</span>
          </div>
        )}
        {bg.type === "Gradient" && (
          <div className="flex items-center gap-2 mt-2">
            <Tooltip content="Top color">
              <input
                type="color"
                value={rgbToHex(bg.top)}
                onChange={(e) => updateBackground({ ...bg, top: hexToRgb(e.target.value) })}
                className="w-8 h-8 border border-border cursor-pointer shrink-0"
              />
            </Tooltip>
            <span className="text-[10px] text-text-muted">→</span>
            <Tooltip content="Bottom color">
              <input
                type="color"
                value={rgbToHex(bg.bottom)}
                onChange={(e) => updateBackground({ ...bg, bottom: hexToRgb(e.target.value) })}
                className="w-8 h-8 border border-border cursor-pointer shrink-0"
              />
            </Tooltip>
          </div>
        )}
      </div>
    </div>
  );
}
