import type { Metadata } from "next";
import Link from "next/link";
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";

export const metadata: Metadata = {
  title: "Onshape Alternative – vcad (Free, Open Source)",
  description:
    "Looking for an Onshape alternative? vcad is a free, open-source CAD library for makers. Local-first, no subscription, direct STL export for 3D printing.",
  openGraph: {
    title: "Onshape Alternative – vcad",
    description: "Free, open-source CAD for makers. No cloud, no subscription.",
    type: "article",
    url: "https://docs.vcad.io/alternatives/onshape",
  },
  alternates: {
    canonical: "https://docs.vcad.io/alternatives/onshape",
  },
};

const painPoints = [
  {
    title: "The subscription adds up",
    description:
      "Onshape's free tier forces your models public. Private models start at $1,500/year. For hobbyists making parts for 3D printing, that's hard to justify.",
  },
  {
    title: "Cloud-only means internet-required",
    description:
      "No internet, no CAD. Onshape's geometry runs on their servers. Your workshop doesn't always have reliable wifi.",
  },
  {
    title: "Your models live on their servers",
    description:
      "On the free tier, your designs are public. Even on paid tiers, you're trusting a company with your IP. Some makers prefer owning their files outright.",
  },
];

const benefits = [
  {
    title: "Free forever",
    description:
      "MIT licensed. No subscription, no account, no cloud. Use it for hobby projects or commercial products.",
  },
  {
    title: "Local-first",
    description:
      "Runs on your machine. Works offline. Your files stay on your disk.",
  },
  {
    title: "Git-native",
    description:
      "Models are code. Track changes, branch, diff, merge. Real version control, not cloud sync.",
  },
  {
    title: "Parametric",
    description:
      "Variables are just variables. Change a dimension, regenerate the part. No feature tree to fight.",
  },
];

const bestFor = [
  "Functional parts – brackets, enclosures, mounts, adapters",
  "Parametric designs – parts you'll customize and regenerate",
  "3D printing workflows – direct STL export, no conversion step",
  "Developers and makers – people comfortable with code",
  "AI-assisted design – agents can generate models from text prompts",
];

const whenOnshapeIsBetter = [
  "Complex assemblies with mates and constraints",
  "Real-time collaboration with teammates",
  "Sketch constraints and geometric relationships",
  "A visual GUI you can click around in",
];

export default function OnshapeAlternativePage() {
  return (
    <div className="min-h-screen flex flex-col">
      <Header />

      <main className="flex-1">
        {/* Hero */}
        <section className="py-12 lg:py-16">
          <div className="max-w-[720px] mx-auto px-8">
            <h1 className="text-3xl lg:text-4xl font-bold tracking-tight mb-6">
              Looking for an Onshape alternative<span className="text-accent">?</span>
            </h1>
            <p className="text-text-muted leading-relaxed">
              If you're hitting Onshape's limitations—the subscription cost, cloud
              dependency, or public-model requirement on the free tier—vcad might
              be what you need. It's a different approach: CAD as code, running
              locally, free forever.
            </p>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Pain points */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-6">
              Why people look for alternatives
            </h2>
            <div className="space-y-4">
              {painPoints.map((point) => (
                <div
                  key={point.title}
                  className="bg-surface border border-border rounded-md p-5"
                >
                  <h3 className="font-bold text-accent mb-2">{point.title}</h3>
                  <p className="text-sm text-text-muted">{point.description}</p>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* vcad as alternative */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-4">
              vcad as an alternative
            </h2>
            <p className="text-sm text-text-muted mb-6">
              vcad takes a different approach. Instead of a cloud GUI, it's a code
              library. You write models in Rust or TypeScript, and they compile to
              STL, glTF, or STEP.
            </p>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
              {benefits.map((benefit) => (
                <div
                  key={benefit.title}
                  className="bg-surface border border-border rounded-md p-4"
                >
                  <h3 className="font-bold text-text text-sm mb-1">
                    {benefit.title}
                  </h3>
                  <p className="text-sm text-text-muted">{benefit.description}</p>
                </div>
              ))}
            </div>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Code example */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-6">
              What it looks like
            </h2>
            <pre className="text-[13px] leading-relaxed overflow-x-auto bg-surface p-5 rounded-lg border border-border">
              <code>
                <span className="text-syntax-keyword">use</span>{" "}
                vcad::{"{"}centered_cube, centered_cylinder, Part{"}"};{"\n\n"}
                <span className="text-syntax-comment">// mounting plate with four holes</span>{"\n"}
                <span className="text-syntax-keyword">let</span> width ={" "}
                <span className="text-syntax-number">100.0</span>;{"\n"}
                <span className="text-syntax-keyword">let</span> height ={" "}
                <span className="text-syntax-number">60.0</span>;{"\n"}
                <span className="text-syntax-keyword">let</span> thickness ={" "}
                <span className="text-syntax-number">5.0</span>;{"\n"}
                <span className="text-syntax-keyword">let</span> hole_diameter ={" "}
                <span className="text-syntax-number">6.0</span>;{"\n\n"}
                <span className="text-syntax-keyword">let</span> plate = centered_cube(
                <span className="text-syntax-string">&quot;plate&quot;</span>, width, height, thickness);{"\n"}
                <span className="text-syntax-keyword">let</span> hole = centered_cylinder(
                <span className="text-syntax-string">&quot;hole&quot;</span>, hole_diameter /{" "}
                <span className="text-syntax-number">2.0</span>, thickness +{" "}
                <span className="text-syntax-number">1.0</span>,{" "}
                <span className="text-syntax-number">32</span>);{"\n\n"}
                <span className="text-syntax-keyword">let</span> holes = hole{"\n"}
                {"    "}.linear_pattern(
                <span className="text-syntax-number">80.0</span>,{" "}
                <span className="text-syntax-number">0.0</span>,{" "}
                <span className="text-syntax-number">0.0</span>,{" "}
                <span className="text-syntax-number">2</span>){"\n"}
                {"    "}.linear_pattern(
                <span className="text-syntax-number">0.0</span>,{" "}
                <span className="text-syntax-number">40.0</span>,{" "}
                <span className="text-syntax-number">0.0</span>,{" "}
                <span className="text-syntax-number">2</span>){"\n"}
                {"    "}.translate(
                <span className="text-syntax-number">-40.0</span>,{" "}
                <span className="text-syntax-number">-20.0</span>,{" "}
                <span className="text-syntax-number">0.0</span>);{"\n\n"}
                <span className="text-syntax-keyword">let</span> part = plate{" "}
                <span className="text-text">-</span> holes;{"\n"}
                part.write_stl(<span className="text-syntax-string">&quot;plate.stl&quot;</span>).unwrap();
              </code>
            </pre>
            <p className="text-sm text-text-muted mt-4">
              Change <code className="bg-surface px-1.5 py-0.5 rounded border border-border">hole_diameter</code> to 8.0, re-run, get a new STL. That's parametric modeling.
            </p>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Best for */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-4">
              Best for
            </h2>
            <ul className="text-sm text-text-muted space-y-2 list-disc list-inside">
              {bestFor.map((item) => (
                <li key={item}>
                  <span className="font-bold text-text">
                    {item.split(" – ")[0]}
                  </span>
                  {item.includes(" – ") && ` – ${item.split(" – ")[1]}`}
                </li>
              ))}
            </ul>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Honest comparison */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <div className="border-l-2 border-text-muted pl-5">
              <h3 className="font-bold text-text-muted mb-2">
                When Onshape is still better
              </h3>
              <p className="text-sm text-text-muted mb-4">
                Be honest with yourself: vcad isn't a full Onshape replacement. If
                you need:
              </p>
              <ul className="text-sm text-text-muted space-y-1 list-disc list-inside mb-4">
                {whenOnshapeIsBetter.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
              <p className="text-sm text-text-muted">
                ...then Onshape (or Fusion, or SolidWorks) is the right tool. vcad
                is for simpler, code-first parametric parts.
              </p>
            </div>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* CTA */}
        <section className="py-12 text-center">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-2xl font-bold tracking-tight mb-3">Try it</h2>
            <p className="text-sm text-text-muted mb-6">
              No signup. No trial period. Just code.
            </p>
            <code className="inline-block bg-surface border border-border rounded px-4 py-2 text-sm">
              <span className="text-text-muted">$</span> cargo add vcad
            </code>
          </div>
        </section>
      </main>

      <Footer />
    </div>
  );
}
