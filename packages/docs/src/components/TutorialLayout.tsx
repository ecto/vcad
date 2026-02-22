import Link from "next/link";
import { ArrowLeft, ArrowRight } from "@phosphor-icons/react/dist/ssr";
import { cn } from "@/lib/utils";

interface NavLink {
  href: string;
  title: string;
}

interface TutorialLayoutProps {
  section: "beginner" | "intermediate" | "advanced";
  title: string;
  prev: NavLink | null;
  next: NavLink | null;
  children: React.ReactNode;
}

const sectionColors = {
  beginner: "bg-green-500/20 text-green-500",
  intermediate: "bg-yellow-500/20 text-yellow-500",
  advanced: "bg-red-500/20 text-red-500",
};

export function TutorialLayout({
  section,
  title,
  prev,
  next,
  children,
}: TutorialLayoutProps) {
  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      {/* Breadcrumb */}
      <Link
        href={`/learn/${section}`}
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to {section.charAt(0).toUpperCase() + section.slice(1)}
      </Link>

      {/* Header */}
      <div className="mb-12">
        <div
          className={cn(
            "inline-block px-2 py-1 text-xs font-medium rounded mb-4 uppercase",
            sectionColors[section]
          )}
        >
          {section}
        </div>
        <h1 className="text-4xl font-bold">{title}</h1>
      </div>

      {/* Content */}
      <article className="mb-16">{children}</article>

      {/* Navigation */}
      <nav className="flex items-center justify-between pt-8 border-t border-border">
        {prev ? (
          <Link
            href={prev.href}
            className="flex items-center gap-3 text-text-muted hover:text-text transition-colors group"
          >
            <ArrowLeft
              size={20}
              className="group-hover:-translate-x-1 transition-transform"
            />
            <div className="text-left">
              <div className="text-xs text-text-muted">Previous</div>
              <div className="font-medium">{prev.title}</div>
            </div>
          </Link>
        ) : (
          <div />
        )}

        {next ? (
          <Link
            href={next.href}
            className="flex items-center gap-3 text-text-muted hover:text-text transition-colors group"
          >
            <div className="text-right">
              <div className="text-xs text-text-muted">Next</div>
              <div className="font-medium">{next.title}</div>
            </div>
            <ArrowRight
              size={20}
              className="group-hover:translate-x-1 transition-transform"
            />
          </Link>
        ) : (
          <div />
        )}
      </nav>
    </div>
  );
}
