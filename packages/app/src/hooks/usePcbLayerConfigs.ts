import { useMemo } from "react";
import { useCoreElectronicsStore, useDocumentStore, getNodePcb } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { resolveLayerConfigs } from "@/lib/pcb-layer-config";

/** Layer configs for the active board — one entry per copper layer it carries. */
export function usePcbLayerConfigs() {
  const pcbLayers = useElectronicsStore((s) => s.pcbLayers);
  const activeBoardNodeId = useCoreElectronicsStore((s) => s.activeBoardNodeId);
  const document = useDocumentStore((s) => s.document);
  const pcb = activeBoardNodeId != null ? getNodePcb(document, activeBoardNodeId) : null;
  return useMemo(() => resolveLayerConfigs(pcbLayers, pcb), [pcbLayers, pcb]);
}
