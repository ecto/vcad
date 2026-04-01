import type { Metadata } from "next";
import Link from "next/link";
import {
  Browser,
  Code,
  Terminal,
  Robot,
  TreeStructure,
  ArrowRight,
} from "@phosphor-icons/react/dist/ssr";
import { getNestedContent } from "@/lib/content";

export const metadata: Metadata = {
  title: "Reference",
  description: "Complete reference documentation for all vcad interfaces",
};

const subcategories = [
  {
    id: "app",
    title: "App",
    description: "Viewport, panels, sketch mode, drawing mode, and all UI features.",
    icon: Browser,
    color: "text-green-500",
  },
  {
    id: "rust",
    title: "Rust API",
    description: "Solid, Part, primitives, booleans, transforms, patterns, and STEP I/O.",
    icon: Code,
    color: "text-blue-500",
  },
  {
    id: "cli",
    title: "CLI",
    description: "Commands, REPL, and printer profiles.",
    icon: Terminal,
    color: "text-yellow-500",
  },
  {
    id: "mcp",
    title: "MCP Tools",
    description: "All MCP server tools for AI agent workflows.",
    icon: Robot,
    color: "text-purple-500",
  },
  {
    id: "format",
    title: "IR & Format",
    description: "Document format, IR operations, and compact format for ML.",
    icon: TreeStructure,
    color: "text-orange-500",
  },
];

export default function ReferenceIndexPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <div className="mb-12">
        <h1 className="text-4xl font-bold mb-4">Reference</h1>
        <p className="text-text-body text-lg max-w-2xl">
          Complete documentation for every vcad interface. Find the section
          that matches what you're looking up.
        </p>
      </div>

      <div className="grid gap-4 sm:grid-cols-2">
        {subcategories.map((sub) => {
          const pages = getNestedContent("reference", sub.id);
          return (
            <Link
              key={sub.id}
              href={`/reference/${sub.id}`}
              className="flex items-start gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
            >
              <div className={`p-3 rounded-lg bg-accent/10 ${sub.color}`}>
                <sub.icon size={24} />
              </div>
              <div className="flex-1">
                <div className="flex items-center justify-between">
                  <h2 className="font-bold group-hover:text-accent transition-colors">
                    {sub.title}
                  </h2>
                  <ArrowRight
                    size={16}
                    className="text-text-muted group-hover:text-accent transition-colors"
                  />
                </div>
                <p className="text-sm text-text-muted mt-1">{sub.description}</p>
                {pages.length > 0 && (
                  <span className="text-xs text-text-muted mt-2 block">
                    {pages.length} pages
                  </span>
                )}
              </div>
            </Link>
          );
        })}
      </div>
    </div>
  );
}
