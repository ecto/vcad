/**
 * get_changelog tool — query vcad changelog.
 */

import {
  changelog,
  CURRENT_VERSION,
  type ChangelogEntry,
  type ChangelogCategory,
} from "@vcad/core";

interface GetChangelogInput {
  version?: string;
  since?: string;
  category?: ChangelogCategory | "all";
  feature?: string;
  tool?: string;
  breaking_only?: boolean;
  limit?: number;
}

export const getChangelogSchema = {
  type: "object" as const,
  properties: {
    version: {
      type: "string" as const,
      description: "Filter by exact version (e.g., '0.8.0')",
    },
    since: {
      type: "string" as const,
      description: "Get entries since version or date (e.g., '0.7.0' or '2026-01-01')",
    },
    category: {
      type: "string" as const,
      enum: ["feat", "fix", "breaking", "perf", "docs", "all"],
      description: "Filter by category",
    },
    feature: {
      type: "string" as const,
      description: "Filter by feature tag (e.g., 'physics', 'sketch')",
    },
    tool: {
      type: "string" as const,
      description: "Filter by MCP tool name (e.g., 'gym_step')",
    },
    breaking_only: {
      type: "boolean" as const,
      description: "Only show breaking changes",
    },
    limit: {
      type: "number" as const,
      description: "Maximum number of entries to return (default: 10)",
    },
  },
};

export function getChangelog(
  input: unknown
): { content: Array<{ type: "text"; text: string }> } {
  const params = input as GetChangelogInput;
  let entries: ChangelogEntry[] = [...changelog.entries];

  // Apply filters
  if (params.version) {
    entries = entries.filter((e) => e.version === params.version);
  }

  if (params.since) {
    // Could be a version or a date
    entries = entries.filter(
      (e) => e.date >= params.since! || e.version >= params.since!
    );
  }

  if (params.category && params.category !== "all") {
    entries = entries.filter((e) => e.category === params.category);
  }

  if (params.feature) {
    const featureLower = params.feature.toLowerCase();
    entries = entries.filter((e) =>
      e.features?.some((f) => f.toLowerCase().includes(featureLower))
    );
  }

  if (params.tool) {
    const toolLower = params.tool.toLowerCase();
    entries = entries.filter((e) =>
      e.mcpTools?.some((t) => t.toLowerCase().includes(toolLower))
    );
  }

  if (params.breaking_only) {
    entries = entries.filter(
      (e) => e.category === "breaking" || e.breaking !== undefined
    );
  }

  // Apply limit
  const limit = params.limit ?? 10;
  entries = entries.slice(0, limit);

  // Format output
  if (entries.length === 0) {
    return {
      content: [{ type: "text", text: "No matching changelog entries found." }],
    };
  }

  const text = entries
    .map((e) => {
      let entry = `## ${e.title} (v${e.version})\n`;
      entry += `**${e.category.toUpperCase()}** · ${e.date}\n\n`;
      entry += e.summary;

      if (e.mcpTools && e.mcpTools.length > 0) {
        entry += `\n\n**MCP Tools:** ${e.mcpTools.map((t) => `\`${t}\``).join(", ")}`;
      }

      if (e.features && e.features.length > 0) {
        entry += `\n\n**Features:** ${e.features.join(", ")}`;
      }

      if (e.breaking) {
        entry += `\n\n⚠️ **Breaking:** ${e.breaking.description}`;
        if (e.breaking.migration) {
          entry += `\n**Migration:** ${e.breaking.migration}`;
        }
      }

      return entry;
    })
    .join("\n\n---\n\n");

  return {
    content: [
      {
        type: "text",
        text: `# vcad Changelog\n\nCurrent version: ${CURRENT_VERSION}\n\n${text}`,
      },
    ],
  };
}
