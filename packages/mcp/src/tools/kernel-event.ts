/**
 * Build the payload for a `kernel` session_events row: the tool name, its args
 * (minus document_id), and the compact `changed` parts diff the dispatch path
 * already merged into the result. Capped so a fat call can't bloat the spine —
 * mirrors the >8KB result-slimming discipline; tool + changed (the cheap,
 * high-value parts) are always kept.
 *
 * Shared by the central persist site in `server.ts` and the cross-session
 * writer `solid_from_board` (which persists its target itself).
 */
export function buildKernelEventPayload(
  name: string,
  args: Record<string, unknown>,
  result: { content: Array<{ type: string; text: string }> },
): Record<string, unknown> {
  const { document_id: _docId, ...rest } = args;
  void _docId;
  let changed: unknown;
  for (const block of result.content) {
    if (block.type !== "text") continue;
    try {
      const parsed = JSON.parse(block.text) as { changed?: unknown };
      if (parsed && parsed.changed !== undefined) {
        changed = parsed.changed;
        break;
      }
    } catch {
      // not JSON — skip
    }
  }
  const payload: Record<string, unknown> = { tool: name, args: rest };
  if (changed !== undefined) payload.changed = changed;
  try {
    if (JSON.stringify(payload).length > 8192) {
      payload.args = { _omitted: true };
    }
  } catch {
    payload.args = { _omitted: true };
  }
  return payload;
}
