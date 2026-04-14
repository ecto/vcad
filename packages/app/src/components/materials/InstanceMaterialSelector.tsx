/**
 * InstanceMaterialSelector - Material selector variant for assembly instances.
 * Uses setInstanceMaterial instead of setPartMaterial.
 */

import { useState, useMemo, useCallback, useEffect } from "react";
import { MagnifyingGlass } from "@phosphor-icons/react/dist/ssr/MagnifyingGlass";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { Clock } from "@phosphor-icons/react/dist/ssr/Clock";
import { Star } from "@phosphor-icons/react/dist/ssr/Star";
import { Tooltip } from "@/components/ui/tooltip";
import { useUiStore, useDocumentStore } from "@vcad/core";
import type { MaterialPreset } from "@/data/materials";
import {
  CATEGORY_ORDER,
  getMaterialsByCategory,
  getMaterialByKey,
  searchMaterials,
} from "@/data/materials";
import { CategorySection } from "./CategorySection";
import { MaterialSwatch } from "./MaterialSwatch";
import { MaterialInfoPanel } from "./MaterialInfoPanel";
import { MaterialEditor } from "./MaterialEditor";

interface InstanceMaterialSelectorProps {
  instanceId: string;
  currentMaterialKey: string;
}

export function InstanceMaterialSelector({
  instanceId,
  currentMaterialKey,
}: InstanceMaterialSelectorProps) {
  const [searchQuery, setSearchQuery] = useState("");
  const [editorOpen, setEditorOpen] = useState(false);
  const [customMaterialCount, setCustomMaterialCount] = useState(0);

  const setInstanceMaterial = useDocumentStore((s) => s.setInstanceMaterial);

  const setPreviewMaterial = useUiStore((s) => s.setPreviewMaterial);
  const addRecentMaterial = useUiStore((s) => s.addRecentMaterial);
  const toggleFavoriteMaterial = useUiStore((s) => s.toggleFavoriteMaterial);
  const recentMaterials = useUiStore((s) => s.recentMaterials);
  const favoriteMaterials = useUiStore((s) => s.favoriteMaterials);

  // Clear preview when unmounting or instanceId changes
  useEffect(() => {
    return () => setPreviewMaterial(null);
  }, [instanceId, setPreviewMaterial]);

  // Get materials matching search
  const filteredMaterials = useMemo(() => {
    if (!searchQuery.trim()) return null;
    return searchMaterials(searchQuery);
  }, [searchQuery]);

  // Get recent materials as presets
  const recentPresets = useMemo(() => {
    return recentMaterials
      .map((key) => getMaterialByKey(key))
      .filter((m): m is MaterialPreset => m !== undefined);
  }, [recentMaterials]);

  // Get favorite materials as presets
  const favoritePresets = useMemo(() => {
    return favoriteMaterials
      .map((key) => getMaterialByKey(key))
      .filter((m): m is MaterialPreset => m !== undefined);
  }, [favoriteMaterials]);

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

      setInstanceMaterial(instanceId, material.key);
      addRecentMaterial(material.key);
      setPreviewMaterial(null);
    },
    [instanceId, setInstanceMaterial, addRecentMaterial, setPreviewMaterial]
  );

  const handlePreview = useCallback(
    (material: MaterialPreset | null) => {
      if (material) {
        // Use instanceId for preview so SceneMesh can match it
        setPreviewMaterial({ partId: instanceId, materialKey: material.key });
      } else {
        setPreviewMaterial(null);
      }
    },
    [instanceId, setPreviewMaterial]
  );

  const handleCustomMaterialSave = useCallback(
    (mat: Omit<MaterialPreset, "key">) => {
      // Generate unique key
      const key = `custom-${Date.now()}-${customMaterialCount}`;
      setCustomMaterialCount((c) => c + 1);

      // Add to document materials
      const state = useDocumentStore.getState();
      const newDoc = structuredClone(state.document);
      newDoc.materials[key] = {
        name: mat.name,
        color: mat.color,
        metallic: mat.metallic,
        roughness: mat.roughness,
      };
      useDocumentStore.setState({ document: newDoc, isDirty: true });

      // Apply to instance
      setInstanceMaterial(instanceId, key);
      addRecentMaterial(key);
    },
    [instanceId, customMaterialCount, setInstanceMaterial, addRecentMaterial]
  );

  // Find which category the current material is in (for default open state)
  const currentMaterial = getMaterialByKey(currentMaterialKey);
  const currentCategory = currentMaterial?.category;

  return (
    <div className="space-y-2">
      {/* Search bar */}
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
          className="w-full bg-card border border-border pl-7 pr-2 py-1 text-xs text-text placeholder-text-muted outline-none hover:border-text-muted focus:border-brand"
        />
      </div>

      {/* Search results */}
      {filteredMaterials !== null ? (
        <div className="space-y-1">
          <div className="text-[10px] text-text-muted">
            {filteredMaterials.length} results
          </div>
          <div className="flex flex-wrap gap-1.5">
            {filteredMaterials.map((mat) => (
              <Tooltip
                key={mat.key}
                content={<MaterialInfoPanel material={mat} />}
                side="right"
              >
                <div>
                  <MaterialSwatch
                    material={mat}
                    selected={mat.key === currentMaterialKey}
                    onClick={() => handleSelect(mat)}
                    onMouseEnter={() => handlePreview(mat)}
                    onMouseLeave={() => handlePreview(null)}
                  />
                </div>
              </Tooltip>
            ))}
          </div>
        </div>
      ) : (
        <>
          {/* Favorites section */}
          {favoritePresets.length > 0 && (
            <div className="space-y-1">
              <div className="flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-text-muted">
                <Star size={10} weight="fill" className="text-yellow-500" />
                Favorites
              </div>
              <div className="flex flex-wrap gap-1.5 pl-3">
                {favoritePresets.map((mat) => (
                  <Tooltip
                    key={mat.key}
                    content={<MaterialInfoPanel material={mat} />}
                    side="right"
                  >
                    <div>
                      <MaterialSwatch
                        material={mat}
                        selected={mat.key === currentMaterialKey}
                        onClick={() => handleSelect(mat)}
                        onMouseEnter={() => handlePreview(mat)}
                        onMouseLeave={() => handlePreview(null)}
                      />
                    </div>
                  </Tooltip>
                ))}
              </div>
            </div>
          )}

          {/* Recent materials section */}
          {recentPresets.length > 0 && (
            <div className="space-y-1">
              <div className="flex items-center gap-1 text-[10px] font-medium uppercase tracking-wider text-text-muted">
                <Clock size={10} />
                Recent
              </div>
              <div className="flex flex-wrap gap-1.5 pl-3">
                {recentPresets.map((mat) => (
                  <Tooltip
                    key={mat.key}
                    content={<MaterialInfoPanel material={mat} />}
                    side="right"
                  >
                    <div>
                      <MaterialSwatch
                        material={mat}
                        selected={mat.key === currentMaterialKey}
                        onClick={() => handleSelect(mat)}
                        onMouseEnter={() => handlePreview(mat)}
                        onMouseLeave={() => handlePreview(null)}
                      />
                    </div>
                  </Tooltip>
                ))}
              </div>
            </div>
          )}

          {/* Category sections */}
          <div className="space-y-0.5 max-h-48 overflow-y-auto scrollbar-thin">
            {CATEGORY_ORDER.map((category) => (
              <CategorySection
                key={category}
                category={category}
                materials={getMaterialsByCategory(category)}
                selectedKey={currentMaterialKey}
                favorites={favoriteMaterials}
                defaultOpen={category === currentCategory || category === "metals"}
                onSelect={handleSelect}
                onPreview={handlePreview}
                onToggleFavorite={toggleFavoriteMaterial}
              />
            ))}
          </div>
        </>
      )}

      {/* Custom material button */}
      <button
        type="button"
        onClick={() => setEditorOpen(true)}
        className="flex items-center gap-1.5 w-full px-2 py-1.5 text-xs text-text-muted hover:text-text hover:bg-hover border border-dashed border-border hover:border-text-muted"
      >
        <Plus size={12} />
        Custom Material
      </button>

      {/* Custom material editor dialog */}
      <MaterialEditor
        open={editorOpen}
        onOpenChange={setEditorOpen}
        onSave={handleCustomMaterialSave}
      />
    </div>
  );
}
