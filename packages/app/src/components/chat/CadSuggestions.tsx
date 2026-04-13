import { Suggestions, Suggestion } from "@/components/ai-elements/suggestion";
import type { SelectionContext } from "@vcad/core";

interface Props {
  selection: SelectionContext[];
  onPick: (text: string) => void;
}

const noSelectionPrompts = [
  "Create a 50mm cube",
  "Sketch a 30mm circle",
  "⌀20 × 40mm cylinder",
  "100×60×10mm plate, rounded corners",
];

const singleSelectionPrompts = (name: string) => [
  `Fillet ${name} by 2mm`,
  `Chamfer the top of ${name}`,
  `Mirror ${name} across XZ`,
  `Shell ${name} 2mm`,
  `Make ${name} 20% larger`,
];

const multiSelectionPrompts = (count: number) => [
  `Subtract the ${count} parts`,
  `Union the ${count} parts`,
  `Arrange in a grid, 30mm apart`,
  `Show the combined bounding box`,
];

export function CadSuggestions({ selection, onPick }: Props) {
  let prompts: string[];
  if (selection.length === 0) {
    prompts = noSelectionPrompts;
  } else if (selection.length === 1 && selection[0]) {
    prompts = singleSelectionPrompts(selection[0].partName);
  } else {
    prompts = multiSelectionPrompts(selection.length);
  }

  return (
    <Suggestions className="px-0">
      {prompts.map((p) => (
        <Suggestion
          key={p}
          suggestion={p}
          onClick={onPick}
          variant="ghost"
          className="h-6 shrink-0 rounded-full border border-border/40 bg-transparent px-2.5 text-[10px] font-normal text-text-muted shadow-none transition-colors hover:border-border hover:bg-hover hover:text-text"
        />
      ))}
    </Suggestions>
  );
}
