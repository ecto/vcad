/**
 * Procedural Shader Registry
 * Central access point for all procedural material shaders.
 */

import type { ProceduralShaderDef, ProceduralShaderType } from "./types";
import { brushedMetalShader } from "./materials/brushed-metal";
import { woodShader, walnutColors, oakColors, bambooColors } from "./materials/wood";
import { carbonFiberShader } from "./materials/carbon-fiber";
import { concreteShader } from "./materials/concrete";
import { pcbCopperShader } from "./materials/pcb-copper";
import * as THREE from "three";

export type { ProceduralShaderDef, ProceduralShaderType } from "./types";

/** All registered procedural shaders */
const SHADERS = new Map<ProceduralShaderType, ProceduralShaderDef>([
  ["brushed-metal", brushedMetalShader],
  ["wood", woodShader],
  ["carbon-fiber", carbonFiberShader],
  ["concrete", concreteShader],
  ["pcb-copper", pcbCopperShader],
]);

/** Material key to shader mapping (for materials that use procedural shaders) */
const MATERIAL_SHADER_MAP: Record<string, ProceduralShaderType> = {
  aluminum: "brushed-metal",
  steel: "brushed-metal",
  titanium: "brushed-metal",
  walnut: "wood",
  oak: "wood",
  bamboo: "wood",
  "carbon-fiber": "carbon-fiber",
  concrete: "concrete",
  __pcb_copper__: "pcb-copper",
};

/** Wood material color overrides */
const WOOD_COLOR_OVERRIDES: Record<
  string,
  { uDarkWood: THREE.Color; uLightWood: THREE.Color }
> = {
  walnut: walnutColors,
  oak: oakColors,
  bamboo: bambooColors,
};

/** Brushed metal color overrides by material key */
const BRUSHED_METAL_OVERRIDES: Record<
  string,
  { uBaseColor: THREE.Color; uMetalness: number; uRoughness: number }
> = {
  aluminum: {
    uBaseColor: new THREE.Color(0.8, 0.8, 0.85),
    uMetalness: 0.9,
    uRoughness: 0.3,
  },
  steel: {
    uBaseColor: new THREE.Color(0.7, 0.7, 0.72),
    uMetalness: 0.95,
    uRoughness: 0.25,
  },
  titanium: {
    uBaseColor: new THREE.Color(0.6, 0.6, 0.65),
    uMetalness: 0.85,
    uRoughness: 0.4,
  },
};

/**
 * Get a procedural shader by type.
 */
export function getProceduralShader(
  shaderType: ProceduralShaderType
): ProceduralShaderDef | null {
  return SHADERS.get(shaderType) ?? null;
}

/**
 * Check if a material key has a procedural shader.
 */
export function hasProceduralShader(materialKey: string): boolean {
  return materialKey in MATERIAL_SHADER_MAP;
}

/**
 * Get the shader type for a material key.
 */
export function getShaderTypeForMaterial(
  materialKey: string
): ProceduralShaderType | null {
  return MATERIAL_SHADER_MAP[materialKey] ?? null;
}

/**
 * Get a procedural shader configured for a specific material.
 * Returns cloned uniforms with material-specific values.
 */
export function getProceduralShaderForMaterial(
  materialKey: string
): ProceduralShaderDef | null {
  const shaderType = MATERIAL_SHADER_MAP[materialKey];
  if (!shaderType) return null;

  const baseShader = SHADERS.get(shaderType);
  if (!baseShader) return null;

  // Clone uniforms to allow per-instance modification
  const clonedUniforms: Record<string, THREE.IUniform> = {};
  for (const [key, uniform] of Object.entries(baseShader.uniforms)) {
    if (uniform.value instanceof THREE.Color) {
      clonedUniforms[key] = { value: uniform.value.clone() };
    } else {
      clonedUniforms[key] = { value: uniform.value };
    }
  }

  // Apply material-specific overrides
  if (shaderType === "wood" && WOOD_COLOR_OVERRIDES[materialKey]) {
    const colors = WOOD_COLOR_OVERRIDES[materialKey];
    clonedUniforms["uDarkWood"] = { value: colors.uDarkWood.clone() };
    clonedUniforms["uLightWood"] = { value: colors.uLightWood.clone() };
  }

  if (shaderType === "brushed-metal" && BRUSHED_METAL_OVERRIDES[materialKey]) {
    const overrides = BRUSHED_METAL_OVERRIDES[materialKey];
    clonedUniforms["uBaseColor"] = { value: overrides.uBaseColor.clone() };
    clonedUniforms["uMetalness"] = { value: overrides.uMetalness };
    clonedUniforms["uRoughness"] = { value: overrides.uRoughness };
  }

  return {
    ...baseShader,
    uniforms: clonedUniforms,
  };
}
