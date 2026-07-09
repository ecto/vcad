import { useCallback, useEffect, useState } from "react";
import { getSupabase, useAuth, SignInButton } from "@vcad/auth";

/**
 * Spend-authorization approval page, rendered at /authorize/<id>.
 *
 * The MCP agent proposes a spend (`authorize_spend` → a spend_authorizations
 * row with status pending_human) and deep-links the human here. This page is
 * the human side of that handshake — the only place an authorization can be
 * approved or declined. RLS scopes the read to the signed-in user's own rows;
 * the approve/decline RPCs (migration 035) re-check ownership, status, and
 * expiry server-side.
 *
 * Mounted standalone from main.tsx (like CliAuth) — it never shares the App
 * render path.
 */

interface AuthorizePageProps {
  authorizationId: string;
}

interface AuthorizationRow {
  id: string;
  kind: "one_time" | "standing";
  max_amount_minor: number;
  daily_cap_minor: number | null;
  process_allowlist: string[] | null;
  fab_allowlist: string[] | null;
  status: "pending_human" | "authorized" | "consumed" | "revoked" | "expired";
  expires_at: string;
  created_at: string;
}

interface RpcResult {
  ok: boolean;
  status?: string;
  reason?: string;
}

/** Minor units (USD cents) → "$12.34". */
function usd(minor: number): string {
  return `$${(minor / 100).toFixed(2)}`;
}

/** Remaining time until an ISO timestamp, e.g. "23h 14m" / "3m 12s". */
function formatRemaining(expiresAt: string, now: number): string | null {
  const ms = new Date(expiresAt).getTime() - now;
  if (!Number.isFinite(ms) || ms <= 0) return null;
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

type PageState =
  | { phase: "loading" }
  | { phase: "not-found" }
  | { phase: "error"; message: string }
  | { phase: "loaded"; row: AuthorizationRow; acting: "approve" | "decline" | null };

export function AuthorizePage({ authorizationId }: AuthorizePageProps) {
  const auth = useAuth();
  const [state, setState] = useState<PageState>({ phase: "loading" });
  const [now, setNow] = useState(() => Date.now());

  const loadRow = useCallback(async () => {
    const supabase = getSupabase();
    if (!supabase) {
      setState({ phase: "error", message: "Sync is not configured in this build." });
      return;
    }
    const { data, error } = await supabase
      .from("spend_authorizations")
      .select(
        "id, kind, max_amount_minor, daily_cap_minor, process_allowlist, fab_allowlist, status, expires_at, created_at",
      )
      .eq("id", authorizationId)
      .maybeSingle();
    if (error) {
      setState({ phase: "error", message: error.message });
      return;
    }
    if (!data) {
      // RLS returns nothing for rows the caller doesn't own, so someone
      // else's id looks identical to a missing one.
      setState({ phase: "not-found" });
      return;
    }
    setState({ phase: "loaded", row: data as AuthorizationRow, acting: null });
  }, [authorizationId]);

  // Load once the user has a permanent identity. Anonymous sessions have an
  // auth.uid(), but authorizations are minted for the signed-in MCP account —
  // an anon read would always come back empty and mislabel the row not-found.
  useEffect(() => {
    if (!auth.initialized || !auth.isAuthenticated) return;
    void loadRow();
  }, [auth.initialized, auth.isAuthenticated, loadRow]);

  // Tick the expiry countdown while a pending authorization is on screen.
  const pending =
    state.phase === "loaded" && state.row.status === "pending_human";
  useEffect(() => {
    if (!pending) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [pending]);

  const act = useCallback(
    async (action: "approve" | "decline") => {
      if (state.phase !== "loaded" || state.acting) return;
      const supabase = getSupabase();
      if (!supabase) return;
      setState({ ...state, acting: action });
      const fn =
        action === "approve"
          ? "approve_spend_authorization"
          : "decline_spend_authorization";
      const { data, error } = await supabase.rpc(fn, {
        p_authorization_id: authorizationId,
      });
      if (error) {
        setState({ phase: "error", message: error.message });
        return;
      }
      const result = (data ?? {}) as RpcResult;
      if (!result.ok && result.reason === "not_found") {
        setState({ phase: "not-found" });
        return;
      }
      // Success, not_pending, and expired all resolve the same way: re-read
      // the row — the DB is the truth about where the lifecycle landed.
      await loadRow();
    },
    [state, authorizationId, loadRow],
  );

  return (
    <div className="flex min-h-screen items-center justify-center bg-bg p-6 font-mono">
      <div className="w-full max-w-md border border-border bg-surface p-6">
        <div className="mb-4 flex items-center gap-2">
          <span className="text-sm font-bold tracking-tighter text-text">
            vcad<span className="text-brand">.</span>
          </span>
          <span className="text-xs text-text-muted">Spend authorization</span>
        </div>

        {renderBody()}
      </div>
    </div>
  );

  function renderBody() {
    if (!auth.initialized || auth.loading) {
      return <p className="text-xs text-text-muted">Loading…</p>;
    }

    if (!auth.isAuthenticated) {
      return (
        <>
          <p className="mb-3 text-xs text-text">
            An agent is asking to spend from your vcad wallet. Sign in with the
            account your agent is connected to, then review and approve or
            decline the request.
          </p>
          <SignInButton className="h-8 w-full border border-border bg-bg px-3 text-xs text-text hover:bg-hover" />
        </>
      );
    }

    switch (state.phase) {
      case "loading":
        return <p className="text-xs text-text-muted">Loading authorization…</p>;

      case "not-found":
        return (
          <p className="text-xs text-text-muted">
            Authorization not found. It may belong to a different account —
            make sure you are signed in as the same user your agent uses.
          </p>
        );

      case "error":
        return (
          <p className="text-xs text-brand">Error: {state.message}</p>
        );

      case "loaded":
        return renderLoaded(state.row, state.acting);
    }
  }

  function renderLoaded(
    row: AuthorizationRow,
    acting: "approve" | "decline" | null,
  ) {
    const remaining = formatRemaining(row.expires_at, now);
    const expired = row.status === "expired" || (row.status === "pending_human" && !remaining);

    const details = (
      <dl className="mb-4 space-y-2 text-xs">
        <div className="flex justify-between gap-4">
          <dt className="text-text-muted">Spending cap</dt>
          <dd className="text-text font-bold">{usd(row.max_amount_minor)}</dd>
        </div>
        {row.daily_cap_minor != null && (
          <div className="flex justify-between gap-4">
            <dt className="text-text-muted">Daily cap</dt>
            <dd className="text-text">{usd(row.daily_cap_minor)}</dd>
          </div>
        )}
        <div className="flex justify-between gap-4">
          <dt className="text-text-muted">Kind</dt>
          <dd className="text-text">
            {row.kind === "one_time" ? "One order" : "Standing budget"}
          </dd>
        </div>
        {row.fab_allowlist && row.fab_allowlist.length > 0 && (
          <div className="flex justify-between gap-4">
            <dt className="text-text-muted">Manufacturers</dt>
            <dd className="text-text text-right">{row.fab_allowlist.join(", ")}</dd>
          </div>
        )}
        {row.process_allowlist && row.process_allowlist.length > 0 && (
          <div className="flex justify-between gap-4">
            <dt className="text-text-muted">Processes</dt>
            <dd className="text-text text-right">{row.process_allowlist.join(", ")}</dd>
          </div>
        )}
        {row.status === "pending_human" && remaining && (
          <div className="flex justify-between gap-4">
            <dt className="text-text-muted">Expires in</dt>
            <dd className="text-text">{remaining}</dd>
          </div>
        )}
      </dl>
    );

    if (row.status === "pending_human" && !expired) {
      return (
        <>
          <p className="mb-3 text-xs text-text">
            An agent wants to place a fabrication order on your behalf. It can
            spend at most the cap below, and only after you approve.
          </p>
          {details}
          <div className="flex gap-2">
            <button
              onClick={() => void act("approve")}
              disabled={acting !== null}
              className="h-8 flex-1 border border-brand bg-brand px-3 text-xs font-bold text-white hover:bg-brand-hover disabled:opacity-50"
            >
              {acting === "approve" ? "Approving…" : `Approve ${usd(row.max_amount_minor)}`}
            </button>
            <button
              onClick={() => void act("decline")}
              disabled={acting !== null}
              className="h-8 flex-1 border border-border bg-bg px-3 text-xs text-text hover:bg-hover disabled:opacity-50"
            >
              {acting === "decline" ? "Declining…" : "Decline"}
            </button>
          </div>
          <p className="mt-3 text-[10px] text-text-muted">
            Funds only move when the order is placed, and never more than the
            cap. You can revoke an approved authorization before it is used.
          </p>
        </>
      );
    }

    if (expired) {
      return (
        <>
          {details}
          <p className="text-xs text-text">
            This authorization expired before it was approved. Nothing was
            charged. Ask the agent to propose the spend again if you still
            want the order.
          </p>
        </>
      );
    }

    switch (row.status) {
      case "authorized":
        return (
          <>
            {details}
            <p className="text-xs text-text">
              Approved. The agent can now place the order — up to the cap
              above, nothing more. You can close this tab.
            </p>
          </>
        );
      case "consumed":
        return (
          <>
            {details}
            <p className="text-xs text-text">
              This authorization has already been used — the order was placed.
              You can close this tab.
            </p>
          </>
        );
      case "revoked":
        return (
          <>
            {details}
            <p className="text-xs text-text">
              Declined. The authorization was revoked and the agent cannot
              charge against it. You can close this tab.
            </p>
          </>
        );
      default:
        return <p className="text-xs text-text-muted">Status: {row.status}</p>;
    }
  }
}
