/**
 * Material Library - 31 real-world materials with PBR properties and physical data.
 */

export type MaterialCategory =
  | "metals"
  | "plastics"
  | "organic"
  | "glass"
  | "composite"
  | "other";

export type ProceduralShaderType =
  | "wood"
  | "brushed-metal"
  | "carbon-fiber"
  | "concrete";

export interface MaterialPreset {
  key: string;
  name: string;
  category: MaterialCategory;
  color: [number, number, number]; // RGB 0-1
  metallic: number;
  roughness: number;
  density: number; // kg/m³
  /** Optional procedural shader for realistic textures */
  proceduralShader?: ProceduralShaderType;
  /**
   * Light transmission, 0..1. Non-zero switches the renderer to a physical
   * material so the part renders as glass / translucent plastic.
   */
  transmission?: number;
  /** Index of refraction. Common: 1.45 (acrylic), 1.5 (glass), 1.585 (polycarbonate). */
  ior?: number;
  /** Volume thickness in mm; controls absorption depth for tinted transmission. */
  thickness?: number;
  /** Distance (mm) over which transmitted light is fully absorbed by `attenuationColor`. */
  attenuationDistance?: number;
  /** Tint color applied to transmitted light, RGB 0-1. */
  attenuationColor?: [number, number, number];
  /** Clearcoat layer strength, 0..1. */
  clearcoat?: number;
  /** Roughness of the clearcoat layer, 0..1. */
  clearcoatRoughness?: number;
}

export const CATEGORY_LABELS: Record<MaterialCategory, string> = {
  metals: "Metals",
  plastics: "Plastics",
  organic: "Organic",
  glass: "Glass",
  composite: "Composites",
  other: "Other",
};

export const CATEGORY_ORDER: MaterialCategory[] = [
  "metals",
  "plastics",
  "organic",
  "glass",
  "composite",
  "other",
];

export const MATERIAL_PRESETS: MaterialPreset[] = [
  // Metals (8)
  {
    key: "aluminum",
    name: "Aluminum",
    category: "metals",
    color: [0.8, 0.8, 0.85],
    metallic: 0.9,
    roughness: 0.3,
    density: 2700,
    proceduralShader: "brushed-metal",
  },
  {
    key: "steel",
    name: "Steel",
    category: "metals",
    color: [0.7, 0.7, 0.72],
    metallic: 0.95,
    roughness: 0.25,
    density: 8000,
    proceduralShader: "brushed-metal",
  },
  {
    key: "brass",
    name: "Brass",
    category: "metals",
    color: [0.85, 0.65, 0.3],
    metallic: 0.9,
    roughness: 0.35,
    density: 8500,
  },
  {
    key: "copper",
    name: "Copper",
    category: "metals",
    color: [0.95, 0.5, 0.35],
    metallic: 0.95,
    roughness: 0.25,
    density: 8960,
  },
  {
    key: "titanium",
    name: "Titanium",
    category: "metals",
    color: [0.6, 0.6, 0.65],
    metallic: 0.85,
    roughness: 0.4,
    density: 4500,
    proceduralShader: "brushed-metal",
  },
  {
    key: "chrome",
    name: "Chrome",
    category: "metals",
    color: [0.9, 0.9, 0.92],
    metallic: 1.0,
    roughness: 0.05,
    density: 7190,
  },
  {
    key: "gold",
    name: "Gold",
    category: "metals",
    color: [1.0, 0.84, 0.0],
    metallic: 1.0,
    roughness: 0.1,
    density: 19300,
  },
  {
    key: "silver",
    name: "Silver",
    category: "metals",
    color: [0.95, 0.95, 0.97],
    metallic: 1.0,
    roughness: 0.1,
    density: 10490,
  },

  // Plastics (10)
  {
    key: "abs-white",
    name: "ABS White",
    category: "plastics",
    color: [0.95, 0.95, 0.93],
    metallic: 0.0,
    roughness: 0.5,
    density: 1050,
  },
  {
    key: "abs-black",
    name: "ABS Black",
    category: "plastics",
    color: [0.1, 0.1, 0.1],
    metallic: 0.0,
    roughness: 0.5,
    density: 1050,
  },
  {
    key: "abs-red",
    name: "ABS Red",
    category: "plastics",
    color: [0.85, 0.15, 0.15],
    metallic: 0.0,
    roughness: 0.5,
    density: 1050,
  },
  {
    key: "abs-blue",
    name: "ABS Blue",
    category: "plastics",
    color: [0.2, 0.4, 0.85],
    metallic: 0.0,
    roughness: 0.5,
    density: 1050,
  },
  {
    key: "pla",
    name: "PLA",
    category: "plastics",
    color: [0.85, 0.85, 0.8],
    metallic: 0.0,
    roughness: 0.45,
    density: 1240,
  },
  {
    key: "petg",
    name: "PETG",
    category: "plastics",
    color: [0.75, 0.85, 0.9],
    metallic: 0.0,
    roughness: 0.35,
    density: 1270,
  },
  {
    key: "nylon",
    name: "Nylon",
    category: "plastics",
    color: [0.92, 0.9, 0.85],
    metallic: 0.0,
    roughness: 0.55,
    density: 1150,
  },
  {
    key: "resin",
    name: "Resin",
    category: "plastics",
    color: [0.6, 0.55, 0.5],
    metallic: 0.0,
    roughness: 0.2,
    density: 1200,
  },
  {
    key: "acrylic",
    name: "Acrylic",
    category: "plastics",
    color: [0.95, 0.95, 0.98],
    metallic: 0.0,
    roughness: 0.1,
    density: 1180,
  },
  {
    key: "rubber",
    name: "Rubber",
    category: "plastics",
    color: [0.15, 0.15, 0.15],
    metallic: 0.0,
    roughness: 0.8,
    density: 1100,
  },

  // Organic (5)
  {
    key: "oak",
    name: "Oak",
    category: "organic",
    color: [0.65, 0.5, 0.35],
    metallic: 0.0,
    roughness: 0.7,
    density: 750,
    proceduralShader: "wood",
  },
  {
    key: "walnut",
    name: "Walnut",
    category: "organic",
    color: [0.4, 0.28, 0.2],
    metallic: 0.0,
    roughness: 0.65,
    density: 650,
    proceduralShader: "wood",
  },
  {
    key: "leather",
    name: "Leather",
    category: "organic",
    color: [0.45, 0.3, 0.2],
    metallic: 0.0,
    roughness: 0.75,
    density: 900,
  },
  {
    key: "cork",
    name: "Cork",
    category: "organic",
    color: [0.75, 0.6, 0.45],
    metallic: 0.0,
    roughness: 0.9,
    density: 200,
  },
  {
    key: "bamboo",
    name: "Bamboo",
    category: "organic",
    color: [0.85, 0.75, 0.55],
    metallic: 0.0,
    roughness: 0.6,
    density: 400,
    proceduralShader: "wood",
  },

  // Glass (4)
  {
    key: "glass",
    name: "Glass",
    category: "glass",
    color: [0.95, 0.97, 1.0],
    metallic: 0.0,
    roughness: 0.02,
    density: 2500,
    transmission: 1.0,
    ior: 1.5,
    thickness: 2.0,
  },
  {
    key: "glass-tinted",
    name: "Tinted Glass",
    category: "glass",
    color: [0.85, 0.9, 0.95],
    metallic: 0.0,
    roughness: 0.05,
    density: 2500,
    transmission: 0.95,
    ior: 1.5,
    thickness: 3.0,
    attenuationDistance: 25,
    attenuationColor: [0.3, 0.45, 0.55],
  },
  {
    key: "acrylic-clear",
    name: "Acrylic (Clear)",
    category: "glass",
    color: [0.98, 0.98, 1.0],
    metallic: 0.0,
    roughness: 0.05,
    density: 1180,
    transmission: 0.95,
    ior: 1.49,
    thickness: 2.0,
    clearcoat: 0.5,
    clearcoatRoughness: 0.05,
  },
  {
    key: "polycarbonate-frosted",
    name: "Polycarbonate (Frosted)",
    category: "glass",
    color: [0.92, 0.94, 0.96],
    metallic: 0.0,
    roughness: 0.35,
    density: 1200,
    transmission: 0.7,
    ior: 1.585,
    thickness: 2.5,
    attenuationDistance: 50,
    attenuationColor: [0.85, 0.9, 0.95],
  },

  // Composites (3)
  {
    key: "carbon-fiber",
    name: "Carbon Fiber",
    category: "composite",
    color: [0.15, 0.15, 0.18],
    metallic: 0.3,
    roughness: 0.3,
    density: 1600,
    proceduralShader: "carbon-fiber",
  },
  {
    key: "fiberglass",
    name: "Fiberglass",
    category: "composite",
    color: [0.85, 0.85, 0.75],
    metallic: 0.0,
    roughness: 0.4,
    density: 1800,
  },
  {
    key: "kevlar",
    name: "Kevlar",
    category: "composite",
    color: [0.75, 0.7, 0.3],
    metallic: 0.0,
    roughness: 0.6,
    density: 1440,
  },

  // Other (3)
  {
    key: "concrete",
    name: "Concrete",
    category: "other",
    color: [0.6, 0.6, 0.58],
    metallic: 0.0,
    roughness: 0.85,
    density: 2400,
    proceduralShader: "concrete",
  },
  {
    key: "ceramic",
    name: "Ceramic",
    category: "other",
    color: [0.95, 0.93, 0.9],
    metallic: 0.0,
    roughness: 0.25,
    density: 2300,
  },
  {
    key: "foam",
    name: "Foam",
    category: "other",
    color: [0.3, 0.3, 0.35],
    metallic: 0.0,
    roughness: 0.95,
    density: 50,
  },
];

/** Get all materials in a category */
export function getMaterialsByCategory(category: MaterialCategory): MaterialPreset[] {
  return MATERIAL_PRESETS.filter((m) => m.category === category);
}

/** Find a material by key */
export function getMaterialByKey(key: string): MaterialPreset | undefined {
  return MATERIAL_PRESETS.find((m) => m.key === key);
}

/** Search materials by name (case-insensitive) */
export function searchMaterials(query: string): MaterialPreset[] {
  const lower = query.toLowerCase();
  return MATERIAL_PRESETS.filter(
    (m) =>
      m.name.toLowerCase().includes(lower) ||
      m.category.toLowerCase().includes(lower)
  );
}
