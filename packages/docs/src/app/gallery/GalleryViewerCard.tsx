"use client";

import type { Document } from "@vcad/ir";
import { GalleryViewer } from "@/components/Gallery/GalleryViewer";

export function GalleryViewerCard({ document }: { document: Document }) {
  return <GalleryViewer document={document} />;
}
