import { generateOgImage, SIZE } from "@/lib/og";
import {
  getContentBySlugResolved,
  getNestedContentPaths,
} from "@/lib/content";

export const size = SIZE;
export const contentType = "image/png";

export async function generateStaticParams() {
  const paths = getNestedContentPaths("tutorials");
  const pageParams = paths.map(({ subcategory, slug }) => ({
    slug: [subcategory, slug],
  }));
  const subcategories = [...new Set(paths.map((p) => p.subcategory))];
  const indexParams = subcategories.map((sub) => ({ slug: [sub] }));
  return [...indexParams, ...pageParams];
}

export default async function Image({ params }: { params: Promise<{ slug: string[] }> }) {
  const { slug } = await params;
  const track = slug[0] ?? "";

  if (slug.length === 1) {
    return generateOgImage({
      title: `${track.charAt(0).toUpperCase() + track.slice(1)} Tutorials`,
      breadcrumb: `tutorials`,
    });
  }

  const data = getContentBySlugResolved("tutorials", slug.join("/"));
  return generateOgImage({
    title: data?.meta.title ?? "Tutorial",
    breadcrumb: `tutorials / ${track}`,
  });
}
