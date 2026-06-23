import { useEffect, useState } from "react";
import { getSupabase } from "@vcad/auth";
import { continueSessionRowKey } from "@/lib/continue-links";

export interface ClaudeLiveStatus {
  /** True once the model's first edit to the continued session lands. */
  live: boolean;
  /** Number of edits observed since the handoff. */
  edits: number;
  /** Wall-clock ms of the most recent edit, or null. */
  lastAt: number | null;
}

const IDLE: ClaudeLiveStatus = { live: false, edits: 0, lastAt: null };

/**
 * Reflect a signed-in "Continue in Claude" handoff back into the vcad.io tab.
 *
 * The model's continued session persists to the user's own `documents` row
 * (`local_id = mcp:cont_<token>`); subscribing to Realtime changes on that row
 * lets us show the part coming alive in Claude — each edit bumps the counter and
 * pulses the badge. Owner RLS means we only ever see our own row.
 *
 * Pass `null` to disable (the inline/anon handoff has no durable row, and we
 * only subscribe once the user actually launches a host). Requires `documents`
 * in the supabase_realtime publication (migration 031).
 */
export function useClaudeLiveStatus(token: string | null): ClaudeLiveStatus {
  const [status, setStatus] = useState<ClaudeLiveStatus>(IDLE);

  useEffect(() => {
    setStatus(IDLE);
    if (!token) return;
    const supabase = getSupabase();
    if (!supabase) return;

    const channel = supabase.channel(`continue-${token}`);
    channel.on(
      "postgres_changes" as never,
      {
        event: "*",
        schema: "public",
        table: "documents",
        filter: `local_id=eq.${continueSessionRowKey(token)}`,
      },
      () => {
        setStatus((s) => ({
          live: true,
          edits: s.edits + 1,
          lastAt: Date.now(),
        }));
      },
    );
    channel.subscribe();

    return () => {
      void supabase.removeChannel(channel);
    };
  }, [token]);

  return status;
}
