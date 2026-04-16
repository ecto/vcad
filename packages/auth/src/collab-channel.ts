/**
 * Supabase Realtime collab transport for vcad multiplayer.
 *
 * Opens a broadcast channel per document. After each local mutation, extracts
 * new CRDT ops via get_ops_since() and publishes them. On receiving a remote
 * broadcast, calls merge_remote() on the engine and re-renders.
 *
 * The sync protocol is delta-based: each client tracks the last clock it sent
 * and only broadcasts new ops. On join, a full state exchange happens via the
 * "sync" event.
 */

import { requireSupabase, isAuthEnabled } from "./client";
import { useAuthStore } from "./stores/auth-store";
import type { RealtimeChannel } from "@supabase/supabase-js";

export interface CollabCallbacks {
  /** Get the local CRDT sync clock as JSON. */
  getSyncClock: () => string;
  /** Get ops since a remote clock (JSON array of Op). */
  getOpsSince: (remoteClockJson: string) => string;
  /** Merge remote ops into the local engine. Returns true if state changed. */
  mergeRemoteOps: (opsJson: string) => boolean;
}

/**
 * Flag that's true while we're applying remote ops. The broadcast subscriber
 * checks this to avoid re-broadcasting ops we just received.
 */
let _applyingRemote = false;
export function isApplyingRemoteOps(): boolean {
  return _applyingRemote;
}

export interface CollabChannel {
  /** Broadcast local ops to the channel. Call after every local mutation. */
  broadcastOps: () => void;
  /** Leave the channel and clean up. */
  leave: () => void;
}

/**
 * Join a collab channel for a document. Returns handles to broadcast and leave.
 *
 * @param documentId The cloud document ID (used as the channel name).
 * @param callbacks Functions to read/write the CRDT engine state.
 */
export function joinCollabChannel(
  documentId: string,
  callbacks: CollabCallbacks,
): CollabChannel | null {
  if (!isAuthEnabled()) return null;
  const { user } = useAuthStore.getState();
  if (!user) return null;

  const supabase = requireSupabase();
  const channelName = `doc:${documentId}:collab`;

  let lastSentClock = callbacks.getSyncClock();
  let channel: RealtimeChannel | null = null;

  channel = supabase.channel(channelName, {
    config: {
      broadcast: { self: false }, // don't receive our own broadcasts
    },
  });

  // ─── Receive remote ops ──────────────────────────────────────────────
  channel.on("broadcast", { event: "ops" }, (payload) => {
    const opsJson = payload.payload?.ops as string | undefined;
    if (!opsJson) return;
    const parsed = JSON.parse(opsJson) as unknown[];
    console.log("[collab] received", parsed.length, "remote ops");
    _applyingRemote = true;
    try {
      callbacks.mergeRemoteOps(opsJson);
    } finally {
      _applyingRemote = false;
    }
  });

  // ─── Sync request: a new peer joined and wants our full state ────────
  channel.on("broadcast", { event: "sync-request" }, (payload) => {
    const remoteClock = (payload.payload?.clock ?? "{}") as string;
    const ops = callbacks.getOpsSince(remoteClock);
    const parsed = JSON.parse(ops) as unknown[];
    if (parsed.length > 0) {
      channel?.send({
        type: "broadcast",
        event: "ops",
        payload: { ops },
      });
    }
  });

  // ─── Subscribe and request sync on join ──────────────────────────────
  channel.subscribe((status) => {
    if (status === "SUBSCRIBED") {
      // Ask existing peers for any ops we're missing.
      channel?.send({
        type: "broadcast",
        event: "sync-request",
        payload: { clock: callbacks.getSyncClock() },
      });
    }
  });

  // ─── Broadcast local ops ─────────────────────────────────────────────
  const broadcastOps = () => {
    if (!channel) return;
    const ops = callbacks.getOpsSince(lastSentClock);
    const parsed = JSON.parse(ops) as unknown[];
    if (parsed.length === 0) return;

    console.log("[collab] broadcasting", parsed.length, "ops");
    channel.send({
      type: "broadcast",
      event: "ops",
      payload: { ops },
    });
    lastSentClock = callbacks.getSyncClock();
  };

  const leave = () => {
    if (channel) {
      supabase.removeChannel(channel);
      channel = null;
    }
  };

  return { broadcastOps, leave };
}
