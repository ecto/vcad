/**
 * Send a `DocCommand` to the Rust document engine.
 *
 * Every document mutation on the web goes through here, so the web speaks
 * the same command vocabulary as the native apps (`vcad_engine_apply`) and
 * the AI planner (`vcad_app::command`). The engine fails closed: an unknown
 * id or malformed payload comes back as a `CommandError`, which this
 * throws as a {@link DocCommandError} instead of the old silent no-op.
 */

import type { CommandError, DocCommand, Document } from "@vcad/ir";
import type { PartInfo } from "../types.js";

/** Materialized engine state returned by every successful command. */
export interface CommandResult {
  document: Document;
  parts: PartInfo[];
  consumedPartIds: string[];
  warnings?: string[];
  createdFeatureId?: string;
}

export type Envelope =
  | { ok: true; result: CommandResult }
  | { ok: false; error: CommandError & { message: string } };

/** A command the engine refused. `error.kind` says why. */
export class DocCommandError extends Error {
  constructor(
    readonly command: DocCommand,
    readonly error: CommandError & { message: string },
  ) {
    super(`${command.type}: ${error.message}`);
    this.name = "DocCommandError";
  }
}

/** The slice of `WasmDocumentEngine` this module needs. */
export interface CommandEngine {
  apply?(command_json: string): string;
  apply_tool?(tool: string, args_json: string): string;
  // Pre-`apply` per-method calls, used only by the fallback below.
  add_feature(input_json: string): CommandResult;
  delete_feature_by_id(stable_id: string): CommandResult;
  set_translation(id: string, x: number, y: number, z: number): CommandResult;
  set_rotation(id: string, x: number, y: number, z: number): CommandResult;
  set_scale(id: string, x: number, y: number, z: number): CommandResult;
  set_material(id: string, material: string): CommandResult;
  set_visible(id: string, visible: boolean): CommandResult;
  rename_feature(id: string, name: string): CommandResult;
  set_joint_state(id: string, state: number): CommandResult;
  // These three predate consumedPartIds; the fallback fills it in empty.
  import_ir(ir_json: string): LegacyResult;
  set_param(feature_id_json: string, key: string, value_json: string): LegacyResult;
  create_feature(kind: string, params_json: string): LegacyResult & { createdFeatureId?: string };
  delete_feature(feature_id_json: string): LegacyResult;
  get_ordered_features_json(): string;
  compute_position_between(before_id_json: string, after_id_json: string): string;
  move_feature(feature_id_json: string, position_json: string): LegacyResult;
  undo(): LegacyResult;
  redo(): LegacyResult;
}

type LegacyResult = Omit<CommandResult, "consumedPartIds">;

/** Apply `command`; throws {@link DocCommandError} if the engine refuses it. */
export function applyCommand(
  engine: CommandEngine,
  command: DocCommand,
): CommandResult {
  if (typeof engine.apply !== "function") return applyLegacy(engine, command);
  const envelope = JSON.parse(engine.apply(JSON.stringify(command))) as Envelope;
  if (!envelope.ok) throw new DocCommandError(command, envelope.error);
  for (const w of envelope.result.warnings ?? []) console.warn(`[vcad] ${w}`);
  return envelope.result;
}

/**
 * Lower an AI tool call to a `DocCommand` in Rust and apply it. Returns
 * `null` when the kernel predates `apply_tool`; otherwise the raw envelope,
 * whose `error.kind === "unsupported"` means "the host handles this tool".
 */
export function applyToolCall(
  engine: CommandEngine,
  tool: string,
  args: unknown,
): Envelope | null {
  if (typeof engine.apply_tool !== "function") return null;
  const envelope = JSON.parse(engine.apply_tool(tool, JSON.stringify(args))) as Envelope;
  if (envelope.ok) {
    for (const w of envelope.result.warnings ?? []) console.warn(`[vcad] ${w}`);
  }
  return envelope;
}

/**
 * Kernel WASM built before `WasmDocumentEngine.apply` existed. The checked-in
 * artifacts are refreshed on main after merge (`wasm-refresh.yml`), so this
 * only bridges that window. Delete once the refreshed artifacts are on main.
 */
function applyLegacy(engine: CommandEngine, c: DocCommand): CommandResult {
  switch (c.type) {
    case "add_feature": {
      const r = engine.add_feature(JSON.stringify(c.feature));
      if (c.name && r.createdFeatureId) {
        return { ...engine.rename_feature(r.createdFeatureId, c.name), createdFeatureId: r.createdFeatureId };
      }
      return r;
    }
    case "delete_feature":
      return engine.delete_feature_by_id(c.id);
    case "set_translation":
      return engine.set_translation(c.id, ...c.offset);
    case "set_rotation":
      return engine.set_rotation(c.id, ...c.angles);
    case "set_scale":
      return engine.set_scale(c.id, ...c.factor);
    case "set_material":
      return engine.set_material(c.id, c.material);
    case "set_visible":
      return engine.set_visible(c.id, c.visible);
    case "rename":
      return engine.rename_feature(c.id, c.name);
    case "set_joint_state":
      return engine.set_joint_state(c.id, c.state);
    case "import_ir":
      return { consumedPartIds: [], ...engine.import_ir(JSON.stringify(c.document)) };
    case "set_param":
      return { consumedPartIds: [], ...engine.set_param(c.id, c.key, JSON.stringify(c.value)) };
    case "create_feature":
      return { consumedPartIds: [], ...engine.create_feature(c.kind, JSON.stringify(c.params ?? {})) };
    case "reorder": {
      const others = (JSON.parse(engine.get_ordered_features_json()) as { id: string }[]).filter(
        (f) => f.id !== c.id,
      );
      const before = c.index > 0 ? (others[c.index - 1]?.id ?? "") : "";
      const after = others[c.index]?.id ?? "";
      const position = engine.compute_position_between(before, after);
      return { consumedPartIds: [], ...engine.move_feature(c.id, position) };
    }
    case "undo":
      return { consumedPartIds: [], ...engine.undo() };
    case "redo":
      return { consumedPartIds: [], ...engine.redo() };
    default:
      throw new Error(`[vcad] ${c.type} needs a kernel WASM with WasmDocumentEngine.apply`);
  }
}
