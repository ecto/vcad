import { HeroPlayground } from "@/components/Playground/HeroPlayground";
import { FeatureGrid } from "@/components/FeatureGrid";
import { GalleryPreview } from "@/components/Gallery/GalleryPreview";
import { LearningPaths } from "@/components/LearningPaths";

export default function DocsHomePage() {
  return (
    <div className="min-h-screen">
      {/* Hero Section */}
      <section className="relative px-8 py-16 lg:px-16 lg:py-24">
        <div className="max-w-6xl mx-auto">
          {/* Heading */}
          <div className="mb-12">
            <h1 className="text-5xl lg:text-7xl font-bold tracking-tight mb-4">
              vcad<span className="text-accent">.</span>
            </h1>
            <p className="text-xl text-text-muted mb-2">
              open-source cad for makers
            </p>
            <p className="text-text-muted max-w-lg">
              web app, rust library, cli, and ai tools.
              custom brep kernel. mit licensed.
            </p>
          </div>

          {/* Hero Playground */}
          <HeroPlayground />

          {/* Install options */}
          <div className="mt-8 flex flex-wrap gap-4">
            <a
              href="https://vcad.io"
              className="inline-flex items-center gap-2 text-sm bg-accent text-white px-4 py-2 rounded-md hover:bg-accent-hover transition-colors"
            >
              vcad.io
              <span className="text-white/60">→</span>
            </a>
            <code className="inline-flex items-center gap-2 text-sm bg-surface px-4 py-2 rounded-md border border-border">
              <span className="text-text-muted select-none">$</span>
              <span>cargo add vcad</span>
              <span className="w-2 h-4 bg-text-muted cursor-blink" />
            </code>
          </div>
        </div>
      </section>

      {/* Divider */}
      <div className="max-w-6xl mx-auto px-8 lg:px-16">
        <hr className="border-border" />
      </div>

      {/* Learning Paths */}
      <section className="px-8 py-16 lg:px-16">
        <div className="max-w-6xl mx-auto">
          <h2 className="text-sm text-text-muted mb-8 tracking-wide">
            // get started
          </h2>
          <LearningPaths />
        </div>
      </section>

      {/* Divider */}
      <div className="max-w-6xl mx-auto px-8 lg:px-16">
        <hr className="border-border" />
      </div>

      {/* Features */}
      <section className="px-8 py-16 lg:px-16">
        <div className="max-w-6xl mx-auto">
          <h2 className="text-sm text-text-muted mb-8 tracking-wide">
            // features
          </h2>
          <FeatureGrid />
        </div>
      </section>

      {/* Divider */}
      <div className="max-w-6xl mx-auto px-8 lg:px-16">
        <hr className="border-border" />
      </div>

      {/* Gallery Preview */}
      <section className="px-8 py-16 lg:px-16">
        <div className="max-w-6xl mx-auto">
          <div className="flex items-center justify-between mb-8">
            <h2 className="text-sm text-text-muted tracking-wide">
              // gallery
            </h2>
            <a href="/gallery" className="text-sm text-accent hover:text-accent-hover">
              view all →
            </a>
          </div>
          <GalleryPreview />
        </div>
      </section>

      {/* Footer */}
      <footer className="px-8 py-8 lg:px-16 border-t border-border">
        <div className="max-w-6xl mx-auto flex flex-col sm:flex-row justify-between items-center gap-4 text-sm text-text-muted">
          <span>mit license</span>
          <div className="flex gap-6">
            <a href="https://github.com/ecto/vcad" className="hover:text-text">github</a>
            <a href="https://vcad.io" className="hover:text-text">vcad.io</a>
            <a href="https://crates.io/crates/vcad" className="hover:text-text">crates.io</a>
            <a href="https://www.npmjs.com/org/vcad" className="hover:text-text">npm</a>
          </div>
        </div>
      </footer>
    </div>
  );
}
