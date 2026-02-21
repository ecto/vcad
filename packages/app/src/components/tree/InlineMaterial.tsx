import { useState, useMemo, useCallback } from "react";
import * as Popover from "@radix-ui/react-popover";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { useDocumentStore } from "@vcad/core";
import type { MaterialPreset } from "@/data/materials";
import {
  getMaterialByKey,
  searchMaterials,
  getMaterialsByCategory,
  CATEGORY_ORDER,
} from "@/data/materials";
import { cn } from "@/lib/utils";

interface InlineMaterialProps {
  partId: string;
  currentMaterialKey: string;
}

/**
 * Generate a PBR-like gradient for visual feedback based on material properties.
 */
function getPbrGradient(mat: MaterialPreset): string {
  const [r, g, b] = mat.color.map((c) => Math.round(c * 255));

  const highlight = mat.metallic > 0.5 ? 60 : 30;
  const shadow = mat.roughness > 0.5 ? 40 : 20;

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

function MaterialSwatchButton({
  material,
  selected,
  onClick,
}: {
  material: MaterialPreset;
  selected: boolean;
  onClick: () => void;
}) {
  const gradient = getPbrGradient(material);

  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "h-6 w-6 rounded-full border-2 cursor-pointer transition-all duration-100",
        "hover:scale-110 hover:shadow-md",
        "focus:outline-none focus:ring-2 focus:ring-accent/50",
        selected
          ? "border-accent ring-2 ring-accent/30"
          : "border-transparent hover:border-border"
      )}
      style={{ background: gradient }}
      title={material.name}
    />
  );
}

export function InlineMaterial({
  partId,
  currentMaterialKey,
}: InlineMaterialProps) {
  const [open, setOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const setPartMaterial = useDocumentStore((s) => s.setPartMaterial);

  const currentMaterial = useMemo(
    () => getMaterialByKey(currentMaterialKey),
    [currentMaterialKey]
  );

  const filteredMaterials = useMemo(() => {
    if (searchQuery.trim()) {
      return searchMaterials(searchQuery);
    }
    return null;
  }, [searchQuery]);

  const handleSelect = useCallback(
    (material: MaterialPreset) => {
      // Ensure material exists in document
      const state = useDocumentStore.getState();
      const newDoc = structuredClone(state.document);
      if (!newDoc.materials[material.key]) {
        newDoc.materials[material.key] = {
          name: material.name,
          color: material.color,
          metallic: material.metallic,
          roughness: material.roughness,
        };
        useDocumentStore.setState({ document: newDoc, isDirty: true });
      }

      setPartMaterial(partId, material.key);
      setOpen(false);
    },
    [partId, setPartMaterial]
  );

  // Default fallback if material not found
  const displayMaterial: MaterialPreset = currentMaterial ?? {
    key: "default",
    name: "Default",
    color: [0.55, 0.55, 0.55] as [number, number, number],
    metallic: 0,
    roughness: 0.7,
    category: "plastics",
    density: 1000,
  };

  const gradient = getPbrGradient(displayMaterial);

  return (
    <div className="px-2 py-1">
      <Popover.Root open={open} onOpenChange={setOpen}>
        <Popover.Trigger asChild>
          <button
            className="flex items-center gap-2 w-full hover:bg-hover rounded px-1 py-0.5 transition-colors"
          >
            <span
              className="h-4 w-4 rounded-full shrink-0 border border-border"
              style={{ background: gradient }}
            />
            <span className="text-[10px] text-text truncate flex-1 text-left">
              {displayMaterial.name}
            </span>
            <span className="text-[9px] text-text-muted">[change]</span>
          </button>
        </Popover.Trigger>
        <Popover.Portal>
          <Popover.Content
            side="right"
            align="start"
            sideOffset={8}
            className="z-50 w-56 max-h-80 overflow-auto bg-surface border border-border shadow-xl rounded"
          >
            {/* Search */}
            <div className="p-2 border-b border-border sticky top-0 bg-surface">
              <div className="relative">
                <MagnifyingGlass
                  size={12}
                  className="absolute left-2 top-1/2 -translate-y-1/2 text-text-muted"
                />
                <input
                  type="text"
                  placeholder="Search materials..."
                  value={searchQuery}
                  onChange={(e) => setSearchQuery(e.target.value)}
                  className="w-full pl-6 pr-2 py-1 text-[10px] bg-card border border-border rounded outline-none focus:border-accent"
                />
              </div>
            </div>

            {/* Results */}
            <div className="p-2">
              {filteredMaterials ? (
                /* Search results */
                <div className="grid grid-cols-6 gap-1">
                  {filteredMaterials.map((mat) => (
                    <MaterialSwatchButton
                      key={mat.key}
                      material={mat}
                      selected={mat.key === currentMaterialKey}
                      onClick={() => handleSelect(mat)}
                    />
                  ))}
                </div>
              ) : (
                /* Categories */
                <div className="space-y-2">
                  {CATEGORY_ORDER.map((category) => {
                    const materials = getMaterialsByCategory(category);
                    if (materials.length === 0) return null;
                    return (
                      <div key={category}>
                        <div className="text-[9px] uppercase text-text-muted mb-1">
                          {category}
                        </div>
                        <div className="grid grid-cols-6 gap-1">
                          {materials.slice(0, 12).map((mat) => (
                            <MaterialSwatchButton
                              key={mat.key}
                              material={mat}
                              selected={mat.key === currentMaterialKey}
                              onClick={() => handleSelect(mat)}
                            />
                          ))}
                        </div>
                      </div>
                    );
                  })}
                </div>
              )}
            </div>
          </Popover.Content>
        </Popover.Portal>
      </Popover.Root>
    </div>
  );
}
