/**
 * "Continue in Claude" host links.
 *
 * Pure builders for the deep links / commands that hand the user's current part
 * off to an AI host. The part's geometry never rides the URL — only an opaque
 * vcad.io share token (a UUID) does, which the model resolves server-side with
 * the `continue_document` MCP tool.
 *
 * Host reality (verified June 2026 — keep this honest, it drives the UX):
 *  - Claude Desktop: `claude://claude.ai/new?q=…` prefills a new chat (~14k cap).
 *  - claude.ai web:  NO prompt-prefill URL (the `?q=` param was removed Oct 2025
 *                    over a prompt-injection exploit) → open + copy-to-clipboard.
 *  - ChatGPT:        `https://chatgpt.com/?q=…` prefills (and submits) a prompt.
 *  - Cursor / VS Code: one-click MCP *install* deep links (no prompt) — the seed
 *                    is copied for the user to paste once the connector is added.
 *  - Claude Code:    a copyable `claude mcp add …` one-liner + the seed.
 *
 * No host installs the connector AND seeds a prompt in a single link, so the
 * dialog pairs an action (open/install) with a copyable seed where needed.
 */

/** Canonical hosted MCP endpoint. Overridable for staging/self-host. */
export const DEFAULT_MCP_URL = "https://mcp.vcad.io/mcp";

export type ContinueHost =
  | "claude-desktop"
  | "claude-web"
  | "chatgpt"
  | "cursor"
  | "vscode"
  | "claude-code";

export interface ContinueTarget {
  host: ContinueHost;
  /** Short label for the menu/button. */
  label: string;
  /** One-line hint shown under the option. */
  hint: string;
  /** A deep link / URL to open (custom scheme or https). Absent for copy-only. */
  url?: string;
  /** Text to put on the clipboard (seed prompt, or a CLI command). */
  clipboard?: string;
  /** When true, the URL should be opened AND the clipboard copied (the host
   *  can't prefill, so the user pastes the seed after it opens). */
  copyWithOpen?: boolean;
}

export interface ContinueLinksInput {
  /** The vcad.io share token (UUID) the model resolves (signed-in handoff). */
  token?: string;
  /** A compressed inline IR blob (from {@link encodeDocForSeed}) for an
   *  accountless handoff. Supply this OR `token`. */
  inlineDoc?: string;
  /** Document name, woven into the seed for a friendlier first turn. */
  docName?: string;
  /** MCP endpoint to install; defaults to {@link DEFAULT_MCP_URL}. */
  mcpUrl?: string;
}

/** Max compressed-IR blob we'll inline into a seed. Keeps a prefilled host URL
 *  under the tightest cap (Claude Desktop ~14k, ChatGPT unknown). Larger parts
 *  fall back to a sign-in handoff. */
export const MAX_INLINE_BLOB = 8000;

/**
 * The Supabase `documents.local_id` a signed-in token handoff persists under —
 * the `mcp:` prefix plus the deterministic `cont_<token>` session id the server
 * keys it by (see continue_document). The vcad.io tab watches this row over
 * Realtime to reflect the model's edits live; must stay in lockstep with the
 * server's keying.
 */
export function continueSessionRowKey(token: string): string {
  return `mcp:cont_${token}`;
}

/** Base64-encode a UTF-8 string. `btoa` is global in the browser and in the
 *  Node ≥16 test runner; we route bytes through it via a binary string so
 *  non-ASCII doc names survive. */
function toBase64(s: string): string {
  const bytes = new TextEncoder().encode(s);
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

/**
 * The natural-language seed handed to a prompt host. Short by design — it
 * carries the token, not the geometry, and tells the model exactly which tool
 * to call so the user's first turn operates on their real part.
 */
export function buildSeedPrompt(token: string, docName?: string): string {
  const subject = docName ? `my vcad part "${docName}"` : "my vcad part";
  return (
    `Continue ${subject}. Call the vcad \`continue_document\` tool with token ` +
    `"${token}" to load it, then render it and tell me what you see. If the ` +
    `vcad tool isn't available, walk me through adding the vcad connector.`
  );
}

/**
 * The inline-doc seed for an accountless handoff — the same shape as
 * {@link buildSeedPrompt} but carrying the compressed geometry instead of a
 * token (the model passes it to `continue_document` as `doc`).
 */
function buildInlineSeed(blob: string, docName?: string): string {
  const subject = docName ? `my vcad part "${docName}"` : "my vcad part";
  return (
    `Continue ${subject}. Call the vcad \`continue_document\` tool with this ` +
    `inline doc to load it, then render it and tell me what you see. If the ` +
    `vcad tool isn't available, walk me through adding the vcad connector.\n\n` +
    `doc="${blob}"`
  );
}

/** Compress an IR document for an accountless inline handoff (gzip + base64url,
 *  matching what `continue_document` decodes). Returns null when the result is
 *  too large to ride a host URL safely — the caller falls back to sign-in. */
export async function encodeDocForSeed(doc: unknown): Promise<string | null> {
  const cs = new CompressionStream("gzip");
  const stream = new Blob([JSON.stringify(doc)]).stream().pipeThrough(cs);
  const buf = new Uint8Array(await new Response(stream).arrayBuffer());
  let bin = "";
  for (const b of buf) bin += String.fromCharCode(b);
  const blob = btoa(bin)
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
  return blob.length > MAX_INLINE_BLOB ? null : blob;
}

/** The MCP server config object a host's install link encodes. */
function serverConfig(mcpUrl: string): { type: "http"; url: string } {
  return { type: "http", url: mcpUrl };
}

/**
 * Build every host target for a handoff. The dialog renders these; the caller
 * decides which to surface (e.g. remembers the user's last pick).
 */
export function buildContinueTargets(
  input: ContinueLinksInput,
): ContinueTarget[] {
  const mcpUrl = input.mcpUrl ?? DEFAULT_MCP_URL;
  const seed = input.inlineDoc
    ? buildInlineSeed(input.inlineDoc, input.docName)
    : buildSeedPrompt(input.token ?? "", input.docName);
  const q = encodeURIComponent(seed);

  // Cursor: base64 server config + separate name param.
  const cursorConfig = toBase64(JSON.stringify(serverConfig(mcpUrl)));
  const cursorUrl =
    `cursor://anysphere.cursor-deeplink/mcp/install?name=vcad&config=${cursorConfig}`;

  // VS Code: URL-encoded JSON with the name inline (NOT base64 — different
  // encoder from Cursor; a shared encoder would silently corrupt one of them).
  const vscodeConfig = encodeURIComponent(
    JSON.stringify({ name: "vcad", ...serverConfig(mcpUrl) }),
  );
  const vscodeUrl = `vscode:mcp/install?${vscodeConfig}`;

  return [
    {
      host: "claude-desktop",
      label: "Claude Desktop",
      hint: "Opens a new chat with your part loaded.",
      url: `claude://claude.ai/new?q=${q}`,
    },
    {
      host: "claude-web",
      label: "Claude (web)",
      hint: "Opens claude.ai and copies the starter prompt to paste.",
      url: "https://claude.ai/new",
      clipboard: seed,
      copyWithOpen: true,
    },
    {
      host: "chatgpt",
      label: "ChatGPT",
      hint: "Opens ChatGPT with the starter prompt.",
      url: `https://chatgpt.com/?q=${q}`,
    },
    {
      host: "cursor",
      label: "Cursor",
      hint: "Installs the vcad connector; paste the copied prompt to start.",
      url: cursorUrl,
      clipboard: seed,
      copyWithOpen: true,
    },
    {
      host: "vscode",
      label: "VS Code",
      hint: "Installs the vcad connector; paste the copied prompt to start.",
      url: vscodeUrl,
      clipboard: seed,
      copyWithOpen: true,
    },
    {
      host: "claude-code",
      label: "Claude Code",
      hint: "Copies a one-line install command + starter prompt.",
      clipboard:
        `claude mcp add --transport http vcad ${mcpUrl}\n` +
        `# then:\nclaude "${seed}"`,
    },
  ];
}
