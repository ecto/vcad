/**
 * Inference module - generates VCode from text prompts.
 * Supports multiple backends: HuggingFace, Anthropic, Ollama, Modal.
 */

import { generateText, streamText } from "ai";
import { createAnthropic } from "@ai-sdk/anthropic";
import { createOpenAI } from "@ai-sdk/openai";
import { gateway } from "@ai-sdk/gateway";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type LanguageModel = any;

/** Default models for each backend. */
export const DEFAULT_MODELS = {
  anthropic: "claude-3-5-haiku-20241022",
  ollama: "qwen2.5:3b",
  gateway: "anthropic/claude-3-5-haiku-latest",
  huggingface: "vcad/cad0-0.5b", // Future: our trained model
} as const;

/** System prompt explaining the VCode format for generation. */
const SYSTEM_PROMPT = `You are a CAD (Computer-Aided Design) code generator. You generate "VCode" - a text representation of 3D geometry.

VCode format:
- C x y z = Cube with dimensions x×y×z mm (corner at origin, extends in +X +Y +Z)
- Y r h = Cylinder with radius r mm and height h mm (base center at origin, +Z up)
- S r = Sphere with radius r mm (centered at origin)
- K rb rt h = Cone with bottom radius rb, top radius rt, height h (base at origin, +Z up)
- T n x y z = Translate node n by (x,y,z) mm
- R n x y z = Rotate node n by (x,y,z) degrees
- X n sx sy sz = Scale node n by (sx,sy,sz) factors
- U a b = Union (combine) nodes a and b
- D a b = Difference (subtract node b from node a)
- I a b = Intersection (overlap of nodes a and b)
- LP n dx dy dz count spacing = Linear pattern of node n
- CP n ox oy oz dx dy dz count angle = Circular pattern around axis

Node IDs are assigned sequentially starting from 0. Each line creates a new node.

Examples:

User: "50x30x5mm mounting plate"
Output:
C 50 30 5

User: "cube with a centered hole"
Output:
C 20 20 20
Y 5 20
T 1 10 10 0
D 0 2

User: "plate with 4 corner holes"
Output:
C 50 30 5
Y 3 5
T 1 5 5 0
Y 3 5
T 3 45 5 0
Y 3 5
T 5 5 25 0
Y 3 5
T 7 45 25 0
D 0 2
D 8 4
D 9 6
D 10 8

Output ONLY the VCode code. No explanations, no markdown, no comments.`;

/** Options for inference. */
export interface InferOptions {
  /** Maximum output tokens. */
  maxTokens?: number;
  /** Temperature for generation (0-1). */
  temperature?: number;
  /** Callback for streaming tokens. */
  onToken?: (token: string, partial: string) => void;
}

/** Result of inference. */
export interface InferResult {
  /** Generated VCode. */
  ir: string;
  /** Time taken in milliseconds. */
  durationMs: number;
  /** Model used. */
  model: string;
  /** Number of tokens generated (if available). */
  outputTokens?: number;
}

/**
 * Create an Anthropic model for inference.
 */
export function createAnthropicInferModel(
  modelId: string = DEFAULT_MODELS.anthropic,
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
 * Create an Ollama model for inference.
 */
export function createOllamaInferModel(
  modelId: string = DEFAULT_MODELS.ollama,
  baseURL: string = "http://localhost:11434/v1",
): LanguageModel {
  const ollama = createOpenAI({
    baseURL,
    apiKey: "ollama",
  });
  return ollama(modelId);
}

/**
 * Create a Gateway model for inference.
 */
export function createGatewayInferModel(
  modelId: string = DEFAULT_MODELS.gateway,
): LanguageModel {
  return gateway(modelId);
}

/**
 * Create a HuggingFace model for inference.
 * Uses the OpenAI-compatible inference API.
 */
export function createHuggingFaceInferModel(
  modelId: string = DEFAULT_MODELS.huggingface,
  apiKey?: string,
): LanguageModel {
  const key = apiKey || process.env.HF_TOKEN;
  if (!key) {
    throw new Error("HF_TOKEN environment variable not set");
  }

  const hf = createOpenAI({
    baseURL: "https://api-inference.huggingface.co/v1",
    apiKey: key,
  });
  return hf(modelId);
}

/**
 * Generate VCode from a text prompt using the specified model.
 */
export async function infer(
  model: LanguageModel,
  prompt: string,
  options: InferOptions = {},
): Promise<InferResult> {
  const { maxTokens = 512, temperature = 0.3 } = options;
  const startTime = performance.now();

  const response = await generateText({
    model,
    system: SYSTEM_PROMPT,
    prompt,
    maxOutputTokens: maxTokens,
    temperature,
  });

  const durationMs = performance.now() - startTime;

  // Clean up the response - remove any markdown formatting
  let ir = response.text.trim();
  if (ir.startsWith("```")) {
    // Remove markdown code blocks
    ir = ir.replace(/^```(?:ir|text)?\n?/, "").replace(/\n?```$/, "");
  }

  return {
    ir,
    durationMs,
    model: model.modelId || "unknown",
    outputTokens: response.usage?.outputTokens,
  };
}

/**
 * Generate VCode with streaming output.
 */
export async function inferStreaming(
  model: LanguageModel,
  prompt: string,
  options: InferOptions = {},
): Promise<InferResult> {
  const { maxTokens = 512, temperature = 0.3, onToken } = options;
  const startTime = performance.now();

  let fullText = "";

  const stream = streamText({
    model,
    system: SYSTEM_PROMPT,
    prompt,
    maxOutputTokens: maxTokens,
    temperature,
  });

  for await (const chunk of stream.textStream) {
    fullText += chunk;
    onToken?.(chunk, fullText);
  }

  const durationMs = performance.now() - startTime;

  // Clean up the response
  let ir = fullText.trim();
  if (ir.startsWith("```")) {
    ir = ir.replace(/^```(?:ir|text)?\n?/, "").replace(/\n?```$/, "");
  }

  const usage = await stream.usage;

  return {
    ir,
    durationMs,
    model: model.modelId || "unknown",
    outputTokens: usage?.outputTokens,
  };
}

/**
 * Validate that a VCode string is syntactically correct.
 * Returns null if valid, or an error message if invalid.
 */
export function validateVCode(ir: string): string | null {
  const lines = ir.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
  const validOpcodes = [
    "C",
    "Y",
    "S",
    "K",
    "T",
    "R",
    "X",
    "U",
    "D",
    "I",
    "LP",
    "CP",
    "SH",
    "SK",
    "L",
    "A",
    "E",
    "V",
    "END",
  ];

  for (let i = 0; i < lines.length; i++) {
    const line = lines[i].trim();
    const parts = line.split(/\s+/);
    const opcode = parts[0];

    if (!validOpcodes.includes(opcode)) {
      return `Line ${i}: Unknown opcode "${opcode}"`;
    }

    // Basic argument count validation
    const argCount = parts.length - 1;
    switch (opcode) {
      case "C":
        if (argCount !== 3) return `Line ${i}: C requires 3 args, got ${argCount}`;
        break;
      case "Y":
        if (argCount !== 2) return `Line ${i}: Y requires 2 args, got ${argCount}`;
        break;
      case "S":
        if (argCount !== 1) return `Line ${i}: S requires 1 arg, got ${argCount}`;
        break;
      case "K":
        if (argCount !== 3) return `Line ${i}: K requires 3 args, got ${argCount}`;
        break;
      case "T":
      case "R":
      case "X":
        if (argCount !== 4) return `Line ${i}: ${opcode} requires 4 args, got ${argCount}`;
        break;
      case "U":
      case "D":
      case "I":
        if (argCount !== 2) return `Line ${i}: ${opcode} requires 2 args, got ${argCount}`;
        break;
    }

    // Validate node references don't exceed current line number
    if (["T", "R", "X", "U", "D", "I", "LP", "CP", "SH"].includes(opcode)) {
      const refs = parts.slice(1).map(Number).filter((n) => !isNaN(n) && n < 100);
      for (const ref of refs) {
        if (ref >= i) {
          return `Line ${i}: Forward reference to node ${ref} not allowed`;
        }
      }
    }
  }

  return null;
}
