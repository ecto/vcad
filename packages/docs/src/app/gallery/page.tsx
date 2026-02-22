import type { Metadata } from "next";
import Link from "next/link";
import { Star, GitFork, Eye } from "@phosphor-icons/react/dist/ssr";

export const metadata: Metadata = {
  title: "Gallery",
  description: "Community showcase of vcad models and examples",
};

const galleryItems = [
  {
    id: "plate",
    title: "Mounting Plate",
    description: "A simple plate with four mounting holes for M3 bolts",
    author: "@vcad",
    stars: 234,
    forks: 45,
    views: 1200,
    tags: ["beginner", "mechanical"],
  },
  {
    id: "bracket",
    title: "L-Bracket",
    description: "L-shaped mounting bracket with reinforcement",
    author: "@vcad",
    stars: 189,
    forks: 32,
    views: 890,
    tags: ["beginner", "mechanical"],
  },
  {
    id: "mascot",
    title: "Robot Mascot",
    description: "Multi-part robot figure with articulated joints",
    author: "@vcad",
    stars: 156,
    forks: 28,
    views: 756,
    tags: ["intermediate", "artistic"],
  },
  {
    id: "hub",
    title: "Flanged Hub",
    description: "Precision flanged hub with circular bolt pattern",
    author: "@maker123",
    stars: 142,
    forks: 19,
    views: 634,
    tags: ["intermediate", "mechanical"],
  },
  {
    id: "vent",
    title: "Radial Vent",
    description: "Decorative vent cover with radial pattern",
    author: "@designer",
    stars: 98,
    forks: 12,
    views: 445,
    tags: ["intermediate", "functional"],
  },
  {
    id: "gear",
    title: "Spur Gear",
    description: "Parametric spur gear with configurable teeth",
    author: "@engineer",
    stars: 267,
    forks: 78,
    views: 1890,
    tags: ["advanced", "mechanical"],
  },
];

export default function GalleryPage() {
  return (
    <div className="max-w-6xl mx-auto px-8 py-16">
      <div className="flex items-center justify-between mb-8">
        <div>
          <h1 className="text-4xl font-bold mb-2">Gallery</h1>
          <p className="text-text-muted">
            Community showcase of vcad models. Fork, learn, and remix.
          </p>
        </div>
        <button className="px-4 py-2 bg-accent hover:bg-accent-hover text-white rounded-lg text-sm font-medium transition-colors">
          Submit Design
        </button>
      </div>

      {/* Filters */}
      <div className="flex gap-2 mb-8 pb-8 border-b border-border">
        <button className="px-3 py-1.5 text-sm bg-accent text-white rounded-md">
          All
        </button>
        <button className="px-3 py-1.5 text-sm text-text-muted hover:text-text hover:bg-hover rounded-md transition-colors">
          Mechanical
        </button>
        <button className="px-3 py-1.5 text-sm text-text-muted hover:text-text hover:bg-hover rounded-md transition-colors">
          Artistic
        </button>
        <button className="px-3 py-1.5 text-sm text-text-muted hover:text-text hover:bg-hover rounded-md transition-colors">
          Functional
        </button>
      </div>

      {/* Gallery grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
        {galleryItems.map((item) => (
          <Link
            key={item.id}
            href={`/gallery/${item.id}`}
            className="group block rounded-lg border border-border overflow-hidden bg-surface hover:border-text-muted transition-all"
          >
            {/* Image placeholder */}
            <div className="aspect-square bg-card flex items-center justify-center relative overflow-hidden">
              <div className="text-6xl text-text-muted opacity-30">
                ◇
              </div>
              {/* Hover overlay */}
              <div className="absolute inset-0 bg-accent/0 group-hover:bg-accent/10 transition-colors flex items-center justify-center">
                <span className="opacity-0 group-hover:opacity-100 text-accent font-medium transition-opacity">
                  View Design
                </span>
              </div>
            </div>

            {/* Info */}
            <div className="p-4 border-t border-border">
              <h3 className="font-bold group-hover:text-accent transition-colors">
                {item.title}
              </h3>
              <p className="text-sm text-text-muted mt-1 line-clamp-2">
                {item.description}
              </p>

              <div className="flex items-center justify-between mt-4">
                <span className="text-xs text-text-muted">{item.author}</span>
                <div className="flex items-center gap-3 text-xs text-text-muted">
                  <span className="flex items-center gap-1">
                    <Star size={12} weight="fill" className="text-yellow-500" />
                    {item.stars}
                  </span>
                  <span className="flex items-center gap-1">
                    <GitFork size={12} />
                    {item.forks}
                  </span>
                  <span className="flex items-center gap-1">
                    <Eye size={12} />
                    {item.views}
                  </span>
                </div>
              </div>

              {/* Tags */}
              <div className="flex gap-2 mt-3">
                {item.tags.map((tag) => (
                  <span
                    key={tag}
                    className="px-2 py-0.5 text-xs bg-border rounded text-text-muted"
                  >
                    {tag}
                  </span>
                ))}
              </div>
            </div>
          </Link>
        ))}
      </div>
    </div>
  );
}
