import { generateOgImage, SIZE } from "@/lib/og";
import { getContentBySlugResolved, getContentPaths } from "@/lib/content";

export const size = SIZE;
export const contentType = "image/png";

export async function generateStaticParams() {
  return getContentPaths("vs").map((slug) => ({ slug }));
}

export default async function Image({ params }: { params: Promise<{ slug: string }> }) {
  const { slug } = await params;
  const data = getContentBySlugResolved("vs", slug);
  return generateOgImage({
    title: data?.meta.title ?? "Comparison",
    breadcrumb: "comparisons",
  });
}
