import type { Metadata } from "next";
import Link from "next/link";
import {
  Browser,
  Code,
  Terminal,
  Robot,
  ArrowRight,
} from "@phosphor-icons/react/dist/ssr";
import { cn } from "@/lib/utils";

export const metadata: Metadata = {
  title: "Get Started",
  description: "Choose your path into vcad",
};

const paths = [
  {
    title: "Use the App",
    description: "Open vcad.io and start modeling in your browser. No install needed.",
    icon: Browser,
    href: "/tutorials/app/first-part",
    color: "text-green-500",
    bgColor: "bg-green-500/10",
    borderColor: "border-green-500/30 hover:border-green-500/60",
  },
  {
    title: "Write Rust",
    description: "Add vcad to your Rust project and build parametric parts in code.",
    icon: Code,
    href: "/tutorials/rust/hello-cube",
    color: "text-blue-500",
    bgColor: "bg-blue-500/10",
    borderColor: "border-blue-500/30 hover:border-blue-500/60",
  },
  {
    title: "Use the CLI",
    description: "Install the vcad CLI for batch processing and scripting.",
    icon: Terminal,
    href: "/tutorials/cli/quickstart",
    color: "text-yellow-500",
    bgColor: "bg-yellow-500/10",
    borderColor: "border-yellow-500/30 hover:border-yellow-500/60",
  },
  {
    title: "Build with AI",
    description: "Configure the MCP server for Claude, Cursor, or other AI agents.",
    icon: Robot,
    href: "/tutorials/mcp/setup",
    color: "text-purple-500",
    bgColor: "bg-purple-500/10",
    borderColor: "border-purple-500/30 hover:border-purple-500/60",
  },
];

export default function StartPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <h1 className="text-4xl font-bold mb-4">Get Started</h1>
      <p className="text-text-body mb-12 max-w-2xl">
        vcad is a web app, Rust library, CLI tool, and MCP server.
        Pick the interface that fits your workflow.
      </p>

      <div className="grid gap-4 sm:grid-cols-2">
        {paths.map((path) => (
          <Link
            key={path.title}
            href={path.href}
            className={cn(
              "flex flex-col gap-3 p-5 rounded-lg border transition-all group",
              path.bgColor,
              path.borderColor
            )}
          >
            <div className={cn("flex items-center gap-3", path.color)}>
              <path.icon size={24} weight="fill" />
              <h2 className="font-bold">{path.title}</h2>
            </div>
            <p className="text-sm text-text-muted flex-1">{path.description}</p>
            <div className={cn("flex items-center gap-1 text-sm font-medium", path.color)}>
              Start tutorial
              <ArrowRight size={16} className="group-hover:translate-x-1 transition-transform" />
            </div>
          </Link>
        ))}
      </div>

      <div className="mt-12 space-y-4">
        <Link
          href="/start/install"
          className="block p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all"
        >
          <h3 className="font-bold">Install & Setup</h3>
          <p className="text-sm text-text-muted mt-1">
            All installation options: vcad.io, cargo, npm, MCP server config
          </p>
        </Link>
        <Link
          href="/start/concepts"
          className="block p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all"
        >
          <h3 className="font-bold">Core Concepts</h3>
          <p className="text-sm text-text-muted mt-1">
            Solids, BRep, parametric modeling, coordinate system, units
          </p>
        </Link>
      </div>
    </div>
  );
}
