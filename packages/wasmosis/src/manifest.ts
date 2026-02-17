/**
 * wasmosis manifest - defines module splitting configuration
 */

/**
 * A function definition in the manifest.
 */
export interface ManifestFunction {
  /** Function name as exported to JS */
  name: string;
  /** Rust function name (if different) */
  rustName?: string;
  /** Whether this function returns a Solid (needs serialization for cross-module) */
  returnsSolid?: boolean;
  /** Whether this function takes a Solid parameter */
  takesSolid?: boolean;
}

/**
 * A module definition in the manifest.
 */
export interface ManifestModule {
  /** Module name */
  name: string;
  /** Module description */
  description?: string;
  /** Whether this is the core module (loaded eagerly) */
  isCore?: boolean;
  /** Functions in this module */
  functions: ManifestFunction[];
  /** Rust dependencies needed (crate names) */
  dependencies?: string[];
}

/**
 * The complete manifest.
 */
export interface Manifest {
  /** Base name for generated crates */
  baseName: string;
  /** Modules */
  modules: ManifestModule[];
}

/**
 * Parse a manifest from JSON.
 */
export function parseManifest(json: string): Manifest {
  return JSON.parse(json) as Manifest;
}

/**
 * Validate a manifest.
 */
export function validateManifest(manifest: Manifest): string[] {
  const errors: string[] = [];

  if (!manifest.baseName) {
    errors.push('Manifest must have a baseName');
  }

  if (!manifest.modules || manifest.modules.length === 0) {
    errors.push('Manifest must have at least one module');
  }

  const coreModules = manifest.modules.filter((m) => m.isCore);
  if (coreModules.length === 0) {
    errors.push('Manifest must have exactly one core module');
  } else if (coreModules.length > 1) {
    errors.push('Manifest cannot have multiple core modules');
  }

  const functionNames = new Set<string>();
  for (const module of manifest.modules) {
    for (const fn of module.functions) {
      if (functionNames.has(fn.name)) {
        errors.push(`Duplicate function name: ${fn.name}`);
      }
      functionNames.add(fn.name);
    }
  }

  return errors;
}

/**
 * Generate a manifest from parsed Rust source.
 */
export function generateManifestFromSource(
  baseName: string,
  parseResult: import('./source-parser.js').ParseResult
): Manifest {
  const modules: ManifestModule[] = [];

  // Core module
  modules.push({
    name: 'core',
    description: 'Core module - always loaded',
    isCore: true,
    functions: parseResult.core.map((f) => ({
      name: f.name,
      returnsSolid: f.dependencies.includes('Solid'),
      takesSolid: f.signature.includes('&Solid') || f.signature.includes('solid:'),
    })),
  });

  // Secondary modules
  for (const [name, functions] of parseResult.modules) {
    modules.push({
      name,
      description: `${name} module - lazy loaded`,
      isCore: false,
      functions: functions.map((f) => ({
        name: f.name,
        returnsSolid: f.dependencies.includes('Solid'),
        takesSolid: f.signature.includes('&Solid') || f.signature.includes('solid:'),
      })),
    });
  }

  return { baseName, modules };
}
