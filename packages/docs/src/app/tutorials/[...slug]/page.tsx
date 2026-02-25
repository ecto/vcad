import type { Metadata } from "next";
import { notFound } from "next/navigation";
import Link from "next/link";
import { ArrowLeft, ArrowRight, Circle } from "@phosphor-icons/react/dist/ssr";
import { MDXRemote } from "next-mdx-remote/rsc";
import {
  getContentBySlugResolved,
  getNestedContentPaths,
  getNestedContent,
  getNavigation,
} from "@/lib/content";
import { getMdxComponents, mdxOptions } from "@/lib/mdx-components";
import { cn } from "@/lib/utils";

interface PageProps {
  params: Promise<{ slug: string[] }>;
}

const trackMeta: Record<string, { title: string; color: string; badgeClass: string }> = {
  app: { title: "App", color: "text-green-500", badgeClass: "bg-green-500/20 text-green-500" },
  rust: { title: "Rust", color: "text-blue-500", badgeClass: "bg-blue-500/20 text-blue-500" },
  cli: { title: "CLI", color: "text-yellow-500", badgeClass: "bg-yellow-500/20 text-yellow-500" },
  mcp: { title: "MCP / AI", color: "text-purple-500", badgeClass: "bg-purple-500/20 text-purple-500" },
};

export async function generateStaticParams() {
  const paths = getNestedContentPaths("tutorials");

  // Generate params for individual pages
  const pageParams = paths.map(({ subcategory, slug }) => ({
    slug: [subcategory, slug],
  }));

  // Generate params for subcategory index pages
  const subcategories = [...new Set(paths.map((p) => p.subcategory))];
  const indexParams = subcategories.map((sub) => ({
    slug: [sub],
  }));

  return [...indexParams, ...pageParams];
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params;

  if (slug.length === 1) {
    const track = trackMeta[slug[0]!];
    return {
      title: track ? `${track.title} Tutorials` : "Tutorials",
    };
  }

  const slugPath = slug.join("/");
  const data = getContentBySlugResolved("tutorials", slugPath);
  if (!data) return { title: "Not Found" };
  return { title: data.meta.title, description: data.meta.description };
}

export default async function TutorialPage({ params }: PageProps) {
  const { slug } = await params;

  // Subcategory index page (e.g., /tutorials/app)
  if (slug.length === 1) {
    const subcategory = slug[0]!;
    const track = trackMeta[subcategory];
    if (!track) notFound();

    const pages = getNestedContent("tutorials", subcategory);

    return (
      <div className="max-w-3xl mx-auto px-8 py-16">
        <Link
          href="/tutorials"
          className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
        >
          <ArrowLeft size={14} />
          Back to Tutorials
        </Link>

        <div className="mb-12">
          <div className={cn("inline-block px-2 py-1 text-xs font-medium rounded mb-4 uppercase", track.badgeClass)}>
            {track.title}
          </div>
          <h1 className="text-4xl font-bold mb-4">{track.title} Tutorials</h1>
          <p className="text-text-muted text-lg">
            {pages.length} tutorials in this track.
          </p>
        </div>

        <div className="space-y-4">
          {pages.map((page, idx) => (
            <Link
              key={page.slug}
              href={`/tutorials/${subcategory}/${page.slug}`}
              className="flex items-center gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
            >
              <div className="flex-shrink-0">
                <Circle size={24} className="text-text-muted" />
              </div>
              <div className="flex-1 min-w-0">
                <div className="flex items-center gap-2">
                  <span className="text-xs text-text-muted">
                    {String(idx + 1).padStart(2, "0")}
                  </span>
                  <h3 className="font-medium group-hover:text-accent transition-colors">
                    {page.meta.title}
                  </h3>
                </div>
                {page.meta.description && (
                  <p className="text-sm text-text-muted truncate">
                    {page.meta.description}
                  </p>
                )}
              </div>
              <ArrowRight
                size={16}
                className="text-text-muted group-hover:text-accent transition-colors flex-shrink-0"
              />
            </Link>
          ))}
        </div>

        {pages.length > 0 && (
          <div className="mt-12 text-center">
            <Link
              href={`/tutorials/${subcategory}/${pages[0]!.slug}`}
              className={cn(
                "inline-flex items-center gap-2 px-6 py-3 text-white rounded-lg font-medium transition-colors",
                subcategory === "app" ? "bg-green-500 hover:bg-green-600" :
                subcategory === "rust" ? "bg-blue-500 hover:bg-blue-600" :
                subcategory === "cli" ? "bg-yellow-500 hover:bg-yellow-600" :
                "bg-purple-500 hover:bg-purple-600"
              )}
            >
              Start Learning
              <ArrowRight size={18} />
            </Link>
          </div>
        )}
      </div>
    );
  }

  // Individual tutorial page (e.g., /tutorials/app/first-part)
  if (slug.length !== 2) notFound();

  const subcategory = slug[0]!;
  const pageSlug = slug[1]!;
  const track = trackMeta[subcategory];
  if (!track) notFound();

  const slugPath = slug.join("/");
  const data = getContentBySlugResolved("tutorials", slugPath);
  if (!data) notFound();

  const sectionPages = getNestedContent("tutorials", subcategory);
  const { prev, next } = getNavigation(sectionPages, pageSlug, `/tutorials/${subcategory}`);

  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <Link
        href={`/tutorials/${subcategory}`}
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to {track.title} Tutorials
      </Link>

      <div className="mb-12">
        <div className={cn("inline-block px-2 py-1 text-xs font-medium rounded mb-4 uppercase", track.badgeClass)}>
          {track.title}
        </div>
        <h1 className="text-4xl font-bold">{data.meta.title}</h1>
      </div>

      <article className="mb-16">
        <div className="prose">
          <MDXRemote source={data.content} components={getMdxComponents()} options={mdxOptions} />
        </div>
      </article>

      <nav className="flex items-center justify-between pt-8 border-t border-border">
        {prev ? (
          <Link
            href={prev.href}
            className="flex items-center gap-3 text-text-muted hover:text-text transition-colors group"
          >
            <ArrowLeft size={20} className="group-hover:-translate-x-1 transition-transform" />
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
            <ArrowRight size={20} className="group-hover:translate-x-1 transition-transform" />
          </Link>
        ) : (
          <div />
        )}
      </nav>
    </div>
  );
}
