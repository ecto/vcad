import type { Metadata } from "next";
import { notFound } from "next/navigation";
import Link from "next/link";
import { ArrowLeft, ArrowRight } from "@phosphor-icons/react/dist/ssr";
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

const subcategoryMeta: Record<string, { title: string; badgeClass: string }> = {
  app: { title: "App", badgeClass: "bg-green-500/20 text-green-500" },
  rust: { title: "Rust API", badgeClass: "bg-blue-500/20 text-blue-500" },
  cli: { title: "CLI", badgeClass: "bg-yellow-500/20 text-yellow-500" },
  mcp: { title: "MCP Tools", badgeClass: "bg-purple-500/20 text-purple-500" },
  format: { title: "IR & Format", badgeClass: "bg-orange-500/20 text-orange-500" },
};

export async function generateStaticParams() {
  const paths = getNestedContentPaths("reference");
  const pageParams = paths.map(({ subcategory, slug }) => ({
    slug: [subcategory, slug],
  }));
  const subcategories = [...new Set(paths.map((p) => p.subcategory))];
  const indexParams = subcategories.map((sub) => ({ slug: [sub] }));
  return [...indexParams, ...pageParams];
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params;
  if (slug.length === 1) {
    const sub = subcategoryMeta[slug[0]!];
    return { title: sub ? `${sub.title} Reference` : "Reference" };
  }
  const data = getContentBySlugResolved("reference", slug.join("/"));
  if (!data) return { title: "Not Found" };
  return { title: `${data.meta.title} | Reference`, description: data.meta.description };
}

export default async function ReferencePage({ params }: PageProps) {
  const { slug } = await params;

  // Subcategory index (e.g., /reference/app)
  if (slug.length === 1) {
    const subcategory = slug[0]!;
    const sub = subcategoryMeta[subcategory];
    if (!sub) notFound();
    const pages = getNestedContent("reference", subcategory);

    return (
      <div className="max-w-3xl mx-auto px-8 py-16">
        <Link
          href="/reference"
          className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
        >
          <ArrowLeft size={14} />
          Back to Reference
        </Link>

        <div className="mb-12">
          <div className={cn("inline-block px-2 py-1 text-xs font-medium rounded mb-4 uppercase", sub.badgeClass)}>
            {sub.title}
          </div>
          <h1 className="text-4xl font-bold mb-4">{sub.title} Reference</h1>
        </div>

        <div className="space-y-4">
          {pages.map((page) => (
            <Link
              key={page.slug}
              href={`/reference/${subcategory}/${page.slug}`}
              className="flex items-center gap-4 p-4 rounded-lg border border-border hover:border-text-muted bg-surface hover:bg-hover transition-all group"
            >
              <div className="flex-1 min-w-0">
                <h3 className="font-medium group-hover:text-accent transition-colors">
                  {page.meta.title}
                </h3>
                {page.meta.description && (
                  <p className="text-sm text-text-muted mt-1">{page.meta.description}</p>
                )}
              </div>
              <ArrowRight size={16} className="text-text-muted group-hover:text-accent transition-colors flex-shrink-0" />
            </Link>
          ))}
        </div>
      </div>
    );
  }

  // Individual reference page (e.g., /reference/app/viewport)
  if (slug.length !== 2) notFound();
  const subcategory = slug[0]!;
  const pageSlug = slug[1]!;
  const sub = subcategoryMeta[subcategory];
  if (!sub) notFound();

  const data = getContentBySlugResolved("reference", slug.join("/"));
  if (!data) notFound();

  const sectionPages = getNestedContent("reference", subcategory);
  const { prev, next } = getNavigation(sectionPages, pageSlug, `/reference/${subcategory}`);

  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      <Link
        href={`/reference/${subcategory}`}
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to {sub.title}
      </Link>

      <div className="mb-12">
        <div className={cn("inline-block px-2 py-1 text-xs font-medium rounded mb-4 uppercase", sub.badgeClass)}>
          {sub.title}
        </div>
        <h1 className="text-4xl font-bold mb-4">{data.meta.title}</h1>
        {data.meta.description && (
          <p className="text-text-muted text-lg">{data.meta.description}</p>
        )}
      </div>

      <article className="mb-16">
        <MDXRemote source={data.content} components={getMdxComponents()} options={mdxOptions} />
      </article>

      <nav className="flex items-center justify-between pt-8 border-t border-border">
        {prev ? (
          <Link href={prev.href} className="flex items-center gap-3 text-text-muted hover:text-text transition-colors group">
            <ArrowLeft size={20} className="group-hover:-translate-x-1 transition-transform" />
            <div className="text-left">
              <div className="text-xs text-text-muted">Previous</div>
              <div className="font-medium">{prev.title}</div>
            </div>
          </Link>
        ) : <div />}
        {next ? (
          <Link href={next.href} className="flex items-center gap-3 text-text-muted hover:text-text transition-colors group">
            <div className="text-right">
              <div className="text-xs text-text-muted">Next</div>
              <div className="font-medium">{next.title}</div>
            </div>
            <ArrowRight size={20} className="group-hover:translate-x-1 transition-transform" />
          </Link>
        ) : <div />}
      </nav>
    </div>
  );
}
