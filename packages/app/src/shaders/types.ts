/**
 * Procedural shader type definitions for realistic material textures.
 */

import type * as THREE from "three";

/** Shader type identifier for procedural materials */
export type ProceduralShaderType =
  | "wood"
  | "brushed-metal"
  | "carbon-fiber"
  | "concrete"
  | "pcb-copper";

/** Definition for a procedural material shader */
export interface ProceduralShaderDef {
  key: ProceduralShaderType;
  vertexShader: string;
  fragmentShader: string;
  /** Uniform definitions - will be cloned per-instance */
  uniforms: Record<string, THREE.IUniform>;
}
