import { artifactFileUrl, storeArtifact, type ArtifactHandle } from "./artifact-store.js";

export interface RenderAsset {
  id: string;
  artifact_id: string;
  artifact_url: string;
  url: string;
  uri: string;
  name: string;
  mimeType: string;
  bytes: number;
  width: number;
  height?: number;
  alt: string;
  role: string;
  display: "inline";
  promoteToTranscript: boolean;
  markdown: string;
  manifest: ArtifactHandle["manifest"];
}

export interface RenderAssetOptions {
  tool: string;
  filename: string;
  width: number;
  height?: number;
  alt: string;
  role?: string;
}

export function makePngRenderAsset(
  png: Buffer,
  opts: RenderAssetOptions,
): RenderAsset {
  const name = sanitizeFilename(opts.filename);
  const handle = storeArtifact([
    {
      name,
      content: png,
      contentType: "image/png",
    },
  ]);
  const url = artifactFileUrl(handle.artifact_id, name);
  const id = `${opts.tool}:${handle.artifact_id}:${name}`;
  const markdown = `![${escapeMarkdownAlt(opts.alt)}](${url})`;
  return {
    id,
    artifact_id: handle.artifact_id,
    artifact_url: handle.artifact_url,
    url,
    uri: `mcp://vcad/artifacts/${handle.artifact_id}/${encodeURIComponent(name)}`,
    name,
    mimeType: "image/png",
    bytes: png.length,
    width: opts.width,
    ...(opts.height ? { height: opts.height } : {}),
    alt: opts.alt,
    role: opts.role ?? "primary_output",
    display: "inline",
    promoteToTranscript: true,
    markdown,
    manifest: handle.manifest,
  };
}

type RenderAssetCarrier<T extends object> = T & {
  structuredContent?: Record<string, unknown>;
};

export function withRenderAssets<T extends object>(
  result: RenderAssetCarrier<T>,
  assets: RenderAsset[],
): RenderAssetCarrier<T> {
  if (assets.length === 0) return result;
  const existing = result.structuredContent ?? {};
  const prevAssets = Array.isArray(existing.assets) ? existing.assets : [];
  const prevMarkdown = Array.isArray(existing.suggestedFinalMarkdown)
    ? existing.suggestedFinalMarkdown
    : [];
  result.structuredContent = {
    ...existing,
    assets: [...prevAssets, ...assets],
    suggestedFinalMarkdown: [
      ...prevMarkdown,
      ...assets.map((a) => a.markdown),
    ],
  };
  return result;
}

export function renderAssetSummary(asset: RenderAsset): Record<string, unknown> {
  return {
    id: asset.id,
    artifact_id: asset.artifact_id,
    artifact_url: asset.artifact_url,
    url: asset.url,
    uri: asset.uri,
    name: asset.name,
    mimeType: asset.mimeType,
    bytes: asset.bytes,
    width: asset.width,
    ...(asset.height ? { height: asset.height } : {}),
    alt: asset.alt,
    role: asset.role,
    display: asset.display,
    promoteToTranscript: asset.promoteToTranscript,
    markdown: asset.markdown,
  };
}

function sanitizeFilename(name: string): string {
  const cleaned = name
    .trim()
    .replace(/[/\\]/g, "-")
    .replace(/[^A-Za-z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return cleaned || "render.png";
}

function escapeMarkdownAlt(text: string): string {
  return text.replace(/[\[\]]/g, "");
}
