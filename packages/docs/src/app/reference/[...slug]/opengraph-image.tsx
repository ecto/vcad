import { generateOgImage, SIZE } from "@/lib/og";
import {
  getContentBySlugResolved,
  getNestedContentPaths,
} from "@/lib/content";

export const size = SIZE;
export const contentType = "image/png";

export async function generateStaticParams() {
  const paths = getNestedContentPaths("reference");
  const pageParams = paths.map(({ subcategory, slug }) => ({
    slug: [subcategory, slug],
  }));
  const subcategories = [...new Set(paths.map((p) => p.subcategory))];
  const indexParams = subcategories.map((sub) => ({ slug: [sub] }));
  return [...indexParams, ...pageParams];
}

export default async function Image({ params }: { params: Promise<{ slug: string[] }> }) {
  const { slug } = await params;
  const sub = slug[0] ?? "";

  if (slug.length === 1) {
    return generateOgImage({
      title: `${sub.charAt(0).toUpperCase() + sub.slice(1)} Reference`,
      breadcrumb: `reference`,
    });
  }

  const data = getContentBySlugResolved("reference", slug.join("/"));
  return generateOgImage({
    title: data?.meta.title ?? "Reference",
    breadcrumb: `reference / ${sub}`,
  });
}
