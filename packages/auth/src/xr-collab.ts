/**
 * Realtime transport for XR presence (head + hand poses).
 *
 * A *sibling* channel to {@link joinCollabChannel} — on the same Supabase
 * realtime backend, keyed by document ID, but on its own topic so the
 * higher broadcast rate of pose updates (~10 Hz) doesn't compete with
 * document op delivery.
 *
 * Poses are exchanged in the kernel scene's local frame (NOT each user's
 * physical room frame), so two users with different desks still see each
 * other's avatars at the same point on the same model.
 */

import { requireSupabase, isAuthEnabled } from "./client";
import { useAuthStore } from "./stores/auth-store";
import type { RealtimeChannel } from "@supabase/supabase-js";

export interface XRPose {
  /** Headset position in scene-local coords (kernel mm). */
  head: [number, number, number];
  /** Headset orientation as a unit quaternion (x, y, z, w). */
  headRot: [number, number, number, number];
  /** Optional left wrist position. */
  leftHand?: [number, number, number];
  /** Optional right wrist position. */
  rightHand?: [number, number, number];
}

export interface XRPresenceUpdate {
  userId: string;
  /** Display name; optional, falls back to user id. */
  name?: string;
  /** Display color; optional, falls back to a default. */
  color?: string;
  pose: XRPose;
  /** Sender clock for staleness detection on the receiver side. */
  ts: number;
}

export interface XRCollabChannel {
  /** Broadcast the local user's XR pose. Drop if not yet subscribed. */
  broadcast: (update: Omit<XRPresenceUpdate, "userId" | "ts">) => void;
  /** Stop sending and close the channel. */
  leave: () => void;
}

export type XRPresenceListener = (update: XRPresenceUpdate) => void;
/** Emitted when a remote peer leaves so the receiver can drop their avatar. */
export type XRPresenceLeaveListener = (userId: string) => void;

/**
 * Join the XR presence channel for a document.
 *
 * @returns null when auth is disabled or the user isn't signed in — the
 * caller should treat presence as unavailable in that case rather than
 * crashing the XR session.
 */
export function joinXRCollabChannel(
  documentId: string,
  onUpdate: XRPresenceListener,
  onLeave?: XRPresenceLeaveListener,
): XRCollabChannel | null {
  if (!isAuthEnabled()) return null;
  const { user } = useAuthStore.getState();
  if (!user) return null;

  const supabase = requireSupabase();
  const channelName = `doc:${documentId}:xr`;

  let subscribed = false;
  let channel: RealtimeChannel | null = supabase.channel(channelName, {
    config: { broadcast: { self: false } },
  });

  channel.on("broadcast", { event: "pose" }, (payload) => {
    const update = payload.payload as XRPresenceUpdate | undefined;
    if (!update?.pose || !update.userId) return;
    onUpdate(update);
  });

  channel.on("broadcast", { event: "leave" }, (payload) => {
    const userId = (payload.payload as { userId?: string } | undefined)?.userId;
    if (userId) onLeave?.(userId);
  });

  channel.subscribe((status) => {
    if (status === "SUBSCRIBED") subscribed = true;
  });

  return {
    broadcast: (update) => {
      if (!channel || !subscribed) return;
      const payload: XRPresenceUpdate = {
        ...update,
        userId: user.id,
        ts: Date.now(),
      };
      channel.send({
        type: "broadcast",
        event: "pose",
        payload,
      });
    },
    leave: () => {
      if (!channel) return;
      // Best-effort goodbye so peers can drop the avatar immediately.
      if (subscribed) {
        channel.send({
          type: "broadcast",
          event: "leave",
          payload: { userId: user.id },
        });
      }
      subscribed = false;
      supabase.removeChannel(channel);
      channel = null;
    },
  };
}
