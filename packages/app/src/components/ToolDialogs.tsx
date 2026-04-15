import { useEffect, useState } from "react";
import { useDocumentStore, useUiStore } from "@vcad/core";
import {
  InsertInstanceDialog,
  AddJointDialog,
  FilletChamferDialog,
  ShellDialog,
  PatternDialog,
  MirrorDialog,
  TextDialog,
  StitchDialog,
  NewPcbDialog,
} from "@/components/dialogs";

/**
 * Centralised home for every tool-palette dialog. Dialogs used to live
 * inside ToolPalette.tsx, but they're now shared by the desktop ToolPalette
 * and the mobile MobileToolPalette — both dispatch `vcad:*` events that
 * this component listens for.
 *
 * Mounted once at App level.
 */
export function ToolDialogs() {
  const [insertOpen, setInsertOpen] = useState(false);
  const [jointOpen, setJointOpen] = useState(false);
  const [filletOpen, setFilletOpen] = useState(false);
  const [chamferOpen, setChamferOpen] = useState(false);
  const [shellOpen, setShellOpen] = useState(false);
  const [patternOpen, setPatternOpen] = useState(false);
  const [mirrorOpen, setMirrorOpen] = useState(false);
  const [textOpen, setTextOpen] = useState(false);
  const [stitchOpen, setStitchOpen] = useState(false);
  const [pcbOpen, setPcbOpen] = useState(false);
  const [pcbFitWidth, setPcbFitWidth] = useState<number | undefined>();
  const [pcbFitHeight, setPcbFitHeight] = useState<number | undefined>();

  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const parts = useDocumentStore((s) => s.parts);
  const selectedPartId =
    selectedPartIds.size === 1
      ? Array.from(selectedPartIds).find((id) => parts.some((p) => p.id === id)) ?? null
      : null;

  useEffect(() => {
    const openIf = (setter: (v: boolean) => void) => () => {
      if (selectedPartId) setter(true);
    };
    const handlers: Record<string, () => void> = {
      "vcad:insert-instance": () => setInsertOpen(true),
      "vcad:open-joint-dialog": () => setJointOpen(true),
      "vcad:open-text-dialog": () => setTextOpen(true),
      "vcad:open-pcb-dialog": () => {
        setPcbFitWidth(undefined);
        setPcbFitHeight(undefined);
        setPcbOpen(true);
      },
      "vcad:apply-fillet": openIf(setFilletOpen),
      "vcad:apply-chamfer": openIf(setChamferOpen),
      "vcad:apply-shell": openIf(setShellOpen),
      "vcad:apply-pattern": openIf(setPatternOpen),
      "vcad:apply-mirror": openIf(setMirrorOpen),
      "vcad:apply-stitch": openIf(setStitchOpen),
    };
    for (const [name, fn] of Object.entries(handlers)) {
      window.addEventListener(name, fn);
    }
    const onFitPcb = (e: Event) => {
      const detail = (e as CustomEvent<{ width: number; height: number }>).detail;
      setPcbFitWidth(detail.width);
      setPcbFitHeight(detail.height);
      setPcbOpen(true);
    };
    window.addEventListener("vcad:fit-pcb-dialog", onFitPcb);
    return () => {
      for (const [name, fn] of Object.entries(handlers)) {
        window.removeEventListener(name, fn);
      }
      window.removeEventListener("vcad:fit-pcb-dialog", onFitPcb);
    };
  }, [selectedPartId]);

  return (
    <>
      <InsertInstanceDialog open={insertOpen} onOpenChange={setInsertOpen} />
      <AddJointDialog open={jointOpen} onOpenChange={setJointOpen} />
      <TextDialog open={textOpen} onOpenChange={setTextOpen} />
      <NewPcbDialog
        open={pcbOpen}
        onOpenChange={setPcbOpen}
        initialWidth={pcbFitWidth}
        initialHeight={pcbFitHeight}
      />
      {selectedPartId && (
        <>
          <FilletChamferDialog
            open={filletOpen}
            onOpenChange={setFilletOpen}
            mode="fillet"
            partId={selectedPartId}
          />
          <FilletChamferDialog
            open={chamferOpen}
            onOpenChange={setChamferOpen}
            mode="chamfer"
            partId={selectedPartId}
          />
          <ShellDialog
            open={shellOpen}
            onOpenChange={setShellOpen}
            partId={selectedPartId}
          />
          <PatternDialog
            open={patternOpen}
            onOpenChange={setPatternOpen}
            partId={selectedPartId}
          />
          <MirrorDialog
            open={mirrorOpen}
            onOpenChange={setMirrorOpen}
            partId={selectedPartId}
          />
          <StitchDialog
            open={stitchOpen}
            onOpenChange={setStitchOpen}
            partId={selectedPartId}
          />
        </>
      )}
    </>
  );
}
