/**
 * Browser-based inference for text-to-CAD generation.
 *
 * Uses @huggingface/transformers (Transformers.js) for in-browser inference.
 * Model weights are cached in IndexedDB for fast subsequent loads.
 */

// Database configuration
const DB_NAME = "vcad-inference";
const DB_VERSION = 1;
const STORE_NAME = "models";

// Model configuration
const DEFAULT_MODEL_ID = "campedersen/cad0-mini";
const MODEL_CACHE_KEY = "cad0-model";

/** System prompt for CAD generation. */
const SYSTEM_PROMPT = `You are a CAD code generator. Generate "Compact IR" - a text representation of 3D geometry.

Format:
- C x y z = Cube (dimensions in mm)
- Y r h = Cylinder (radius, height)
- S r = Sphere (radius)
- T n x y z = Translate node n
- R n x y z = Rotate node n (degrees)
- U a b = Union nodes
- D a b = Difference (subtract b from a)

Node IDs are sequential from 0. Output ONLY the IR code, no explanations.

Alternative: You may also output loon source (starts with [):
[cube 20.0 20.0 20.0]
[pipe [cube 50.0 30.0 5.0] [difference [cylinder 3.0 10.0]] [fillet 1.0]]`;

/** Progress callback type. */
export type ProgressCallback = (
  loaded: number,
  total: number,
  status: string,
  /** Download progress in bytes (only set during model download) */
  downloadBytes?: { loaded: number; total: number }
) => void;

/** Token streaming callback. */
export type TokenCallback = (token: string, partial: string) => void;

/** Inference result. */
export interface BrowserInferResult {
  ir: string;
  durationMs: number;
  fromCache: boolean;
}

/** Model loading state. */
interface ModelState {
  pipeline: unknown | null;
  loading: Promise<void> | null;
  loaded: boolean;
}

// Module-level state
const modelState: ModelState = {
  pipeline: null,
  loading: null,
  loaded: false,
};

/**
 * Open the IndexedDB database for model caching.
 */
function openDatabase(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, DB_VERSION);

    request.onerror = () => reject(request.error);
    request.onsuccess = () => resolve(request.result);

    request.onupgradeneeded = (event) => {
      const db = (event.target as IDBOpenDBRequest).result;
      if (!db.objectStoreNames.contains(STORE_NAME)) {
        db.createObjectStore(STORE_NAME);
      }
    };
  });
}

/**
 * Check if model is cached in IndexedDB.
 */
export async function isModelCached(): Promise<boolean> {
  try {
    const db = await openDatabase();
    return new Promise((resolve) => {
      const tx = db.transaction(STORE_NAME, "readonly");
      const store = tx.objectStore(STORE_NAME);
      const request = store.get(MODEL_CACHE_KEY);

      request.onerror = () => resolve(false);
      request.onsuccess = () => resolve(request.result != null);
    });
  } catch {
    return false;
  }
}

/**
 * Get the estimated model size in bytes.
 */
export function getModelSize(): number {
  // Qwen 0.5B quantized (ONNX) is approximately 350MB
  return 350 * 1024 * 1024;
}

/**
 * Check if WebGPU is available for acceleration.
 */
export async function isWebGPUAvailable(): Promise<boolean> {
  if (!("gpu" in navigator)) {
    return false;
  }
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const gpu = (navigator as any).gpu;
    const adapter = await gpu.requestAdapter();
    return adapter != null;
  } catch {
    return false;
  }
}

/**
 * Load the inference model with progress tracking.
 *
 * The model is loaded lazily on first use and cached in IndexedDB.
 * Subsequent calls return immediately if model is already loaded.
 */
export async function loadModel(
  onProgress?: ProgressCallback,
  modelId: string = DEFAULT_MODEL_ID,
): Promise<void> {
  // Already loaded
  if (modelState.loaded && modelState.pipeline) {
    return;
  }

  // Loading in progress - wait for it
  if (modelState.loading) {
    return modelState.loading;
  }

  // Start loading
  modelState.loading = (async () => {
    try {
      onProgress?.(0, 100, "Loading inference library...");

      // Dynamic import of transformers.js
      const { pipeline, env } = await import("@huggingface/transformers");

      // Configure cache location
      env.cacheDir = ".cache/transformers";
      env.allowLocalModels = false;

      // Check for WebGPU
      const hasWebGPU = await isWebGPUAvailable();
      const device = hasWebGPU ? "webgpu" : "wasm";

      onProgress?.(10, 100, `Downloading model (${device})...`);

      // Create the text generation pipeline
      modelState.pipeline = await pipeline("text-generation", modelId, {
        device,
        // Use default ONNX model (our int8 quantized version)
        progress_callback: (progress: { status: string; progress?: number; loaded?: number; total?: number }) => {
          if (progress.status === "progress" && progress.progress != null) {
            const pct = Math.round(10 + progress.progress * 0.85);
            // Pass through byte-level progress when available
            const downloadBytes = (progress.loaded != null && progress.total != null)
              ? { loaded: progress.loaded, total: progress.total }
              : undefined;
            onProgress?.(pct, 100, "Downloading model...", downloadBytes);
          } else if (progress.status === "ready") {
            onProgress?.(95, 100, "Initializing model...");
          }
        },
      });

      modelState.loaded = true;
      onProgress?.(100, 100, "Model ready");
    } catch (error) {
      modelState.loading = null;
      throw error;
    }
  })();

  return modelState.loading;
}

/**
 * Check if the model is currently loaded.
 */
export function isModelLoaded(): boolean {
  return modelState.loaded;
}

/**
 * Unload the model to free memory.
 */
export function unloadModel(): void {
  modelState.pipeline = null;
  modelState.loaded = false;
  modelState.loading = null;
}

/**
 * Generate Compact IR from a text prompt using browser inference.
 *
 * @param prompt - Text description of the desired CAD part
 * @param onToken - Optional callback for streaming tokens
 * @param onProgress - Optional callback for model loading progress
 * @returns The generated Compact IR
 */
export async function generateCAD(
  prompt: string,
  onToken?: TokenCallback,
  onProgress?: ProgressCallback,
): Promise<BrowserInferResult> {
  const startTime = performance.now();
  const wasCached = modelState.loaded;

  // Ensure model is loaded
  await loadModel(onProgress);

  if (!modelState.pipeline) {
    throw new Error("Failed to load model");
  }

  // Format the prompt for the model
  const messages = [
    { role: "system", content: SYSTEM_PROMPT },
    { role: "user", content: prompt },
  ];

  // Generate with the pipeline
  // @ts-expect-error - Pipeline type not fully typed
  const result = await modelState.pipeline(messages, {
    max_new_tokens: 256,
    temperature: 0.3,
    do_sample: true,
    return_full_text: false,
    // Streaming callback if provided
    ...(onToken && {
      callback_function: (output: { token: { text: string } }) => {
        const token = output?.token?.text ?? "";
        onToken(token, ""); // partial text tracking would require accumulation
      },
    }),
  });

  // Extract the generated text
  let ir = "";
  if (Array.isArray(result) && result[0]?.generated_text) {
    const generated = result[0].generated_text;
    // Handle chat format vs raw text
    if (typeof generated === "string") {
      ir = generated;
    } else if (Array.isArray(generated)) {
      // Chat format returns array of messages
      const lastMessage = generated[generated.length - 1];
      ir = lastMessage?.content ?? "";
    }
  }

  // Clean up the output
  ir = cleanGeneratedIR(ir);

  const durationMs = performance.now() - startTime;

  return {
    ir,
    durationMs,
    fromCache: wasCached,
  };
}

/**
 * Clean up generated IR text by removing markdown and extra whitespace.
 */
function cleanGeneratedIR(text: string): string {
  let ir = text.trim();

  // Remove markdown code blocks
  if (ir.startsWith("```")) {
    ir = ir.replace(/^```(?:ir|text|plaintext)?\n?/, "").replace(/\n?```$/, "");
  }

  // Remove any leading/trailing explanation text
  // Find the first line that looks like valid IR
  const lines = ir.split("\n");
  const validOpcodes = ["C", "Y", "S", "K", "T", "R", "X", "U", "D", "I", "LP", "CP", "SH", "SK", "E", "V"];

  let startIndex = 0;
  let endIndex = lines.length;

  // Find start of IR
  for (let i = 0; i < lines.length; i++) {
    const lineParts = lines[i]?.trim().split(/\s+/);
    const opcode = lineParts?.[0] ?? "";
    if (validOpcodes.includes(opcode)) {
      startIndex = i;
      break;
    }
  }

  // Find end of IR (last valid line)
  for (let i = lines.length - 1; i >= startIndex; i--) {
    const line = lines[i]?.trim() ?? "";
    if (!line) continue;
    const opcode = line.split(/\s+/)[0] ?? "";
    if (validOpcodes.includes(opcode) || opcode === "END" || opcode === "L" || opcode === "A") {
      endIndex = i + 1;
      break;
    }
  }

  return lines.slice(startIndex, endIndex).join("\n").trim();
}

/**
 * Validate that a Compact IR string is syntactically correct.
 * Returns null if valid, or an error message if invalid.
 */
export function validateCompactIR(ir: string): string | null {
  const lines = ir.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
  const validOpcodes = [
    "C", "Y", "S", "K", "T", "R", "X", "U", "D", "I",
    "LP", "CP", "SH", "SK", "L", "A", "E", "V", "END",
  ];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]?.trim() ?? "";
    const parts = line.split(/\s+/);
    const opcode = parts[0] ?? "";

    if (!validOpcodes.includes(opcode)) {
      return `Line ${i}: Unknown opcode "${opcode}"`;
    }
  }

  return null;
}

/**
 * Clear the model cache from IndexedDB.
 */
export async function clearModelCache(): Promise<void> {
  try {
    const db = await openDatabase();
    const tx = db.transaction(STORE_NAME, "readwrite");
    const store = tx.objectStore(STORE_NAME);
    store.delete(MODEL_CACHE_KEY);
    await new Promise<void>((resolve, reject) => {
      tx.oncomplete = () => resolve();
      tx.onerror = () => reject(tx.error);
    });
  } catch (error) {
    console.warn("Failed to clear model cache:", error);
  }

  // Also unload from memory
  unloadModel();
}

/**
 * Get information about the inference engine status.
 */
export async function getInferenceStatus(): Promise<{
  modelLoaded: boolean;
  modelCached: boolean;
  webgpuAvailable: boolean;
  estimatedModelSize: number;
}> {
  const [modelCached, webgpuAvailable] = await Promise.all([
    isModelCached(),
    isWebGPUAvailable(),
  ]);

  return {
    modelLoaded: isModelLoaded(),
    modelCached,
    webgpuAvailable,
    estimatedModelSize: getModelSize(),
  };
}
