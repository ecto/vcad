/**
 * Changelog module for vcad.
 *
 * Provides typed access to the centralized CHANGELOG.json file
 * at the repository root.
 */

// Note: CHANGELOG.json is copied to dist/ during build
// @ts-expect-error JSON import (path resolves correctly at runtime)
import changelogData from "../CHANGELOG.json" with { type: "json" };

export type ChangelogCategory = "feat" | "fix" | "breaking" | "perf" | "docs";

export interface ChangelogEntry {
  id: string;
  version: string;
  date: string;
  category: ChangelogCategory;
  title: string;
  summary: string;
  details?: string;
  features?: string[];
  mcpTools?: string[];
  breaking?: { description: string; migration?: string };
}

export interface Changelog {
  version: string;
  entries: ChangelogEntry[];
}

export const changelog: Changelog = changelogData as Changelog;
export const CURRENT_VERSION = changelog.version;

/**
 * Get changelog entries newer than a given version.
 * Compares versions lexicographically (works for semver).
 */
export function getEntriesSince(version: string): ChangelogEntry[] {
  return changelog.entries.filter((e) => e.version > version);
}

/**
 * Get changelog entries for a specific version.
 */
export function getEntriesForVersion(version: string): ChangelogEntry[] {
  return changelog.entries.filter((e) => e.version === version);
}

/**
 * Get changelog entries by category.
 */
export function getEntriesByCategory(
  category: ChangelogCategory
): ChangelogEntry[] {
  return changelog.entries.filter((e) => e.category === category);
}

/**
 * Get changelog entries that mention a specific MCP tool.
 */
export function getEntriesForTool(toolName: string): ChangelogEntry[] {
  return changelog.entries.filter((e) =>
    e.mcpTools?.some((t) => t.includes(toolName))
  );
}
