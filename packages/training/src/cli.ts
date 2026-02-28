#!/usr/bin/env node
/**
 * CLI for synthetic training data generation.
 */

import { Command } from "commander";
import * as fs from "node:fs";
import * as path from "node:path";
import * as readline from "node:readline";
import {
  generators,
  generatorFamilies,
  defaultCounts,
  type GeneratedPart,
  type TrainingExample,
} from "./generators/index.js";
import {
  annotate,
  generateSyntheticExamples,
  createAnthropicModel,
  createOllamaModel,
  createGatewayModel,
  DEFAULT_MODEL,
  DEFAULT_OLLAMA_MODEL,
  DEFAULT_GATEWAY_MODEL,
} from "./annotate.js";
import {
  validateExamples,
  computeValidationStats,
  filterValidExamples,
} from "./validate.js";
import {
  infer,
  inferStreaming,
  validateVCode,
  createAnthropicInferModel,
  createOllamaInferModel,
  createGatewayInferModel,
  createHuggingFaceInferModel,
  DEFAULT_MODELS,
} from "./infer.js";
import {
  generateConversations,
  toShareGPTFormat,
  type ConversationExample,
} from "./conversation.js";
import {
  generateImageIRPairs,
  writeMetadata,
  computeMultimodalStats,
  type ImageIRPair,
} from "./multimodal.js";
import { Renderer } from "./render.js";

const program = new Command();

program
  .name("vcad-training")
  .description("Synthetic training data generation for cad0")
  .version("0.1.0");

/**
 * Generate command - creates raw IR examples without annotation.
 */
program
  .command("generate")
  .description("Generate raw VCode examples")
  .option("-f, --family <family>", "Part family to generate", "all")
  .option("-c, --count <count>", "Number of examples to generate", "100")
  .option("-o, --output <path>", "Output file path", "data/raw/output.jsonl")
  .action(async (options) => {
    const count = parseInt(options.count, 10);
    const outputPath = path.resolve(options.output);

    // Ensure output directory exists
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });

    const families =
      options.family === "all"
        ? generatorFamilies
        : [options.family];

    console.log(`Generating ${count} examples from families: ${families.join(", ")}`);

    const parts: GeneratedPart[] = [];
    const countsPerFamily =
      options.family === "all"
        ? distributeCount(count, families)
        : { [options.family]: count };

    for (const family of families) {
      const generator = generators[family];
      if (!generator) {
        console.error(`Unknown family: ${family}`);
        process.exit(1);
      }

      const familyCount = countsPerFamily[family];
      console.log(`  ${family}: ${familyCount} examples`);

      for (let i = 0; i < familyCount; i++) {
        parts.push(generator.generate());
      }
    }

    // Write to JSONL
    const output = fs.createWriteStream(outputPath);
    for (const part of parts) {
      output.write(JSON.stringify(part) + "\n");
    }
    output.end();

    console.log(`\nWrote ${parts.length} examples to ${outputPath}`);
  });

/**
 * Annotate command - adds text descriptions using Claude API.
 */
program
  .command("annotate")
  .description("Annotate raw IR with text descriptions")
  .option("-i, --input <path>", "Input JSONL file", "data/raw/output.jsonl")
  .option("-o, --output <path>", "Output JSONL file", "data/annotated/output.jsonl")
  .option("--synthetic", "Use synthetic descriptions instead of API", false)
  .option("--ollama [model]", "Use local Ollama model")
  .option("--gateway [model]", "Use Vercel AI Gateway (e.g., anthropic/claude-3-5-haiku-latest)")
  .option("--prompts <count>", "Number of prompts per part (1-5)", "5")
  .option(
    "-m, --model <model>",
    "Anthropic model ID",
    DEFAULT_MODEL,
  )
  .action(async (options) => {
    const inputPath = path.resolve(options.input);
    const outputPath = path.resolve(options.output);

    // Ensure output directory exists
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });

    // Read input
    const parts = await readJsonlFile<GeneratedPart>(inputPath);
    console.log(`Read ${parts.length} parts from ${inputPath}`);

    let examples: TrainingExample[];

    if (options.synthetic) {
      console.log("Generating synthetic descriptions...");
      examples = generateSyntheticExamples(parts);
    } else if (options.ollama !== undefined) {
      // Use Ollama (local model)
      const modelId = typeof options.ollama === "string" ? options.ollama : DEFAULT_OLLAMA_MODEL;

      try {
        const model = createOllamaModel(modelId);
        console.log(`Annotating with Ollama ${modelId}...`);

        const promptsPerPart = Math.min(
          5,
          Math.max(1, parseInt(options.prompts, 10)),
        );

        // Open file for incremental writes
        const output = fs.createWriteStream(outputPath);

        examples = await annotate(parts, {
          model,
          promptsPerPart,
          onProgress: (completed, total) => {
            process.stdout.write(`\r  Progress: ${completed}/${total}`);
          },
          onExample: (example) => {
            output.write(JSON.stringify(example) + "\n");
          },
        });
        output.end();
        console.log("");
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        console.error("Make sure Ollama is running: ollama serve");
        process.exit(1);
      }
    } else if (options.gateway !== undefined) {
      // Use Vercel AI Gateway
      const modelId = typeof options.gateway === "string" ? options.gateway : DEFAULT_GATEWAY_MODEL;

      try {
        const model = createGatewayModel(modelId);
        console.log(`Annotating with Gateway ${modelId}...`);

        const promptsPerPart = Math.min(
          5,
          Math.max(1, parseInt(options.prompts, 10)),
        );

        // Open file for incremental writes
        const output = fs.createWriteStream(outputPath);

        examples = await annotate(parts, {
          model,
          promptsPerPart,
          onProgress: (completed, total) => {
            process.stdout.write(`\r  Progress: ${completed}/${total}`);
          },
          onExample: (example) => {
            output.write(JSON.stringify(example) + "\n");
          },
        });
        output.end();
        console.log("");
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        console.error("Check AI_GATEWAY_API_KEY env var");
        process.exit(1);
      }
    } else {
      // Use Anthropic API
      const modelId = options.model as string;

      try {
        const model = createAnthropicModel(modelId);
        console.log(`Annotating with ${modelId}...`);

        const promptsPerPart = Math.min(
          5,
          Math.max(1, parseInt(options.prompts, 10)),
        );

        // Open file for incremental writes
        const output = fs.createWriteStream(outputPath);

        examples = await annotate(parts, {
          model,
          promptsPerPart,
          onProgress: (completed, total) => {
            process.stdout.write(`\r  Progress: ${completed}/${total}`);
          },
          onExample: (example) => {
            output.write(JSON.stringify(example) + "\n");
          },
        });
        output.end();
        console.log("");
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        console.error("Use --synthetic or --ollama for local generation");
        process.exit(1);
      }
    }

    console.log(`Wrote ${examples.length} examples to ${outputPath}`);
  });

/**
 * Validate command - checks generated examples for validity.
 */
program
  .command("validate")
  .description("Validate training examples")
  .option("-i, --input <path>", "Input JSONL file", "data/annotated/output.jsonl")
  .option("--geometry", "Validate geometry with engine (slower)", false)
  .action(async (options) => {
    const inputPath = path.resolve(options.input);

    const examples = await readJsonlFile<TrainingExample>(inputPath);
    console.log(`Validating ${examples.length} examples from ${inputPath}`);

    let engine;
    if (options.geometry) {
      console.log("Loading engine for geometry validation...");
      const { Engine } = await import("@vcad/engine");
      engine = await Engine.init();
    }

    const results = await validateExamples(
      examples,
      {
        onProgress: (completed, total, errors) => {
          process.stdout.write(`\r  Progress: ${completed}/${total} (${errors} errors)`);
        },
      },
      engine,
    );
    console.log("");

    const stats = computeValidationStats(results);

    console.log("\nValidation Statistics:");
    console.log(`  Total: ${stats.total}`);
    console.log(`  Valid: ${stats.valid} (${((stats.valid / stats.total) * 100).toFixed(1)}%)`);
    console.log(`  Invalid: ${stats.invalid}`);

    if (Object.keys(stats.errorCounts).length > 0) {
      console.log("\nError breakdown:");
      for (const [error, count] of Object.entries(stats.errorCounts)) {
        console.log(`  ${error}: ${count}`);
      }
    }

    if (stats.avgVolume !== undefined) {
      console.log(`\nAverage bounding box volume: ${stats.avgVolume.toFixed(2)} mm³`);
    }
    if (stats.avgTriangles !== undefined) {
      console.log(`Average triangle count: ${Math.round(stats.avgTriangles)}`);
    }
  });

/**
 * Pipeline command - full generation + annotation + split pipeline.
 */
program
  .command("pipeline")
  .description("Run full data generation pipeline")
  .option("-c, --count <count>", "Total examples to generate", "1000")
  .option("-o, --output <dir>", "Output directory", "data")
  .option("--synthetic", "Use synthetic descriptions instead of API", false)
  .option("--validate", "Validate examples before splitting", false)
  .option(
    "-m, --model <model>",
    "Anthropic model ID",
    DEFAULT_MODEL,
  )
  .action(async (options) => {
    const totalCount = parseInt(options.count, 10);
    const outputDir = path.resolve(options.output);

    console.log(`Running pipeline for ${totalCount} examples`);
    console.log(`Output directory: ${outputDir}`);

    // Ensure directories exist
    fs.mkdirSync(path.join(outputDir, "raw"), { recursive: true });
    fs.mkdirSync(path.join(outputDir, "annotated"), { recursive: true });

    // 1. Generate
    console.log("\n=== Step 1: Generate ===");
    const parts: GeneratedPart[] = [];
    const countsPerFamily = distributeCount(totalCount, generatorFamilies);

    for (const family of generatorFamilies) {
      const generator = generators[family];
      const familyCount = countsPerFamily[family];
      console.log(`  ${family}: ${familyCount} examples`);

      for (let i = 0; i < familyCount; i++) {
        parts.push(generator.generate());
      }
    }

    // Save raw
    const rawPath = path.join(outputDir, "raw", "all.jsonl");
    writeJsonlFile(rawPath, parts);
    console.log(`  Wrote ${parts.length} parts to ${rawPath}`);

    // 2. Annotate
    console.log("\n=== Step 2: Annotate ===");
    let examples: TrainingExample[];

    if (options.synthetic) {
      console.log("  Using synthetic descriptions");
      examples = generateSyntheticExamples(parts);
    } else {
      const modelId = options.model as string;

      try {
        const model = createAnthropicModel(modelId);
        console.log(`  Using ${modelId}`);

        examples = await annotate(parts, {
          model,
          promptsPerPart: 5,
          onProgress: (completed, total) => {
            process.stdout.write(`\r  Progress: ${completed}/${total}`);
          },
        });
        console.log("");
      } catch (error) {
        console.log(`  ${(error as Error).message}`);
        console.log("  Falling back to synthetic descriptions");
        examples = generateSyntheticExamples(parts);
      }
    }

    // Save annotated
    const annotatedPath = path.join(outputDir, "annotated", "all.jsonl");
    writeJsonlFile(annotatedPath, examples);
    console.log(`  Wrote ${examples.length} examples to ${annotatedPath}`);

    // 3. Validate (optional)
    if (options.validate) {
      console.log("\n=== Step 3: Validate ===");
      const results = await validateExamples(examples, {
        onProgress: (completed, total, errors) => {
          process.stdout.write(`\r  Progress: ${completed}/${total} (${errors} errors)`);
        },
      });
      console.log("");

      const validExamples = filterValidExamples(examples, results);
      const stats = computeValidationStats(results);
      console.log(`  Valid: ${stats.valid}/${stats.total}`);
      examples = validExamples;
    }

    // 4. Split
    console.log("\n=== Step 4: Split ===");
    const shuffled = shuffle(examples);
    const trainEnd = Math.floor(shuffled.length * 0.9);
    const valEnd = Math.floor(shuffled.length * 0.95);

    const train = shuffled.slice(0, trainEnd);
    const val = shuffled.slice(trainEnd, valEnd);
    const test = shuffled.slice(valEnd);

    writeJsonlFile(path.join(outputDir, "train.jsonl"), train);
    writeJsonlFile(path.join(outputDir, "val.jsonl"), val);
    writeJsonlFile(path.join(outputDir, "test.jsonl"), test);

    console.log(`  train: ${train.length} examples`);
    console.log(`  val: ${val.length} examples`);
    console.log(`  test: ${test.length} examples`);

    console.log("\n=== Pipeline Complete ===");
  });

/**
 * Split command - split annotated data into train/val/test sets.
 */
program
  .command("split")
  .description("Split annotated data into train/val/test sets")
  .option("-i, --input <path>", "Input JSONL file", "data/annotated/all.jsonl")
  .option("-o, --output <dir>", "Output directory", "data")
  .option("--train <ratio>", "Training set ratio", "0.9")
  .option("--val <ratio>", "Validation set ratio", "0.05")
  .option("--seed <seed>", "Random seed for reproducibility", "42")
  .option("--stratify", "Stratify split by family for balanced distribution", false)
  .action(async (options) => {
    const inputPath = path.resolve(options.input);
    const outputDir = path.resolve(options.output);
    const trainRatio = parseFloat(options.train);
    const valRatio = parseFloat(options.val);
    const testRatio = 1 - trainRatio - valRatio;
    const seed = parseInt(options.seed, 10);

    if (trainRatio + valRatio >= 1) {
      console.error("Error: train + val ratios must be less than 1");
      process.exit(1);
    }

    if (testRatio < 0) {
      console.error("Error: invalid ratios (train + val > 1)");
      process.exit(1);
    }

    // Read input
    const examples = await readJsonlFile<TrainingExample>(inputPath);
    console.log(`Read ${examples.length} examples from ${inputPath}`);
    console.log(`Split ratios: train=${trainRatio}, val=${valRatio}, test=${testRatio.toFixed(2)}`);
    console.log(`Random seed: ${seed}`);

    // Ensure output directory exists
    fs.mkdirSync(outputDir, { recursive: true });

    let train: TrainingExample[];
    let val: TrainingExample[];
    let test: TrainingExample[];

    if (options.stratify) {
      console.log("Using stratified split by family...");
      const result = stratifiedSplit(examples, trainRatio, valRatio, seed);
      train = result.train;
      val = result.val;
      test = result.test;
    } else {
      // Simple random split
      const shuffled = seededShuffle(examples, seed);
      const trainEnd = Math.floor(shuffled.length * trainRatio);
      const valEnd = Math.floor(shuffled.length * (trainRatio + valRatio));

      train = shuffled.slice(0, trainEnd);
      val = shuffled.slice(trainEnd, valEnd);
      test = shuffled.slice(valEnd);
    }

    // Write output files
    const trainPath = path.join(outputDir, "train.jsonl");
    const valPath = path.join(outputDir, "val.jsonl");
    const testPath = path.join(outputDir, "test.jsonl");

    writeJsonlFile(trainPath, train);
    writeJsonlFile(valPath, val);
    writeJsonlFile(testPath, test);

    console.log(`\nOutput files:`);
    console.log(`  ${trainPath}: ${train.length} examples`);
    console.log(`  ${valPath}: ${val.length} examples`);
    console.log(`  ${testPath}: ${test.length} examples`);

    // Print distribution stats
    console.log("\n=== Distribution Statistics ===");
    printSplitStats("Train", train);
    printSplitStats("Val", val);
    printSplitStats("Test", test);
  });

/**
 * Stats command - show statistics about generated data.
 */
program
  .command("stats")
  .description("Show statistics about training data")
  .option("-i, --input <path>", "Input JSONL file", "data/train.jsonl")
  .action(async (options) => {
    const inputPath = path.resolve(options.input);
    const examples = await readJsonlFile<TrainingExample>(inputPath);

    console.log(`Statistics for ${inputPath}`);
    console.log(`Total examples: ${examples.length}\n`);

    // By family
    const byFamily: Record<string, number> = {};
    const byComplexity: Record<number, number> = {};

    for (const ex of examples) {
      byFamily[ex.family] = (byFamily[ex.family] || 0) + 1;
      byComplexity[ex.complexity] = (byComplexity[ex.complexity] || 0) + 1;
    }

    console.log("By family:");
    for (const [family, count] of Object.entries(byFamily).sort((a, b) => b[1] - a[1])) {
      console.log(`  ${family}: ${count} (${((count / examples.length) * 100).toFixed(1)}%)`);
    }

    console.log("\nBy complexity:");
    for (const [complexity, count] of Object.entries(byComplexity).sort()) {
      console.log(`  ${complexity}: ${count} (${((count / examples.length) * 100).toFixed(1)}%)`);
    }

    // Text length stats
    const textLengths = examples.map((e) => e.text.length);
    const irLengths = examples.map((e) => e.ir.length);

    console.log("\nText lengths:");
    console.log(`  Min: ${Math.min(...textLengths)}`);
    console.log(`  Max: ${Math.max(...textLengths)}`);
    console.log(`  Avg: ${Math.round(textLengths.reduce((a, b) => a + b, 0) / textLengths.length)}`);

    console.log("\nIR lengths:");
    console.log(`  Min: ${Math.min(...irLengths)}`);
    console.log(`  Max: ${Math.max(...irLengths)}`);
    console.log(`  Avg: ${Math.round(irLengths.reduce((a, b) => a + b, 0) / irLengths.length)}`);
  });

// Helper functions

async function readJsonlFile<T>(filePath: string): Promise<T[]> {
  const items: T[] = [];
  const fileStream = fs.createReadStream(filePath);
  const rl = readline.createInterface({
    input: fileStream,
    crlfDelay: Infinity,
  });

  for await (const line of rl) {
    if (line.trim()) {
      items.push(JSON.parse(line) as T);
    }
  }

  return items;
}

function writeJsonlFile<T>(filePath: string, items: T[]): void {
  const output = fs.createWriteStream(filePath);
  for (const item of items) {
    output.write(JSON.stringify(item) + "\n");
  }
  output.end();
}

function distributeCount(
  total: number,
  families: string[],
): Record<string, number> {
  // Distribute proportionally based on defaultCounts
  const totalDefault = Object.values(defaultCounts).reduce((a, b) => a + b, 0);
  const result: Record<string, number> = {};
  let remaining = total;

  for (let i = 0; i < families.length; i++) {
    const family = families[i];
    if (i === families.length - 1) {
      result[family] = remaining;
    } else {
      const proportion = (defaultCounts[family] || 1000) / totalDefault;
      const count = Math.round(total * proportion);
      result[family] = count;
      remaining -= count;
    }
  }

  return result;
}

function shuffle<T>(array: T[]): T[] {
  const result = [...array];
  for (let i = result.length - 1; i > 0; i--) {
    const j = Math.floor(Math.random() * (i + 1));
    [result[i], result[j]] = [result[j], result[i]];
  }
  return result;
}

/**
 * Seeded random number generator (mulberry32).
 */
function createSeededRandom(seed: number): () => number {
  let state = seed;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/**
 * Shuffle array with a seeded random number generator.
 */
function seededShuffle<T>(array: T[], seed: number): T[] {
  const result = [...array];
  const random = createSeededRandom(seed);
  for (let i = result.length - 1; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    [result[i], result[j]] = [result[j], result[i]];
  }
  return result;
}

/**
 * Stratified split that maintains family distribution across splits.
 */
function stratifiedSplit(
  examples: TrainingExample[],
  trainRatio: number,
  valRatio: number,
  seed: number,
): { train: TrainingExample[]; val: TrainingExample[]; test: TrainingExample[] } {
  // Group by family
  const byFamily: Record<string, TrainingExample[]> = {};
  for (const ex of examples) {
    if (!byFamily[ex.family]) {
      byFamily[ex.family] = [];
    }
    byFamily[ex.family].push(ex);
  }

  const train: TrainingExample[] = [];
  const val: TrainingExample[] = [];
  const test: TrainingExample[] = [];

  // Split each family proportionally
  let familySeed = seed;
  for (const family of Object.keys(byFamily).sort()) {
    const familyExamples = seededShuffle(byFamily[family], familySeed++);
    const trainEnd = Math.floor(familyExamples.length * trainRatio);
    const valEnd = Math.floor(familyExamples.length * (trainRatio + valRatio));

    // Use for loop instead of spread to avoid stack overflow with large arrays
    for (let i = 0; i < trainEnd; i++) {
      train.push(familyExamples[i]);
    }
    for (let i = trainEnd; i < valEnd; i++) {
      val.push(familyExamples[i]);
    }
    for (let i = valEnd; i < familyExamples.length; i++) {
      test.push(familyExamples[i]);
    }
  }

  // Shuffle the final arrays to mix families
  return {
    train: seededShuffle(train, seed),
    val: seededShuffle(val, seed + 1),
    test: seededShuffle(test, seed + 2),
  };
}

/**
 * Print distribution statistics for a split.
 */
function printSplitStats(name: string, examples: TrainingExample[]): void {
  if (examples.length === 0) {
    console.log(`\n${name}: 0 examples`);
    return;
  }

  console.log(`\n${name} (${examples.length} examples):`);

  // By family
  const byFamily: Record<string, number> = {};
  const byComplexity: Record<number, number> = {};

  for (const ex of examples) {
    byFamily[ex.family] = (byFamily[ex.family] || 0) + 1;
    byComplexity[ex.complexity] = (byComplexity[ex.complexity] || 0) + 1;
  }

  console.log("  By family:");
  for (const [family, count] of Object.entries(byFamily).sort((a, b) => b[1] - a[1])) {
    console.log(`    ${family}: ${count} (${((count / examples.length) * 100).toFixed(1)}%)`);
  }

  console.log("  By complexity:");
  for (const [complexity, count] of Object.entries(byComplexity).sort()) {
    console.log(`    ${complexity}: ${count} (${((count / examples.length) * 100).toFixed(1)}%)`);
  }
}

/**
 * Infer command - generate VCode from a text prompt.
 */
program
  .command("infer")
  .description("Generate VCode from a text prompt")
  .requiredOption("-p, --prompt <prompt>", "Text description of the CAD part")
  .option("--ollama [model]", "Use local Ollama model")
  .option("--gateway [model]", "Use Vercel AI Gateway")
  .option("--huggingface [model]", "Use HuggingFace Inference API")
  .option("-m, --model <model>", "Anthropic model ID", DEFAULT_MODELS.anthropic)
  .option("--stream", "Stream output tokens", false)
  .option("--validate", "Validate the generated IR", false)
  .option("--json", "Output as JSON", false)
  .option("-o, --output <path>", "Write IR to file")
  .action(async (options) => {
    const prompt = options.prompt as string;

    // Select backend and create model
    let model;
    let backendName: string;

    if (options.ollama !== undefined) {
      const modelId = typeof options.ollama === "string" ? options.ollama : DEFAULT_MODELS.ollama;
      backendName = `ollama/${modelId}`;
      try {
        model = createOllamaInferModel(modelId);
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        console.error("Make sure Ollama is running: ollama serve");
        process.exit(1);
      }
    } else if (options.gateway !== undefined) {
      const modelId = typeof options.gateway === "string" ? options.gateway : DEFAULT_MODELS.gateway;
      backendName = `gateway/${modelId}`;
      try {
        model = createGatewayInferModel(modelId);
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        process.exit(1);
      }
    } else if (options.huggingface !== undefined) {
      const modelId = typeof options.huggingface === "string" ? options.huggingface : DEFAULT_MODELS.huggingface;
      backendName = `huggingface/${modelId}`;
      try {
        model = createHuggingFaceInferModel(modelId);
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        console.error("Set HF_TOKEN environment variable");
        process.exit(1);
      }
    } else {
      const modelId = options.model as string;
      backendName = `anthropic/${modelId}`;
      try {
        model = createAnthropicInferModel(modelId);
      } catch (error) {
        console.error(`Error: ${(error as Error).message}`);
        console.error("Set ANTHROPIC_API_KEY or use --ollama for local inference");
        process.exit(1);
      }
    }

    if (!options.json) {
      console.log(`Using ${backendName}`);
      console.log(`Prompt: "${prompt}"`);
      console.log("");
    }

    try {
      let result;

      if (options.stream && !options.json) {
        // Streaming mode - show tokens as they arrive
        process.stdout.write("Generated IR:\n");
        result = await inferStreaming(model, prompt, {
          onToken: (token) => {
            process.stdout.write(token);
          },
        });
        console.log("\n");
      } else {
        // Non-streaming mode
        result = await infer(model, prompt);
        if (!options.json) {
          console.log("Generated IR:");
          console.log(result.ir);
          console.log("");
        }
      }

      // Validate if requested
      let validationError: string | null = null;
      if (options.validate) {
        validationError = validateVCode(result.ir);
        if (!options.json) {
          if (validationError) {
            console.log(`Validation: FAILED - ${validationError}`);
          } else {
            console.log("Validation: PASSED");
          }
        }
      }

      // Output as JSON if requested
      if (options.json) {
        const output = {
          prompt,
          ir: result.ir,
          model: backendName,
          durationMs: result.durationMs,
          outputTokens: result.outputTokens,
          ...(options.validate && { valid: validationError === null, validationError }),
        };
        console.log(JSON.stringify(output, null, 2));
      } else {
        console.log(`Duration: ${result.durationMs.toFixed(0)}ms`);
        if (result.outputTokens) {
          console.log(`Output tokens: ${result.outputTokens}`);
        }
      }

      // Write to file if requested
      if (options.output) {
        fs.writeFileSync(options.output, result.ir);
        if (!options.json) {
          console.log(`\nWrote IR to ${options.output}`);
        }
      }
    } catch (error) {
      console.error(`Inference failed: ${(error as Error).message}`);
      process.exit(1);
    }
  });

/**
 * Generate-conversations command - creates multi-turn conversation data.
 */
program
  .command("generate-conversations")
  .description("Generate multi-turn conversation training data")
  .option("-c, --count <count>", "Number of conversations to generate", "1000")
  .option("-o, --output <path>", "Output file path", "data/conversations.jsonl")
  .option("-f, --families <families>", "Comma-separated list of families", "all")
  .option("--min-turns <turns>", "Minimum turns per conversation", "2")
  .option("--max-turns <turns>", "Maximum turns per conversation", "4")
  .option("--sharegpt", "Output in ShareGPT format", false)
  .action(async (options) => {
    const count = parseInt(options.count, 10);
    const outputPath = path.resolve(options.output);
    const minTurns = parseInt(options.minTurns, 10);
    const maxTurns = parseInt(options.maxTurns, 10);

    // Ensure output directory exists
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });

    const families =
      options.families === "all"
        ? generatorFamilies
        : (options.families as string).split(",").map((f: string) => f.trim());

    console.log(`Generating ${count} conversations`);
    console.log(`  Families: ${families.join(", ")}`);
    console.log(`  Turns: ${minTurns}-${maxTurns}`);

    const conversations = generateConversations(count, {
      families,
      minTurns,
      maxTurns,
      onProgress: (completed, total) => {
        process.stdout.write(`\r  Progress: ${completed}/${total}`);
      },
    });
    console.log("");

    // Write to JSONL
    const output = fs.createWriteStream(outputPath);
    for (const conv of conversations) {
      if (options.sharegpt) {
        output.write(JSON.stringify(toShareGPTFormat(conv)) + "\n");
      } else {
        output.write(JSON.stringify(conv) + "\n");
      }
    }
    output.end();

    // Print stats
    const avgTurns = conversations.reduce((sum, c) => sum + c.turns, 0) / conversations.length;
    const byFamily: Record<string, number> = {};
    for (const conv of conversations) {
      byFamily[conv.family] = (byFamily[conv.family] || 0) + 1;
    }

    console.log(`\nWrote ${conversations.length} conversations to ${outputPath}`);
    console.log(`  Average turns: ${avgTurns.toFixed(1)}`);
    console.log(`  By family:`);
    for (const [family, familyCount] of Object.entries(byFamily).sort((a, b) => b[1] - a[1])) {
      console.log(`    ${family}: ${familyCount}`);
    }
  });

/**
 * Convert-conversations command - flattens multi-turn conversations to single-turn training examples.
 * Each turn in a conversation becomes a separate training example with full context.
 */
program
  .command("convert-conversations")
  .description("Convert multi-turn conversations to flat training examples")
  .requiredOption("-i, --input <path>", "Input conversations JSONL file")
  .option("-o, --output <path>", "Output training JSONL file", "data/annotated/conversations-flat.jsonl")
  .option("--include-context", "Include previous turns as context in the prompt", true)
  .action(async (options) => {
    const inputPath = path.resolve(options.input);
    const outputPath = path.resolve(options.output);
    const includeContext = options.includeContext as boolean;

    // Ensure output directory exists
    fs.mkdirSync(path.dirname(outputPath), { recursive: true });

    console.log(`Converting conversations from ${inputPath}`);
    console.log(`  Include context: ${includeContext}`);

    // Read conversations
    const conversations: ConversationExample[] = [];
    const fileStream = fs.createReadStream(inputPath);
    const rl = readline.createInterface({
      input: fileStream,
      crlfDelay: Infinity,
    });

    for await (const line of rl) {
      if (line.trim()) {
        conversations.push(JSON.parse(line) as ConversationExample);
      }
    }

    console.log(`  Read ${conversations.length} conversations`);

    // Convert to flat training examples
    const output = fs.createWriteStream(outputPath);
    let totalExamples = 0;

    for (const conv of conversations) {
      const turns = conv.conversation;
      let context: string[] = [];

      // Process each user-assistant pair
      for (let i = 0; i < turns.length; i += 2) {
        const userTurn = turns[i];
        const assistantTurn = turns[i + 1];

        if (!userTurn || !assistantTurn) continue;
        if (userTurn.role !== "user" || assistantTurn.role !== "assistant") continue;

        // Build the prompt with context
        let text: string;
        if (includeContext && context.length > 0) {
          // Format: "Previous: <context>\n\nNow: <current request>"
          text = `Previous:\n${context.join("\n")}\n\nNow: ${userTurn.content}`;
        } else {
          text = userTurn.content;
        }

        // Write training example
        const example: TrainingExample = {
          text,
          ir: assistantTurn.content,
          family: conv.family,
          complexity: conv.turns,
        };
        output.write(JSON.stringify(example) + "\n");
        totalExamples++;

        // Add to context for next turn
        context.push(`User: ${userTurn.content}`);
        context.push(`Assistant: ${assistantTurn.content}`);
      }
    }

    output.end();
    console.log(`\nWrote ${totalExamples} training examples to ${outputPath}`);
  });

/**
 * Generate-images command - creates multimodal image-IR pairs.
 */
program
  .command("generate-images")
  .description("Generate multimodal image-IR training pairs")
  .option("-c, --count <count>", "Number of parts to render", "100")
  .option("-o, --output <dir>", "Output directory", "data/multimodal")
  .option("-f, --families <families>", "Comma-separated list of families", "all")
  .option("--width <pixels>", "Image width", "512")
  .option("--height <pixels>", "Image height", "512")
  .option("--views <views>", "Comma-separated views (isometric,front,side,top,random)", "isometric")
  .action(async (options) => {
    const count = parseInt(options.count, 10);
    const outputDir = path.resolve(options.output);
    const width = parseInt(options.width, 10);
    const height = parseInt(options.height, 10);
    const views = (options.views as string).split(",").map((v: string) => v.trim()) as any[];

    const families =
      options.families === "all"
        ? generatorFamilies
        : (options.families as string).split(",").map((f: string) => f.trim());

    console.log(`Generating ${count} image-IR pairs`);
    console.log(`  Families: ${families.join(", ")}`);
    console.log(`  Views: ${views.join(", ")}`);
    console.log(`  Size: ${width}x${height}`);
    console.log(`  Output: ${outputDir}`);

    // Generate parts
    console.log("\n=== Step 1: Generate parts ===");
    const parts: GeneratedPart[] = [];
    const countsPerFamily = distributeCount(count, families);

    for (const family of families) {
      const generator = generators[family];
      if (!generator) {
        console.warn(`  Warning: Unknown family '${family}', skipping`);
        continue;
      }
      const familyCount = countsPerFamily[family] || 0;
      for (let i = 0; i < familyCount; i++) {
        parts.push(generator.generate());
      }
    }
    console.log(`  Generated ${parts.length} parts`);

    // Load engine
    console.log("\n=== Step 2: Load engine ===");
    let engine;
    try {
      const { Engine } = await import("@vcad/engine");
      engine = await Engine.init();
      console.log("  Engine loaded");
    } catch (error) {
      console.error(`  Error loading engine: ${(error as Error).message}`);
      console.error("  Make sure @vcad/engine is built: npm run build -w @vcad/engine");
      process.exit(1);
    }

    // Generate images
    console.log("\n=== Step 3: Render images ===");
    let pairs: ImageIRPair[];
    try {
      pairs = await generateImageIRPairs(parts, engine, {
        outputDir,
        width,
        height,
        views,
        onProgress: (completed, total, errors) => {
          process.stdout.write(`\r  Progress: ${completed}/${total} (${errors} errors)`);
        },
      });
      console.log("");
    } catch (error) {
      console.error(`\n  Error: ${(error as Error).message}`);
      console.error("  Make sure puppeteer is installed: npm install puppeteer");
      process.exit(1);
    }

    // Write metadata
    const metadataPath = path.join(outputDir, "metadata.jsonl");
    writeMetadata(pairs, metadataPath);

    // Print stats
    const stats = computeMultimodalStats(pairs, outputDir);
    console.log(`\n=== Results ===`);
    console.log(`  Total pairs: ${stats.totalPairs}`);
    console.log(`  Avg image size: ${(stats.avgImageSizeBytes / 1024).toFixed(1)} KB`);
    console.log(`  By family:`);
    for (const [family, familyCount] of Object.entries(stats.byFamily).sort((a, b) => b[1] - a[1])) {
      console.log(`    ${family}: ${familyCount}`);
    }
    console.log(`  By view:`);
    for (const [view, viewCount] of Object.entries(stats.byView)) {
      console.log(`    ${view}: ${viewCount}`);
    }
    console.log(`\n  Metadata: ${metadataPath}`);
    console.log(`  Images: ${path.join(outputDir, "images/")}`);
  });

/**
 * Render-test command - test rendering pipeline with a few examples.
 */
program
  .command("render-test")
  .description("Test the rendering pipeline with a few examples")
  .option("-n, --count <count>", "Number of test renders", "5")
  .option("-o, --output <dir>", "Output directory", "data/test-renders")
  .option("-f, --family <family>", "Part family to test", "plate")
  .action(async (options) => {
    const count = parseInt(options.count, 10);
    const outputDir = path.resolve(options.output);
    const family = options.family as string;

    console.log(`Testing render pipeline with ${count} ${family} parts`);

    // Ensure output directory exists
    fs.mkdirSync(outputDir, { recursive: true });

    // Get generator
    const generator = generators[family];
    if (!generator) {
      console.error(`Unknown family: ${family}`);
      process.exit(1);
    }

    // Generate parts
    const parts: GeneratedPart[] = [];
    for (let i = 0; i < count; i++) {
      parts.push(generator.generate());
    }

    // Load engine
    console.log("Loading engine...");
    let engine;
    try {
      const { Engine } = await import("@vcad/engine");
      engine = await Engine.init();
    } catch (error) {
      console.error(`Error loading engine: ${(error as Error).message}`);
      process.exit(1);
    }

    // Initialize renderer
    console.log("Initializing renderer...");
    const renderer = new Renderer();
    try {
      await renderer.init();
    } catch (error) {
      console.error(`Error initializing renderer: ${(error as Error).message}`);
      console.error("Make sure puppeteer is installed: npm install puppeteer");
      process.exit(1);
    }

    // Render each part
    const { fromVCode } = await import("@vcad/ir");
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      console.log(`\nRendering ${i + 1}/${count}...`);
      console.log(`  IR:\n${part.vcode.split("\n").map(l => "    " + l).join("\n")}`);

      try {
        const doc = fromVCode(part.vcode);
        const scene = engine.evaluate(doc);

        if (!scene.parts || scene.parts.length === 0) {
          console.log("  Error: No geometry produced");
          continue;
        }

        const mesh = scene.parts[0].mesh;
        console.log(`  Mesh: ${mesh.positions.length / 3} vertices, ${mesh.indices.length / 3} triangles`);

        const result = await renderer.render(mesh.positions, mesh.indices, {
          width: 512,
          height: 512,
          view: "isometric",
        });

        const imagePath = path.join(outputDir, `${family}_${i}.png`);
        fs.writeFileSync(imagePath, result.image);
        console.log(`  Saved: ${imagePath} (${result.durationMs.toFixed(0)}ms)`);
      } catch (error) {
        console.log(`  Error: ${(error as Error).message}`);
      }
    }

    await renderer.close();
    console.log("\nDone!");
  });

program.parse();
