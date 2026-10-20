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
  apply(command_json: string): string;
  apply_tool(tool: string, args_json: string): string;
}

/** Apply `command`; throws {@link DocCommandError} if the engine refuses it. */
export function applyCommand(
  engine: CommandEngine,
  command: DocCommand,
): CommandResult {
  const envelope = JSON.parse(engine.apply(JSON.stringify(command))) as Envelope;
  if (!envelope.ok) throw new DocCommandError(command, envelope.error);
  for (const w of envelope.result.warnings ?? []) console.warn(`[vcad] ${w}`);
  return envelope.result;
}

/**
 * Lower an AI tool call to a `DocCommand` in Rust and apply it. Returns the
 * raw envelope; `error.kind === "unsupported"` means "the host handles this
 * tool".
 */
export function applyToolCall(
  engine: CommandEngine,
  tool: string,
  args: unknown,
): Envelope {
  const envelope = JSON.parse(engine.apply_tool(tool, JSON.stringify(args))) as Envelope;
  if (envelope.ok) {
    for (const w of envelope.result.warnings ?? []) console.warn(`[vcad] ${w}`);
  }
  return envelope;
}
