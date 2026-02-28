/**
 * Multimodal data generation - creates image-IR pairs for vision-language training.
 *
 * Pipeline: VCode → fromVCode() → Document → Engine.evaluate() → Mesh → Render → PNG
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fromVCode } from "@vcad/ir";
import type { GeneratedPart } from "./generators/types.js";
import { Renderer, type ViewPreset, type RenderOptions } from "./render.js";

/** A single image-IR training pair. */
export interface ImageIRPair {
  /** Path to the rendered image file (relative). */
  imagePath: string;
  /** VCode representation. */
  ir: string;
  /** Part family name. */
  family: string;
  /** Camera view used. */
  view: ViewPreset;
  /** Image width in pixels. */
  width: number;
  /** Image height in pixels. */
  height: number;
}

/** Options for multimodal data generation. */
export interface MultimodalOptions {
  /** Output directory for images. */
  outputDir: string;
  /** Image width in pixels. */
  width?: number;
  /** Image height in pixels. */
  height?: number;
  /** Views to render for each part. */
  views?: ViewPreset[];
  /** Progress callback. */
  onProgress?: (completed: number, total: number, errors: number) => void;
  /** Called for each generated pair. */
  onPair?: (pair: ImageIRPair) => void;
}

/** Engine interface for mesh evaluation. */
interface Engine {
  evaluate(doc: any): {
    parts: Array<{
      mesh: {
        positions: Float32Array;
        indices: Uint32Array;
      };
    }>;
  };
}

/**
 * Generate image-IR pairs from generated parts.
 *
 * @param parts - Array of generated parts
 * @param engine - vcad engine instance for mesh evaluation
 * @param options - Generation options
 * @returns Array of image-IR pairs with metadata
 */
export async function generateImageIRPairs(
  parts: GeneratedPart[],
  engine: Engine,
  options: MultimodalOptions,
): Promise<ImageIRPair[]> {
  const {
    outputDir,
    width = 512,
    height = 512,
    views = ["isometric"],
    onProgress,
    onPair,
  } = options;

  // Ensure output directory exists
  const imagesDir = path.join(outputDir, "images");
  fs.mkdirSync(imagesDir, { recursive: true });

  const pairs: ImageIRPair[] = [];
  const renderer = new Renderer();
  let errors = 0;

  try {
    await renderer.init();

    const totalItems = parts.length * views.length;
    let completed = 0;

    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];

      try {
        // Parse IR and evaluate to mesh
        const doc = fromVCode(part.vcode);
        const scene = engine.evaluate(doc);

        if (!scene.parts || scene.parts.length === 0) {
          errors++;
          completed += views.length;
          onProgress?.(completed, totalItems, errors);
          continue;
        }

        const mesh = scene.parts[0].mesh;

        // Render each view
        for (const view of views) {
          try {
            const result = await renderer.render(
              mesh.positions,
              mesh.indices,
              { width, height, view },
            );

            // Generate filename
            const filename = `${part.family}_${i.toString().padStart(6, "0")}_${view}.png`;
            const imagePath = path.join(imagesDir, filename);

            // Write image
            fs.writeFileSync(imagePath, result.image);

            const pair: ImageIRPair = {
              imagePath: path.relative(outputDir, imagePath),
              ir: part.vcode,
              family: part.family,
              view,
              width,
              height,
            };

            pairs.push(pair);
            onPair?.(pair);
          } catch (renderError) {
            errors++;
          }

          completed++;
          onProgress?.(completed, totalItems, errors);
        }
      } catch (evalError) {
        errors++;
        completed += views.length;
        onProgress?.(completed, totalItems, errors);
      }
    }
  } finally {
    await renderer.close();
  }

  return pairs;
}

/**
 * Write metadata file for image-IR pairs.
 */
export function writeMetadata(
  pairs: ImageIRPair[],
  outputPath: string,
): void {
  const output = fs.createWriteStream(outputPath);
  for (const pair of pairs) {
    output.write(JSON.stringify(pair) + "\n");
  }
  output.end();
}

/**
 * Generate image-IR pairs with base64-encoded images (single file format).
 */
export interface Base64ImageIRPair {
  /** Base64-encoded PNG image. */
  imageBase64: string;
  /** VCode representation. */
  ir: string;
  /** Part family name. */
  family: string;
  /** Camera view used. */
  view: ViewPreset;
}

/**
 * Generate image-IR pairs with inline base64 images.
 * Useful for creating a single JSONL file without separate image files.
 */
export async function generateBase64ImageIRPairs(
  parts: GeneratedPart[],
  engine: Engine,
  options: Omit<MultimodalOptions, "outputDir"> & {
    onPair?: (pair: Base64ImageIRPair) => void;
  },
): Promise<Base64ImageIRPair[]> {
  const {
    width = 512,
    height = 512,
    views = ["isometric"],
    onProgress,
    onPair,
  } = options;

  const pairs: Base64ImageIRPair[] = [];
  const renderer = new Renderer();
  let errors = 0;

  try {
    await renderer.init();

    const totalItems = parts.length * views.length;
    let completed = 0;

    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];

      try {
        const doc = fromVCode(part.vcode);
        const scene = engine.evaluate(doc);

        if (!scene.parts || scene.parts.length === 0) {
          errors++;
          completed += views.length;
          onProgress?.(completed, totalItems, errors);
          continue;
        }

        const mesh = scene.parts[0].mesh;

        for (const view of views) {
          try {
            const result = await renderer.render(
              mesh.positions,
              mesh.indices,
              { width, height, view },
            );

            const pair: Base64ImageIRPair = {
              imageBase64: result.image.toString("base64"),
              ir: part.vcode,
              family: part.family,
              view,
            };

            pairs.push(pair);
            onPair?.(pair);
          } catch (renderError) {
            errors++;
          }

          completed++;
          onProgress?.(completed, totalItems, errors);
        }
      } catch (evalError) {
        errors++;
        completed += views.length;
        onProgress?.(completed, totalItems, errors);
      }
    }
  } finally {
    await renderer.close();
  }

  return pairs;
}

/**
 * Compute statistics for multimodal dataset.
 */
export interface MultimodalStats {
  totalPairs: number;
  byFamily: Record<string, number>;
  byView: Record<string, number>;
  avgImageSizeBytes: number;
}

export function computeMultimodalStats(pairs: ImageIRPair[], imagesDir: string): MultimodalStats {
  const byFamily: Record<string, number> = {};
  const byView: Record<string, number> = {};
  let totalSize = 0;

  for (const pair of pairs) {
    byFamily[pair.family] = (byFamily[pair.family] || 0) + 1;
    byView[pair.view] = (byView[pair.view] || 0) + 1;

    try {
      const imagePath = path.join(imagesDir, "..", pair.imagePath);
      const stats = fs.statSync(imagePath);
      totalSize += stats.size;
    } catch {
      // Ignore missing files
    }
  }

  return {
    totalPairs: pairs.length,
    byFamily,
    byView,
    avgImageSizeBytes: pairs.length > 0 ? Math.round(totalSize / pairs.length) : 0,
  };
}
