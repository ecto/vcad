/**
 * Platform-agnostic chord type — mirrors `vcad_app::Chord` on the Rust side.
 *
 * A `Chord` is the abstract representation of a key press: one "primary"
 * modifier (Cmd on macOS, Ctrl elsewhere), optional Shift/Alt, and a key.
 * The TS side normalizes browser `KeyboardEvent`s into this shape and hands
 * them to the wasm registry via JSON — serde on the Rust side parses it with
 * zero ambiguity because the field names and enum tags match exactly.
 */

/** A single key — `Char` for printable characters, enum variants for named
 * keys and function keys. Must stay in sync with `vcad_app::keybinding::Key`. */
export type Key =
  | { type: "Char"; value: string }
  | { type: "F"; value: number }
  | { type: "Enter" }
  | { type: "Esc" }
  | { type: "Tab" }
  | { type: "Backspace" }
  | { type: "Delete" }
  | { type: "Space" }
  | { type: "ArrowLeft" }
  | { type: "ArrowRight" }
  | { type: "ArrowUp" }
  | { type: "ArrowDown" }
  | { type: "Home" }
  | { type: "End" }
  | { type: "PageUp" }
  | { type: "PageDown" }
  | { type: "Backtick" };

/** Mirror of `vcad_app::Chord`. Field names match Rust exactly so serde can
 * deserialize the JSON produced here without any remapping. */
export interface Chord {
  primary: boolean;
  shift: boolean;
  alt: boolean;
  key: Key;
}

const NAMED_KEY: Record<string, Key | undefined> = {
  Enter: { type: "Enter" },
  Escape: { type: "Esc" },
  Tab: { type: "Tab" },
  Backspace: { type: "Backspace" },
  Delete: { type: "Delete" },
  " ": { type: "Space" },
  Spacebar: { type: "Space" },
  ArrowLeft: { type: "ArrowLeft" },
  ArrowRight: { type: "ArrowRight" },
  ArrowUp: { type: "ArrowUp" },
  ArrowDown: { type: "ArrowDown" },
  Home: { type: "Home" },
  End: { type: "End" },
  PageUp: { type: "PageUp" },
  PageDown: { type: "PageDown" },
  "`": { type: "Backtick" },
};

/** Convert a browser `KeyboardEvent` to a `Chord`, or `null` if the event
 * carries a key we don't handle (e.g. raw "Shift" with no other key).
 *
 * The "primary" modifier maps to `metaKey || ctrlKey` — on macOS users press
 * Cmd, on PC they press Ctrl, but bindings are declared once. */
export function chordFromEvent(e: KeyboardEvent): Chord | null {
  // Ignore bare modifier keys — we only dispatch when a real key is pressed.
  if (
    e.key === "Shift" ||
    e.key === "Control" ||
    e.key === "Alt" ||
    e.key === "Meta" ||
    e.key === "AltGraph" ||
    e.key === "CapsLock"
  ) {
    return null;
  }

  const key = normalizeKey(e);
  if (!key) return null;

  return {
    primary: e.metaKey || e.ctrlKey,
    shift: e.shiftKey,
    alt: e.altKey,
    key,
  };
}

function normalizeKey(e: KeyboardEvent): Key | null {
  // Function keys F1..F24
  const fMatch = /^F(\d{1,2})$/.exec(e.key);
  if (fMatch) {
    const n = Number(fMatch[1]);
    if (n >= 1 && n <= 24) return { type: "F", value: n };
  }

  // Named keys
  const named = NAMED_KEY[e.key];
  if (named) return named;

  // Single printable character — normalize to lowercase so "s" and "Shift+S"
  // both emit `Char("s")` (shift state lives in the modifier, not the case).
  if (e.key.length === 1) {
    return { type: "Char", value: e.key.toLowerCase() };
  }

  return null;
}

/** Pretty-format a chord for display in menus and tooltips. Uses glyphs on
 * macOS (`⌘⇧F`), written-out modifiers elsewhere (`Ctrl+Shift+F`). */
export function formatChord(chord: Chord, platform: "mac" | "pc"): string {
  return platform === "mac" ? formatMac(chord) : formatPc(chord);
}

function formatMac(chord: Chord): string {
  let out = "";
  if (chord.alt) out += "⌥";
  if (chord.shift) out += "⇧";
  if (chord.primary) out += "⌘";
  out += keyGlyph(chord.key);
  return out;
}

function formatPc(chord: Chord): string {
  const parts: string[] = [];
  if (chord.primary) parts.push("Ctrl");
  if (chord.shift) parts.push("Shift");
  if (chord.alt) parts.push("Alt");
  parts.push(keyLabel(chord.key));
  return parts.join("+");
}

function keyGlyph(key: Key): string {
  switch (key.type) {
    case "Char": return key.value.toUpperCase();
    case "F": return `F${key.value}`;
    case "Enter": return "↵";
    case "Esc": return "Esc";
    case "Tab": return "⇥";
    case "Backspace": return "⌫";
    case "Delete": return "⌦";
    case "Space": return "Space";
    case "ArrowLeft": return "←";
    case "ArrowRight": return "→";
    case "ArrowUp": return "↑";
    case "ArrowDown": return "↓";
    case "Home": return "Home";
    case "End": return "End";
    case "PageUp": return "PgUp";
    case "PageDown": return "PgDn";
    case "Backtick": return "`";
  }
}

function keyLabel(key: Key): string {
  switch (key.type) {
    case "Char": return key.value.toUpperCase();
    case "F": return `F${key.value}`;
    default: return key.type.replace(/^Arrow/, "");
  }
}

/** True when the user's platform is macOS — used for display formatting. */
export function isMac(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Mac|iPhone|iPad|iPod/i.test(navigator.platform || navigator.userAgent || "");
}
