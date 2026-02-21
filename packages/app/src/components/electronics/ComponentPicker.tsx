/**
 * Popover listing available schematic symbols for placement.
 * Shown when the Place tool is active and no symbol is selected.
 */

import { SYMBOL_LIBRARY } from "./symbol-library";
import { useElectronicsStore } from "@/stores/electronics-store";

export function ComponentPicker() {
  const setPlacing = useElectronicsStore((s) => s.setSchPlacingSymbol);

  return (
    <div className="absolute top-12 left-3 w-44 rounded-lg border border-border bg-surface/95 backdrop-blur-sm shadow-lg p-2 text-[11px] pointer-events-auto z-10">
      <div className="font-medium text-text mb-1.5 px-1">Components</div>
      <div className="space-y-0.5">
        {SYMBOL_LIBRARY.map((sym) => (
          <button
            key={sym.id}
            className="w-full text-left px-2 py-1 rounded text-text-muted hover:text-text hover:bg-surface-hover transition-colors"
            onClick={() => setPlacing(sym.id)}
          >
            <span className="font-medium text-text">{sym.name}</span>
            <span className="ml-1.5 text-[10px] text-text-muted">
              {sym.prefix} &middot; {sym.defaultValue}
            </span>
          </button>
        ))}
      </div>
      <div className="mt-1.5 pt-1 border-t border-border text-[10px] text-text-muted px-1">
        Click to select, then click on schematic to place
      </div>
    </div>
  );
}
