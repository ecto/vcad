import { Image } from "@phosphor-icons/react/dist/ssr/Image";
import { Cube } from "@phosphor-icons/react/dist/ssr/Cube";
import { CircleHalf } from "@phosphor-icons/react/dist/ssr/CircleHalf";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { cn } from "@/lib/utils";
import { Tooltip } from "@/components/ui/tooltip";
import { useDocumentStore } from "@vcad/core";
import type { EnvironmentPreset, Background } from "@vcad/ir";

// Environment presets
const PRESETS: EnvironmentPreset[] = [
  "studio", "dawn", "sunset", "night", "warehouse", "park", "city", "forest", "apartment", "neutral"
];

/** Convert RGB array to hex */
function rgbToHex(color: [number, number, number]): string {
  return `#${color.map((c) => Math.round(c * 255).toString(16).padStart(2, "0")).join("")}`;
}

/** Convert hex to RGB array */
function hexToRgb(hex: string): [number, number, number] {
  return [
    parseInt(hex.slice(1, 3), 16) / 255,
    parseInt(hex.slice(3, 5), 16) / 255,
    parseInt(hex.slice(5, 7), 16) / 255,
  ];
}

/** Labeled row */
function LabeledRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-3 px-2 py-1.5">
      <span className="text-[10px] text-text-muted/70 w-16 shrink-0">{label}</span>
      <div className="flex-1 flex items-center gap-2 min-w-0">{children}</div>
    </div>
  );
}

/** Main scene section */
export function SceneSection() {
  const document = useDocumentStore((s) => s.document);
  const updateEnvironment = useDocumentStore((s) => s.updateEnvironment);
  const updateBackground = useDocumentStore((s) => s.updateBackground);

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

  return (
    <div className="space-y-0.5 overflow-hidden">
      {/* Environment */}
      <LabeledRow label="Environment">
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
          className="flex-1 min-w-0 px-1.5 py-0.5 text-xs bg-surface/50 rounded border-none focus:outline-none cursor-pointer capitalize"
        >
          <option value="none" className="bg-surface">none</option>
          {PRESETS.map((p) => (
            <option key={p} value={p} className="bg-surface">
              {p}
            </option>
          ))}
        </select>
      </LabeledRow>

      {/* Background */}
      <LabeledRow label="Background">
        <div className="flex items-center gap-1">
          {bgTypes.map(({ type, icon: TypeIcon, label }) => (
            <Tooltip key={type} content={label}>
              <button
                onClick={() => setBgType(type)}
                className={cn(
                  "p-1.5 rounded",
                  bg.type === type
                    ? "bg-accent/20 text-accent"
                    : "bg-surface/30 opacity-60 hover:opacity-100"
                )}
              >
                <TypeIcon size={14} />
              </button>
            </Tooltip>
          ))}
        </div>
        {bg.type === "Solid" && (
          <Tooltip content="Background color">
            <input
              type="color"
              value={rgbToHex(bg.color)}
              onChange={(e) => updateBackground({ type: "Solid", color: hexToRgb(e.target.value) })}
              className="w-6 h-6 border border-border/50 cursor-pointer rounded shrink-0"
            />
          </Tooltip>
        )}
        {bg.type === "Gradient" && (
          <>
            <Tooltip content="Top color">
              <input
                type="color"
                value={rgbToHex(bg.top)}
                onChange={(e) => updateBackground({ ...bg, top: hexToRgb(e.target.value) })}
                className="w-6 h-6 border border-border/50 cursor-pointer rounded shrink-0"
              />
            </Tooltip>
            <Tooltip content="Bottom color">
              <input
                type="color"
                value={rgbToHex(bg.bottom)}
                onChange={(e) => updateBackground({ ...bg, bottom: hexToRgb(e.target.value) })}
                className="w-6 h-6 border border-border/50 cursor-pointer rounded shrink-0"
              />
            </Tooltip>
          </>
        )}
      </LabeledRow>
    </div>
  );
}
