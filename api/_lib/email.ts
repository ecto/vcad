// Resend email helper. Uses the REST API directly — no SDK needed, just
// fetch. All emails go through Resend for consistent deliverability and
// styling, including Supabase auth emails (via SMTP config) and app-
// triggered transactional emails (via this module).
//
// Env: RESEND_API_KEY, optionally RESEND_FROM (defaults to noreply@vcad.io).

const DEFAULT_FROM = "vcad <noreply@vcad.io>";

interface SendEmailOptions {
  to: string;
  subject: string;
  html: string;
  /** Plain-text fallback. Auto-stripped from HTML if omitted. */
  text?: string;
}

export async function sendEmail(opts: SendEmailOptions): Promise<boolean> {
  const apiKey = process.env.RESEND_API_KEY;
  if (!apiKey) {
    console.warn("[email] RESEND_API_KEY not set — skipping email");
    return false;
  }

  const from = process.env.RESEND_FROM ?? DEFAULT_FROM;
  try {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: {
        Authorization: `Bearer ${apiKey}`,
        "Content-Type": "application/json",
      },
      body: JSON.stringify({
        from,
        to: [opts.to],
        subject: opts.subject,
        html: opts.html,
        text: opts.text,
      }),
    });

    if (!res.ok) {
      const err = await res.text();
      console.error("[email] Resend error:", res.status, err.slice(0, 200));
      return false;
    }
    return true;
  } catch (err) {
    console.error("[email] send failed:", err);
    return false;
  }
}

// ---------------------------------------------------------------------------
// Base template — dark monokai aesthetic matching vcad's UI. Inline styles
// everywhere because email clients strip <style> tags.
// ---------------------------------------------------------------------------

interface EmailLayoutOptions {
  /** Main heading */
  title: string;
  /** Body HTML (goes inside the content area) */
  body: string;
  /** Optional CTA button */
  cta?: { label: string; url: string };
  /** Optional footer text (below the divider) */
  footer?: string;
}

export function emailLayout(opts: EmailLayoutOptions): string {
  const ctaBlock = opts.cta
    ? `
      <div style="margin: 28px 0 20px;">
        <a href="${opts.cta.url}"
           style="display: inline-block; padding: 12px 28px;
                  background-color: #F92672; color: #ffffff;
                  font-family: 'Berkeley Mono', ui-monospace, monospace;
                  font-size: 13px; font-weight: 700;
                  text-transform: uppercase; letter-spacing: 0.08em;
                  text-decoration: none; mso-padding-alt: 0;">
          ${opts.cta.label}
        </a>
      </div>`
    : "";

  const footerBlock = opts.footer
    ? `<p style="margin: 0; font-size: 11px; color: #75715E;">${opts.footer}</p>`
    : "";

  return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <meta name="color-scheme" content="dark" />
  <meta name="supported-color-schemes" content="dark" />
  <title>${opts.title}</title>
</head>
<body style="margin: 0; padding: 0; background-color: #1a1a1a;
             font-family: 'Berkeley Mono', ui-monospace, 'SFMono-Regular', Menlo, monospace;
             color: #F8F8F2; -webkit-font-smoothing: antialiased;">
  <div style="max-width: 520px; margin: 0 auto; padding: 40px 24px;">
    <!-- Logo -->
    <div style="margin-bottom: 32px;">
      <span style="font-size: 22px; font-weight: 700; letter-spacing: -0.04em; color: #F8F8F2;">vcad</span><span style="font-size: 22px; font-weight: 700; color: #F92672;">.</span>
    </div>

    <!-- Content card -->
    <div style="background-color: #222222; border: 1px solid #444444; padding: 28px 24px;">
      <h1 style="margin: 0 0 16px; font-size: 18px; font-weight: 700;
                 letter-spacing: -0.02em; color: #F8F8F2;">
        ${opts.title}
      </h1>
      <div style="font-size: 13px; line-height: 1.6; color: #F8F8F2;">
        ${opts.body}
      </div>
      ${ctaBlock}
    </div>

    <!-- Footer -->
    <div style="margin-top: 24px; padding-top: 16px; border-top: 1px solid #333333;">
      ${footerBlock}
      <p style="margin: 8px 0 0; font-size: 10px; color: #555555;">
        vcad &mdash; open-source parametric CAD
      </p>
    </div>
  </div>
</body>
</html>`;
}

// ---------------------------------------------------------------------------
// Specific email builders
// ---------------------------------------------------------------------------

import { TIERS, formatTokens, type TierId } from "@vcad/core";

export function usageAlertEmail(opts: {
  firstName: string;
  tier: TierId;
  used: number;
  limit: number;
  periodEnd: string;
}): { subject: string; html: string } {
  const pct = Math.round((opts.used / opts.limit) * 100);
  const tierInfo = TIERS[opts.tier];
  const remaining = Math.max(0, opts.limit - opts.used);
  const resetDate = (() => {
    try {
      return new Date(opts.periodEnd).toLocaleDateString("en-US", {
        month: "long",
        day: "numeric",
      });
    } catch {
      return "the start of your next billing period";
    }
  })();

  const upgradeNote =
    opts.tier === "max"
      ? `<p style="margin: 12px 0 0; color: #75715E;">
           You're on the Max plan. If you need a higher limit, reach out at
           <a href="mailto:hello@vcad.io" style="color: #F92672; text-decoration: none;">hello@vcad.io</a>.
         </p>`
      : "";

  return {
    subject: `You've used ${pct}% of your ${tierInfo.name} chat tokens`,
    html: emailLayout({
      title: `You've used ${pct}% of your monthly tokens.`,
      body: `
        <p style="margin: 0 0 12px;">
          You've consumed <strong>${formatTokens(opts.used)}</strong> of your
          <strong>${formatTokens(opts.limit)}</strong> token budget on the
          ${tierInfo.name} plan. You have
          <strong>${formatTokens(remaining)}</strong> remaining until your
          limit resets on <strong>${resetDate}</strong>.
        </p>
        <div style="margin: 16px 0; background-color: #333333; height: 6px;">
          <div style="height: 6px; width: ${Math.min(100, pct)}%;
                      background-color: ${pct >= 100 ? "#F92672" : "#f59e0b"};"></div>
        </div>
        <p style="margin: 0; font-size: 11px; color: #75715E;">
          ${formatTokens(opts.used)} / ${formatTokens(opts.limit)} &middot; ${pct}% used
        </p>
        ${upgradeNote}`,
      cta:
        opts.tier !== "max"
          ? { label: "Upgrade for more tokens", url: "https://vcad.io/?billing=upgrade" }
          : undefined,
      footer: `Your limit resets on ${resetDate}. You can keep chatting until then — we won't cut you off mid-conversation.`,
    }),
  };
}

export function upgradeWelcomeEmail(opts: {
  firstName: string;
  tier: TierId;
}): { subject: string; html: string } {
  const tierInfo = TIERS[opts.tier];
  return {
    subject: `Welcome to vcad ${tierInfo.name}`,
    html: emailLayout({
      title: `Welcome to ${tierInfo.name}, ${opts.firstName}.`,
      body: `
        <p style="margin: 0 0 12px;">
          Your upgrade is active. You now have
          <strong>${formatTokens(tierInfo.monthlyTokenLimit)}</strong> chat
          tokens per month — plenty of room to build.
        </p>
        <p style="margin: 0 0 12px;">
          Your subscription renews monthly. You can manage your payment method,
          switch plans, or cancel anytime from the customer portal.
        </p>`,
      cta: { label: "Back to building", url: "https://vcad.io" },
      footer:
        "Manage your subscription from the avatar menu in vcad, or reply to this email if you need help.",
    }),
  };
}
