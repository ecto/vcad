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

interface PageProps {
  params: Promise<{ slug: string }>;
}

export async function generateStaticParams() {
  const paths = getContentPaths("reference");
  return paths.map((slug) => ({ slug }));
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params;
  const data = getContentBySlugResolved("reference", slug);

  if (!data) {
    return { title: "Not Found" };
  }

  return {
    title: `${data.meta.title} | API Reference`,
    description: data.meta.description,
  };
}

export default async function ReferencePage({ params }: PageProps) {
  const { slug } = await params;
  const data = getContentBySlugResolved("reference", slug);

  if (!data) {
    notFound();
  }

  // Get all reference pages for navigation
  const allPages = getAllContent("reference");
  const { prev, next } = getNavigation(allPages, slug, "/reference");

  return (
    <div className="max-w-4xl mx-auto px-8 py-16">
      {/* Breadcrumb */}
      <Link
        href="/reference"
        className="inline-flex items-center gap-2 text-sm text-text-muted hover:text-text mb-8"
      >
        <ArrowLeft size={14} />
        Back to Reference
      </Link>

      {/* Header */}
      <div className="mb-12">
        <div className="inline-block px-2 py-1 text-xs font-medium bg-blue-500/20 text-blue-500 rounded mb-4 uppercase">
          Reference
        </div>
        <h1 className="text-4xl font-bold mb-4">{data.meta.title}</h1>
        {data.meta.description && (
          <p className="text-text-muted text-lg">{data.meta.description}</p>
        )}
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
