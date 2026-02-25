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

interface BeforeAfterProps {
  before: Document;
  after: Document;
  beforeLabel?: string;
  afterLabel?: string;
  height?: string;
}

/**
 * Side-by-side comparison of two 3D documents.
 * Stacks vertically on mobile, horizontal on desktop.
 */
export function BeforeAfter({
  before,
  after,
  beforeLabel = "Before",
  afterLabel = "After",
  height = "300px",
}: BeforeAfterProps) {
  return (
    <div className="my-6 grid grid-cols-1 md:grid-cols-2 gap-4">
      <div>
        <div className="text-sm font-medium text-text-muted mb-2">
          {beforeLabel}
        </div>
        <div
          className="rounded-lg border border-border overflow-hidden"
          style={{ height }}
        >
          <Viewport document={before} />
        </div>
      </div>
      <div>
        <div className="text-sm font-medium text-text-muted mb-2">
          {afterLabel}
        </div>
        <div
          className="rounded-lg border border-border overflow-hidden"
          style={{ height }}
        >
          <Viewport document={after} />
        </div>
      </div>
    </div>
  );
}
