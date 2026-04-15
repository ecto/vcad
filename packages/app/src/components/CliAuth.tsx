import { useEffect, useState } from "react";
import { useAuth, SignInButton } from "@vcad/auth";

/**
 * Device-code browser flow completion page.
 *
 * Rendered at `/cli-auth?code=<code>` when the user runs `vcad login`
 * in a terminal. Flow:
 *
 * 1. Parse `?code=X` from the URL.
 * 2. If not signed in, show a sign-in prompt. The normal OAuth flow
 *    (Google / GitHub via Supabase) redirects back here on completion.
 * 3. Once signed in, POST the current access_token + refresh_token to
 *    `/api/cli-auth` with the code so the TUI's polling picks it up.
 * 4. Show a friendly "you can close this tab" message.
 *
 * This is a standalone component mounted from `main.tsx` when the
 * pathname matches — it doesn't share the App render path, so it can't
 * accidentally break the normal editor load.
 */
export function CliAuth() {
  const auth = useAuth();
  const code = new URLSearchParams(window.location.search).get("code") ?? "";
  const [status, setStatus] = useState<"idle" | "posting" | "done" | "error">(
    "idle",
  );
  const [errorMessage, setErrorMessage] = useState<string>("");

  useEffect(() => {
    if (!code) {
      setStatus("error");
      setErrorMessage("Missing ?code parameter. Run `vcad login` from your terminal to get a fresh code.");
      return;
    }
    if (!auth.session?.access_token) {
      // Wait for the user to sign in.
      return;
    }
    if (status !== "idle") {
      return;
    }

    setStatus("posting");
    fetch("/api/cli-auth", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${auth.session.access_token}`,
      },
      body: JSON.stringify({
        code,
        access_token: auth.session.access_token,
        refresh_token: auth.session.refresh_token ?? null,
        expires_at: auth.session.expires_at ?? null,
      }),
    })
      .then((res) => {
        if (!res.ok) {
          throw new Error(`server returned ${res.status}`);
        }
        setStatus("done");
      })
      .catch((err) => {
        setStatus("error");
        setErrorMessage(err instanceof Error ? err.message : String(err));
      });
  }, [auth.session, code, status]);

  return (
    <div className="flex min-h-screen items-center justify-center bg-bg p-6 font-mono">
      <div className="w-full max-w-md border border-border bg-surface p-6">
        <div className="mb-4 flex items-center gap-2">
          <span className="text-sm font-bold tracking-tighter text-text">
            vcad<span className="text-brand">.</span>
          </span>
          <span className="text-xs text-text-muted">CLI sign-in</span>
        </div>

        {!code && (
          <p className="text-xs text-text-muted">
            Missing <code className="text-brand">?code</code> parameter. Run{" "}
            <code className="text-brand">vcad login</code> from your terminal
            to get a fresh code.
          </p>
        )}

        {code && !auth.session && (
          <>
            <p className="mb-3 text-xs text-text">
              Sign in to authorize the vcad CLI. Once you sign in this tab
              will automatically forward your token to the terminal.
            </p>
            <SignInButton className="h-8 w-full border border-border bg-bg px-3 text-xs text-text hover:bg-hover" />
          </>
        )}

        {code && auth.session && status === "posting" && (
          <p className="text-xs text-text-muted">Forwarding token to the CLI…</p>
        )}

        {status === "done" && (
          <>
            <p className="mb-2 text-xs text-text">You're signed in.</p>
            <p className="text-xs text-text-muted">
              You can close this tab and return to your terminal.
            </p>
          </>
        )}

        {status === "error" && (
          <p className="text-xs text-brand">
            Error: {errorMessage || "Something went wrong — try `vcad login` again."}
          </p>
        )}
      </div>
    </div>
  );
}
