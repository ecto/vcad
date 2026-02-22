import type { Metadata } from "next";
import Link from "next/link";
import {
  Cube,
  ArrowsOutCardinal,
  Intersect,
  MagnifyingGlass,
  Export,
  TreeStructure,
} from "@phosphor-icons/react/dist/ssr";
import { getAllContent } from "@/lib/content";

export const metadata: Metadata = {
  title: "API Reference",
  description: "Complete API documentation for vcad",
};

const categoryIcons: Record<string, typeof Cube> = {
  primitives: Cube,
  transforms: ArrowsOutCardinal,
  "csg-operations": Intersect,
  inspection: MagnifyingGlass,
  export: Export,
  "ir-types": TreeStructure,
};

export default function ReferencePage() {
  const pages = getAllContent("reference");

  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <div className="mb-12">
        <h1 className="text-4xl font-bold mb-4">API Reference</h1>
        <p className="text-text-muted text-lg max-w-2xl">
          Complete documentation for all vcad primitives, transforms, CSG operations,
          and export formats.
        </p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        {pages.map((page) => {
          const Icon = categoryIcons[page.slug] || Cube;
          return (
            <Link
              key={page.slug}
              href={`/reference/${page.slug}`}
              className="flex items-start gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
            >
              <div className="p-3 rounded-lg bg-accent/10 text-accent">
                <Icon size={24} />
              </div>
              <div>
                <h2 className="font-bold group-hover:text-accent transition-colors">
                  {page.meta.title}
                </h2>
                <p className="text-sm text-text-muted mt-1">
                  {page.meta.description}
                </p>
              </div>
            </Link>
          );
        })}
      </div>

      {/* Quick links */}
      <div className="mt-12 p-6 rounded-lg border border-border bg-surface">
        <h3 className="font-bold mb-4">Common Tasks</h3>
        <div className="grid gap-2 sm:grid-cols-2 text-sm">
          <Link
            href="/reference/primitives#cube"
            className="text-accent hover:text-accent-hover"
          >
            Create a cube
          </Link>
          <Link
            href="/reference/csg-operations#difference"
            className="text-accent hover:text-accent-hover"
          >
            Cut a hole
          </Link>
          <Link
            href="/reference/transforms#linear-pattern"
            className="text-accent hover:text-accent-hover"
          >
            Create a pattern
          </Link>
          <Link
            href="/reference/export#stl"
            className="text-accent hover:text-accent-hover"
          >
            Export to STL
          </Link>
        </div>
      </div>
    </div>
  );
}
