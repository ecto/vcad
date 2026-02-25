import Link from "next/link";
import {
  Browser,
  Code,
  Terminal,
  Robot,
} from "@phosphor-icons/react/dist/ssr";
import { cn } from "@/lib/utils";

const paths = [
  {
    id: "app",
    icon: Browser,
    color: "text-green-500",
    bgColor: "bg-green-500/10",
    borderColor: "border-green-500/20",
    title: "use the app",
    href: "/tutorials/app/first-part",
    lessons: [
      { title: "Your First Part", href: "/tutorials/app/first-part" },
      { title: "Combining Shapes", href: "/tutorials/app/booleans" },
      { title: "Sketch & Extrude", href: "/tutorials/app/sketch-extrude" },
      { title: "Assemblies", href: "/tutorials/app/assembly" },
    ],
  },
  {
    id: "rust",
    icon: Code,
    color: "text-blue-500",
    bgColor: "bg-blue-500/10",
    borderColor: "border-blue-500/20",
    title: "write rust",
    href: "/tutorials/rust/hello-cube",
    lessons: [
      { title: "Hello Cube", href: "/tutorials/rust/hello-cube" },
      { title: "Transforms & Booleans", href: "/tutorials/rust/transforms-booleans" },
      { title: "Parametric Functions", href: "/tutorials/rust/parametric" },
      { title: "STEP Import/Export", href: "/tutorials/rust/step" },
    ],
  },
  {
    id: "cli",
    icon: Terminal,
    color: "text-yellow-500",
    bgColor: "bg-yellow-500/10",
    borderColor: "border-yellow-500/20",
    title: "use the cli",
    href: "/tutorials/cli/quickstart",
    lessons: [
      { title: "CLI Quick Start", href: "/tutorials/cli/quickstart" },
      { title: "Interactive REPL", href: "/tutorials/cli/repl" },
      { title: "Terminal UI", href: "/tutorials/cli/tui" },
      { title: "Scripting", href: "/tutorials/cli/scripting" },
    ],
  },
  {
    id: "ai",
    icon: Robot,
    color: "text-purple-500",
    bgColor: "bg-purple-500/10",
    borderColor: "border-purple-500/20",
    title: "build with ai",
    href: "/tutorials/mcp/setup",
    lessons: [
      { title: "MCP Setup", href: "/tutorials/mcp/setup" },
      { title: "Create Geometry", href: "/tutorials/mcp/create" },
      { title: "Loon Language", href: "/tutorials/mcp/loon" },
      { title: "Physics Simulation", href: "/tutorials/mcp/physics" },
    ],
  },
];

export function LearningPaths() {
  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-6">
      {paths.map((path) => (
        <div
          key={path.id}
          className={cn(
            "rounded-lg border p-5",
            path.borderColor,
            path.bgColor
          )}
        >
          <div className={cn("flex items-center gap-2 mb-4", path.color)}>
            <path.icon size={20} weight="fill" />
            <h3 className="font-bold">{path.title}</h3>
          </div>
          <ul className="space-y-2">
            {path.lessons.map((lesson) => (
              <li key={lesson.href}>
                <Link
                  href={lesson.href}
                  className="block text-sm text-text-muted hover:text-text transition-colors"
                >
                  {lesson.title}
                </Link>
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}
