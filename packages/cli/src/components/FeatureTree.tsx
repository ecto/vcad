import React from "react";
import { Box, Text } from "ink";
import { getPartGlyph } from "@vcad/core";
import type { PartInfo } from "@vcad/core";

interface Props {
  parts: PartInfo[];
  selectedIds: Set<string>;
  focusedIndex: number;
  onFocusChange: (index: number) => void;
}

export function FeatureTree({ parts, selectedIds, focusedIndex }: Props) {
  if (parts.length === 0) {
    return (
      <Box flexDirection="column" paddingX={1}>
        <Text dimColor>No parts</Text>
        <Text dimColor>Press 1/2/3</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column">
      {parts.map((part, i) => {
        const isSelected = selectedIds.has(part.id);
        const isFocused = i === focusedIndex;
        const icon = getPartGlyph(part.kind);

        return (
          <Box key={part.id} paddingX={1}>
            <Text
              inverse={isFocused}
              color={isSelected ? "cyan" : undefined}
              bold={isSelected}
            >
              {icon} {part.name.slice(0, 18)}
            </Text>
          </Box>
        );
      })}
    </Box>
  );
}
