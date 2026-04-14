/**
 * MaterialSwatch - PBR-like preview swatch for a material.
 */

import type { MaterialPreset } from "@/data/materials";
import { cn } from "@/lib/utils";

interface MaterialSwatchProps {
  material: MaterialPreset;
  selected?: boolean;
  size?: "sm" | "md" | "lg";
  onClick?: () => void;
  onMouseEnter?: () => void;
  onMouseLeave?: () => void;
}

/**
 * Generate a PBR-like gradient for visual feedback based on material properties.
 */
function getPbrGradient(mat: MaterialPreset): string {
  const [r, g, b] = mat.color.map((c) => Math.round(c * 255));

  // Metallic materials get stronger highlights
  const highlight = mat.metallic > 0.5 ? 60 : 30;
  // Rough materials get softer shadows
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

const SIZE_CLASSES = {
  sm: "h-5 w-5",
  md: "h-7 w-7",
  lg: "h-10 w-10",
};

export function MaterialSwatch({
  material,
  selected = false,
  size = "md",
  onClick,
  onMouseEnter,
  onMouseLeave,
}: MaterialSwatchProps) {
  const gradient = getPbrGradient(material);

  return (
    <button
      type="button"
      onClick={onClick}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      className={cn(
        SIZE_CLASSES[size],
        "rounded-full border-2 cursor-pointer transition-all duration-100",
        "hover:scale-110 hover:shadow-md",
        "focus:outline-none focus:ring-2 focus:ring-brand/50",
        selected
          ? "border-brand ring-2 ring-brand/30"
          : "border-transparent hover:border-border"
      )}
      style={{ background: gradient }}
      title={material.name}
    />
  );
}
