"use client";

import Link from "next/link";
import { examples } from "@/lib/examples";
import { GalleryViewer } from "./GalleryViewer";

const previewIds = ["plate", "bracket", "flanged-hub"];

export function GalleryPreview() {
  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-6">
      {previewIds.map((id) => {
        const example = examples.find((e) => e.id === id);
        if (!example) return null;

        return (
          <Link
            key={id}
            href={`/playground?example=${id}`}
            className="group block"
          >
            <div className="rounded-lg border border-border overflow-hidden bg-surface transition-colors group-hover:border-text-muted">
              <GalleryViewer document={example.document} />

              {/* Info */}
              <div className="p-3 border-t border-border">
                <h3 className="font-medium text-sm text-text group-hover:text-accent transition-colors">
                  {example.name}
                </h3>
                <p className="text-xs text-text-muted">
                  {example.description}
                </p>
              </div>
            </div>
          </Link>
        );
      })}
    </div>
  );
}
