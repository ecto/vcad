// Discord webhook helper. Fire-and-forget activity notifications for events
// we want visibility into (new signups, new docs, new chat threads, billing).
//
// Env:
//   DISCORD_WEBHOOK_URL — default webhook for all event types.
//   DISCORD_WEBHOOK_URL_<KIND> — optional per-kind override (e.g.
//   DISCORD_WEBHOOK_URL_BILLING). Falls back to DISCORD_WEBHOOK_URL.
//
// Only fires from production deploys — VERCEL_ENV must be "production".
// Preview deployments and local dev are silenced so test events don't spam
// the channel. Set DISCORD_FORCE=1 to override (useful for debugging).
//
// Failures are logged but never thrown — Discord being down must never break
// a user-facing request.

const BRAND_COLOR = 0xf92672; // vcad pink

type EventKind = "signup" | "document" | "chat" | "billing";

const KIND_META: Record<EventKind, { emoji: string; color: number }> = {
  signup: { emoji: "👋", color: 0x66d9ef },
  document: { emoji: "📐", color: BRAND_COLOR },
  chat: { emoji: "💬", color: 0xa6e22e },
  billing: { emoji: "💳", color: 0xfd971f },
};

interface NotifyOpts {
  kind: EventKind;
  title: string;
  description?: string;
  fields?: Array<{ name: string; value: string; inline?: boolean }>;
  url?: string;
}

function resolveWebhook(kind: EventKind): string | null {
  const specific = process.env[`DISCORD_WEBHOOK_URL_${kind.toUpperCase()}`];
  if (specific && specific.length > 0) return specific;
  const fallback = process.env.DISCORD_WEBHOOK_URL;
  return fallback && fallback.length > 0 ? fallback : null;
}

export async function notifyDiscord(opts: NotifyOpts): Promise<void> {
  if (process.env.VERCEL_ENV !== "production" && process.env.DISCORD_FORCE !== "1") return;
  const webhook = resolveWebhook(opts.kind);
  if (!webhook) return;

  const meta = KIND_META[opts.kind];
  const embed: Record<string, unknown> = {
    title: `${meta.emoji} ${opts.title}`,
    color: meta.color,
    timestamp: new Date().toISOString(),
  };
  if (opts.description) embed.description = opts.description;
  if (opts.url) embed.url = opts.url;
  if (opts.fields && opts.fields.length > 0) embed.fields = opts.fields;

  try {
    const res = await fetch(webhook, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ embeds: [embed] }),
    });
    if (!res.ok) {
      const body = await res.text().catch(() => "");
      console.error("[discord] webhook error:", res.status, body.slice(0, 200));
    }
  } catch (err) {
    console.error("[discord] webhook failed:", err);
  }
}

/** Safe email masking: "alex@example.com" → "a***@example.com". */
export function maskEmail(email: string | null | undefined): string {
  if (!email) return "unknown";
  const [local, domain] = email.split("@");
  if (!local || !domain) return "unknown";
  const head = local.slice(0, 1);
  return `${head}${"*".repeat(Math.max(1, local.length - 1))}@${domain}`;
}
