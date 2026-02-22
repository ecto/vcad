import type { Metadata } from "next";
import Link from "next/link";
import {
  Wrench,
  CircleNotch,
  Gear,
  Package,
  Cube,
  Cylinder,
} from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Cookbook",
  description: "Recipe-style tutorials for common vcad patterns",
};

const recipes = [
  {
    id: "mounting-plate",
    title: "Mounting Plate",
    description: "Design a plate with bolt pattern holes",
    icon: Cube,
    difficulty: "Beginner",
    time: "10 min",
  },
  {
    id: "bracket",
    title: "L-Bracket",
    description: "Create an L-shaped mounting bracket",
    icon: Wrench,
    difficulty: "Beginner",
    time: "15 min",
  },
  {
    id: "circular-pattern",
    title: "Circular Hole Pattern",
    description: "Bolt circle and radial patterns",
    icon: CircleNotch,
    difficulty: "Intermediate",
    time: "10 min",
  },
  {
    id: "flanged-hub",
    title: "Flanged Hub",
    description: "Shaft hub with mounting flange",
    icon: Cylinder,
    difficulty: "Intermediate",
    time: "20 min",
  },
  {
    id: "enclosure",
    title: "Electronics Enclosure",
    description: "Box with lid, standoffs, and vent holes",
    icon: Package,
    difficulty: "Intermediate",
    time: "30 min",
  },
  {
    id: "gear",
    title: "Parametric Spur Gear",
    description: "Generate gears with any tooth count",
    icon: Gear,
    difficulty: "Advanced",
    time: "25 min",
  },
];

const difficultyColors = {
  Beginner: "bg-green-500/20 text-green-500",
  Intermediate: "bg-yellow-500/20 text-yellow-500",
  Advanced: "bg-red-500/20 text-red-500",
};

export default function CookbookPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <div className="mb-12">
        <h1 className="text-4xl font-bold mb-4">Cookbook</h1>
        <p className="text-text-muted text-lg max-w-2xl">
          Recipe-style tutorials for common CAD patterns. Each recipe is self-contained
          and includes a working example you can run immediately.
        </p>
      </div>

      <div className="grid gap-4">
        {recipes.map((recipe) => (
          <Link
            key={recipe.id}
            href={`/cookbook/${recipe.id}`}
            className="flex items-center gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
          >
            <div className="p-3 rounded-lg bg-accent/10 text-accent">
              <recipe.icon size={24} />
            </div>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-3">
                <h2 className="font-bold group-hover:text-accent transition-colors">
                  {recipe.title}
                </h2>
                <span
                  className={`px-2 py-0.5 text-xs rounded ${difficultyColors[recipe.difficulty as keyof typeof difficultyColors]}`}
                >
                  {recipe.difficulty}
                </span>
              </div>
              <p className="text-sm text-text-muted mt-1">{recipe.description}</p>
            </div>
            <span className="text-xs text-text-muted flex-shrink-0">
              {recipe.time}
            </span>
          </Link>
        ))}
      </div>

      {/* Coming soon notice */}
      <div className="mt-12 p-6 rounded-lg border border-border bg-surface text-center">
        <h3 className="font-bold mb-2">More recipes coming soon</h3>
        <p className="text-sm text-text-muted">
          Have a pattern you'd like to see documented?{" "}
          <a
            href="https://github.com/ecto/vcad/issues"
            className="text-accent hover:text-accent-hover"
          >
            Open an issue
          </a>
        </p>
      </div>
    </div>
  );
}
