/**
 * AI document-metadata tool executors — `get_document_name`, `set_document_name`.
 *
 * Lets the chat agent inspect and rename the currently open document. The
 * name is stored in the document store (not the IR) and reactive consumers
 * (DocTitle, window title, cloud sync) pick up changes automatically.
 */

import { useDocumentStore } from "@vcad/core";
import type { AnthropicTool, ExecutionResult } from "@vcad/core";
import type { ToolCall } from "@/lib/chat-api";

const DEFAULT_NAME = "Untitled";
const MAX_NAME_LENGTH = 120;

export const GET_DOCUMENT_NAME_TOOL: AnthropicTool = {
  name: "get_document_name",
  description:
    "Read the current document's name. Returns the name and a flag indicating whether it is still the default placeholder (\"Untitled\" or empty). Call this before deciding whether to rename.",
  input_schema: {
    type: "object",
    properties: {},
  },
};

export const SET_DOCUMENT_NAME_TOOL: AnthropicTool = {
  name: "set_document_name",
  description:
    "Rename the current document. Use a short, descriptive name based on what the user is modeling (e.g. \"Bike Frame\", \"Gearbox Housing\", \"Desk Lamp\"). Prefer 1–4 words, Title Case, no file extension, no trailing punctuation. Only rename when the current name is the default placeholder or clearly unrelated — do not overwrite a name the user has already chosen.",
  input_schema: {
    type: "object",
    properties: {
      name: {
        type: "string",
        description:
          "The new document name. 1–120 chars, non-empty after trim. Title Case, no extension.",
      },
    },
    required: ["name"],
  },
};

export const AI_DOCUMENT_TOOL_NAMES = new Set([
  "get_document_name",
  "set_document_name",
]);

function isDefaultName(name: string): boolean {
  const trimmed = name.trim();
  return trimmed === "" || trimmed === DEFAULT_NAME || /^Untitled(\s+\d+)?$/i.test(trimmed);
}

function exec(tool: ToolCall): ExecutionResult {
  switch (tool.name) {
    case "get_document_name": {
      const name = useDocumentStore.getState().documentName ?? "";
      const isDefault = isDefaultName(name);
      const payload = {
        name: name || DEFAULT_NAME,
        is_default: isDefault,
        hint: isDefault
          ? "This document has no user-chosen name yet. If you can infer what the user is modeling, call set_document_name with a short descriptive title."
          : "The user has chosen this name — do not rename unless they ask.",
      };
      return {
        status: "success",
        result: JSON.stringify(payload),
        display: {
          summary: [
            { type: "text", text: `Document name: ${payload.name}` },
          ],
        },
      };
    }

    case "set_document_name": {
      const raw = tool.args.name;
      if (typeof raw !== "string") {
        return { status: "error", result: "set_document_name requires a string `name`." };
      }
      const next = raw.trim();
      if (!next) {
        return { status: "error", result: "Document name cannot be empty." };
      }
      if (next.length > MAX_NAME_LENGTH) {
        return {
          status: "error",
          result: `Document name too long (${next.length} chars, max ${MAX_NAME_LENGTH}).`,
        };
      }
      const prev = useDocumentStore.getState().documentName ?? "";
      if (next === prev) {
        return {
          status: "success",
          result: `Document already named "${next}"; no change.`,
          display: {
            summary: [{ type: "text", text: `Already "${next}"` }],
          },
        };
      }
      useDocumentStore.getState().setDocumentName(next);
      return {
        status: "success",
        result: `Renamed document from "${prev || DEFAULT_NAME}" to "${next}".`,
        display: {
          summary: [
            { type: "text", text: `Renamed to "${next}"` },
          ],
        },
      };
    }

    default:
      return { status: "error", result: `Unknown document tool "${tool.name}".` };
  }
}

/** Execute a document-metadata tool call, measuring duration. */
export function executeAiDocumentTool(tool: ToolCall): ExecutionResult {
  const t0 = performance.now();
  const result = exec(tool);
  result.duration = performance.now() - t0;
  return result;
}

/**
 * System-prompt appendix teaching the model when to use the document-metadata
 * tools. Appended alongside the screenshot / camera appendices in the chat
 * handler so the core package stays agnostic of app-only capabilities.
 */
export const AI_DOCUMENT_SYSTEM_PROMPT_APPENDIX = `

## Document name

You can read and set the document's display name.

- get_document_name() — returns the current name plus an \`is_default\` flag
  that's true when the name is still the placeholder ("Untitled" / empty).
- set_document_name(name) — rename the document. Keep it short and
  descriptive (1–4 words, Title Case, no extension) — e.g. "Bike Frame",
  "Gearbox Housing", "Desk Lamp", "Solar Panel Bracket".

When to rename: if the document is still untitled and the user's request
makes the subject clear (e.g. "model a desk lamp", "build me a bike frame"),
call set_document_name once early in the turn with a name that reflects what
you're building. Don't ask first — a good default is better than Untitled,
and the user can rename from the title bar at any time.

When NOT to rename: the user has already chosen a name (is_default is false),
the request is an ambiguous tweak that doesn't reveal a subject, or you're
making a trivial modification. Never rename on every turn — one name per
document is plenty.`;
