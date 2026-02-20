import React, { useState } from "react";
import { Box, Text, useInput } from "ink";
import TextInput from "ink-text-input";
import { createCommandRegistry, createDefaultCommandActions } from "@vcad/core";

interface Props {
  active: boolean;
  onActivate: () => void;
  onDeactivate: () => void;
}

export function CommandInput({ active, onActivate, onDeactivate }: Props) {
  const [query, setQuery] = useState("");
  const [selectedIndex, setSelectedIndex] = useState(0);

  const commands = createCommandRegistry(
    createDefaultCommandActions(onDeactivate),
  );

  const filteredCommands = query.trim()
    ? commands.filter((cmd) => {
        const q = query.toLowerCase();
        if (cmd.label.toLowerCase().includes(q)) return true;
        return cmd.keywords.some((kw) => kw.includes(q));
      })
    : commands;

  useInput((input, key) => {
    if (!active) {
      if (key.ctrl && input === "p") {
        onActivate();
        setQuery("");
        setSelectedIndex(0);
      }
      return;
    }

    if (key.escape) {
      onDeactivate();
      setQuery("");
      return;
    }

    if (key.upArrow) {
      setSelectedIndex((i) => Math.max(0, i - 1));
      return;
    }
    if (key.downArrow) {
      setSelectedIndex((i) => Math.min(filteredCommands.length - 1, i + 1));
      return;
    }
    if (key.return && filteredCommands.length > 0) {
      const cmd = filteredCommands[selectedIndex];
      if (cmd && (!cmd.enabled || cmd.enabled())) {
        cmd.action();
      }
      return;
    }
  }, { isActive: active });

  if (!active) {
    return (
      <Box paddingX={1}>
        <Text dimColor>Ctrl+P: command palette</Text>
      </Box>
    );
  }

  return (
    <Box flexDirection="column" borderStyle="single" borderColor="cyan">
      <Box paddingX={1}>
        <Text color="cyan">&gt; </Text>
        <TextInput value={query} onChange={(v) => { setQuery(v); setSelectedIndex(0); }} />
      </Box>
      <Box flexDirection="column" paddingX={1}>
        {filteredCommands.slice(0, 6).map((cmd, idx) => {
          const isDisabled = cmd.enabled && !cmd.enabled();
          const isSelected = idx === selectedIndex;
          return (
            <Text
              key={cmd.id}
              inverse={isSelected}
              dimColor={isDisabled}
            >
              {cmd.label}
            </Text>
          );
        })}
        {filteredCommands.length === 0 && (
          <Text dimColor>No commands found</Text>
        )}
      </Box>
    </Box>
  );
}
