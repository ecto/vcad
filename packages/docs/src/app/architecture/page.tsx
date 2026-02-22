import type { Metadata } from "next";
import Link from "next/link";
import {
  Graph,
  Tree,
  Package,
  FileCode,
  ArrowRight,
} from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Architecture",
  description: "Deep dives into vcad internals - radical transparency",
};

const topics = [
  {
    id: "booleans",
    title: "How Booleans Work",
    description:
      "Deep dive into CSG boolean operations. How Manifold achieves robust mesh booleans, BSP trees, and mesh repair.",
    icon: Graph,
    readTime: "15 min",
  },
  {
    id: "ir",
    title: "The IR Format",
    description:
      "Understanding the intermediate representation. Why a DAG, serialization format, versioning strategy.",
    icon: Tree,
    readTime: "10 min",
  },
  {
    id: "wasm",
    title: "WASM Pipeline",
    description:
      "How @vcad/engine compiles and runs. The Rust → WASM → JavaScript bridge and memory management.",
    icon: Package,
    readTime: "12 min",
  },
  {
    id: "exports",
    title: "Export Formats",
    description:
      "STL vs GLTF vs STEP tradeoffs. When to use each format, limitations, and conversion strategies.",
    icon: FileCode,
    readTime: "8 min",
  },
];

export default function ArchitecturePage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <div className="mb-12">
        <h1 className="text-4xl font-bold mb-4">Architecture</h1>
        <p className="text-text-muted text-lg max-w-2xl">
          Radical transparency. These deep-dives explain how vcad works under the hood,
          the design decisions we made, and the tradeoffs involved.
        </p>
      </div>

      <div className="space-y-4">
        {topics.map((topic) => (
          <Link
            key={topic.id}
            href={`/architecture/${topic.id}`}
            className="flex items-start gap-4 p-5 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
          >
            <div className="p-3 rounded-lg bg-accent/10 text-accent">
              <topic.icon size={24} />
            </div>
            <div className="flex-1 min-w-0">
              <h2 className="text-lg font-bold group-hover:text-accent transition-colors">
                {topic.title}
              </h2>
              <p className="text-text-muted mt-1">{topic.description}</p>
              <span className="inline-block mt-2 text-xs text-text-muted">
                {topic.readTime} read
              </span>
            </div>
            <ArrowRight
              size={20}
              className="text-text-muted group-hover:text-accent group-hover:translate-x-1 transition-all mt-1"
            />
          </Link>
        ))}
      </div>

      {/* Additional resources */}
      <div className="mt-16 pt-8 border-t border-border">
        <h2 className="text-xl font-bold mb-6">Additional Resources</h2>
        <div className="grid grid-cols-1 sm:grid-cols-2 gap-4">
          <a
            href="https://github.com/elalish/manifold"
            target="_blank"
            rel="noopener noreferrer"
            className="p-4 rounded-lg border border-border hover:border-text-muted bg-surface transition-all"
          >
            <h3 className="font-medium">Manifold Geometry Kernel</h3>
            <p className="text-sm text-text-muted mt-1">
              The underlying boolean engine powering vcad
            </p>
          </a>
          <a
            href="https://docs.rs/vcad"
            target="_blank"
            rel="noopener noreferrer"
            className="p-4 rounded-lg border border-border hover:border-text-muted bg-surface transition-all"
          >
            <h3 className="font-medium">Rust API Documentation</h3>
            <p className="text-sm text-text-muted mt-1">
              Complete API reference on docs.rs
            </p>
          </a>
        </div>
      </div>
    </div>
  );
}
