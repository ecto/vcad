/**
 * Changelog module for vcad.
 *
 * Provides typed access to the centralized CHANGELOG.json file
 * at the repository root.
 */

// In dev the app resolves `@vcad/core` to `src/index.ts`, which pulls
// `../CHANGELOG.json` through the symlink → repo root. In prod, tsc
// compiles against the same path and the build step copies
// CHANGELOG.json into `dist/` next to the compiled JS. Same relative
// import works in both worlds.
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

/** Matches the on-disk shape of `CHANGELOG.json`. */
export interface Changelog {
  /** JSON Schema reference (optional, ignored at runtime). */
  $schema?: string;
  /** Entries in reverse-chronological order — newest first. */
  entries: ChangelogEntry[];
}

export const changelog: Changelog = changelogData as unknown as Changelog;

/**
 * Semver of the most recent changelog entry. The JSON is kept in
 * newest-first order by convention (see the `Adding a changelog
 * entry` section of CLAUDE.md), so the first entry is "current."
 */
export const CURRENT_VERSION: string =
  changelog.entries[0]?.version ?? "0.0.0";

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
