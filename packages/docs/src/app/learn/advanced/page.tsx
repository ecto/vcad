import type { Metadata } from "next";
import Link from "next/link";
import { ArrowLeft, ArrowRight, CheckCircle, Circle } from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Advanced",
  description: "Master parametric design and kernel internals",
};

const lessons = [
  {
    id: "parametric",
    title: "Parametric Design",
    description: "Build configuration-driven models with variables",
    duration: "20 min",
    completed: false,
  },
  {
    id: "step",
    title: "STEP Import/Export",
    description: "Exchange models with industry-standard CAD software",
    duration: "15 min",
    completed: false,
  },
  {
    id: "kernel",
    title: "Kernel Internals",
    description: "Understand how the geometry engine works",
    duration: "25 min",
    completed: false,
  },
  {
    id: "contributing",
    title: "Contributing",
    description: "Help build vcad - code, docs, and examples",
    duration: "10 min",
    completed: false,
  },
];

export default function AdvancedPage() {
  return (
    <div className="max-w-3xl mx-auto px-8 py-16">
      <Link
        href="/learn"
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to Learning Paths
      </Link>

      <div className="mb-12">
        <div className="inline-block px-2 py-1 text-xs font-medium bg-red-500/20 text-red-500 rounded mb-4">
          ADVANCED
        </div>
        <h1 className="text-4xl font-bold mb-4">Mastery</h1>
        <p className="text-text-muted text-lg">
          Dive deep into parametric design patterns and understand how vcad works
          under the hood. These topics will make you a power user.
        </p>
      </div>

      <div className="flex items-center gap-4 mb-8 pb-8 border-b border-border">
        <div className="text-sm text-text-muted">
          0 of {lessons.length} completed
        </div>
        <div className="flex-1 h-2 bg-border rounded-full overflow-hidden">
          <div className="h-full bg-red-500 rounded-full" style={{ width: "0%" }} />
        </div>
      </div>

      <div className="space-y-4">
        {lessons.map((lesson, idx) => (
          <Link
            key={lesson.id}
            href={`/learn/advanced/${lesson.id}`}
            className="flex items-center gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
          >
            <div className="flex-shrink-0">
              {lesson.completed ? (
                <CheckCircle size={24} weight="fill" className="text-red-500" />
              ) : (
                <Circle size={24} className="text-text-muted" />
              )}
            </div>
            <div className="flex-1 min-w-0">
              <div className="flex items-center gap-2">
                <span className="text-xs text-text-muted">
                  {String(idx + 1).padStart(2, "0")}
                </span>
                <h3 className="font-medium group-hover:text-accent transition-colors">
                  {lesson.title}
                </h3>
              </div>
              <p className="text-sm text-text-muted truncate">
                {lesson.description}
              </p>
            </div>
            <div className="flex items-center gap-4 flex-shrink-0">
              <span className="text-xs text-text-muted">{lesson.duration}</span>
              <ArrowRight
                size={16}
                className="text-text-muted group-hover:text-accent transition-colors"
              />
            </div>
          </Link>
        ))}
      </div>

      <div className="mt-12 text-center">
        <Link
          href="/learn/advanced/parametric"
          className="inline-flex items-center gap-2 px-6 py-3 bg-red-500 hover:bg-red-600 text-white rounded-lg font-medium transition-colors"
        >
          Start Learning
          <ArrowRight size={18} />
        </Link>
      </div>
    </div>
  );
}
