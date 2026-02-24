import type { Metadata } from "next";
import Link from "next/link";
import {
  Wrench,
  CircleNotch,
  Gear,
  Package,
  Cube,
  Cylinder,
  Spiral,
  Drop,
  Circuitry,
  Robot,
  Aperture,
  Nut,
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
    difficulty: "Simple",
    time: "10 min",
  },
  {
    id: "l-bracket",
    title: "L-Bracket",
    description: "Create an L-shaped mounting bracket",
    icon: Wrench,
    difficulty: "Simple",
    time: "15 min",
  },
  {
    id: "flanged-hub",
    title: "Flanged Hub",
    description: "Shaft hub with mounting flange",
    icon: Cylinder,
    difficulty: "Medium",
    time: "20 min",
  },
  {
    id: "enclosure",
    title: "Electronics Enclosure",
    description: "Box with lid, standoffs, and vent holes",
    icon: Package,
    difficulty: "Medium",
    time: "30 min",
  },
  {
    id: "parametric-gear",
    title: "Parametric Gear",
    description: "Generate gears with any tooth count",
    icon: Gear,
    difficulty: "Advanced",
    time: "25 min",
  },
  {
    id: "spoke-wheel",
    title: "Spoke Wheel",
    description: "Multi-spoke wheel with hub and rim",
    icon: Aperture,
    difficulty: "Medium",
    time: "20 min",
  },
  {
    id: "turned-part",
    title: "Turned Part (Lathe)",
    description: "Revolved profile for CNC turning",
    icon: CircleNotch,
    difficulty: "Medium",
    time: "15 min",
  },
  {
    id: "spring",
    title: "Spring (Helix Sweep)",
    description: "Helical spring via swept circle",
    icon: Spiral,
    difficulty: "Medium",
    time: "15 min",
  },
  {
    id: "bottle",
    title: "Bottle (Loft)",
    description: "Multi-section loft with varying profiles",
    icon: Drop,
    difficulty: "Medium",
    time: "20 min",
  },
  {
    id: "pcb-standoff",
    title: "PCB Standoff",
    description: "Threaded standoff for PCB mounting",
    icon: Nut,
    difficulty: "Medium",
    time: "10 min",
  },
  {
    id: "robot-arm",
    title: "Robot Arm Assembly",
    description: "Multi-joint robot arm with revolute joints",
    icon: Robot,
    difficulty: "Advanced",
    time: "45 min",
  },
  {
    id: "enclosure-pcb",
    title: "Enclosure with PCB",
    description: "Full electronics enclosure with integrated PCB layout",
    icon: Circuitry,
    difficulty: "Advanced",
    time: "60 min",
  },
];

const difficultyColors = {
  Simple: "bg-green-500/20 text-green-500",
  Medium: "bg-yellow-500/20 text-yellow-500",
  Advanced: "bg-red-500/20 text-red-500",
};

export default function CookbookPage() {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <div className="mb-12">
        <h1 className="text-4xl font-bold mb-4">Cookbook</h1>
        <p className="text-text-body text-lg max-w-2xl">
          Recipe-style tutorials for common CAD patterns. Each recipe is self-contained
          and builds something real you can export and use.
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
    </div>
  );
}
