/**
 * Type definitions for part generators.
 */

/** Parameter value types supported by generators. */
export type ParamValue = number | boolean | string;

/** Generic parameter map for a generated part. */
export interface PartParams {
  [key: string]: ParamValue;
}

/** A generated part with its VCode and metadata. */
export interface GeneratedPart {
  /** VCode representation. */
  vcode: string;
  /** Loon source equivalent (for training loon-native models). */
  loon?: string;
  /** Parameters used to generate this part. */
  params: PartParams;
  /** Part family name (e.g., "plate", "bracket"). */
  family: string;
  /** Complexity score (1-5 scale). */
  complexity: number;
}

/** Range for numeric parameters. */
export interface ParamRange {
  min: number;
  max: number;
  /** Step size (for discrete values). */
  step?: number;
}

/** Definition of a parameter including range and type. */
export interface ParamDef {
  type: "number" | "boolean" | "choice";
  range?: ParamRange;
  choices?: string[];
  default?: ParamValue;
  description?: string;
}

/** Interface that all part generators must implement. */
export interface PartGenerator {
  /** Unique family name (e.g., "plate", "bracket"). */
  readonly family: string;
  /** Human-readable description of this part family. */
  readonly description: string;

  /**
   * Generate a part with the given parameters.
   * Missing parameters will be filled with random values.
   */
  generate(params?: Partial<PartParams>): GeneratedPart;

  /** Generate random parameters within valid ranges. */
  randomParams(): PartParams;

  /** Get parameter definitions for this generator. */
  paramDefs(): Record<string, ParamDef>;
}

/** A training example with text description and VCode. */
export interface TrainingExample {
  /** Natural language description. */
  text: string;
  /** VCode representation. */
  ir: string;
  /** Loon source equivalent (for training loon output). */
  loon?: string;
  /** Part family name. */
  family: string;
  /** Complexity score. */
  complexity: number;
}

/** Result of validating a training example. */
export interface ValidationResult {
  valid: boolean;
  error?: string;
  message?: string;
  volume?: number;
  triangles?: number;
}
