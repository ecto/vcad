import type { Metadata } from "next";
import Link from "next/link";
import { Rocket, Lightning, Fire, ArrowRight } from "@phosphor-icons/react/dist/ssr";
import { cn } from "@/lib/utils";

export const metadata: Metadata = {
  title: "Learn",
  description: "Learn vcad from beginner to advanced with interactive tutorials",
};

const paths = [
  {
    level: "beginner",
    title: "Beginner",
    description: "Start here. Learn the fundamentals of vcad.",
    icon: Rocket,
    color: "text-green-500",
    bgColor: "bg-green-500/10",
    borderColor: "border-green-500/30 hover:border-green-500/60",
    href: "/learn/beginner",
    lessons: [
      { title: "Hello Cube", description: "Create your first 3D shape" },
      { title: "Basic Transforms", description: "Move, rotate, and scale" },
      { title: "Your First Hole", description: "Boolean difference operations" },
      { title: "Export to STL", description: "Save for 3D printing" },
    ],
  },
  {
    level: "intermediate",
    title: "Intermediate",
    description: "Build real parts with patterns and assemblies.",
    icon: Lightning,
    color: "text-yellow-500",
    bgColor: "bg-yellow-500/10",
    borderColor: "border-yellow-500/30 hover:border-yellow-500/60",
    href: "/learn/intermediate",
    lessons: [
      { title: "Bolt Patterns", description: "Linear and circular arrays" },
      { title: "Multi-Part Assembly", description: "Combine multiple parts" },
      { title: "Materials & GLB", description: "PBR materials and visualization" },
      { title: "Scene Composition", description: "Build complex scenes" },
    ],
  },
  {
    level: "advanced",
    title: "Advanced",
    description: "Master parametric design and kernel internals.",
    icon: Fire,
    color: "text-red-500",
    bgColor: "bg-red-500/10",
    borderColor: "border-red-500/30 hover:border-red-500/60",
    href: "/learn/advanced",
    lessons: [
      { title: "Parametric Design", description: "Configuration-driven models" },
      { title: "STEP Import/Export", description: "Industry-standard CAD exchange" },
      { title: "Kernel Internals", description: "How the geometry engine works" },
      { title: "Contributing", description: "Help build vcad" },
    ],
  },
];

export default function LearnPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <h1 className="text-4xl font-bold mb-4">Learn vcad</h1>
      <p className="text-text-muted mb-12 max-w-2xl">
        Master parametric CAD from first principles. Each path builds on the previous,
        taking you from zero to creating production-ready parts.
      </p>

      <div className="space-y-8">
        {paths.map((path) => (
          <Link
            key={path.level}
            href={path.href}
            className={cn(
              "block rounded-lg border p-6 transition-all",
              path.bgColor,
              path.borderColor
            )}
          >
            <div className="flex items-start justify-between mb-4">
              <div className={cn("flex items-center gap-3", path.color)}>
                <path.icon size={24} weight="fill" />
                <h2 className="text-xl font-bold">{path.title}</h2>
              </div>
              <ArrowRight size={20} className={cn("mt-1", path.color)} />
            </div>

            <p className="text-text-muted mb-6">{path.description}</p>

            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              {path.lessons.map((lesson, idx) => (
                <div key={idx} className="text-sm">
                  <span className="text-text font-medium">{lesson.title}</span>
                  <span className="text-text-muted"> — {lesson.description}</span>
                </div>
              ))}
            </div>
          </Link>
        ))}
      </div>
    </div>
  );
}
