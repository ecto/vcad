/**
 * Rust source code parser for wasmosis module annotations.
 *
 * Parses Rust source files to extract #[module("name")] annotations
 * and function signatures for multi-crate code generation.
 *
 * ## Automatic Module Inference
 *
 * Functions are automatically assigned to modules based on:
 * 1. Explicit `#[module("name")]` annotation (highest priority)
 * 2. Feature gate: `#[cfg(feature = "X")]` → module "X"
 * 3. Dependency detection in function body (crate paths)
 * 4. Default: "core"
 */

import { readFileSync } from 'node:fs';

/**
 * How a function's module was determined.
 */
export type InferenceReason =
  | 'explicit' // #[module("name")]
  | 'feature-gate' // #[cfg(feature = "name")]
  | 'dependency' // Uses vcad_kernel_physics::, etc.
  | 'default'; // No inference, falls to core

/**
 * Inferred module assignment with reason.
 */
export interface InferredModule {
  /** Module name */
  name: string;
  /** How the module was determined */
  reason: InferenceReason;
  /** Additional detail (e.g., which dependency triggered it) */
  detail?: string;
}

/**
 * A parsed function from the source.
 */
export interface ParsedFunction {
  /** Function name */
  name: string;
  /** Module assignment from #[module("name")], or null for core */
  module: string | null;
  /** Full function signature (for reference) */
  signature: string;
  /** Function body (for copying to generated crate) */
  body: string;
  /** Line number in source */
  line: number;
  /** Associated types and structs this function uses */
  dependencies: string[];
  /** Feature gate wrapping this function, if any */
  featureGate?: string;
  /** How the module was inferred */
  inference?: InferredModule;
}

/**
 * A parsed type definition (struct, enum, type alias).
 */
export interface ParsedType {
  /** Type name */
  name: string;
  /** Full definition */
  definition: string;
  /** Line number in source */
  line: number;
}

/**
 * Result of parsing a Rust source file.
 */
export interface ParseResult {
  /** All functions grouped by module */
  modules: Map<string, ParsedFunction[]>;
  /** Core functions (no module annotation) */
  core: ParsedFunction[];
  /** Type definitions that should be shared */
  types: ParsedType[];
  /** Use statements at the top of the file */
  uses: string[];
  /** Module-level attributes */
  attributes: string[];
}

/**
 * Parse a Rust source file for wasmosis annotations.
 */
export function parseRustSource(filePath: string): ParseResult {
  const source = readFileSync(filePath, 'utf-8');
  return parseRustSourceContent(source);
}

/**
 * Parse Rust source content for wasmosis annotations.
 */
export function parseRustSourceContent(source: string): ParseResult {
  const modules = new Map<string, ParsedFunction[]>();
  const core: ParsedFunction[] = [];
  const types: ParsedType[] = [];
  const uses: string[] = [];
  const attributes: string[] = [];

  const lines = source.split('\n');
  let i = 0;

  // Extract module-level attributes and use statements
  while (i < lines.length) {
    const line = lines[i]!.trim();

    // Module-level attributes (#![...])
    if (line.startsWith('#![')) {
      attributes.push(lines[i]!);
      i++;
      continue;
    }

    // Use statements
    if (line.startsWith('use ')) {
      let useStmt = lines[i]!;
      // Handle multi-line use statements
      while (!useStmt.trim().endsWith(';')) {
        i++;
        if (i >= lines.length) break;
        useStmt += '\n' + lines[i]!;
      }
      uses.push(useStmt);
      i++;
      continue;
    }

    // Skip doc comments and empty lines at the top
    if (line.startsWith('//') || line === '') {
      i++;
      continue;
    }

    // Stop when we hit non-header content
    break;
  }

  // Parse the rest of the file
  while (i < lines.length) {
    const line = lines[i]!;
    const trimmedLine = line.trim();

    // Skip empty lines and comments
    if (trimmedLine === '' || trimmedLine.startsWith('//')) {
      i++;
      continue;
    }

    // Parse type definitions (struct, enum, type)
    if (
      trimmedLine.startsWith('pub struct ') ||
      trimmedLine.startsWith('pub enum ') ||
      trimmedLine.startsWith('pub type ') ||
      trimmedLine.startsWith('#[derive')
    ) {
      const typeResult = parseTypeDefinition(lines, i);
      if (typeResult) {
        types.push(typeResult.type);
        i = typeResult.endLine + 1;
        continue;
      }
    }

    // Parse impl blocks (skip them for now, we're focused on functions)
    if (trimmedLine.startsWith('impl ')) {
      const endLine = findBlockEnd(lines, i);
      i = endLine + 1;
      continue;
    }

    // Look for #[module("name")] or #[wasmosis::module("name")]
    const moduleMatch = trimmedLine.match(
      /^#\[(?:wasmosis::)?module\("([^"]+)"\)\]$/
    );
    if (moduleMatch) {
      const moduleName = moduleMatch[1]!;
      // Parse the following function
      const funcResult = parseFunctionAfterAttribute(lines, i + 1);
      if (funcResult) {
        funcResult.func.module = moduleName;
        if (!modules.has(moduleName)) {
          modules.set(moduleName, []);
        }
        modules.get(moduleName)!.push(funcResult.func);
        i = funcResult.endLine + 1;
        continue;
      }
    }

    // Look for #[wasm_bindgen] without #[module]
    if (trimmedLine === '#[wasm_bindgen]' || trimmedLine.startsWith('#[wasm_bindgen(')) {
      // Check if there's a module annotation before this
      const prevLine = i > 0 ? lines[i - 1]!.trim() : '';
      const isModuleAnnotated = prevLine.match(
        /^#\[(?:wasmosis::)?module\("([^"]+)"\)\]$/
      );

      if (!isModuleAnnotated) {
        // This is a core function
        const funcResult = parseFunctionAfterAttribute(lines, i + 1);
        if (funcResult) {
          funcResult.func.module = null;
          core.push(funcResult.func);
          i = funcResult.endLine + 1;
          continue;
        }
      }
    }

    // Look for pub fn without annotations (also core)
    if (trimmedLine.startsWith('pub fn ') || trimmedLine.startsWith('pub async fn ')) {
      const funcResult = parseFunction(lines, i);
      if (funcResult) {
        funcResult.func.module = null;
        core.push(funcResult.func);
        i = funcResult.endLine + 1;
        continue;
      }
    }

    i++;
  }

  return { modules, core, types, uses, attributes };
}

interface FunctionParseResult {
  func: ParsedFunction;
  endLine: number;
}

interface TypeParseResult {
  type: ParsedType;
  endLine: number;
}

/**
 * Parse a function definition starting after an attribute.
 */
function parseFunctionAfterAttribute(
  lines: string[],
  startLine: number
): FunctionParseResult | null {
  let i = startLine;

  // Skip additional attributes
  while (i < lines.length) {
    const line = lines[i]!.trim();
    if (line.startsWith('#[') || line.startsWith('///') || line === '') {
      i++;
      continue;
    }
    break;
  }

  return parseFunction(lines, i);
}

/**
 * Parse a function definition.
 */
function parseFunction(lines: string[], startLine: number): FunctionParseResult | null {
  const line = lines[startLine]!;
  const trimmedLine = line.trim();

  // Match function signature
  const fnMatch = trimmedLine.match(/^pub\s+(?:async\s+)?fn\s+(\w+)/);
  if (!fnMatch) return null;

  const name = fnMatch[1]!;

  // Find the function body
  let signature = '';
  let braceCount = 0;
  let bodyStart = startLine;
  let foundBody = false;

  // First, collect the signature (up to the opening brace)
  for (let i = startLine; i < lines.length; i++) {
    const currentLine = lines[i]!;
    signature += (i > startLine ? '\n' : '') + currentLine;

    for (const char of currentLine) {
      if (char === '{') {
        if (!foundBody) {
          foundBody = true;
          bodyStart = i;
        }
        braceCount++;
      } else if (char === '}') {
        braceCount--;
        if (foundBody && braceCount === 0) {
          // Found the end of the function
          const body = lines.slice(bodyStart, i + 1).join('\n');
          return {
            func: {
              name,
              module: null,
              signature: signature.trim(),
              body,
              line: startLine + 1,
              dependencies: extractDependencies(signature + '\n' + body),
            },
            endLine: i,
          };
        }
      }
    }
  }

  return null;
}

/**
 * Parse a type definition (struct, enum, type alias).
 */
function parseTypeDefinition(
  lines: string[],
  startLine: number
): TypeParseResult | null {
  let i = startLine;
  let definition = '';

  // Collect any attributes
  while (i < lines.length) {
    const line = lines[i]!.trim();
    if (line.startsWith('#[') || line.startsWith('///')) {
      definition += lines[i] + '\n';
      i++;
    } else {
      break;
    }
  }

  if (i >= lines.length) return null;

  const line = lines[i]!.trim();

  // Type alias
  if (line.startsWith('pub type ')) {
    const match = line.match(/^pub\s+type\s+(\w+)/);
    if (!match) return null;
    definition += lines[i];
    return {
      type: { name: match[1]!, definition: definition.trim(), line: startLine + 1 },
      endLine: i,
    };
  }

  // Struct or enum
  const match = line.match(/^pub\s+(struct|enum)\s+(\w+)/);
  if (!match) return null;

  const name = match[2]!;
  const endLine = findBlockEnd(lines, i);

  definition += lines.slice(i, endLine + 1).join('\n');

  return {
    type: { name, definition: definition.trim(), line: startLine + 1 },
    endLine,
  };
}

/**
 * Find the end of a block (matching braces).
 */
function findBlockEnd(lines: string[], startLine: number): number {
  let braceCount = 0;
  let foundBrace = false;

  for (let i = startLine; i < lines.length; i++) {
    for (const char of lines[i]!) {
      if (char === '{') {
        foundBrace = true;
        braceCount++;
      } else if (char === '}') {
        braceCount--;
        if (foundBrace && braceCount === 0) {
          return i;
        }
      }
    }
  }

  return lines.length - 1;
}

/**
 * Extract type dependencies from code.
 */
function extractDependencies(code: string): string[] {
  const deps = new Set<string>();

  // Match type references (capitalized identifiers)
  const typeMatches = code.matchAll(/\b([A-Z][a-zA-Z0-9]*)\b/g);
  for (const match of typeMatches) {
    deps.add(match[1]!);
  }

  // Remove common built-in types
  const builtIns = new Set([
    'String',
    'Vec',
    'Option',
    'Result',
    'Box',
    'Arc',
    'Rc',
    'HashMap',
    'HashSet',
    'BTreeMap',
    'BTreeSet',
    'JsValue',
    'JsError',
    'Self',
    'Ok',
    'Err',
    'Some',
    'None',
  ]);

  for (const builtin of builtIns) {
    deps.delete(builtin);
  }

  return Array.from(deps);
}

// =============================================================================
// Automatic Module Inference
// =============================================================================

/**
 * Mapping from crate/module paths to wasmosis module names.
 * Order matters - first match wins.
 */
const DEPENDENCY_TO_MODULE: [RegExp, string][] = [
  [/vcad_kernel_physics/, 'physics'],
  [/vcad_kernel_gpu/, 'gpu'],
  [/vcad_kernel_raytrace/, 'raytrace'],
  [/vcad_slicer/, 'slicer'],
  [/vcad_kernel_cam/, 'cam'],
  [/vcad_kernel_drafting/, 'drafting'],
  [/stepperoni/, 'step'],
];

/**
 * Detect which crate dependencies are used in code.
 */
function detectCrateDependencies(code: string): string[] {
  const found: string[] = [];

  for (const [pattern, moduleName] of DEPENDENCY_TO_MODULE) {
    if (pattern.test(code)) {
      found.push(moduleName);
    }
  }

  return found;
}

/**
 * Feature names that should NOT be treated as module names.
 * These are compile-time features that don't correspond to WASM modules.
 */
const NON_MODULE_FEATURES = new Set([
  'ts-rs',
  'console_error_panic_hook',
  'default',
  'std',
  'alloc',
  'test',
]);

/**
 * Infer the module for a function based on feature gates and dependencies.
 *
 * Priority:
 * 1. Explicit #[module("name")] annotation
 * 2. Feature gate: #[cfg(feature = "X")] (if X is a module-type feature)
 * 3. Crate dependency detection
 * 4. Default to "core"
 */
export function inferModule(
  func: ParsedFunction,
  explicitModule: string | null
): InferredModule {
  // 1. Explicit annotation takes priority
  if (explicitModule !== null) {
    return { name: explicitModule, reason: 'explicit' };
  }

  // 2. Feature gate (only if it's a module-type feature)
  if (func.featureGate && !NON_MODULE_FEATURES.has(func.featureGate)) {
    return { name: func.featureGate, reason: 'feature-gate' };
  }

  // 3. Dependency detection
  const deps = detectCrateDependencies(func.signature + '\n' + func.body);
  if (deps.length > 0) {
    // Use the first detected dependency
    return {
      name: deps[0]!,
      reason: 'dependency',
      detail: deps.join(', '),
    };
  }

  // 4. Default to core
  return { name: 'core', reason: 'default' };
}

/**
 * Parse with automatic module inference enabled.
 *
 * Uses automatic inference instead of
 * requiring explicit #[module("name")] annotations.
 */
export function parseRustSourceWithInference(filePath: string): ParseResult {
  const source = readFileSync(filePath, 'utf-8');
  return parseRustSourceContentWithInference(source);
}

/**
 * Parse source content with automatic module inference.
 */
export function parseRustSourceContentWithInference(source: string): ParseResult {
  const modules = new Map<string, ParsedFunction[]>();
  const core: ParsedFunction[] = [];
  const types: ParsedType[] = [];
  const uses: string[] = [];
  const attributes: string[] = [];

  const lines = source.split('\n');
  let i = 0;

  // Track current feature gate scope
  let currentFeatureGate: string | null = null;
  let featureGateBraceDepth = 0;

  // Extract module-level attributes and use statements
  while (i < lines.length) {
    const line = lines[i]!.trim();

    if (line.startsWith('#![')) {
      attributes.push(lines[i]!);
      i++;
      continue;
    }

    if (line.startsWith('use ')) {
      let useStmt = lines[i]!;
      while (!useStmt.trim().endsWith(';')) {
        i++;
        if (i >= lines.length) break;
        useStmt += '\n' + lines[i]!;
      }
      uses.push(useStmt);
      i++;
      continue;
    }

    if (line.startsWith('//') || line === '') {
      i++;
      continue;
    }

    break;
  }

  // Parse the rest of the file
  while (i < lines.length) {
    const line = lines[i]!;
    const trimmedLine = line.trim();

    // Track brace depth for feature gate scope
    for (const char of line) {
      if (char === '{') featureGateBraceDepth++;
      if (char === '}') {
        featureGateBraceDepth--;
        if (featureGateBraceDepth === 0) {
          currentFeatureGate = null;
        }
      }
    }

    if (trimmedLine === '' || trimmedLine.startsWith('//')) {
      i++;
      continue;
    }

    // Detect feature gate
    const featureMatch = trimmedLine.match(/^#\[cfg\(feature\s*=\s*"([^"]+)"\)\]$/);
    if (featureMatch) {
      currentFeatureGate = featureMatch[1]!;
      featureGateBraceDepth = 0; // Will be set when we hit the first brace
      i++;
      continue;
    }

    // Parse type definitions
    if (
      trimmedLine.startsWith('pub struct ') ||
      trimmedLine.startsWith('pub enum ') ||
      trimmedLine.startsWith('pub type ') ||
      trimmedLine.startsWith('#[derive')
    ) {
      const typeResult = parseTypeDefinition(lines, i);
      if (typeResult) {
        types.push(typeResult.type);
        i = typeResult.endLine + 1;
        continue;
      }
    }

    // Parse impl blocks
    if (trimmedLine.startsWith('impl ')) {
      const endLine = findBlockEnd(lines, i);
      i = endLine + 1;
      continue;
    }

    // Look for explicit #[module("name")] or #[wasmosis::module("name")]
    const moduleMatch = trimmedLine.match(
      /^#\[(?:wasmosis::)?module\("([^"]+)"\)\]$/
    );
    if (moduleMatch) {
      const explicitModule = moduleMatch[1]!;
      const funcResult = parseFunctionAfterAttributeWithFeature(lines, i + 1, currentFeatureGate);
      if (funcResult) {
        const inference = inferModule(funcResult.func, explicitModule);
        funcResult.func.module = inference.name;
        funcResult.func.inference = inference;

        if (inference.name === 'core') {
          core.push(funcResult.func);
        } else {
          if (!modules.has(inference.name)) {
            modules.set(inference.name, []);
          }
          modules.get(inference.name)!.push(funcResult.func);
        }
        i = funcResult.endLine + 1;
        continue;
      }
    }

    // Look for #[wasm_bindgen] without explicit #[module]
    if (trimmedLine === '#[wasm_bindgen]' || trimmedLine.startsWith('#[wasm_bindgen(')) {
      const prevLine = i > 0 ? lines[i - 1]!.trim() : '';
      const isModuleAnnotated = prevLine.match(
        /^#\[(?:wasmosis::)?module\("([^"]+)"\)\]$/
      );

      if (!isModuleAnnotated) {
        const funcResult = parseFunctionAfterAttributeWithFeature(lines, i + 1, currentFeatureGate);
        if (funcResult) {
          // Automatic inference (no explicit module)
          const inference = inferModule(funcResult.func, null);
          funcResult.func.module = inference.name === 'core' ? null : inference.name;
          funcResult.func.inference = inference;

          if (inference.name === 'core') {
            core.push(funcResult.func);
          } else {
            if (!modules.has(inference.name)) {
              modules.set(inference.name, []);
            }
            modules.get(inference.name)!.push(funcResult.func);
          }
          i = funcResult.endLine + 1;
          continue;
        }
      }
    }

    // Look for pub fn without annotations
    if (trimmedLine.startsWith('pub fn ') || trimmedLine.startsWith('pub async fn ')) {
      const funcResult = parseFunctionWithFeature(lines, i, currentFeatureGate);
      if (funcResult) {
        const inference = inferModule(funcResult.func, null);
        funcResult.func.module = inference.name === 'core' ? null : inference.name;
        funcResult.func.inference = inference;

        if (inference.name === 'core') {
          core.push(funcResult.func);
        } else {
          if (!modules.has(inference.name)) {
            modules.set(inference.name, []);
          }
          modules.get(inference.name)!.push(funcResult.func);
        }
        i = funcResult.endLine + 1;
        continue;
      }
    }

    i++;
  }

  return { modules, core, types, uses, attributes };
}

/**
 * Parse a function definition with feature gate tracking.
 */
function parseFunctionWithFeature(
  lines: string[],
  startLine: number,
  featureGate: string | null
): FunctionParseResult | null {
  const result = parseFunction(lines, startLine);
  if (result && featureGate) {
    result.func.featureGate = featureGate;
  }
  return result;
}

/**
 * Parse a function after an attribute with feature gate tracking.
 */
function parseFunctionAfterAttributeWithFeature(
  lines: string[],
  startLine: number,
  featureGate: string | null
): FunctionParseResult | null {
  const result = parseFunctionAfterAttribute(lines, startLine);
  if (result && featureGate) {
    result.func.featureGate = featureGate;
  }
  return result;
}
