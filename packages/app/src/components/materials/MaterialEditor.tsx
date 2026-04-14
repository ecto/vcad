/**
 * MaterialEditor - Dialog for creating/editing custom materials.
 */

import { useState, useMemo } from "react";
import { Dialog, DialogContent, DialogFooter } from "@/components/ui/dialog";
import { ScrubInput } from "@/components/ui/scrub-input";
import type { MaterialPreset, MaterialCategory } from "@/data/materials";

interface MaterialEditorProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSave: (material: Omit<MaterialPreset, "key">) => void;
  initialMaterial?: Partial<MaterialPreset>;
}

function hexToRgb(hex: string): [number, number, number] {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  return [r, g, b];
}

function rgbToHex(rgb: [number, number, number]): string {
  const r = Math.round(rgb[0] * 255)
    .toString(16)
    .padStart(2, "0");
  const g = Math.round(rgb[1] * 255)
    .toString(16)
    .padStart(2, "0");
  const b = Math.round(rgb[2] * 255)
    .toString(16)
    .padStart(2, "0");
  return `#${r}${g}${b}`;
}

function getPbrGradient(color: [number, number, number], metallic: number, roughness: number): string {
  const [r, g, b] = color.map((c) => Math.round(c * 255));
  const highlight = metallic > 0.5 ? 60 : 30;
  const shadow = roughness > 0.5 ? 40 : 20;

  const rHi = Math.min(255, r! + highlight);
  const gHi = Math.min(255, g! + highlight);
  const bHi = Math.min(255, b! + highlight);
  const rLo = Math.max(0, r! - shadow);
  const gLo = Math.max(0, g! - shadow);
  const bLo = Math.max(0, b! - shadow);

  return `radial-gradient(ellipse at 30% 30%,
    rgb(${rHi}, ${gHi}, ${bHi}) 0%,
    rgb(${r}, ${g}, ${b}) 50%,
    rgb(${rLo}, ${gLo}, ${bLo}) 100%
  )`;
}

export function MaterialEditor({
  open,
  onOpenChange,
  onSave,
  initialMaterial,
}: MaterialEditorProps) {
  const [name, setName] = useState(initialMaterial?.name ?? "Custom Material");
  const [colorHex, setColorHex] = useState(
    initialMaterial?.color ? rgbToHex(initialMaterial.color) : "#808080"
  );
  const [metallic, setMetallic] = useState(initialMaterial?.metallic ?? 0.0);
  const [roughness, setRoughness] = useState(initialMaterial?.roughness ?? 0.5);
  const [density, setDensity] = useState(initialMaterial?.density ?? 1000);

  const color = useMemo(() => hexToRgb(colorHex), [colorHex]);

  const previewGradient = useMemo(
    () => getPbrGradient(color, metallic, roughness),
    [color, metallic, roughness]
  );

  const handleSave = () => {
    onSave({
      name,
      category: "other" as MaterialCategory,
      color,
      metallic,
      roughness,
      density,
    });
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Custom Material" className="max-w-sm">
        <div className="space-y-4">
          {/* Preview swatch */}
          <div className="flex items-center gap-4">
            <div
              className="h-16 w-16 rounded-lg shadow-inner"
              style={{ background: previewGradient }}
            />
            <div className="flex-1 space-y-1">
              <label className="text-[10px] text-text-muted uppercase tracking-wider">
                Name
              </label>
              <input
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="w-full bg-card border border-border px-2 py-1 text-xs text-text outline-none hover:border-text-muted focus:border-brand"
              />
            </div>
          </div>

          {/* Color picker */}
          <div className="space-y-1">
            <label className="text-[10px] text-text-muted uppercase tracking-wider">
              Color
            </label>
            <div className="flex items-center gap-2">
              <input
                type="color"
                value={colorHex}
                onChange={(e) => setColorHex(e.target.value)}
                className="h-8 w-8 cursor-pointer border border-border rounded"
              />
              <input
                type="text"
                value={colorHex.toUpperCase()}
                onChange={(e) => {
                  const val = e.target.value;
                  if (/^#[0-9A-Fa-f]{6}$/.test(val)) {
                    setColorHex(val);
                  }
                }}
                className="flex-1 bg-card border border-border px-2 py-1 text-xs text-text font-mono outline-none hover:border-text-muted focus:border-brand"
              />
            </div>
          </div>

          {/* PBR properties */}
          <div className="space-y-2">
            <label className="text-[10px] text-text-muted uppercase tracking-wider">
              Surface Properties
            </label>
            <ScrubInput
              label="Metallic"
              value={metallic}
              onChange={setMetallic}
              step={0.01}
              min={0}
              max={1}
            />
            <ScrubInput
              label="Roughness"
              value={roughness}
              onChange={setRoughness}
              step={0.01}
              min={0}
              max={1}
            />
          </div>

          {/* Physical properties */}
          <div className="space-y-2">
            <label className="text-[10px] text-text-muted uppercase tracking-wider">
              Physical Properties
            </label>
            <ScrubInput
              label="Density"
              value={density}
              onChange={setDensity}
              step={10}
              min={1}
              max={25000}
              unit="kg/m³"
            />
          </div>
        </div>

        <DialogFooter>
          <button
            type="button"
            onClick={() => onOpenChange(false)}
            className="px-3 py-1.5 text-xs text-text-muted hover:text-text hover:bg-hover"
          >
            Cancel
          </button>
          <button
            type="button"
            onClick={handleSave}
            className="px-3 py-1.5 text-xs bg-brand text-white hover:bg-brand/90"
          >
            Save Material
          </button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
