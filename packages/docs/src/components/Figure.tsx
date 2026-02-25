"use client";

import dynamic from "next/dynamic";
import type { Document } from "@vcad/ir";

const Viewport = dynamic(
  () => import("./Playground/Viewport").then((m) => m.Viewport),
  {
    ssr: false,
    loading: () => (
      <div className="h-full bg-surface animate-pulse rounded-lg" />
    ),
  }
);

interface FigureProps {
  document: Document;
  caption?: string;
  height?: string;
}

/**
 * Viewport-only 3D viewer with an optional caption.
 * Use in MDX pages to show geometry without the code editor.
 */
export function Figure({ document, caption, height = "300px" }: FigureProps) {
  return (
    <figure className="my-6">
      <div
        className="rounded-lg border border-border overflow-hidden"
        style={{ height }}
      >
        <Viewport document={document} />
      </div>
      {caption && (
        <figcaption className="mt-2 text-sm text-text-muted text-center">
          {caption}
        </figcaption>
      )}
    </figure>
  );
}
