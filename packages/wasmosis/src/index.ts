/**
 * wasmosis - Lazy WASM module splitting
 *
 * @packageDocumentation
 */

// Core types
export type {
  ModuleDefinition,
  ModuleState,
  RegistryConfig,
  SharedImports,
  SplitConfig,
  SplitOutput,
  WasmExports,
  WasmLoader,
  WasmMetadata,
  WasmModule,
} from './types.js';

export { WASMOSIS_SECTION_NAME } from './types.js';

// Module definition helpers
export { defineModule, urlLoader, bytesLoader } from './module.js';
export type { DefineModuleOptions } from './module.js';

// Registry
export { Registry, createRegistry } from './registry.js';

// Graph utilities
export { topologicalSort, getDependents, validateGraph } from './graph.js';

// Environment utilities
export {
  extractSharedImports,
  createSecondaryImports,
  createStandaloneImports,
  mergeImports,
} from './environment.js';

// Source parsing (for codegen)
export {
  parseRustSource,
  parseRustSourceContent,
  parseRustSourceWithInference,
  parseRustSourceContentWithInference,
  inferModule,
} from './source-parser.js';
export type {
  ParseResult,
  ParsedFunction,
  ParsedType,
  InferredModule,
  InferenceReason,
} from './source-parser.js';

// Crate generation
export { generateCrates, generateTypeScriptLoader } from './crate-generator.js';
export type { CrateGeneratorOptions, GeneratedCrate } from './crate-generator.js';

// Manifest
export {
  parseManifest,
  validateManifest,
  generateManifestFromSource,
} from './manifest.js';
export type { Manifest, ManifestModule, ManifestFunction } from './manifest.js';
