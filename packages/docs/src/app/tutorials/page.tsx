import type { Metadata } from "next";
import Link from "next/link";
import {
  Browser,
  Terminal,
  Code,
  Robot,
  ArrowRight,
} from "@phosphor-icons/react/dist/ssr";
import { cn } from "@/lib/utils";
import { getNestedContent } from "@/lib/content";

export const metadata: Metadata = {
  title: "Tutorials",
  description: "Step-by-step tutorials for every vcad interface",
};

const tracks = [
  {
    id: "app",
    title: "App Tutorials",
    description: "Learn the vcad web app from your first part to advanced workflows.",
    icon: Browser,
    color: "text-green-500",
    bgColor: "bg-green-500/10",
    borderColor: "border-green-500/30 hover:border-green-500/60",
    count: 11,
  },
  {
    id: "rust",
    title: "Rust Tutorials",
    description: "Build parametric CAD models with the Rust library.",
    icon: Code,
    color: "text-blue-500",
    bgColor: "bg-blue-500/10",
    borderColor: "border-blue-500/30 hover:border-blue-500/60",
    count: 7,
  },
  {
    id: "cli",
    title: "CLI Tutorials",
    description: "Master the command-line interface and scripting.",
    icon: Terminal,
    color: "text-yellow-500",
    bgColor: "bg-yellow-500/10",
    borderColor: "border-yellow-500/30 hover:border-yellow-500/60",
    count: 4,
  },
  {
    id: "mcp",
    title: "MCP / AI Tutorials",
    description: "Build AI agent workflows with the MCP server.",
    icon: Robot,
    color: "text-purple-500",
    bgColor: "bg-purple-500/10",
    borderColor: "border-purple-500/30 hover:border-purple-500/60",
    count: 6,
  },
];

export default function TutorialsPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <h1 className="text-4xl font-bold mb-4">Tutorials</h1>
      <p className="text-text-body mb-12 max-w-2xl">
        Step-by-step tutorials organized by interface. Pick the track that matches
        how you want to use vcad.
      </p>

      <div className="space-y-6">
        {tracks.map((track) => {
          const pages = getNestedContent("tutorials", track.id);
          return (
            <Link
              key={track.id}
              href={`/tutorials/${track.id}`}
              className={cn(
                "block rounded-lg border p-6 transition-all",
                track.bgColor,
                track.borderColor
              )}
            >
              <div className="flex items-start justify-between mb-4">
                <div className={cn("flex items-center gap-3", track.color)}>
                  <track.icon size={24} weight="fill" />
                  <h2 className="text-xl font-bold">{track.title}</h2>
                </div>
                <ArrowRight size={20} className={cn("mt-1", track.color)} />
              </div>
              <p className="text-text-muted mb-4">{track.description}</p>
              {pages.length > 0 && (
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                  {pages.slice(0, 4).map((page) => (
                    <div key={page.slug} className="text-sm">
                      <span className="text-text font-medium">{page.meta.title}</span>
                    </div>
                  ))}
                  {pages.length > 4 && (
                    <div className="text-sm text-text-muted">
                      +{pages.length - 4} more
                    </div>
                  )}
                </div>
              )}
            </Link>
          );
        })}
      </div>
    </div>
  );
}
