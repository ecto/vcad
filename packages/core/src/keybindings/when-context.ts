/**
 * `WhenContext` flag bits — mirrors `vcad_app::context::WhenContext`.
 *
 * Hosts build a context value each dispatch by reading zustand store state
 * (selection size, sketch active, etc.) and pack it into a single `u32` that
 * crosses the wasm boundary. The Rust side decodes it and evaluates each
 * command's `when` clause against it.
 *
 * Bit positions must stay in exact sync with the Rust constants — a change
 * on either side requires a matching change on the other.
 */

export const WHEN = {
  INPUT_FOCUSED: 1 << 0,
  MENU_OPEN: 1 << 1,
  COMMAND_MODE: 1 << 2,
  HAS_SELECTION: 1 << 3,
  TWO_SELECTED: 1 << 4,
  ONE_PART: 1 << 5,
  HAS_PARTS: 1 << 6,
  CAN_UNDO: 1 << 7,
  CAN_REDO: 1 << 8,
  SKETCH_HAS_POINTS: 1 << 9,
  PHYSICS_RUNNING: 1 << 10,
  ELECTRONICS_ACTIVE: 1 << 11,
} as const;

export type WhenBits = number;

/** Individual inputs the dispatcher reads to build the flag set. Each
 * caller-side layer (useKeybindingDispatcher) gathers these from its stores
 * just before dispatch, then calls `buildWhenContext`. */
export interface WhenInputs {
  inputFocused: boolean;
  menuOpen: boolean;
  commandMode: boolean;
  selectionSize: number;
  partCount: number;
  canUndo: boolean;
  canRedo: boolean;
  sketchHasPoints: boolean;
  physicsRunning: boolean;
  electronicsActive: boolean;
}

/** Pack a `WhenInputs` into the bit layout the wasm side expects. */
export function buildWhenContext(inputs: WhenInputs): WhenBits {
  let bits = 0;
  if (inputs.inputFocused) bits |= WHEN.INPUT_FOCUSED;
  if (inputs.menuOpen) bits |= WHEN.MENU_OPEN;
  if (inputs.commandMode) bits |= WHEN.COMMAND_MODE;
  if (inputs.selectionSize > 0) bits |= WHEN.HAS_SELECTION;
  if (inputs.selectionSize === 2) bits |= WHEN.TWO_SELECTED;
  if (inputs.selectionSize === 1) bits |= WHEN.ONE_PART;
  if (inputs.partCount > 0) bits |= WHEN.HAS_PARTS;
  if (inputs.canUndo) bits |= WHEN.CAN_UNDO;
  if (inputs.canRedo) bits |= WHEN.CAN_REDO;
  if (inputs.sketchHasPoints) bits |= WHEN.SKETCH_HAS_POINTS;
  if (inputs.physicsRunning) bits |= WHEN.PHYSICS_RUNNING;
  if (inputs.electronicsActive) bits |= WHEN.ELECTRONICS_ACTIVE;
  return bits;
}

/** Detect whether the user is currently typing into a text input — the
 * single most important when-context flag because almost every binding
 * should be suppressed inside inputs. */
export function isInputFocused(target: EventTarget | null): boolean {
  if (!target || !(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (target.isContentEditable) return true;
  return false;
}
