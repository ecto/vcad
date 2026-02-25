import { generateOgImage, SIZE } from "@/lib/og";
import {
  getContentBySlugResolved,
  getNestedContentPaths,
} from "@/lib/content";

export const size = SIZE;
export const contentType = "image/png";

export async function generateStaticParams() {
  const paths = getNestedContentPaths("guides");
  const pageParams = paths.map(({ subcategory, slug }) => ({
    slug: [subcategory, slug],
  }));
  const subcategories = [...new Set(paths.map((p) => p.subcategory))];
  const indexParams = subcategories.map((sub) => ({ slug: [sub] }));
  return [...indexParams, ...pageParams];
}

export default async function Image({ params }: { params: Promise<{ slug: string[] }> }) {
  const { slug } = await params;
  const category = slug[0] ?? "";

  if (slug.length === 1) {
    return generateOgImage({
      title: `${category.charAt(0).toUpperCase() + category.slice(1)} Guides`,
      breadcrumb: `guides`,
    });
  }

  const data = getContentBySlugResolved("guides", slug.join("/"));
  return generateOgImage({
    title: data?.meta.title ?? "Guide",
    breadcrumb: `guides / ${category}`,
  });
}
