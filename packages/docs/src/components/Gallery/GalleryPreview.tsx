import Link from "next/link";
import { Star } from "@phosphor-icons/react/dist/ssr";

// Gallery items (in production, this would come from a database)
const galleryItems = [
  {
    id: "plate",
    title: "Mounting Plate",
    author: "@vcad",
    stars: 234,
    image: "/gallery/plate.png",
  },
  {
    id: "bracket",
    title: "L-Bracket",
    author: "@vcad",
    stars: 189,
    image: "/gallery/bracket.png",
  },
  {
    id: "mascot",
    title: "Robot Mascot",
    author: "@vcad",
    stars: 156,
    image: "/gallery/mascot.png",
  },
];

export function GalleryPreview() {
  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
      {galleryItems.map((item) => (
        <Link
          key={item.id}
          href={`/gallery/${item.id}`}
          className="group block"
        >
          <div className="rounded-lg border border-border overflow-hidden bg-surface transition-colors group-hover:border-text-muted">
            {/* Image placeholder */}
            <div className="aspect-square bg-card flex items-center justify-center">
              <div className="text-4xl text-text-muted">
                {/* Placeholder - would be actual image */}
                ◇
              </div>
            </div>

            {/* Info */}
            <div className="p-3 border-t border-border">
              <div className="flex items-center justify-between">
                <div>
                  <h3 className="font-medium text-sm text-text group-hover:text-accent transition-colors">
                    {item.title}
                  </h3>
                  <p className="text-xs text-text-muted">
                    {item.author}
                  </p>
                </div>
                <div className="flex items-center gap-1 text-xs text-text-muted">
                  <Star size={12} weight="fill" className="text-yellow-500" />
                  {item.stars}
                </div>
              </div>
            </div>
          </div>
        </Link>
      ))}
    </div>
  );
}
