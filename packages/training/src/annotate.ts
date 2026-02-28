/**
 * Annotation pipeline - uses LLM to generate diverse text descriptions.
 * Supports Anthropic API and local Ollama models.
 */

import { generateText } from "ai";
import { createAnthropic } from "@ai-sdk/anthropic";
import { createOpenAI } from "@ai-sdk/openai";
import { gateway } from "@ai-sdk/gateway";
import type { GeneratedPart, TrainingExample } from "./generators/types.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type LanguageModel = any;

/** Default Anthropic model. */
export const DEFAULT_MODEL = "claude-3-5-haiku-20241022";

/** Default Ollama model. */
export const DEFAULT_OLLAMA_MODEL = "qwen2.5:3b";

/** Default gateway model - fast and cheap. */
export const DEFAULT_GATEWAY_MODEL = "anthropic/claude-3-5-haiku-latest";

/** System prompt explaining the VCode format. */
const SYSTEM_PROMPT = `You are describing CAD (Computer-Aided Design) mechanical parts.

The input is in "VCode" format - a text representation of 3D geometry:
- C x y z = Cube with dimensions x×y×z mm
- Y r h = Cylinder with radius r mm and height h mm
- S r = Sphere with radius r mm
- K r_bot r_top h = Cone with bottom radius, top radius, height
- T n x y z = Translate node n by (x,y,z) mm
- R n x y z = Rotate node n by (x,y,z) degrees
- X n sx sy sz = Scale node n by (sx,sy,sz) factors
- U a b = Union (combine) nodes a and b
- D a b = Difference (subtract b from a)
- I a b = Intersection of nodes a and b
- LP n dx dy dz count spacing = Linear pattern along direction
- CP n ox oy oz dx dy dz count angle = Circular pattern around axis
- SH n t = Shell node n with wall thickness t
- SK ... END = 2D sketch profile
- E n dx dy dz = Extrude sketch along direction
- V n ox oy oz dx dy dz angle = Revolve sketch around axis

Output ONLY a brief description of the physical part (1-2 sentences max). Do NOT explain the IR format or mention "VCode".`;

/** Prompts for generating diverse text descriptions. */
const ANNOTATION_PROMPTS = [
  "Describe this mechanical part technically with dimensions:",
  "Describe this part as a maker/hobbyist would:",
  "What search terms would find this part?",
  "One-line manufacturing spec:",
  "What could this part be used for?",
];

/** Rate limit delay between batches (ms). */
const RATE_LIMIT_DELAY = 20;

/** Batch size for parallel API calls. */
const BATCH_SIZE = 100;

/** Max retries per request. */
const MAX_RETRIES = 5;

/** Base delay for exponential backoff (ms). */
const RETRY_BASE_DELAY = 500;

/** Split array into chunks. */
function chunk<T>(arr: T[], size: number): T[][] {
  const chunks: T[][] = [];
  for (let i = 0; i < arr.length; i += size) {
    chunks.push(arr.slice(i, i + size));
  }
  return chunks;
}

/** Sleep for a given number of milliseconds. */
function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/**
 * Create an Anthropic language model.
 *
 * @param modelId - Model ID (e.g., "claude-3-5-haiku-20241022")
 * @param apiKey - Anthropic API key (defaults to ANTHROPIC_API_KEY env var)
 */
export function createAnthropicModel(
  modelId: string = DEFAULT_MODEL,
  apiKey?: string,
): LanguageModel {
  const key = apiKey || process.env.ANTHROPIC_API_KEY;

  if (!key) {
    throw new Error("ANTHROPIC_API_KEY environment variable not set");
  }

  const anthropic = createAnthropic({ apiKey: key });
  return anthropic(modelId);
}

/**
 * Create an Ollama language model (local).
 *
 * @param modelId - Model ID (e.g., "qwen2.5:3b", "llama3.2:3b")
 * @param baseURL - Ollama API URL (defaults to http://localhost:11434/v1)
 */
export function createOllamaModel(
  modelId: string = DEFAULT_OLLAMA_MODEL,
  baseURL: string = "http://localhost:11434/v1",
): LanguageModel {
  // Ollama exposes an OpenAI-compatible API
  const ollama = createOpenAI({
    baseURL,
    apiKey: "ollama", // Ollama doesn't need a real key
  });
  return ollama(modelId);
}

/**
 * Create a Vercel AI Gateway model.
 *
 * @param modelId - Model ID (e.g., "anthropic/claude-3-5-haiku-latest", "openai/gpt-4o-mini")
 */
export function createGatewayModel(
  modelId: string = DEFAULT_GATEWAY_MODEL,
): LanguageModel {
  return gateway(modelId);
}

/** Options for annotation. */
export interface AnnotateOptions {
  /** Language model to use. */
  model: LanguageModel;
  /** Number of prompts to use per part (1-5, default 5). */
  promptsPerPart?: number;
  /** Callback for progress updates. */
  onProgress?: (completed: number, total: number) => void;
  /** Callback for each generated example (for incremental writes). */
  onExample?: (example: TrainingExample) => void;
}

/**
 * Annotate generated parts with text descriptions using AI SDK.
 *
 * @param parts - Array of generated parts to annotate
 * @param options - Annotation options
 * @returns Array of training examples with text-IR pairs
 */
export async function annotate(
  parts: GeneratedPart[],
  options: AnnotateOptions,
): Promise<TrainingExample[]> {
  const { model, promptsPerPart = 5, onProgress, onExample } = options;

  const examples: TrainingExample[] = [];
  const prompts = ANNOTATION_PROMPTS.slice(0, promptsPerPart);
  const totalTasks = parts.length * prompts.length;
  let completed = 0;

  // Process in batches to respect rate limits
  const batches = chunk(parts, BATCH_SIZE);

  for (const batch of batches) {
    // Create all prompt-part combinations for this batch
    const tasks = batch.flatMap((part) =>
      prompts.map((prompt) => ({
        part,
        prompt: `${prompt}\n\nVCode:\n${part.vcode}`,
      })),
    );

    // Execute API calls in parallel within batch with retry logic
    const responses = await Promise.all(
      tasks.map(async (task) => {
        for (let attempt = 0; attempt < MAX_RETRIES; attempt++) {
          try {
            const response = await generateText({
              model,
              system: SYSTEM_PROMPT,
              prompt: task.prompt,
              maxOutputTokens: 150,
            });

            return { task, text: response.text.trim(), error: null };
          } catch (error) {
            const isRateLimit =
              error instanceof Error &&
              (error.message.includes("429") ||
                error.message.includes("rate") ||
                error.message.includes("Too Many"));

            if (isRateLimit && attempt < MAX_RETRIES - 1) {
              // Exponential backoff with jitter
              const delay =
                RETRY_BASE_DELAY * Math.pow(2, attempt) +
                Math.random() * 1000;
              await sleep(delay);
              continue;
            }
            return { task, text: null, error };
          }
        }
        return { task, text: null, error: new Error("Max retries exceeded") };
      }),
    );

    // Process responses
    for (const { task, text, error } of responses) {
      if (text && !error) {
        const example: TrainingExample = {
          text,
          ir: task.part.vcode,
          family: task.part.family,
          complexity: task.part.complexity,
        };
        examples.push(example);
        onExample?.(example);
      }
      completed++;
      onProgress?.(completed, totalTasks);
    }

    // Rate limit delay between batches
    if (batches.indexOf(batch) < batches.length - 1) {
      await sleep(RATE_LIMIT_DELAY);
    }
  }

  return examples;
}

/**
 * Generate a single synthetic description without calling the API.
 * Useful for testing or generating deterministic examples.
 */
export function generateSyntheticDescription(part: GeneratedPart): string {
  const params = part.params;

  switch (part.family) {
    case "plate": {
      const width = params.width as number;
      const depth = params.depth as number;
      const thickness = params.thickness as number;
      const pattern = params.holePattern as string;

      if (pattern === "none") {
        return `${width}x${depth}x${thickness}mm flat plate`;
      }
      const holeDiam = params.holeDiameter as number;
      return `${width}x${depth}x${thickness}mm mounting plate with ${pattern} ${holeDiam}mm holes`;
    }

    case "spacer": {
      const od = params.outerDiameter as number;
      const h = params.height as number;
      const type = params.spacerType as string;

      if (type === "solid") {
        return `${od}mm diameter ${h}mm tall standoff`;
      }
      const id = params.innerDiameter as number;
      if (type === "hollow") {
        return `${od}mm OD ${id}mm ID ${h}mm tall spacer tube`;
      }
      return `${od}mm flanged spacer ${h}mm tall`;
    }

    case "bracket": {
      const w = params.legWidth as number;
      const l1 = params.leg1Length as number;
      const l2 = params.leg2Length as number;
      const type = params.bracketType as string;
      const holes = params.hasHoles as boolean;

      let desc = `${l1}x${l2}mm L-bracket`;
      if (type === "gusseted") desc = "gusseted " + desc;
      if (holes) desc += " with mounting holes";
      return desc;
    }

    case "flange": {
      const od = params.outerDiameter as number;
      const thickness = params.thickness as number;
      const boltCount = params.boltCount as number;
      const type = params.flangeType as string;

      let desc = `${od}mm flange ${thickness}mm thick`;
      if (type === "hubbed") desc += " with hub";
      desc += ` ${boltCount}-bolt pattern`;
      return desc;
    }

    case "shaft": {
      const type = params.shaftType as string;
      const d1 = params.diameter1 as number;
      const l1 = params.length1 as number;

      if (type === "simple") {
        return `${d1}mm diameter ${l1}mm shaft`;
      }

      const d2 = params.diameter2 as number;
      const l2 = params.length2 as number;
      const hasKeyway = params.hasKeyway as boolean;

      let desc = `stepped shaft ${d1}/${d2}mm`;
      if (hasKeyway) desc += " with keyway";
      return desc;
    }

    case "enclosure": {
      const w = params.width as number;
      const d = params.depth as number;
      const h = params.height as number;
      const type = params.enclosureType as string;

      if (type === "lid") {
        return `${w}x${d}mm enclosure lid`;
      }
      let desc = `${w}x${d}x${h}mm enclosure box`;
      if (type === "boxWithStandoffs") desc += " with standoffs";
      if (type === "boxWithVents") desc += " with vents";
      return desc;
    }

    case "mount": {
      const type = params.mountType as string;
      const w = params.plateWidth as number;
      const h = params.plateHeight as number;

      if (type === "nema17") return "NEMA 17 motor mount plate";
      if (type === "nema23") return "NEMA 23 motor mount plate";
      if (type === "sensor") {
        const sd = params.sensorDiameter as number;
        return `${sd}mm sensor mount bracket`;
      }
      return `${w}x${h}mm adjustable mount plate`;
    }

    // New generators for expanded IR coverage

    case "ball": {
      const r = params.radius as number;
      const type = params.ballType as string;

      switch (type) {
        case "sphere":
          return `${r * 2}mm solid ball`;
        case "dome":
          return `${r * 2}mm dome/hemisphere`;
        case "drilled":
          const holeDiam = params.holeDiameter as number;
          return `${r * 2}mm ball with ${holeDiam}mm through-hole`;
        case "knob":
          return `${r * 2}mm spherical knob with flat base`;
        case "handle":
          return `${r * 2}mm ball handle with stem`;
        default:
          return `${r * 2}mm spherical part`;
      }
    }

    case "funnel": {
      const bottomR = params.bottomRadius as number;
      const topR = params.topRadius as number;
      const h = params.height as number;
      const type = params.funnelType as string;

      switch (type) {
        case "cone":
          return `${bottomR * 2}mm base ${h}mm tall cone`;
        case "frustum":
          return `${bottomR * 2}mm to ${topR * 2}mm truncated cone ${h}mm tall`;
        case "adapter":
          return `conical adapter ${bottomR * 2}mm to ${topR * 2}mm`;
        case "countersink":
          return `countersunk hole plate`;
        case "hopper":
          return `${bottomR * 2}mm hollow funnel/hopper`;
        default:
          return `conical part ${h}mm tall`;
      }
    }

    case "clip": {
      const type = params.clipType as string;

      switch (type) {
        case "saddle":
          const pipeDiam = params.pipeDiameter as number;
          return `pipe saddle for ${pipeDiam}mm pipe`;
        case "rounded":
          return `rounded corner block`;
        case "quarter":
          return `quarter-round trim profile`;
        case "channel":
          return `stepped channel block`;
        case "lens":
          return `lens-shaped intersection`;
        default:
          return `intersection-based clip`;
      }
    }

    case "scaled": {
      const type = params.scaledType as string;
      const baseSize = params.baseSize as number;

      switch (type) {
        case "ellipse":
          return `${baseSize}mm elliptical disc`;
        case "stretched":
          return `stretched rectangular block`;
        case "ellipsoid":
          return `${baseSize}mm ellipsoid`;
        case "tapered":
          return `tapered stepped block`;
        case "oval":
          return `oval cross-section tube`;
        default:
          return `scaled geometric part`;
      }
    }

    case "array": {
      const type = params.arrayType as string;
      const count = params.count as number;
      const length = params.length as number;

      switch (type) {
        case "rail":
          return `${length}mm rail with ${count} evenly-spaced holes`;
        case "din":
          return `${length}mm DIN rail mounting strip with ${count} slots`;
        case "rack":
          return `${length}mm toothed rack with ${count} teeth`;
        case "perforated":
          return `${length}mm perforated bar with ${count} holes`;
        case "slotted":
          return `${length}mm slotted plate with ${count} slots`;
        default:
          return `linear pattern part with ${count} features`;
      }
    }

    case "radial": {
      const type = params.radialType as string;
      const count = params.count as number;
      const outerDiam = params.outerDiameter as number;

      switch (type) {
        case "boltCircle":
          return `${outerDiam}mm flange with ${count}-bolt pattern`;
        case "spoked":
          return `${outerDiam}mm ${count}-spoke wheel`;
        case "fan":
          return `${count}-blade fan impeller`;
        case "ventilation":
          return `${outerDiam}mm ventilation disc with ${count} holes`;
        case "star":
          return `${count}-point star shape`;
        default:
          return `circular pattern with ${count} elements`;
      }
    }

    case "hollow": {
      const type = params.hollowType as string;
      const wallThickness = params.wallThickness as number;

      switch (type) {
        case "box":
          const w = params.outerWidth as number;
          const d = params.outerDepth as number;
          const h = params.outerHeight as number;
          return `${w}x${d}x${h}mm hollow box ${wallThickness}mm walls`;
        case "tube":
          const od = params.outerDiameter as number;
          const th = params.outerHeight as number;
          return `${od}mm diameter ${th}mm tall tube ${wallThickness}mm wall`;
        case "cup":
          return `cup/container with ${wallThickness}mm walls`;
        case "housing":
          return `electronics housing with mounting tabs`;
        case "domeShell":
          return `dome shell ${wallThickness}mm thick`;
        default:
          return `hollow shelled part`;
      }
    }

    case "profile": {
      const type = params.profileType as string;
      const length = params.length as number;

      switch (type) {
        case "lChannel":
          return `${length}mm L-channel extrusion`;
        case "tSlot":
          return `${length}mm T-slot profile`;
        case "cChannel":
          return `${length}mm C-channel extrusion`;
        case "iBeam":
          return `${length}mm I-beam profile`;
        case "polygon":
          const sides = params.sides as number;
          return `${length}mm ${sides}-sided polygon extrusion`;
        default:
          return `${length}mm extruded profile`;
      }
    }

    case "turned": {
      const type = params.turnedType as string;
      const h = params.height as number;

      switch (type) {
        case "bottle":
          return `${h}mm tall bottle/vase shape`;
        case "pulley":
          const maxR = params.maxRadius as number;
          return `${maxR * 2}mm pulley with V-groove`;
        case "knob":
          return `${h}mm tall turned knob`;
        case "steppedShaft":
          const steps = params.steps as number;
          return `${h}mm ${steps}-step turned shaft`;
        case "bowl":
          return `${h}mm tall bowl shape`;
        default:
          return `${h}mm revolved/turned part`;
      }
    }

    default:
      return `${part.family} part`;
  }
}

/**
 * Generate training examples without API calls using synthetic descriptions.
 * Useful for testing or when API access is not available.
 */
export function generateSyntheticExamples(
  parts: GeneratedPart[],
): TrainingExample[] {
  return parts.map((part) => ({
    text: generateSyntheticDescription(part),
    ir: part.vcode,
    family: part.family,
    complexity: part.complexity,
  }));
}
