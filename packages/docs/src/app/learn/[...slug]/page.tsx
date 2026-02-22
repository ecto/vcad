import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { MDXRemote } from "next-mdx-remote/rsc";
import { TutorialLayout } from "@/components/TutorialLayout";
import {
  getContentBySlugResolved,
  getNestedContentPaths,
  getNestedContent,
  getNavigation,
} from "@/lib/content";
import { getMdxComponents } from "@/lib/mdx-components";

interface PageProps {
  params: Promise<{ slug: string[] }>;
}

export async function generateStaticParams() {
  const paths = getNestedContentPaths("learn");
  return paths.map(({ subcategory, slug }) => ({
    slug: [subcategory, slug],
  }));
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params;
  const slugPath = slug.join("/");
  const data = getContentBySlugResolved("learn", slugPath);

  if (!data) {
    return { title: "Not Found" };
  }

  return {
    title: data.meta.title,
    description: data.meta.description,
  };
}

export default async function LearnPage({ params }: PageProps) {
  const { slug } = await params;

  if (slug.length !== 2) {
    notFound();
  }

  const section = slug[0]!;
  const pageSlug = slug[1]!;
  const slugPath = slug.join("/");
  const data = getContentBySlugResolved("learn", slugPath);

  if (!data) {
    notFound();
  }

  // Get all pages in this section for navigation
  const sectionPages = getNestedContent("learn", section);
  const { prev, next } = getNavigation(sectionPages, pageSlug, `/learn/${section}`);

  const validSection = section as "beginner" | "intermediate" | "advanced";

  return (
    <TutorialLayout
      section={validSection}
      title={data.meta.title}
      prev={prev}
      next={next}
    >
      <div className="prose">
        <MDXRemote source={data.content} components={getMdxComponents()} />
      </div>
    </TutorialLayout>
  );
}
