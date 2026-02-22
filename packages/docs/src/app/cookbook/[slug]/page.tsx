import type { Metadata } from "next";
import { notFound } from "next/navigation";
import Link from "next/link";
import { ArrowLeft, ArrowRight } from "@phosphor-icons/react/dist/ssr";
import { MDXRemote } from "next-mdx-remote/rsc";
import {
  getContentBySlugResolved,
  getContentPaths,
  getAllContent,
  getNavigation,
} from "@/lib/content";
import { getMdxComponents } from "@/lib/mdx-components";
import { cn } from "@/lib/utils";

interface PageProps {
  params: Promise<{ slug: string }>;
}

export async function generateStaticParams() {
  const paths = getContentPaths("cookbook");
  return paths.map((slug) => ({ slug }));
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params;
  const data = getContentBySlugResolved("cookbook", slug);

  if (!data) {
    return { title: "Not Found" };
  }

  return {
    title: data.meta.title,
    description: data.meta.description,
  };
}

const difficultyColors = {
  Beginner: "bg-green-500/20 text-green-500",
  Intermediate: "bg-yellow-500/20 text-yellow-500",
  Advanced: "bg-red-500/20 text-red-500",
};

export default async function CookbookRecipePage({ params }: PageProps) {
  const { slug } = await params;
  const data = getContentBySlugResolved("cookbook", slug);

  if (!data) {
    notFound();
  }

  // Get all recipes for navigation
  const allRecipes = getAllContent("cookbook");
  const { prev, next } = getNavigation(allRecipes, slug, "/cookbook");

  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      {/* Breadcrumb */}
      <Link
        href="/cookbook"
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to Cookbook
      </Link>

      {/* Header */}
      <div className="mb-12">
        <div className="flex items-center gap-3 mb-4">
          {data.meta.difficulty && (
            <span
              className={cn(
                "px-2 py-1 text-xs font-medium rounded uppercase",
                difficultyColors[data.meta.difficulty]
              )}
            >
              {data.meta.difficulty}
            </span>
          )}
          {data.meta.time && (
            <span className="text-xs text-text-muted">{data.meta.time}</span>
          )}
        </div>
        <h1 className="text-4xl font-bold mb-4">{data.meta.title}</h1>
        <p className="text-text-muted text-lg">{data.meta.description}</p>
      </div>

      {/* Content */}
      <article className="mb-16">
        <MDXRemote source={data.content} components={getMdxComponents()} />
      </article>

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
