/**
 * CategorySection - Collapsible section for a material category.
 */

import { useState } from "react";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { CaretDown } from "@phosphor-icons/react/dist/ssr/CaretDown";
import { Star } from "@phosphor-icons/react/dist/ssr/Star";
import { Tooltip } from "@/components/ui/tooltip";
import type { MaterialPreset, MaterialCategory } from "@/data/materials";
import { CATEGORY_LABELS } from "@/data/materials";
import { MaterialSwatch } from "./MaterialSwatch";
import { MaterialInfoPanel } from "./MaterialInfoPanel";

interface CategorySectionProps {
  category: MaterialCategory;
  materials: MaterialPreset[];
  selectedKey: string;
  favorites: string[];
  volumeMm3?: number;
  defaultOpen?: boolean;
  onSelect: (material: MaterialPreset) => void;
  onPreview: (material: MaterialPreset | null) => void;
  onToggleFavorite: (key: string) => void;
}

export function CategorySection({
  category,
  materials,
  selectedKey,
  favorites,
  volumeMm3,
  defaultOpen = false,
  onSelect,
  onPreview,
  onToggleFavorite,
}: CategorySectionProps) {
  const [open, setOpen] = useState(defaultOpen);
  const hasSelectedMaterial = materials.some((m) => m.key === selectedKey);

  return (
    <div>
      {/* Category header */}
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className="flex items-center w-full gap-1 py-1 text-[10px] font-medium uppercase tracking-wider text-text-muted hover:text-text"
      >
        {open ? <CaretDown size={10} /> : <CaretRight size={10} />}
        <span className="flex-1 text-left">{CATEGORY_LABELS[category]}</span>
        <span className="text-text-muted/50">({materials.length})</span>
      </button>

      {/* Materials grid */}
      {open && (
        <div className="flex flex-wrap gap-1.5 py-1.5 pl-3">
          {materials.map((mat) => {
            const isFavorite = favorites.includes(mat.key);
            return (
              <Tooltip
                key={mat.key}
                content={<MaterialInfoPanel material={mat} volumeMm3={volumeMm3} />}
                side="right"
              >
                <div className="relative group">
                  <MaterialSwatch
                    material={mat}
                    selected={mat.key === selectedKey}
                    onClick={() => onSelect(mat)}
                    onMouseEnter={() => onPreview(mat)}
                    onMouseLeave={() => onPreview(null)}
                  />
                  {/* Favorite star indicator */}
                  {isFavorite && (
                    <Star
                      weight="fill"
                      size={8}
                      className="absolute -top-0.5 -right-0.5 text-yellow-500"
                    />
                  )}
                  {/* Favorite toggle on hover */}
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggleFavorite(mat.key);
                    }}
                    className="absolute -top-1 -right-1 p-0.5 bg-surface border border-border rounded opacity-0 group-hover:opacity-100 transition-opacity"
                  >
                    <Star
                      weight={isFavorite ? "fill" : "regular"}
                      size={8}
                      className={isFavorite ? "text-yellow-500" : "text-text-muted"}
                    />
                  </button>
                </div>
              </Tooltip>
            );
          })}
        </div>
      )}

      {/* Show indicator if collapsed but has selected material */}
      {!open && hasSelectedMaterial && (
        <div className="text-[10px] text-accent pl-4 pb-1">• selected</div>
      )}
    </div>
  );
}
