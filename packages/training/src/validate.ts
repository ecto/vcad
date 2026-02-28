/**
 * Validation module - validates generated VCode using the vcad engine.
 */

import { fromVCode, type Document } from "@vcad/ir";
import type { TrainingExample, ValidationResult } from "./generators/types.js";

/** Options for validation. */
export interface ValidateOptions {
  /** Whether to compute full geometry (slower but more thorough). */
  fullGeometry?: boolean;
  /** Callback for progress updates. */
  onProgress?: (completed: number, total: number, errors: number) => void;
}

/**
 * Validate a single training example by parsing and optionally evaluating its IR.
 *
 * @param example - The training example to validate
 * @param engine - Optional engine instance for geometry evaluation
 * @returns Validation result
 */
export function validateExample(
  example: TrainingExample,
  engine?: { evaluate: (doc: Document) => unknown },
): ValidationResult {
  // 1. Parse VCode
  let doc: Document;
  try {
    doc = fromVCode(example.ir);
  } catch (e) {
    return {
      valid: false,
      error: "parse_error",
      message: e instanceof Error ? e.message : String(e),
    };
  }

  // 2. Check for non-empty document
  if (Object.keys(doc.nodes).length === 0) {
    return {
      valid: false,
      error: "empty_document",
      message: "Document has no nodes",
    };
  }

  // 3. Check for valid roots
  if (doc.roots.length === 0) {
    return {
      valid: false,
      error: "no_roots",
      message: "Document has no root nodes",
    };
  }

  // 4. Optional: evaluate geometry with engine
  if (engine) {
    try {
      const scene = engine.evaluate(doc) as {
        parts: Array<{
          mesh: {
            positions: Float32Array;
            indices: Uint32Array;
          };
        }>;
      };

      if (!scene.parts || scene.parts.length === 0) {
        return {
          valid: false,
          error: "empty_geometry",
          message: "Evaluation produced no parts",
        };
      }

      const mesh = scene.parts[0].mesh;
      const triangles = mesh.indices.length / 3;

      if (triangles === 0) {
        return {
          valid: false,
          error: "no_triangles",
          message: "Mesh has no triangles",
        };
      }

      // Estimate volume (simplified - just check mesh is non-degenerate)
      const positions = mesh.positions;
      let minX = Infinity,
        maxX = -Infinity;
      let minY = Infinity,
        maxY = -Infinity;
      let minZ = Infinity,
        maxZ = -Infinity;

      for (let i = 0; i < positions.length; i += 3) {
        minX = Math.min(minX, positions[i]);
        maxX = Math.max(maxX, positions[i]);
        minY = Math.min(minY, positions[i + 1]);
        maxY = Math.max(maxY, positions[i + 1]);
        minZ = Math.min(minZ, positions[i + 2]);
        maxZ = Math.max(maxZ, positions[i + 2]);
      }

      const bboxVolume = (maxX - minX) * (maxY - minY) * (maxZ - minZ);

      if (bboxVolume <= 0 || !isFinite(bboxVolume)) {
        return {
          valid: false,
          error: "invalid_volume",
          message: "Mesh has invalid bounding box",
        };
      }

      return {
        valid: true,
        volume: bboxVolume,
        triangles,
      };
    } catch (e) {
      return {
        valid: false,
        error: "evaluation_error",
        message: e instanceof Error ? e.message : String(e),
      };
    }
  }

  // Without engine, just return parse success
  return { valid: true };
}

/**
 * Validate multiple training examples.
 *
 * @param examples - Array of training examples to validate
 * @param options - Validation options
 * @param engine - Optional engine instance for geometry evaluation
 * @returns Array of validation results
 */
export async function validateExamples(
  examples: TrainingExample[],
  options: ValidateOptions = {},
  engine?: { evaluate: (doc: Document) => unknown },
): Promise<ValidationResult[]> {
  const { onProgress } = options;
  const results: ValidationResult[] = [];
  let errors = 0;

  for (let i = 0; i < examples.length; i++) {
    const result = validateExample(examples[i], engine);
    results.push(result);

    if (!result.valid) {
      errors++;
    }

    onProgress?.(i + 1, examples.length, errors);
  }

  return results;
}

/**
 * Compute statistics from validation results.
 */
export interface ValidationStats {
  total: number;
  valid: number;
  invalid: number;
  errorCounts: Record<string, number>;
  avgVolume?: number;
  avgTriangles?: number;
}

export function computeValidationStats(results: ValidationResult[]): ValidationStats {
  const stats: ValidationStats = {
    total: results.length,
    valid: 0,
    invalid: 0,
    errorCounts: {},
  };

  let totalVolume = 0;
  let totalTriangles = 0;
  let volumeCount = 0;
  let triangleCount = 0;

  for (const result of results) {
    if (result.valid) {
      stats.valid++;
      if (result.volume !== undefined) {
        totalVolume += result.volume;
        volumeCount++;
      }
      if (result.triangles !== undefined) {
        totalTriangles += result.triangles;
        triangleCount++;
      }
    } else {
      stats.invalid++;
      const error = result.error || "unknown";
      stats.errorCounts[error] = (stats.errorCounts[error] || 0) + 1;
    }
  }

  if (volumeCount > 0) {
    stats.avgVolume = totalVolume / volumeCount;
  }
  if (triangleCount > 0) {
    stats.avgTriangles = totalTriangles / triangleCount;
  }

  return stats;
}

/**
 * Filter out invalid examples from an array.
 */
export function filterValidExamples(
  examples: TrainingExample[],
  results: ValidationResult[],
): TrainingExample[] {
  return examples.filter((_, i) => results[i].valid);
}
