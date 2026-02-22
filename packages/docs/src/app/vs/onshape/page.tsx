import type { Metadata } from "next";
import { Header } from "@/components/Header";
import { Footer } from "@/components/Footer";

export const metadata: Metadata = {
  title: "vcad vs Onshape – Comparison for Makers",
  description:
    "Compare vcad and Onshape for 3D modeling. vcad is free, runs locally, and exports to STL. Onshape is cloud-based with a subscription. See which fits your workflow.",
  openGraph: {
    title: "vcad vs Onshape",
    description: "Free local CAD vs cloud subscription. Compare for 3D printing and making.",
    type: "article",
    url: "https://docs.vcad.io/vs/onshape",
  },
  alternates: {
    canonical: "https://docs.vcad.io/vs/onshape",
  },
};

const comparisonData = [
  { feature: "Price", vcad: "Free (MIT license)", onshape: "Free tier limited; $1,500+/yr for full", vcadWins: true },
  { feature: "Runs", vcad: "Locally on your machine", onshape: "Cloud only (browser)", vcadWins: null },
  { feature: "Offline", vcad: "Yes, always", onshape: "No", vcadWins: true },
  { feature: "Interface", vcad: "Code (Rust/TypeScript)", onshape: "GUI (browser)", vcadWins: null },
  { feature: "Parametric", vcad: "Yes (variables in code)", onshape: "Yes (feature tree)", vcadWins: null },
  { feature: "Assemblies", vcad: "Not yet", onshape: "Yes, full constraints", vcadWins: false },
  { feature: "Collaboration", vcad: "Git (standard version control)", onshape: "Real-time multi-user", vcadWins: null },
  { feature: "STL Export", vcad: "Yes, built-in", onshape: "Yes", vcadWins: null },
  { feature: "STEP Export", vcad: "Yes", onshape: "Yes", vcadWins: null },
  { feature: "Your files", vcad: "On your disk", onshape: "On their servers", vcadWins: true },
  { feature: "Open source", vcad: "Yes", onshape: "No", vcadWins: true },
];

const whenOnshape = [
  "You need full mechanical assemblies with constraints",
  "You prefer a traditional CAD GUI",
  "Collaboration features are worth the subscription",
  "You're already invested in the Onshape ecosystem",
];

const whenVcad = [
  "You're making parametric parts for 3D printing",
  "You want free, open-source, local-first CAD",
  "You prefer code to GUI (or want AI-generated models)",
  "You want Git version control for your designs",
  "You don't need complex assemblies",
];

export default function VsOnshapePage() {
  return (
    <div className="min-h-screen flex flex-col">
      <Header />

      <main className="flex-1">
        {/* Hero */}
        <section className="py-12 lg:py-16">
          <div className="max-w-[720px] mx-auto px-8">
            <h1 className="text-3xl lg:text-4xl font-bold tracking-tight mb-6">
              vcad vs Onshape<span className="text-accent">.</span>
            </h1>
            <p className="text-text-muted leading-relaxed">
              <span className="font-bold text-text">TL;DR:</span> Onshape is
              professional cloud CAD with a subscription model and browser-based
              UI. vcad is a free, open-source Rust library for code-driven 3D
              modeling. Choose Onshape if you need assemblies, constraints, and
              collaboration. Choose vcad if you want simple parametric parts,
              local-first workflow, and direct STL export for 3D printing.
            </p>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Comparison table */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-6">
              At a glance
            </h2>
            <div className="overflow-x-auto">
              <table className="w-full text-sm border-collapse">
                <thead>
                  <tr className="bg-surface">
                    <th className="text-left p-3 font-bold text-text border-b border-border" />
                    <th className="text-left p-3 font-bold text-text border-b border-border">
                      vcad
                    </th>
                    <th className="text-left p-3 font-bold text-text border-b border-border">
                      Onshape
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {comparisonData.map((row) => (
                    <tr key={row.feature}>
                      <td className="p-3 text-text border-b border-border">
                        {row.feature}
                      </td>
                      <td
                        className={`p-3 border-b border-border ${
                          row.vcadWins === true
                            ? "text-syntax-string"
                            : row.vcadWins === false
                            ? "text-text-muted"
                            : "text-text-muted"
                        }`}
                      >
                        {row.vcad}
                      </td>
                      <td
                        className={`p-3 border-b border-border ${
                          row.vcadWins === false
                            ? "text-syntax-string"
                            : row.vcadWins === true
                            ? "text-text-muted"
                            : "text-text-muted"
                        }`}
                      >
                        {row.onshape}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Architecture */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-6">
              Architecture
            </h2>

            <h3 className="font-bold text-text mb-2">
              Onshape: Cloud-native CAD
            </h3>
            <p className="text-sm text-text-muted mb-6">
              Onshape runs entirely in the cloud. The browser handles rendering
              (WebGL), but all geometry computation happens on their servers using
              the Parasolid kernel. This enables real-time collaboration and works
              on any device—but requires internet and means your models live on
              their infrastructure.
            </p>

            <h3 className="font-bold text-text mb-2">vcad: Local-first library</h3>
            <p className="text-sm text-text-muted">
              vcad runs on your machine. It's a Rust library (with TypeScript
              bindings) built on the Manifold geometry kernel. Your models are
              source code—version-controlled with Git, built with cargo or npm,
              tested in CI. No account needed, no cloud dependency.
            </p>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* For makers */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-6">
              For makers and 3D printing
            </h2>

            <h3 className="font-bold text-text mb-2">When Onshape makes sense</h3>
            <ul className="text-sm text-text-muted space-y-1 list-disc list-inside mb-6">
              <li>You need complex assemblies with mates and constraints</li>
              <li>You want a visual GUI with click-and-drag sketching</li>
              <li>Real-time collaboration with others is important</li>
              <li>
                You're doing professional/commercial work and budget isn't the
                main concern
              </li>
            </ul>

            <h3 className="font-bold text-text mb-2">When vcad makes sense</h3>
            <ul className="text-sm text-text-muted space-y-1 list-disc list-inside">
              <li>You're making functional parts: brackets, enclosures, mounting plates</li>
              <li>You want parametric models you can tweak and regenerate</li>
              <li>You prefer code over GUI (or want AI agents to generate models)</li>
              <li>You want to keep files local and use Git for versioning</li>
              <li>Budget matters—vcad is free forever</li>
            </ul>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Detailed comparison */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8">
            <h2 className="text-xs font-normal text-text-muted uppercase tracking-wider mb-6">
              Detailed comparison
            </h2>

            <h3 className="font-bold text-text mb-2">Modeling approach</h3>
            <p className="text-sm text-text-muted mb-4">
              Onshape uses traditional feature-based modeling: you create 2D
              sketches on planes, then extrude, revolve, or sweep them into 3D. It
              has a visual feature tree and constraint-based sketching.
            </p>
            <p className="text-sm text-text-muted mb-6">
              vcad uses CSG (Constructive Solid Geometry): you combine primitives
              (cubes, cylinders, spheres) with boolean operations (union,
              difference, intersection). Models are Rust code with operator
              overloads: <code className="bg-surface px-1.5 py-0.5 rounded border border-border">plate - holes</code> creates a plate with holes.
            </p>

            <h3 className="font-bold text-text mb-2">Ease of use</h3>
            <p className="text-sm text-text-muted mb-4">
              Onshape has a learning curve but is approachable if you've used CAD
              before. The GUI gives immediate visual feedback.
            </p>
            <p className="text-sm text-text-muted mb-6">
              vcad requires programming knowledge. If you're comfortable with
              code, it's simpler than learning CAD UI conventions. If you've never
              coded, there's a steeper ramp.
            </p>

            <h3 className="font-bold text-text mb-2">Export and 3D printing</h3>
            <p className="text-sm text-text-muted mb-6">
              Both export to STL for 3D printing. vcad also exports glTF (for
              rendering), USD (for simulation), and DXF (for laser cutting) from
              the same model. One codebase, every format.
            </p>

            <h3 className="font-bold text-text mb-2">Pricing</h3>
            <p className="text-sm text-text-muted mb-6">
              Onshape's free tier requires public models and has limitations. The
              paid tiers start at $1,500/year. vcad is MIT licensed—free for any
              use, commercial included.
            </p>

            <h3 className="font-bold text-text mb-2">Privacy and ownership</h3>
            <p className="text-sm text-text-muted">
              On Onshape's free tier, your models are public and stored on their
              servers. vcad models are files on your computer—share them or don't,
              your choice.
            </p>
          </div>
        </section>

        {/* Divider */}
        <div className="max-w-[720px] mx-auto px-8">
          <hr className="border-border" />
        </div>

        {/* Verdict boxes */}
        <section className="py-10">
          <div className="max-w-[720px] mx-auto px-8 space-y-4">
            <div className="bg-surface border border-border rounded-md p-5">
              <h3 className="font-bold text-text mb-3">Choose Onshape if:</h3>
              <ul className="text-sm text-text-muted space-y-1 list-disc list-inside">
                {whenOnshape.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
            </div>

            <div className="bg-surface border border-border rounded-md p-5">
              <h3 className="font-bold text-text mb-3">Choose vcad if:</h3>
              <ul className="text-sm text-text-muted space-y-1 list-disc list-inside">
                {whenVcad.map((item) => (
                  <li key={item}>{item}</li>
                ))}
              </ul>
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
            <h2 className="text-2xl font-bold tracking-tight mb-3">Try vcad</h2>
            <p className="text-sm text-text-muted mb-6">
              One command to get started. No account, no cloud, no subscription.
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
