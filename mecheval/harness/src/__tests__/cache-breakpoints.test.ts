/**
 * Prompt-caching breakpoint placement for the claude-mcp solver.
 *
 * The API allows max 4 cache_control breakpoints per request: one on the
 * last tool, one on the system block (both static per run), and two
 * sliding marks on the most recent user messages. These tests pin the
 * sliding behavior: stale marks are stripped, at most two user messages
 * are marked, and only the LAST content block of each carries the mark.
 */

import { describe, it, expect } from "vitest";
import { applyConversationCacheBreakpoints } from "../solvers/claude-mcp.js";

type Block = Record<string, unknown>;
type Message = { role: "user" | "assistant"; content: unknown };

function marks(messages: Message[]): Array<[number, number]> {
  const out: Array<[number, number]> = [];
  messages.forEach((m, mi) => {
    if (!Array.isArray(m.content)) return;
    (m.content as Block[]).forEach((b, bi) => {
      if (b && typeof b === "object" && "cache_control" in b) out.push([mi, bi]);
    });
  });
  return out;
}

function userMsg(blocks: number): Message {
  return {
    role: "user",
    content: Array.from({ length: blocks }, (_, n) => ({
      type: "tool_result",
      tool_use_id: `tu_${n}`,
      content: [{ type: "text", text: "ok" }],
    })),
  };
}

function assistantMsg(): Message {
  return {
    role: "assistant",
    content: [{ type: "text", text: "thinking..." }],
  };
}

describe("applyConversationCacheBreakpoints", () => {
  it("marks the last block of the single user message", () => {
    const messages: Message[] = [
      { role: "user", content: [{ type: "text", text: "kickoff" }] },
    ];
    applyConversationCacheBreakpoints(messages);
    expect(marks(messages)).toEqual([[0, 0]]);
  });

  it("marks at most the two most recent user messages, last block only", () => {
    const messages: Message[] = [
      userMsg(1),
      assistantMsg(),
      userMsg(3),
      assistantMsg(),
      userMsg(2),
    ];
    applyConversationCacheBreakpoints(messages);
    // Last block of message 4 (block 1) and message 2 (block 2); message 0 unmarked.
    expect(marks(messages)).toEqual([
      [2, 2],
      [4, 1],
    ]);
  });

  it("strips stale marks on re-application (never accumulates past the API max)", () => {
    const messages: Message[] = [userMsg(2), assistantMsg(), userMsg(1)];
    applyConversationCacheBreakpoints(messages);
    messages.push(assistantMsg(), userMsg(2));
    applyConversationCacheBreakpoints(messages);
    expect(marks(messages)).toEqual([
      [2, 0],
      [4, 1],
    ]);
    // Total message-level marks stays ≤ 2 (tools + system take the other 2 slots).
    expect(marks(messages).length).toBeLessThanOrEqual(2);
  });

  it("skips string-content and assistant messages without throwing", () => {
    const messages: Message[] = [
      { role: "user", content: "bare string" },
      assistantMsg(),
      { role: "user", content: [] },
    ];
    expect(() => applyConversationCacheBreakpoints(messages)).not.toThrow();
    expect(marks(messages)).toEqual([]);
  });
});
