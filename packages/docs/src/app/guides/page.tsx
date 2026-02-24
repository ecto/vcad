import type { Metadata } from "next";
import Link from "next/link";
import {
  Cube,
  Gear,
  Factory,
  Circuitry,
  Robot,
  ArrowRight,
} from "@phosphor-icons/react/dist/ssr";
import { cn } from "@/lib/utils";
import { getNestedContent } from "@/lib/content";

export const metadata: Metadata = {
  title: "Guides",
  description: "Task-oriented guides for vcad workflows",
};

const categories = [
  {
    id: "modeling",
    title: "Modeling",
    description: "Deep dives into sketches, sweeps, fillets, patterns, and more.",
    icon: Cube,
    color: "text-blue-500",
    bgColor: "bg-blue-500/10",
    borderColor: "border-blue-500/30 hover:border-blue-500/60",
  },
  {
    id: "assembly",
    title: "Assembly & Motion",
    description: "Parts, instances, joints, kinematics, and clash detection.",
    icon: Gear,
    color: "text-green-500",
    bgColor: "bg-green-500/10",
    borderColor: "border-green-500/30 hover:border-green-500/60",
  },
  {
    id: "mfg",
    title: "Manufacturing",
    description: "3D printing, CNC, laser cutting, and export formats.",
    icon: Factory,
    color: "text-yellow-500",
    bgColor: "bg-yellow-500/10",
    borderColor: "border-yellow-500/30 hover:border-yellow-500/60",
  },
  {
    id: "electronics",
    title: "Electronics",
    description: "Schematic design, PCB layout, DRC, and fabrication export.",
    icon: Circuitry,
    color: "text-red-500",
    bgColor: "bg-red-500/10",
    borderColor: "border-red-500/30 hover:border-red-500/60",
  },
  {
    id: "ai",
    title: "AI & Automation",
    description: "Text-to-CAD, MCP workflows, RL training, and Loon language.",
    icon: Robot,
    color: "text-purple-500",
    bgColor: "bg-purple-500/10",
    borderColor: "border-purple-500/30 hover:border-purple-500/60",
  },
];

export default function GuidesPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <h1 className="text-4xl font-bold mb-4">Guides</h1>
      <p className="text-text-body mb-12 max-w-2xl">
        Task-oriented guides for specific workflows. Pick the topic you need.
      </p>

      <div className="space-y-4">
        {categories.map((cat) => {
          const pages = getNestedContent("guides", cat.id);
          return (
            <Link
              key={cat.id}
              href={`/guides/${cat.id}`}
              className={cn(
                "flex items-start gap-4 p-5 rounded-lg border transition-all group",
                cat.bgColor,
                cat.borderColor
              )}
            >
              <div className={cn("p-3 rounded-lg", cat.color)}>
                <cat.icon size={24} weight="fill" />
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center justify-between">
                  <h2 className="text-lg font-bold">{cat.title}</h2>
                  <ArrowRight
                    size={20}
                    className="text-text-muted group-hover:text-accent group-hover:translate-x-1 transition-all"
                  />
                </div>
                <p className="text-text-muted mt-1">{cat.description}</p>
                {pages.length > 0 && (
                  <span className="inline-block mt-2 text-xs text-text-muted">
                    {pages.length} guides
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
