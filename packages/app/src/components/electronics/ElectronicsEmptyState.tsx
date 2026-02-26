import { useCallback } from "react";
import { Circuitry } from "@phosphor-icons/react/dist/ssr/Circuitry";
import { PencilSimple } from "@phosphor-icons/react/dist/ssr/PencilSimple";
import { Upload } from "@phosphor-icons/react/dist/ssr/Upload";
import { ArrowLeft } from "@phosphor-icons/react/dist/ssr/ArrowLeft";
import { useDocumentStore } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useNotificationStore } from "@/stores/notification-store";

export function ElectronicsEmptyState() {
  const exit = useElectronicsStore((s) => s.exit);
  const addToast = useNotificationStore((s) => s.addToast);

  const handleNewPcb = useCallback(() => {
    window.dispatchEvent(new CustomEvent("vcad:open-pcb-dialog"));
  }, []);

  const handleSchematicFirst = useCallback(() => {
    const store = useDocumentStore.getState();
    store.initSchematic();
    store.initPcb();
    addToast("Created schematic + PCB", "success");
  }, [addToast]);

  const handleImportKicad = useCallback(() => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = ".kicad_pcb";
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return;
      try {
        const text = await file.text();
        const { parseKicadPcb } = await import("@vcad/engine");
        const pcb = await parseKicadPcb(text);
        if (pcb) {
          useDocumentStore.getState().importPcb(pcb, file.name);
          useElectronicsStore.getState().enter();
          addToast(`Imported ${file.name}`, "success");
        } else {
          addToast("Failed to parse KiCad PCB", "error");
        }
      } catch {
        addToast("KiCad import not available", "error");
      }
    };
    input.click();
  }, [addToast]);

  return (
    <div className="flex flex-col items-center justify-center h-full gap-6 p-8">
      <div className="flex flex-col items-center gap-2">
        <Circuitry size={48} className="text-text-muted" />
        <h2 className="text-lg font-medium text-text">Electronics Workspace</h2>
        <p className="text-sm text-text-muted text-center max-w-md">
          Design PCBs with schematic capture, interactive routing, and DRC.
        </p>
      </div>

      <div className="grid grid-cols-1 sm:grid-cols-3 gap-3 max-w-lg w-full">
        <button
          onClick={handleNewPcb}
          className="flex flex-col items-center gap-2 p-4 rounded border border-border hover:border-accent hover:bg-accent/5 transition-colors group"
        >
          <Circuitry size={24} className="text-text-muted group-hover:text-accent transition-colors" />
          <span className="text-sm font-medium text-text">New PCB</span>
          <span className="text-[11px] text-text-muted text-center">Configure board size, layers, and rules</span>
        </button>

        <button
          onClick={handleSchematicFirst}
          className="flex flex-col items-center gap-2 p-4 rounded border border-border hover:border-accent hover:bg-accent/5 transition-colors group"
        >
          <PencilSimple size={24} className="text-text-muted group-hover:text-accent transition-colors" />
          <span className="text-sm font-medium text-text">Start with Schematic</span>
          <span className="text-[11px] text-text-muted text-center">Design circuit first, then push to PCB</span>
        </button>

        <button
          onClick={handleImportKicad}
          className="flex flex-col items-center gap-2 p-4 rounded border border-border hover:border-accent hover:bg-accent/5 transition-colors group"
        >
          <Upload size={24} className="text-text-muted group-hover:text-accent transition-colors" />
          <span className="text-sm font-medium text-text">Import KiCad</span>
          <span className="text-[11px] text-text-muted text-center">Open a .kicad_pcb file</span>
        </button>
      </div>

      <button
        onClick={exit}
        className="flex items-center gap-1.5 text-xs text-text-muted hover:text-text transition-colors mt-2"
      >
        <ArrowLeft size={14} />
        Back to 3D
      </button>
    </div>
  );
}
