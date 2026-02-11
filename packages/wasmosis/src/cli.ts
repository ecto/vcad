#!/usr/bin/env node

/**
 * wasmosis CLI - WASM module splitting and multi-crate generation
 *
 * Usage:
 *   wasmosis split input.wasm --out-dir ./dist       # Split existing WASM binary
 *   wasmosis analyze input.wasm                       # Analyze WASM for modules
 *   wasmosis codegen src/lib.rs --out-dir ./generated # Generate separate crates
 *   wasmosis build --crates-dir ./generated          # Build all crates with wasm-pack
 */

import { readFileSync, writeFileSync, mkdirSync, existsSync, readdirSync } from 'node:fs';
import { resolve, basename, dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { splitWasm, analyzeSplitConfig, checkBinaryenAvailable } from './splitter.js';
import { generateBundle } from './codegen.js';
import {
  parseRustSource,
  parseRustSourceWithInference,
  type ParseResult,
  type InferredModule,
} from './source-parser.js';
import {
  generateCrates,
  generateTypeScriptLoader,
  type CrateGeneratorOptions,
} from './crate-generator.js';

// =============================================================================
// Split command (existing)
// =============================================================================

interface SplitArgs {
  input: string;
  outDir: string;
  optimize: boolean;
  optimizeLevel: number;
  shrinkLevel: number;
}

function parseSplitArgs(args: string[]): SplitArgs | null {
  if (args.length === 0) {
    console.error('Error: Missing input file');
    return null;
  }

  const result: SplitArgs = {
    input: args[0]!,
    outDir: './dist',
    optimize: true,
    optimizeLevel: 2,
    shrinkLevel: 1,
  };

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case '--out-dir':
      case '-o':
        result.outDir = args[++i] ?? './dist';
        break;
      case '--no-optimize':
        result.optimize = false;
        break;
      case '--optimize-level':
        result.optimizeLevel = parseInt(args[++i] ?? '2', 10);
        break;
      case '--shrink-level':
        result.shrinkLevel = parseInt(args[++i] ?? '1', 10);
        break;
      default:
        console.error(`Unknown option: ${arg}`);
        return null;
    }
  }

  return result;
}

async function runSplit(args: SplitArgs): Promise<number> {
  if (!checkBinaryenAvailable()) {
    console.error('Error: Binaryen is not available. Install with: brew install binaryen');
    return 1;
  }

  const inputPath = resolve(args.input);
  if (!existsSync(inputPath)) {
    console.error(`Error: Input file not found: ${inputPath}`);
    return 1;
  }

  console.log(`Reading ${basename(inputPath)}...`);
  const input = new Uint8Array(readFileSync(inputPath));

  console.log(`Splitting WASM (${(input.length / 1024).toFixed(1)} KB)...`);
  const result = splitWasm({
    input,
    optimize: args.optimize,
    optimizeLevel: args.optimizeLevel,
    shrinkLevel: args.shrinkLevel,
  });

  // Create output directory
  const outDir = resolve(args.outDir);
  if (!existsSync(outDir)) {
    mkdirSync(outDir, { recursive: true });
  }

  // Write core module
  const corePath = join(outDir, 'core.wasm');
  writeFileSync(corePath, result.core);
  console.log(`  core.wasm (${(result.core.length / 1024).toFixed(1)} KB)`);

  // Write secondary modules
  for (const [name, bytes] of result.secondary) {
    const secondaryPath = join(outDir, `${name}.wasm`);
    writeFileSync(secondaryPath, bytes);
    console.log(`  ${name}.wasm (${(bytes.length / 1024).toFixed(1)} KB)`);
  }

  // Generate TypeScript code
  const bundle = generateBundle(result.config, {
    wasmBasePath: './',
    moduleFormat: 'esm',
  });

  const registryPath = join(outDir, 'registry.ts');
  writeFileSync(registryPath, bundle.registry);
  console.log(`  registry.ts`);

  const typesPath = join(outDir, 'types.d.ts');
  writeFileSync(typesPath, bundle.types);
  console.log(`  types.d.ts`);

  console.log(`\nSplit complete! ${1 + result.secondary.size} modules written to ${outDir}`);
  return 0;
}

// =============================================================================
// Analyze command (existing)
// =============================================================================

interface AnalyzeArgs {
  input: string;
  json: boolean;
  /** Show inference reasoning for each function */
  showInference: boolean;
}

function parseAnalyzeArgs(args: string[]): AnalyzeArgs | null {
  if (args.length === 0) {
    console.error('Error: Missing input file');
    return null;
  }

  const result: AnalyzeArgs = {
    input: args[0]!,
    json: false,
    showInference: false,
  };

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case '--json':
        result.json = true;
        break;
      case '--show-inference':
        result.showInference = true;
        break;
      default:
        console.error(`Unknown option: ${arg}`);
        return null;
    }
  }

  return result;
}

async function runAnalyze(args: AnalyzeArgs): Promise<number> {
  const inputPath = resolve(args.input);
  if (!existsSync(inputPath)) {
    console.error(`Error: Input file not found: ${inputPath}`);
    return 1;
  }

  // Check if it's a Rust source file or WASM binary
  if (inputPath.endsWith('.rs')) {
    return analyzeRustSource(inputPath, args.json, args.showInference);
  } else if (inputPath.endsWith('.wasm')) {
    return analyzeWasmBinary(inputPath, args.json);
  } else {
    console.error('Error: Input must be a .rs or .wasm file');
    return 1;
  }
}

function analyzeRustSource(inputPath: string, json: boolean, showInference: boolean): number {
  // Use inference-aware parser when showing inference
  const parseResult = showInference
    ? parseRustSourceWithInference(inputPath)
    : parseRustSource(inputPath);

  if (json) {
    const output: Record<string, unknown> = {
      core: {
        functions: parseResult.core.map((f) => ({
          name: f.name,
          ...(showInference && f.inference ? { inference: f.inference } : {}),
        })),
        functionCount: parseResult.core.length,
      },
      modules: Object.fromEntries(
        [...parseResult.modules.entries()].map(([name, funcs]) => [
          name,
          {
            functions: funcs.map((f) => ({
              name: f.name,
              ...(showInference && f.inference ? { inference: f.inference } : {}),
            })),
            functionCount: funcs.length,
          },
        ])
      ),
      types: parseResult.types.map((t) => t.name),
    };
    console.log(JSON.stringify(output, null, 2));
  } else {
    console.log(`Rust Source Analysis: ${basename(inputPath)}`);
    if (showInference) {
      console.log('Mode: Automatic Module Inference');
    }
    console.log('');

    // Helper to format inference reason
    const formatInference = (inf?: { reason: string; detail?: string }) => {
      if (!inf) return '';
      const detail = inf.detail ? `: ${inf.detail}` : '';
      return ` [${inf.reason}${detail}]`;
    };

    console.log(`Core module (${parseResult.core.length} functions):`);
    for (const fn of parseResult.core.slice(0, 15)) {
      if (showInference) {
        console.log(`    - ${fn.name}${formatInference(fn.inference)}`);
      } else {
        console.log(`    - ${fn.name}`);
      }
    }
    if (parseResult.core.length > 15) {
      console.log(`    ... and ${parseResult.core.length - 15} more`);
    }
    console.log('');

    if (parseResult.modules.size === 0) {
      console.log('No secondary modules detected.');
      if (!showInference) {
        console.log('Add #[wasmosis::module("name")] to functions to enable splitting.');
        console.log('Or use --show-inference to see automatic module inference.');
      }
    } else {
      console.log('Secondary modules:');
      for (const [name, funcs] of parseResult.modules) {
        console.log(`  ${name} (${funcs.length} functions):`);
        for (const fn of funcs) {
          if (showInference) {
            console.log(`      - ${fn.name}${formatInference(fn.inference)}`);
          } else {
            console.log(`      - ${fn.name}`);
          }
        }
      }
    }

    console.log('');
    console.log(`Types: ${parseResult.types.length}`);
    for (const t of parseResult.types.slice(0, 5)) {
      console.log(`  - ${t.name}`);
    }
    if (parseResult.types.length > 5) {
      console.log(`  ... and ${parseResult.types.length - 5} more`);
    }

    // Summary of inference reasons
    if (showInference) {
      console.log('');
      console.log('Inference Summary:');
      const reasons = new Map<string, number>();
      for (const fn of [...parseResult.core, ...[...parseResult.modules.values()].flat()]) {
        if (fn.inference) {
          const key = fn.inference.reason;
          reasons.set(key, (reasons.get(key) || 0) + 1);
        }
      }
      for (const [reason, count] of reasons) {
        console.log(`  ${reason}: ${count} functions`);
      }
    }
  }

  return 0;
}

function analyzeWasmBinary(inputPath: string, json: boolean): number {
  if (!checkBinaryenAvailable()) {
    console.error('Error: Binaryen is not available. Install with: brew install binaryen');
    return 1;
  }

  const input = new Uint8Array(readFileSync(inputPath));
  const config = analyzeSplitConfig(input);

  if (json) {
    const output = {
      core: {
        functions: config.core.functions,
        functionCount: config.core.functions.length,
      },
      secondary: Object.fromEntries(
        [...config.secondary.entries()].map(([name, cfg]) => [
          name,
          {
            functions: cfg.functions,
            functionCount: cfg.functions.length,
            depends: cfg.depends,
          },
        ])
      ),
    };
    console.log(JSON.stringify(output, null, 2));
  } else {
    console.log(`WASM Analysis: ${basename(inputPath)}`);
    console.log(`Size: ${(input.length / 1024).toFixed(1)} KB`);
    console.log('');
    console.log(`Core module:`);
    console.log(`  Functions: ${config.core.functions.length}`);
    if (config.core.functions.length <= 10) {
      for (const fn of config.core.functions) {
        console.log(`    - ${fn}`);
      }
    }
    console.log('');

    if (config.secondary.size === 0) {
      console.log('No secondary modules detected.');
      console.log('Add #[wasmosis::module("name")] to functions to enable splitting.');
    } else {
      console.log('Secondary modules:');
      for (const [name, cfg] of config.secondary) {
        console.log(`  ${name}:`);
        console.log(`    Functions: ${cfg.functions.length}`);
        console.log(`    Depends: ${cfg.depends.join(', ')}`);
        for (const fn of cfg.functions) {
          console.log(`      - ${fn}`);
        }
      }
    }
  }

  return 0;
}

// =============================================================================
// Codegen command (new)
// =============================================================================

interface CodegenArgs {
  input: string;
  outDir: string;
  baseName: string;
  tsOutDir: string | null;
  deps: string | null;
}

function parseCodegenArgs(args: string[]): CodegenArgs | null {
  if (args.length === 0) {
    console.error('Error: Missing input file');
    return null;
  }

  const result: CodegenArgs = {
    input: args[0]!,
    outDir: './generated',
    baseName: 'wasm-module',
    tsOutDir: null,
    deps: null,
  };

  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case '--out-dir':
      case '-o':
        result.outDir = args[++i] ?? './generated';
        break;
      case '--base-name':
      case '-n':
        result.baseName = args[++i] ?? 'wasm-module';
        break;
      case '--ts-out-dir':
      case '-t':
        result.tsOutDir = args[++i] ?? null;
        break;
      case '--deps':
      case '-d':
        result.deps = args[++i] ?? null;
        break;
      default:
        console.error(`Unknown option: ${arg}`);
        return null;
    }
  }

  return result;
}

async function runCodegen(args: CodegenArgs): Promise<number> {
  const inputPath = resolve(args.input);
  if (!existsSync(inputPath)) {
    console.error(`Error: Input file not found: ${inputPath}`);
    return 1;
  }

  if (!inputPath.endsWith('.rs')) {
    console.error('Error: Input must be a Rust source file (.rs)');
    return 1;
  }

  console.log(`Parsing ${basename(inputPath)}...`);
  const parseResult = parseRustSource(inputPath);

  // Parse dependencies from JSON file if provided
  let dependencies: Record<string, string | { version: string; features?: string[] }> = {};
  if (args.deps) {
    const depsPath = resolve(args.deps);
    if (existsSync(depsPath)) {
      dependencies = JSON.parse(readFileSync(depsPath, 'utf-8'));
    } else {
      console.error(`Warning: Dependencies file not found: ${depsPath}`);
    }
  }

  console.log(`Generating crates...`);
  const crates = generateCrates(parseResult, {
    baseName: args.baseName,
    outDir: resolve(args.outDir),
    dependencies,
  });

  console.log(`Generated ${crates.length} crates:`);
  for (const crate of crates) {
    console.log(`  ${crate.name} (${crate.functions.length} functions)`);
  }

  // Generate TypeScript loader if requested
  if (args.tsOutDir) {
    console.log(`\nGenerating TypeScript loader...`);
    generateTypeScriptLoader(crates, {
      packageName: `@${args.baseName}/loader`,
      outDir: resolve(args.tsOutDir),
      wasmBasePath: './',
    });
    console.log(`  ${args.tsOutDir}/index.ts`);
  }

  console.log(`\nCodegen complete!`);
  console.log(`\nNext steps:`);
  console.log(`  1. Add generated crates to workspace Cargo.toml`);
  console.log(`  2. Run: wasmosis build --crates-dir ${args.outDir}`);

  return 0;
}

// =============================================================================
// Build command (new)
// =============================================================================

interface BuildArgs {
  cratesDir: string;
  outDir: string;
  target: 'web' | 'bundler' | 'nodejs';
  release: boolean;
  tsOutDir: string | null;
}

function parseBuildArgs(args: string[]): BuildArgs | null {
  const result: BuildArgs = {
    cratesDir: './generated',
    outDir: './dist',
    target: 'web',
    release: true,
    tsOutDir: null,
  };

  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    switch (arg) {
      case '--crates-dir':
      case '-c':
        result.cratesDir = args[++i] ?? './generated';
        break;
      case '--out-dir':
      case '-o':
        result.outDir = args[++i] ?? './dist';
        break;
      case '--target':
      case '-t':
        const target = args[++i];
        if (target === 'web' || target === 'bundler' || target === 'nodejs') {
          result.target = target;
        } else {
          console.error(`Invalid target: ${target}. Must be web, bundler, or nodejs.`);
          return null;
        }
        break;
      case '--dev':
        result.release = false;
        break;
      case '--ts-out-dir':
        result.tsOutDir = args[++i] ?? null;
        break;
      default:
        console.error(`Unknown option: ${arg}`);
        return null;
    }
  }

  return result;
}

async function runBuild(args: BuildArgs): Promise<number> {
  const cratesDir = resolve(args.cratesDir);
  if (!existsSync(cratesDir)) {
    console.error(`Error: Crates directory not found: ${cratesDir}`);
    return 1;
  }

  // Find all crates in the directory
  const entries = readdirSync(cratesDir, { withFileTypes: true });
  const crates = entries
    .filter((e) => e.isDirectory() && existsSync(join(cratesDir, e.name, 'Cargo.toml')))
    .map((e) => e.name);

  if (crates.length === 0) {
    console.error('Error: No crates found in directory');
    return 1;
  }

  console.log(`Found ${crates.length} crates to build:`);
  for (const crate of crates) {
    console.log(`  - ${crate}`);
  }
  console.log('');

  // Check if wasm-pack is available
  const wpCheck = spawnSync('wasm-pack', ['--version'], { encoding: 'utf-8' });
  if (wpCheck.status !== 0) {
    console.error('Error: wasm-pack not found. Install with: cargo install wasm-pack');
    return 1;
  }

  const outDir = resolve(args.outDir);
  if (!existsSync(outDir)) {
    mkdirSync(outDir, { recursive: true });
  }

  // Build each crate
  let success = 0;
  let failed = 0;

  for (const crate of crates) {
    const cratePath = join(cratesDir, crate);
    const crateOutDir = join(outDir, crate);

    console.log(`Building ${crate}...`);

    const buildArgs = [
      'build',
      cratePath,
      '--target', args.target,
      '-d', crateOutDir,
    ];

    if (args.release) {
      buildArgs.push('--release');
    } else {
      buildArgs.push('--dev');
    }

    const result = spawnSync('wasm-pack', buildArgs, {
      encoding: 'utf-8',
      stdio: 'inherit',
    });

    if (result.status === 0) {
      success++;
      console.log(`  Done!`);
    } else {
      failed++;
      console.error(`  Failed!`);
    }
  }

  console.log('');
  console.log(`Build complete: ${success} succeeded, ${failed} failed`);

  // Generate unified TypeScript loader if requested
  if (args.tsOutDir && success > 0) {
    console.log(`\nGenerating TypeScript loader...`);
    const generatedCrates = crates.map((name) => ({
      name,
      module: name.split('-').pop() ?? name,
      path: join(outDir, name),
      functions: [], // We don't have function info here, would need to parse
    }));
    generateTypeScriptLoader(generatedCrates, {
      packageName: '@wasmosis/loader',
      outDir: resolve(args.tsOutDir),
      wasmBasePath: './',
    });
  }

  return failed > 0 ? 1 : 0;
}

// =============================================================================
// Main
// =============================================================================

function printUsage(): void {
  console.log(`wasmosis - WASM module splitting and multi-crate generation

Usage:
  wasmosis <command> [options]
  wasmosis --help

Commands:
  split      Split a WASM binary into core and secondary modules (uses wasm-split)
  analyze    Analyze a WASM binary or Rust source for module definitions
  codegen    Generate separate crates from annotated Rust source
  build      Build generated crates with wasm-pack

Split Options:
  wasmosis split <input.wasm> [options]
  --out-dir, -o <dir>    Output directory (default: ./dist)
  --no-optimize          Disable optimization
  --optimize-level <n>   Optimization level 0-4 (default: 2)
  --shrink-level <n>     Shrink level 0-2 (default: 1)

Analyze Options:
  wasmosis analyze <input.wasm|input.rs> [options]
  --json                 Output as JSON
  --show-inference       Show automatic module inference reasoning

Codegen Options:
  wasmosis codegen <input.rs> [options]
  --out-dir, -o <dir>    Output directory for crates (default: ./generated)
  --base-name, -n <name> Base name for crates (default: wasm-module)
  --ts-out-dir, -t <dir> Output directory for TypeScript loader
  --deps, -d <file>      JSON file with dependencies

Build Options:
  wasmosis build [options]
  --crates-dir, -c <dir> Directory containing generated crates (default: ./generated)
  --out-dir, -o <dir>    Output directory for WASM files (default: ./dist)
  --target, -t <target>  wasm-pack target: web, bundler, nodejs (default: web)
  --dev                  Build in dev mode (default: release)
  --ts-out-dir <dir>     Output directory for TypeScript loader

Examples:
  # Split existing WASM binary (uses Binaryen wasm-split)
  wasmosis split kernel.wasm -o ./wasm

  # Analyze Rust source for modules
  wasmosis analyze src/lib.rs --json

  # Generate separate crates from annotated source
  wasmosis codegen src/lib.rs -o ./generated -n my-kernel -t ./loader

  # Build all generated crates
  wasmosis build -c ./generated -o ./dist
`);
}

async function main(): Promise<number> {
  const args = process.argv.slice(2);

  if (args.length === 0 || args.includes('--help') || args.includes('-h')) {
    printUsage();
    return 0;
  }

  const command = args[0];
  const commandArgs = args.slice(1);

  switch (command) {
    case 'split': {
      const splitArgs = parseSplitArgs(commandArgs);
      if (!splitArgs) {
        printUsage();
        return 1;
      }
      return runSplit(splitArgs);
    }

    case 'analyze': {
      const analyzeArgs = parseAnalyzeArgs(commandArgs);
      if (!analyzeArgs) {
        printUsage();
        return 1;
      }
      return runAnalyze(analyzeArgs);
    }

    case 'codegen': {
      const codegenArgs = parseCodegenArgs(commandArgs);
      if (!codegenArgs) {
        printUsage();
        return 1;
      }
      return runCodegen(codegenArgs);
    }

    case 'build': {
      const buildArgs = parseBuildArgs(commandArgs);
      if (!buildArgs) {
        printUsage();
        return 1;
      }
      return runBuild(buildArgs);
    }

    default:
      console.error(`Unknown command: ${command}`);
      printUsage();
      return 1;
  }
}

main()
  .then((code) => process.exit(code))
  .catch((err) => {
    console.error('Fatal error:', err);
    process.exit(1);
  });
