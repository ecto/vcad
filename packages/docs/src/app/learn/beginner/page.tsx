import type { Metadata } from "next";
import Link from "next/link";
import { ArrowLeft, ArrowRight, CheckCircle, Circle } from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Beginner",
  description: "Start your vcad journey with the fundamentals",
};

const lessons = [
  {
    id: "hello-cube",
    title: "Hello Cube",
    description: "Create your first 3D shape using vcad primitives",
    duration: "5 min",
    completed: false,
  },
  {
    id: "transforms",
    title: "Basic Transforms",
    description: "Learn to translate, rotate, and scale your parts",
    duration: "8 min",
    completed: false,
  },
  {
    id: "first-hole",
    title: "Your First Hole",
    description: "Use boolean difference to create holes and cutouts",
    duration: "10 min",
    completed: false,
  },
  {
    id: "export",
    title: "Export to STL",
    description: "Save your model for 3D printing or CAM",
    duration: "5 min",
    completed: false,
  },
];

export default function BeginnerPage() {
  return (
    <div className="max-w-3xl mx-auto px-8 py-16">
      {/* Breadcrumb */}
      <Link
        href="/learn"
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to Learning Paths
      </Link>

      {/* Header */}
      <div className="mb-12">
        <div className="inline-block px-2 py-1 text-xs font-medium bg-green-500/20 text-green-500 rounded mb-4">
          BEGINNER
        </div>
        <h1 className="text-4xl font-bold mb-4">Getting Started</h1>
        <p className="text-text-muted text-lg">
          Learn the fundamentals of vcad. By the end of this path, you'll be able to
          create basic parts and export them for 3D printing.
        </p>
      </div>

      {/* Progress */}
      <div className="flex items-center gap-4 mb-8 pb-8 border-b border-border">
        <div className="text-sm text-text-muted">
          0 of {lessons.length} completed
        </div>
        <div className="flex-1 h-2 bg-border rounded-full overflow-hidden">
          <div className="h-full bg-green-500 rounded-full" style={{ width: "0%" }} />
        </div>
      </div>

      {/* Lessons */}
      <div className="space-y-4">
        {lessons.map((lesson, idx) => (
          <Link
            key={lesson.id}
            href={`/learn/beginner/${lesson.id}`}
            className="flex items-center gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
          >
            <div className="flex-shrink-0">
              {lesson.completed ? (
                <CheckCircle size={24} weight="fill" className="text-green-500" />
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

      {/* Start button */}
      <div className="mt-12 text-center">
        <Link
          href="/learn/beginner/hello-cube"
          className="inline-flex items-center gap-2 px-6 py-3 bg-green-500 hover:bg-green-600 text-white rounded-lg font-medium transition-colors"
        >
          Start Learning
          <ArrowRight size={18} />
        </Link>
      </div>
    </div>
  );
}
