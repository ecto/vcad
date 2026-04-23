import { useEffect } from "react";
import type { LegalSlug } from "@/lib/url-document";

interface LegalContent {
  title: string;
  updated: string;
  sections: { heading: string; body: string[] }[];
}

const CONTACT_EMAIL = "info@muni.works";
const COMPANY = "Municipal Robotics, Inc.";
const LAST_UPDATED = "April 23, 2026";

const PRIVACY: LegalContent = {
  title: "Privacy Policy",
  updated: LAST_UPDATED,
  sections: [
    {
      heading: "Who we are",
      body: [
        `vcad is operated by ${COMPANY} ("we", "us"). This policy describes what data we collect when you use vcad.io, the vcad desktop app, the vcad CLI, and the vcad MCP server, and what we do with it.`,
      ],
    },
    {
      heading: "What we collect",
      body: [
        "Account data: email address, display name, and avatar URL from Google or GitHub OAuth. We do not receive your OAuth password.",
        "Document data: the .vcad files you choose to sync to the cloud, plus their version history. Local-only files are never transmitted.",
        "Usage data: feature events (e.g., which export formats are used, how often AI generation is invoked), anonymized error reports, and coarse performance metrics.",
        "Billing data: if you subscribe to a paid plan, Stripe handles your payment method. We receive a customer ID, plan tier, and subscription status — never your full card number.",
      ],
    },
    {
      heading: "How we use it",
      body: [
        "To provide the product: authenticating you, syncing your documents, enforcing plan limits, and rendering AI responses.",
        "To improve the product: aggregated analytics on which features succeed or fail.",
        "To support you: responding when you contact us for help.",
        "We do not sell personal data. We do not use your CAD files to train models without your explicit, opt-in consent.",
      ],
    },
    {
      heading: "Third-party services",
      body: [
        "Supabase (database + auth), Stripe (billing), Google and GitHub (OAuth), and our AI inference providers receive the minimum data needed to perform their function.",
      ],
    },
    {
      heading: "Your rights",
      body: [
        "You may export, correct, or delete your data at any time from your profile settings, or by emailing " + CONTACT_EMAIL + ". Deleting your account removes your documents and version history within 30 days.",
      ],
    },
    {
      heading: "Children",
      body: [
        "vcad is not directed to children under 13. We do not knowingly collect data from children under 13.",
      ],
    },
    {
      heading: "Changes",
      body: [
        "We'll post any material changes to this policy on this page and update the date above.",
      ],
    },
    {
      heading: "Contact",
      body: [`Questions: ${CONTACT_EMAIL}`],
    },
  ],
};

const TERMS: LegalContent = {
  title: "Terms of Service",
  updated: LAST_UPDATED,
  sections: [
    {
      heading: "Acceptance",
      body: [
        `By using vcad you agree to these terms. vcad is operated by ${COMPANY}.`,
      ],
    },
    {
      heading: "Your account",
      body: [
        "You are responsible for activity under your account. Keep your credentials safe. Notify us immediately if you suspect unauthorized access.",
        "One human per account. You may not share credentials.",
      ],
    },
    {
      heading: "Your content",
      body: [
        "You retain ownership of the CAD files and other content you create in vcad. You grant us the narrow license needed to host, sync, render, and back up that content so we can provide the service.",
        "You are responsible for making sure you have the right to upload any content you import (STEP files, meshes, images, etc.).",
      ],
    },
    {
      heading: "Acceptable use",
      body: [
        "Don't use vcad to break the law, infringe others' rights, transmit malware, or attempt to access accounts or data that aren't yours. Don't abuse our AI features to generate content that would violate our providers' policies.",
        "We may suspend accounts that violate these terms.",
      ],
    },
    {
      heading: "Paid plans",
      body: [
        "Paid plans renew automatically at the interval you select until cancelled. You can cancel from your profile; cancellations take effect at the end of the current billing period. Fees are non-refundable except where required by law.",
        "We may adjust pricing with at least 30 days' notice; existing paid subscribers keep their current price until their next renewal after the change.",
      ],
    },
    {
      heading: "Open source components",
      body: [
        "The vcad source is available under the Apache License 2.0. This service-level agreement governs the hosted service at vcad.io and related binaries; the Apache license governs the source code itself.",
      ],
    },
    {
      heading: "Warranty disclaimer",
      body: [
        `THE SERVICE IS PROVIDED "AS IS" WITHOUT WARRANTIES OF ANY KIND. DO NOT RELY ON vcad AS THE SOLE AUTHORITY FOR SAFETY-CRITICAL ENGINEERING DECISIONS — always verify exports with independent review before manufacturing.`,
      ],
    },
    {
      heading: "Limitation of liability",
      body: [
        `To the maximum extent permitted by law, ${COMPANY} is not liable for indirect, incidental, or consequential damages. Our total liability is capped at the greater of $100 or the fees you paid us in the 12 months before the claim.`,
      ],
    },
    {
      heading: "Termination",
      body: [
        "Either of us may terminate at any time. Upon termination, your right to use the hosted service ends; you may export your documents for at least 30 days after termination.",
      ],
    },
    {
      heading: "Governing law",
      body: [
        "These terms are governed by the laws of the State of Delaware, USA, without regard to conflict-of-law principles.",
      ],
    },
    {
      heading: "Contact",
      body: [`Questions: ${CONTACT_EMAIL}`],
    },
  ],
};

const SECURITY: LegalContent = {
  title: "Security",
  updated: LAST_UPDATED,
  sections: [
    {
      heading: "Our approach",
      body: [
        "We take a defense-in-depth approach: encryption in transit (TLS 1.2+) and at rest, row-level security on user data, least-privilege access for engineers, and automated dependency and secret scanning on every commit.",
      ],
    },
    {
      heading: "Authentication",
      body: [
        "Sign-in uses Google or GitHub OAuth through Supabase Auth. We never see your OAuth password. Session tokens are scoped per device and can be revoked from your profile.",
      ],
    },
    {
      heading: "Data isolation",
      body: [
        "Your documents are isolated per user via Postgres row-level security. A user can only read or write rows they own unless they have explicitly shared a document.",
      ],
    },
    {
      heading: "Reporting a vulnerability",
      body: [
        `If you believe you've found a security vulnerability, please email ${CONTACT_EMAIL} with details. Please do not file a public GitHub issue for security reports.`,
        "We aim to acknowledge reports within 2 business days and triage within 5 business days. We commit not to pursue legal action against researchers who report in good faith and follow responsible disclosure.",
      ],
    },
    {
      heading: "Scope",
      body: [
        "In scope: vcad.io, its API endpoints, the published desktop binaries, and the @vcad/mcp npm package.",
        "Out of scope: third-party services (Supabase, Stripe, Google, GitHub) — please report those to the respective vendors.",
      ],
    },
  ],
};

const CONTENT: Record<LegalSlug, LegalContent> = {
  privacy: PRIVACY,
  terms: TERMS,
  security: SECURITY,
};

export function LegalPage({ slug }: { slug: LegalSlug }) {
  const content = CONTENT[slug];

  useEffect(() => {
    const prev = document.title;
    document.title = `${content.title} · vcad`;
    return () => {
      document.title = prev;
    };
  }, [content.title]);

  return (
    <div className="min-h-screen bg-bg text-text">
      <div className="mx-auto max-w-2xl px-6 py-16">
        <a
          href="/"
          className="text-xs uppercase tracking-wider text-text-muted hover:text-text transition-colors"
        >
          ← vcad
        </a>
        <h1 className="mt-6 text-3xl font-bold tracking-tight">{content.title}</h1>
        <p className="mt-2 text-xs text-text-muted">Last updated {content.updated}</p>

        <div className="mt-10 space-y-8">
          {content.sections.map((section) => (
            <section key={section.heading}>
              <h2 className="text-sm font-semibold uppercase tracking-wider text-text-muted">
                {section.heading}
              </h2>
              <div className="mt-3 space-y-3 text-sm leading-relaxed">
                {section.body.map((para, i) => (
                  <p key={i}>{para}</p>
                ))}
              </div>
            </section>
          ))}
        </div>

        <footer className="mt-16 flex gap-6 border-t border-border pt-6 text-xs text-text-muted">
          <a href="/privacy" className="hover:text-text transition-colors">Privacy</a>
          <a href="/terms" className="hover:text-text transition-colors">Terms</a>
          <a href="/security" className="hover:text-text transition-colors">Security</a>
        </footer>
      </div>
    </div>
  );
}
