/**
 * Top-center segmented control that switches the electronics workspace between
 * the schematic and the 3D board. The two views are mutually exclusive; this is
 * the primary, always-visible way to flip (Tab also toggles).
 */

import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Cpu } from "@phosphor-icons/react/dist/ssr/Cpu";
import { useElectronicsStore, type ElectronicsLayout } from "@/stores/electronics-store";
import { cn } from "@/lib/utils";

export function ElectronicsViewToggle() {
  const layout = useElectronicsStore((s) => s.layout);
  const setLayout = useElectronicsStore((s) => s.setLayout);

  const option = (value: ElectronicsLayout, label: string, Icon: typeof Cpu) => (
    <button
      type="button"
      onClick={() => setLayout(value)}
      aria-pressed={layout === value}
      className={cn(
        "flex items-center gap-1.5 rounded-md px-3 py-1 text-[11px] font-medium transition-colors",
        layout === value
          ? "bg-brand text-white shadow-sm"
          : "text-text-muted hover:text-text",
      )}
    >
      <Icon size={13} weight={layout === value ? "fill" : "regular"} />
      {label}
    </button>
  );

  return (
    <div className="pointer-events-auto absolute left-1/2 top-3 z-30 flex -translate-x-1/2 items-center gap-0.5 rounded-lg border border-border bg-surface/95 p-0.5 shadow-lg backdrop-blur-sm">
      {option("schematic", "Schematic", PencilSimple)}
      {option("board", "Board", Cpu)}
    </div>
  );
}
