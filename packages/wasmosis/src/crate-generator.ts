/**
 * Rust crate generator for wasmosis multi-crate architecture.
 *
 * Generates separate crates from parsed module definitions.
 */

import { mkdirSync, writeFileSync, existsSync } from 'node:fs';
import { join, dirname, basename } from 'node:path';
import type { ParseResult, ParsedFunction, ParsedType } from './source-parser.js';

/**
 * Options for crate generation.
 */
export interface CrateGeneratorOptions {
  /** Base name for generated crates (e.g., "vcad-kernel-wasm") */
  baseName: string;
  /** Output directory for generated crates */
  outDir: string;
  /** Dependencies to include in Cargo.toml */
  dependencies: Record<string, string | { version: string; features?: string[] }>;
  /** Workspace path (for path dependencies) */
  workspacePath?: string;
  /** Additional features for wasm-bindgen */
  wasmBindgenFeatures?: string[];
}

/**
 * Result of crate generation.
 */
export interface GeneratedCrate {
  /** Crate name */
  name: string;
  /** Module name (core, step, physics, etc.) */
  module: string;
  /** Path to generated crate */
  path: string;
  /** Functions in this crate */
  functions: string[];
}

/**
 * Generate separate crates from parsed module definitions.
 */
export function generateCrates(
  parseResult: ParseResult,
  options: CrateGeneratorOptions
): GeneratedCrate[] {
  const { baseName, outDir, dependencies, workspacePath, wasmBindgenFeatures = [] } = options;

  const generatedCrates: GeneratedCrate[] = [];

  // Ensure output directory exists
  if (!existsSync(outDir)) {
    mkdirSync(outDir, { recursive: true });
  }

  // Generate core crate
  if (parseResult.core.length > 0) {
    const crateName = `${baseName}-core`;
    const cratePath = join(outDir, crateName);
    generateCrate({
      crateName,
      cratePath,
      moduleName: 'core',
      functions: parseResult.core,
      types: parseResult.types,
      uses: parseResult.uses,
      attributes: parseResult.attributes,
      dependencies,
      workspacePath,
      wasmBindgenFeatures,
      isCore: true,
    });
    generatedCrates.push({
      name: crateName,
      module: 'core',
      path: cratePath,
      functions: parseResult.core.map((f) => f.name),
    });
  }

  // Generate secondary crates
  for (const [moduleName, functions] of parseResult.modules) {
    const crateName = `${baseName}-${moduleName}`;
    const cratePath = join(outDir, crateName);
    generateCrate({
      crateName,
      cratePath,
      moduleName,
      functions,
      types: [], // Types are only in core
      uses: parseResult.uses,
      attributes: parseResult.attributes,
      dependencies,
      workspacePath,
      wasmBindgenFeatures,
      isCore: false,
    });
    generatedCrates.push({
      name: crateName,
      module: moduleName,
      path: cratePath,
      functions: functions.map((f) => f.name),
    });
  }

  return generatedCrates;
}

interface GenerateCrateOptions {
  crateName: string;
  cratePath: string;
  moduleName: string;
  functions: ParsedFunction[];
  types: ParsedType[];
  uses: string[];
  attributes: string[];
  dependencies: Record<string, string | { version: string; features?: string[] }>;
  workspacePath?: string;
  wasmBindgenFeatures: string[];
  isCore: boolean;
}

/**
 * Generate a single crate.
 */
function generateCrate(options: GenerateCrateOptions): void {
  const {
    crateName,
    cratePath,
    moduleName,
    functions,
    types,
    uses,
    attributes,
    dependencies,
    workspacePath,
    wasmBindgenFeatures,
    isCore,
  } = options;

  // Create crate directory structure
  if (!existsSync(cratePath)) {
    mkdirSync(cratePath, { recursive: true });
  }
  const srcPath = join(cratePath, 'src');
  if (!existsSync(srcPath)) {
    mkdirSync(srcPath);
  }

  // Generate Cargo.toml
  const cargoToml = generateCargoToml({
    crateName,
    dependencies,
    workspacePath,
    wasmBindgenFeatures,
  });
  writeFileSync(join(cratePath, 'Cargo.toml'), cargoToml);

  // Generate lib.rs
  const libRs = generateLibRs({
    moduleName,
    functions,
    types,
    uses,
    attributes,
    isCore,
  });
  writeFileSync(join(srcPath, 'lib.rs'), libRs);
}

interface CargoTomlOptions {
  crateName: string;
  dependencies: Record<string, string | { version: string; features?: string[] }>;
  workspacePath?: string;
  wasmBindgenFeatures: string[];
}

/**
 * Generate Cargo.toml content.
 */
function generateCargoToml(options: CargoTomlOptions): string {
  const { crateName, dependencies, workspacePath, wasmBindgenFeatures } = options;

  const depsLines: string[] = [];
  for (const [name, value] of Object.entries(dependencies)) {
    if (typeof value === 'string') {
      depsLines.push(`${name} = "${value}"`);
    } else {
      const parts = [`version = "${value.version}"`];
      if (value.features && value.features.length > 0) {
        parts.push(`features = [${value.features.map((f) => `"${f}"`).join(', ')}]`);
      }
      depsLines.push(`${name} = { ${parts.join(', ')} }`);
    }
  }

  // Add wasm-bindgen with features
  const wbFeatures =
    wasmBindgenFeatures.length > 0
      ? `, features = [${wasmBindgenFeatures.map((f) => `"${f}"`).join(', ')}]`
      : '';
  depsLines.push(`wasm-bindgen = { version = "0.2"${wbFeatures} }`);

  // Add common WASM dependencies
  depsLines.push(`serde = { version = "1", features = ["derive"] }`);
  depsLines.push(`serde-wasm-bindgen = "0.6"`);
  depsLines.push(`serde_json = "1"`);
  depsLines.push(`web-sys = { version = "0.3", features = ["console"] }`);
  depsLines.push(`console_error_panic_hook = { version = "0.1", optional = true }`);

  return `# Auto-generated by wasmosis - do not edit manually
[package]
name = "${crateName}"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
${depsLines.join('\n')}

[features]
default = ["console_error_panic_hook"]

[profile.release]
opt-level = "s"
lto = true
`;
}

interface LibRsOptions {
  moduleName: string;
  functions: ParsedFunction[];
  types: ParsedType[];
  uses: string[];
  attributes: string[];
  isCore: boolean;
}

/**
 * Generate lib.rs content.
 */
function generateLibRs(options: LibRsOptions): string {
  const { moduleName, functions, types, uses, attributes, isCore } = options;

  const lines: string[] = [];

  // Module doc
  lines.push(`//! WASM bindings - ${moduleName} module`);
  lines.push(`//!`);
  lines.push(`//! Auto-generated by wasmosis - do not edit manually`);
  lines.push('');

  // Module-level attributes
  for (const attr of attributes) {
    lines.push(attr);
  }
  lines.push('');

  // Use statements
  for (const use of uses) {
    lines.push(use);
  }
  lines.push('use wasm_bindgen::prelude::*;');
  lines.push('');

  // Init function for core module
  if (isCore) {
    lines.push('/// Initialize the WASM module.');
    lines.push('#[wasm_bindgen(start)]');
    lines.push('pub fn init() {');
    lines.push('    #[cfg(feature = "console_error_panic_hook")]');
    lines.push('    console_error_panic_hook::set_once();');
    lines.push(`    web_sys::console::log_1(&format!("[WASM] ${moduleName} module loaded").into());`);
    lines.push('}');
    lines.push('');
  }

  // Type definitions (for core only)
  for (const type of types) {
    lines.push(type.definition);
    lines.push('');
  }

  // Functions
  for (const func of functions) {
    lines.push(func.signature);
    lines.push('');
  }

  return lines.join('\n');
}

/**
 * Options for TypeScript loader generation.
 */
export interface TypeScriptLoaderOptions {
  /** Package name for generated loader */
  packageName: string;
  /** Output directory */
  outDir: string;
  /** Base path for WASM imports */
  wasmBasePath: string;
  /** Path to ts-rs generated types (optional) */
  typesPath?: string;
  /** Generate type-safe wrapper with ts-rs types */
  typeSafe?: boolean;
}

/**
 * Generate TypeScript loader from generated crates.
 *
 * When `typeSafe: true` is set, the generated loader imports types from
 * ts-rs generated definitions and uses proper typing instead of `any`.
 */
export function generateTypeScriptLoader(
  crates: GeneratedCrate[],
  options: TypeScriptLoaderOptions
): void {
  const {
    packageName,
    outDir,
    wasmBasePath,
    typesPath = './generated/types',
    typeSafe = false,
  } = options;

  if (!existsSync(outDir)) {
    mkdirSync(outDir, { recursive: true });
  }

  const coreCrate = crates.find((c) => c.module === 'core');
  const secondaryCrates = crates.filter((c) => c.module !== 'core');

  // Generate Kernel class
  const lines: string[] = [];
  lines.push('/**');
  lines.push(' * Auto-generated WASM Kernel loader');
  lines.push(' * Generated by wasmosis - do not edit manually');
  lines.push(' */');
  lines.push('');

  // Import ts-rs generated types if type-safe mode
  if (typeSafe) {
    lines.push(`// Import type definitions generated by ts-rs`);
    lines.push(`import type {`);
    lines.push(`  WasmMesh,`);
    lines.push(`  WasmSketchProfile,`);
    lines.push(`  WasmSketchSegment,`);
    lines.push(`  GpuGeometryResult,`);
    lines.push(`  TextBoundsResult,`);
    lines.push(`} from '${typesPath}';`);
    lines.push('');
    lines.push(`// Re-export types for consumers`);
    lines.push(`export type {`);
    lines.push(`  WasmMesh,`);
    lines.push(`  WasmSketchProfile,`);
    lines.push(`  WasmSketchSegment,`);
    lines.push(`  GpuGeometryResult,`);
    lines.push(`  TextBoundsResult,`);
    lines.push(`};`);
    lines.push('');
  }

  lines.push('type WasmModule = Record<string, unknown>;');
  lines.push('');

  // Solid type placeholder (wasm-bindgen class, not serializable)
  lines.push('/** Opaque solid geometry handle from WASM */');
  lines.push('export type Solid = unknown;');
  lines.push('');

  lines.push('export class Kernel {');
  lines.push('  private core: WasmModule | null = null;');
  lines.push('  private modules = new Map<string, Promise<WasmModule>>();');
  lines.push('');
  lines.push('  private constructor() {}');
  lines.push('');
  lines.push('  static async init(): Promise<Kernel> {');
  lines.push('    const kernel = new Kernel();');
  if (coreCrate) {
    const wasmPath = `${wasmBasePath}${coreCrate.name.replace(/-/g, '_')}.js`;
    lines.push(`    const core = await import('${wasmPath}');`);
    lines.push('    await core.default();');
    lines.push('    kernel.core = core;');
  }
  lines.push('    return kernel;');
  lines.push('  }');
  lines.push('');

  // Core function wrappers with type hints
  if (coreCrate) {
    for (const fn of coreCrate.functions) {
      const camelName = snakeToCamel(fn);
      const typeInfo = typeSafe ? getTypeInfo(fn) : { params: 'args: unknown[]', returns: 'unknown' };

      if (typeInfo.params.includes('...args')) {
        lines.push(`  ${camelName}(${typeInfo.params}): ${typeInfo.returns} {`);
        lines.push(`    return (this.core as any).${camelName}(...args);`);
      } else {
        const paramNames = extractParamNames(typeInfo.params);
        lines.push(`  ${camelName}(${typeInfo.params}): ${typeInfo.returns} {`);
        lines.push(`    return (this.core as any).${camelName}(${paramNames});`);
      }
      lines.push('  }');
      lines.push('');
    }
  }

  // Secondary module loaders
  for (const crate of secondaryCrates) {
    const wasmPath = `${wasmBasePath}${crate.name.replace(/-/g, '_')}.js`;
    const methodName = `load${capitalize(crate.module)}`;

    lines.push(`  private async ${methodName}(): Promise<WasmModule> {`);
    lines.push(`    if (!this.modules.has('${crate.module}')) {`);
    lines.push(`      this.modules.set('${crate.module}', (async () => {`);
    lines.push(`        const mod = await import('${wasmPath}');`);
    lines.push('        await mod.default();');
    lines.push('        return mod;');
    lines.push('      })());');
    lines.push('    }');
    lines.push(`    return this.modules.get('${crate.module}')!;`);
    lines.push('  }');
    lines.push('');

    // Function wrappers for this module
    for (const fn of crate.functions) {
      const camelName = snakeToCamel(fn);
      const typeInfo = typeSafe ? getTypeInfo(fn) : { params: 'args: unknown[]', returns: 'unknown' };

      if (typeInfo.params.includes('...args')) {
        lines.push(`  async ${camelName}(${typeInfo.params}): Promise<${typeInfo.returns}> {`);
        lines.push(`    const mod = await this.${methodName}();`);
        lines.push(`    return (mod as any).${camelName}(...args);`);
      } else {
        const paramNames = extractParamNames(typeInfo.params);
        lines.push(`  async ${camelName}(${typeInfo.params}): Promise<${typeInfo.returns}> {`);
        lines.push(`    const mod = await this.${methodName}();`);
        lines.push(`    return (mod as any).${camelName}(${paramNames});`);
      }
      lines.push('  }');
      lines.push('');
    }
  }

  lines.push('}');
  lines.push('');
  lines.push('export default Kernel;');

  writeFileSync(join(outDir, 'index.ts'), lines.join('\n'));

  // Generate package.json
  const pkg = {
    name: packageName,
    version: '0.1.0',
    type: 'module',
    main: 'dist/index.js',
    types: 'dist/index.d.ts',
    files: ['dist'],
    scripts: {
      build: 'tsc',
    },
  };
  writeFileSync(join(outDir, 'package.json'), JSON.stringify(pkg, null, 2));

  // Generate tsconfig.json
  const tsconfig = {
    compilerOptions: {
      target: 'ES2020',
      module: 'ESNext',
      moduleResolution: 'bundler',
      declaration: true,
      outDir: './dist',
      strict: true,
      esModuleInterop: true,
      skipLibCheck: true,
    },
    include: ['*.ts'],
  };
  writeFileSync(join(outDir, 'tsconfig.json'), JSON.stringify(tsconfig, null, 2));
}

/**
 * Known function type signatures for type-safe generation.
 *
 * This mapping provides TypeScript types for common WASM functions.
 * Functions not in this map fall back to unknown types.
 */
const KNOWN_FUNCTION_TYPES: Record<string, { params: string; returns: string }> = {
  // Primitives
  cube: { params: 'sx: number, sy: number, sz: number', returns: 'Solid' },
  cylinder: { params: 'radius: number, height: number, segments?: number', returns: 'Solid' },
  sphere: { params: 'radius: number, segments?: number', returns: 'Solid' },
  cone: { params: 'radiusBottom: number, radiusTop: number, height: number, segments?: number', returns: 'Solid' },

  // Boolean operations
  union: { params: 'a: Solid, b: Solid', returns: 'Solid' },
  difference: { params: 'a: Solid, b: Solid', returns: 'Solid' },
  intersection: { params: 'a: Solid, b: Solid', returns: 'Solid' },

  // Transforms
  translate: { params: 'solid: Solid, x: number, y: number, z: number', returns: 'Solid' },
  rotate: { params: 'solid: Solid, axis: [number, number, number], angle: number', returns: 'Solid' },
  scale: { params: 'solid: Solid, sx: number, sy: number, sz: number', returns: 'Solid' },

  // Mesh
  get_mesh: { params: 'solid: Solid, segments?: number', returns: 'WasmMesh' },
  getMesh: { params: 'solid: Solid, segments?: number', returns: 'WasmMesh' },

  // Sketch operations
  extrude: { params: 'profile: WasmSketchProfile, direction: [number, number, number]', returns: 'Solid' },
  revolve: { params: 'profile: WasmSketchProfile, axisOrigin: [number, number, number], axisDir: [number, number, number], angleDeg: number', returns: 'Solid' },

  // GPU
  init_gpu: { params: '', returns: 'boolean' },
  initGpu: { params: '', returns: 'boolean' },
  is_gpu_available: { params: '', returns: 'boolean' },
  isGpuAvailable: { params: '', returns: 'boolean' },
  process_geometry_gpu: { params: 'positions: number[], indices: number[], creaseAngle: number, generateLod: boolean', returns: 'GpuGeometryResult[]' },
  processGeometryGpu: { params: 'positions: number[], indices: number[], creaseAngle: number, generateLod: boolean', returns: 'GpuGeometryResult[]' },
  compute_creased_normals_gpu: { params: 'positions: number[], indices: number[], creaseAngle: number', returns: 'number[]' },
  computeCreasedNormalsGpu: { params: 'positions: number[], indices: number[], creaseAngle: number', returns: 'number[]' },

  // Physics
  is_physics_available: { params: '', returns: 'boolean' },
  isPhysicsAvailable: { params: '', returns: 'boolean' },

  // Text
  get_text_bounds: { params: 'text: string, fontData: Uint8Array, fontSize: number', returns: 'TextBoundsResult' },
  getTextBounds: { params: 'text: string, fontData: Uint8Array, fontSize: number', returns: 'TextBoundsResult' },

  // Version
  get_kernel_version: { params: '', returns: 'string' },
  getKernelVersion: { params: '', returns: 'string' },
};

/**
 * Get type info for a function name.
 */
function getTypeInfo(fnName: string): { params: string; returns: string } {
  // Try exact match
  if (KNOWN_FUNCTION_TYPES[fnName]) {
    return KNOWN_FUNCTION_TYPES[fnName]!;
  }

  // Try camelCase version
  const camelName = snakeToCamel(fnName);
  if (KNOWN_FUNCTION_TYPES[camelName]) {
    return KNOWN_FUNCTION_TYPES[camelName]!;
  }

  // Fallback to unknown
  return { params: '...args: unknown[]', returns: 'unknown' };
}

/**
 * Extract parameter names from a type signature for forwarding.
 */
function extractParamNames(params: string): string {
  if (params === '' || params === '...args: unknown[]') {
    return '';
  }

  // Parse "name: type, name?: type, ..." and extract names
  return params
    .split(',')
    .map((p) => p.trim().split(':')[0]?.replace('?', '').trim())
    .filter(Boolean)
    .join(', ');
}

function snakeToCamel(name: string): string {
  return name.replace(/_([a-z])/g, (_, letter) => letter.toUpperCase());
}

function capitalize(str: string): string {
  return str.charAt(0).toUpperCase() + str.slice(1);
}
