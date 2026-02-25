import type { Metadata } from "next";
import Link from "next/link";
import { examples } from "@/lib/examples";
import { GalleryViewerCard } from "./GalleryViewerCard";

export const metadata: Metadata = {
  title: "Gallery",
  description: "Example models built with vcad",
};

const galleryItems = [
  {
    id: "plate",
    title: "Mounting Plate",
    description: "A simple plate with four mounting holes for M3 bolts",
    tags: ["beginner", "mechanical"],
  },
  {
    id: "bracket",
    title: "L-Bracket",
    description: "L-shaped mounting bracket with reinforcement",
    tags: ["beginner", "mechanical"],
  },
  {
    id: "flanged-hub",
    title: "Flanged Hub",
    description: "Precision flanged hub with circular bolt pattern",
    tags: ["intermediate", "mechanical"],
  },
  {
    id: "circular-pattern",
    title: "Radial Vent",
    description: "Ventilated disc with radial slot pattern",
    tags: ["intermediate", "functional"],
  },
  {
    id: "enclosure",
    title: "Electronics Enclosure",
    description: "Box shell with ventilation slots",
    tags: ["intermediate", "functional"],
  },
  {
    id: "first-hole",
    title: "First Hole",
    description: "Boolean difference to punch a hole through a plate",
    tags: ["beginner", "mechanical"],
  },
];

export default function GalleryPage() {
  return (
    <div className="max-w-6xl mx-auto px-8 py-16">
      <div className="mb-8">
        <h1 className="text-4xl font-bold mb-2">Gallery</h1>
        <p className="text-text-body">
          Example models built with vcad. Click to open in the playground.
        </p>
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
          Functional
        </button>
      </div>

      {/* Gallery grid */}
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
        {galleryItems.map((item) => {
          const example = examples.find((e) => e.id === item.id);
          if (!example) return null;

          return (
            <Link
              key={item.id}
              href={`/playground?example=${item.id}`}
              className="group block rounded-lg border border-border overflow-hidden bg-surface hover:border-text-muted transition-all"
            >
              <GalleryViewerCard document={example.document} />

              {/* Info */}
              <div className="p-4 border-t border-border">
                <h3 className="font-bold group-hover:text-accent transition-colors">
                  {item.title}
                </h3>
                <p className="text-sm text-text-muted mt-1 line-clamp-2">
                  {item.description}
                </p>

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
          );
        })}
      </div>
    </div>
  );
}
