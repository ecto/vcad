import type { Metadata } from "next";
import Link from "next/link";
import {
  Graph,
  Tree,
  Package,
  FileCode,
  Cpu,
  GridFour,
  Cube,
  Eye,
  Atom,
  GitBranch,
  Users,
  ClockCounterClockwise,
  ArrowRight,
} from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Architecture",
  description: "Deep dives into vcad internals",
};

const topics = [
  {
    id: "overview",
    title: "System Overview",
    description: "Crate map, package map, data flow from IR to viewport.",
    icon: GridFour,
    readTime: "10 min",
  },
  {
    id: "kernel",
    title: "BRep Kernel",
    description: "Half-edge topology, slotmap arenas, surface types, exact predicates.",
    icon: Cube,
    readTime: "15 min",
  },
  {
    id: "booleans",
    title: "Boolean Pipeline",
    description: "AABB filter, surface-surface intersection, face classification, sewing.",
    icon: Graph,
    readTime: "15 min",
  },
  {
    id: "constraints",
    title: "Constraint Solver",
    description: "Levenberg-Marquardt, residual functions, convergence criteria.",
    icon: GitBranch,
    readTime: "12 min",
  },
  {
    id: "wasm",
    title: "WASM Pipeline",
    description: "Rust to WASM to JavaScript bridge, memory management.",
    icon: Package,
    readTime: "12 min",
  },
  {
    id: "ir",
    title: "IR Format Design",
    description: "DAG-based IR, serialization, compact format, versioning.",
    icon: Tree,
    readTime: "10 min",
  },
  {
    id: "tessellation",
    title: "Tessellation",
    description: "BRep to triangle mesh, per-surface strategies, quality tradeoffs.",
    icon: GridFour,
    readTime: "10 min",
  },
  {
    id: "ray-tracing",
    title: "Ray Tracing",
    description: "Analytic ray-surface intersection, BVH/SAH, trimmed surfaces, WebGPU.",
    icon: Eye,
    readTime: "15 min",
  },
  {
    id: "physics",
    title: "Physics Engine",
    description: "Rapier3D integration, BRep-to-physics, joint mapping, gym interface.",
    icon: Atom,
    readTime: "12 min",
  },
  {
    id: "exports",
    title: "Export Formats",
    description: "STL, GLB, STEP AP214, DXF, URDF. Tradeoffs and limitations.",
    icon: FileCode,
    readTime: "8 min",
  },
  {
    id: "contributing",
    title: "Contributing Guide",
    description: "Dev environment, tests, conventions, PR process.",
    icon: Users,
    readTime: "8 min",
  },
  {
    id: "changelog",
    title: "Changelog",
    description: "Entry format, when to add entries, schema validation.",
    icon: ClockCounterClockwise,
    readTime: "5 min",
  },
];

export default function ArchitecturePage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <div className="mb-12">
        <h1 className="text-4xl font-bold mb-4">Architecture</h1>
        <p className="text-text-body text-lg max-w-2xl">
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
    </div>
  );
}
