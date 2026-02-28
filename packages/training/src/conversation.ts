/**
 * Conversation generator - creates multi-turn conversational training data.
 *
 * Generates examples where a user iteratively refines a CAD part through
 * multiple turns of natural language instructions.
 */

import type { GeneratedPart, PartGenerator, PartParams } from "./generators/types.js";
import { generators, randChoice, randInt, randFloat, randBool } from "./generators/index.js";

/** A single turn in a conversation. */
export interface Turn {
  role: "user" | "assistant";
  content: string;
}

/** A multi-turn conversation training example. */
export interface ConversationExample {
  /** Array of conversation turns. */
  conversation: Turn[];
  /** Part family name. */
  family: string;
  /** Number of user-assistant turn pairs. */
  turns: number;
}

/** A modification to apply to part parameters. */
export interface Modification {
  type: "dimension" | "feature" | "quantity" | "position" | "property";
  /** User-facing description of the modification. */
  description: string;
  /** Changes to apply to parameters. */
  paramChanges: Partial<PartParams>;
}

/** Modification generators per part family. */
type ModificationGenerator = (params: PartParams, generator: PartGenerator) => Modification | null;

/** Generate a dimension modification (make bigger/smaller/thicker). */
function generateDimensionMod(params: PartParams): Modification | null {
  const numericParams = Object.entries(params).filter(
    ([_, v]) => typeof v === "number" && v > 0
  ) as [string, number][];
  if (numericParams.length === 0) return null;

  const [key, value] = randChoice(numericParams);
  const increase = randBool(0.5);
  const factor = increase ? randFloat({ min: 1.1, max: 1.5 }, 1) : randFloat({ min: 0.6, max: 0.9 }, 1);
  const newValue = Math.round(value * factor * 10) / 10;

  const dimNames: Record<string, string> = {
    width: "wider",
    depth: "deeper",
    height: "taller",
    thickness: increase ? "thicker" : "thinner",
    radius: increase ? "larger radius" : "smaller radius",
    length: increase ? "longer" : "shorter",
    outerDiameter: increase ? "larger diameter" : "smaller diameter",
    innerDiameter: increase ? "larger bore" : "smaller bore",
    holeDiameter: increase ? "larger holes" : "smaller holes",
  };

  const friendlyName = dimNames[key] || (increase ? `larger ${key}` : `smaller ${key}`);
  const descriptions = [
    `make it ${friendlyName}`,
    `change the ${key} to ${newValue}mm`,
    increase ? `increase the ${key}` : `reduce the ${key}`,
    `${newValue}mm ${key} instead`,
  ];

  return {
    type: "dimension",
    description: randChoice(descriptions),
    paramChanges: { [key]: newValue },
  };
}

/** Generate a feature modification (add/remove holes, etc). */
function generateFeatureMod(params: PartParams, generator: PartGenerator): Modification | null {
  const family = generator.family;

  // Family-specific feature modifications
  switch (family) {
    case "plate": {
      const patterns = ["corners", "edges", "grid", "center", "corners+center", "none"];
      const currentPattern = params.holePattern as string;
      const newPattern = randChoice(patterns.filter(p => p !== currentPattern));

      const descriptions: Record<string, string> = {
        "corners": "add corner mounting holes",
        "edges": "add edge holes instead",
        "grid": "use a grid hole pattern",
        "center": "add a center hole only",
        "corners+center": "add corner holes and a center hole",
        "none": "remove all the holes",
      };

      return {
        type: "feature",
        description: descriptions[newPattern] || `change to ${newPattern} pattern`,
        paramChanges: { holePattern: newPattern },
      };
    }

    case "ball": {
      const types = ["sphere", "dome", "drilled", "knob", "handle"];
      const currentType = params.ballType as string;
      const newType = randChoice(types.filter(t => t !== currentType));

      const descriptions: Record<string, string> = {
        "sphere": "make it a solid ball",
        "dome": "make it a dome/hemisphere",
        "drilled": "add a through-hole",
        "knob": "make it a knob with flat base",
        "handle": "add a stem for a handle",
      };

      return {
        type: "feature",
        description: descriptions[newType] || `change to ${newType}`,
        paramChanges: { ballType: newType },
      };
    }

    case "hollow": {
      const hasHandle = params.hasHandle as boolean;
      const hasTabs = params.hasTabs as boolean;

      if (params.hollowType === "cup" && !hasHandle) {
        return {
          type: "feature",
          description: "add a handle to the cup",
          paramChanges: { hasHandle: true },
        };
      }
      if (params.hollowType === "housing" && !hasTabs) {
        return {
          type: "feature",
          description: "add mounting tabs",
          paramChanges: { hasTabs: true },
        };
      }
      break;
    }

    case "flange": {
      const types = ["flat", "hubbed", "raised"];
      const currentType = params.flangeType as string;
      const newType = randChoice(types.filter(t => t !== currentType));

      const descriptions: Record<string, string> = {
        "flat": "make it a flat flange",
        "hubbed": "add a hub to the flange",
        "raised": "add a raised face",
      };

      return {
        type: "feature",
        description: descriptions[newType],
        paramChanges: { flangeType: newType },
      };
    }
  }

  return null;
}

/** Generate a quantity modification (more/fewer holes, teeth, etc). */
function generateQuantityMod(params: PartParams): Modification | null {
  const countParams = Object.entries(params).filter(
    ([k, v]) => typeof v === "number" && (k.includes("count") || k.includes("Count") || k === "boltCount" || k === "sides" || k === "steps")
  ) as [string, number][];

  if (countParams.length === 0) return null;

  const [key, value] = randChoice(countParams);
  const increase = randBool(0.5);
  const delta = randInt({ min: 1, max: 3 });
  const newValue = increase ? value + delta : Math.max(2, value - delta);

  if (newValue === value) return null;

  const itemNames: Record<string, string> = {
    count: "copies",
    boltCount: "bolt holes",
    sides: "sides",
    steps: "steps",
  };

  const itemName = itemNames[key] || key.replace("Count", "").toLowerCase() + "s";
  const descriptions = [
    increase ? `add ${delta} more ${itemName}` : `remove ${delta} ${itemName}`,
    `change to ${newValue} ${itemName}`,
    increase ? `more ${itemName}` : `fewer ${itemName}`,
  ];

  return {
    type: "quantity",
    description: randChoice(descriptions),
    paramChanges: { [key]: newValue },
  };
}

/** Generate a position/spacing modification. */
function generatePositionMod(params: PartParams): Modification | null {
  const spacingParams = Object.entries(params).filter(
    ([k, v]) => typeof v === "number" && (k.includes("spacing") || k.includes("Spacing") || k.includes("inset") || k.includes("Inset"))
  ) as [string, number][];

  if (spacingParams.length === 0) return null;

  const [key, value] = randChoice(spacingParams);
  const increase = randBool(0.5);
  const factor = increase ? randFloat({ min: 1.2, max: 1.5 }, 1) : randFloat({ min: 0.6, max: 0.9 }, 1);
  const newValue = Math.round(value * factor * 10) / 10;

  const descriptions = [
    increase ? `space them further apart` : `move them closer together`,
    `change spacing to ${newValue}mm`,
    increase ? `increase the spacing` : `reduce the spacing`,
  ];

  return {
    type: "position",
    description: randChoice(descriptions),
    paramChanges: { [key]: newValue },
  };
}

/** Generate a random modification for a part. */
function generateModification(params: PartParams, generator: PartGenerator): Modification | null {
  const modGenerators: ModificationGenerator[] = [
    generateDimensionMod,
    (p, g) => generateFeatureMod(p, g),
    generateQuantityMod,
    generatePositionMod,
  ];

  // Try each generator until one succeeds
  const shuffled = [...modGenerators].sort(() => Math.random() - 0.5);
  for (const gen of shuffled) {
    const mod = gen(params, generator);
    if (mod) return mod;
  }

  // Fallback to dimension modification
  return generateDimensionMod(params);
}

/** Generate initial request text for a part. */
function describeInitialRequest(params: PartParams, family: string): string {
  // Generate a natural language request based on family and key parameters
  const templates: Record<string, (p: PartParams) => string[]> = {
    plate: (p) => [
      `${p.width}x${p.depth}mm mounting plate ${p.thickness}mm thick`,
      `rectangular plate ${p.width} by ${p.depth} by ${p.thickness}mm`,
      `flat plate ${p.width}x${p.depth}x${p.thickness}`,
    ],
    ball: (p) => [
      `${(p.radius as number) * 2}mm ball`,
      `sphere with ${p.radius}mm radius`,
      `${(p.radius as number) * 2}mm diameter ball`,
    ],
    funnel: (p) => [
      `cone ${p.bottomRadius}mm base ${p.height}mm tall`,
      `conical shape ${p.bottomRadius}mm to ${p.topRadius}mm`,
      `funnel ${p.height}mm height`,
    ],
    array: (p) => [
      `${p.length}mm bar with ${p.count} holes`,
      `linear pattern of ${p.count} holes along ${p.length}mm`,
      `${p.length}mm rail with hole pattern`,
    ],
    radial: (p) => [
      `${p.outerDiameter}mm disc with ${p.count} radial features`,
      `circular pattern ${p.count} elements around ${p.outerDiameter}mm diameter`,
      `${p.outerDiameter}mm wheel with ${p.count} spokes`,
    ],
    hollow: (p) => [
      `hollow box ${p.outerWidth}x${p.outerDepth}x${p.outerHeight}mm`,
      `container ${p.outerWidth} by ${p.outerDepth} by ${p.outerHeight}mm`,
      `shelled enclosure ${p.wallThickness}mm walls`,
    ],
    profile: (p) => [
      `${p.profileType} profile ${p.length}mm long`,
      `extruded ${p.profileType} channel ${p.length}mm`,
      `${p.length}mm ${p.profileType} extrusion`,
    ],
    turned: (p) => [
      `turned part ${p.height}mm tall ${p.maxRadius}mm radius`,
      `${p.turnedType} shape revolved`,
      `lathe-style ${p.turnedType} ${p.height}mm height`,
    ],
  };

  const familyTemplates = templates[family];
  if (familyTemplates) {
    return randChoice(familyTemplates(params));
  }

  // Generic fallback
  return `${family} part`;
}

/**
 * Generate a multi-turn conversation for iterative part refinement.
 *
 * @param generator - Part generator to use
 * @param numTurns - Number of user-assistant turn pairs (2-5)
 * @returns Conversation example with all turns
 */
export function generateConversation(
  generator: PartGenerator,
  numTurns: number = 3,
): ConversationExample {
  // Clamp turns to reasonable range
  numTurns = Math.max(2, Math.min(5, numTurns));

  let params = generator.randomParams();
  const turns: Turn[] = [];

  // Turn 1: Initial request
  const initialRequest = describeInitialRequest(params, generator.family);
  turns.push({ role: "user", content: initialRequest });

  let part = generator.generate(params);
  turns.push({ role: "assistant", content: part.vcode });

  // Subsequent turns: modifications
  for (let i = 1; i < numTurns; i++) {
    const mod = generateModification(params, generator);
    if (!mod) continue;

    // Apply modification (merge changes, filtering out undefined values)
    const changes = Object.fromEntries(
      Object.entries(mod.paramChanges).filter(([_, v]) => v !== undefined)
    ) as PartParams;
    params = { ...params, ...changes };

    turns.push({ role: "user", content: mod.description });

    part = generator.generate(params);
    turns.push({ role: "assistant", content: part.vcode });
  }

  return {
    conversation: turns,
    family: generator.family,
    turns: Math.floor(turns.length / 2),
  };
}

/**
 * Generate multiple conversation examples.
 *
 * @param count - Number of conversations to generate
 * @param options - Generation options
 * @returns Array of conversation examples
 */
export function generateConversations(
  count: number,
  options: {
    families?: string[];
    minTurns?: number;
    maxTurns?: number;
    onProgress?: (completed: number, total: number) => void;
  } = {},
): ConversationExample[] {
  const {
    families = Object.keys(generators),
    minTurns = 2,
    maxTurns = 4,
    onProgress,
  } = options;

  const examples: ConversationExample[] = [];
  const availableGenerators = families
    .map(f => generators[f])
    .filter(Boolean);

  if (availableGenerators.length === 0) {
    throw new Error("No valid generators found for specified families");
  }

  for (let i = 0; i < count; i++) {
    const generator = randChoice(availableGenerators);
    const numTurns = randInt({ min: minTurns, max: maxTurns });

    const conversation = generateConversation(generator, numTurns);
    examples.push(conversation);

    onProgress?.(i + 1, count);
  }

  return examples;
}

/**
 * Format conversation for training (chat template format).
 */
export function formatConversationForTraining(conv: ConversationExample): string {
  return conv.conversation
    .map(t => `<|${t.role}|>\n${t.content}`)
    .join("\n");
}

/**
 * Convert conversation to ShareGPT format (common for fine-tuning).
 */
export function toShareGPTFormat(conv: ConversationExample): {
  conversations: Array<{ from: "human" | "gpt"; value: string }>;
} {
  return {
    conversations: conv.conversation.map(t => ({
      from: t.role === "user" ? "human" : "gpt",
      value: t.content,
    })),
  };
}
