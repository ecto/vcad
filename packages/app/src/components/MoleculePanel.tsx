/**
 * Floating panel to load atomic-structure demos into the viewport and switch
 * representation. Reads/writes the dedicated {@link useMoleculeStore}; the
 * viewport's `AtomInstances` renders whatever is selected. Independent of the
 * CAD document, so it works without any kernel round-trip.
 */

import { Atom, X } from "lucide-react";
import { MOLECULE_DEMOS, useMoleculeStore } from "../stores/molecule-store";
import type { AtomRepresentation } from "./AtomInstances";

const REPS: { id: AtomRepresentation; label: string }[] = [
  { id: "ball_and_stick", label: "Ball & stick" },
  { id: "space_filling", label: "Space-fill" },
  { id: "wireframe", label: "Wire" },
];

export function MoleculePanel() {
  const activeDemoId = useMoleculeStore((s) => s.activeDemoId);
  const representation = useMoleculeStore((s) => s.representation);
  const molecule = useMoleculeStore((s) => s.molecule);
  const loadDemo = useMoleculeStore((s) => s.loadDemo);
  const setRepresentation = useMoleculeStore((s) => s.setRepresentation);
  const clear = useMoleculeStore((s) => s.clear);

  const atomCount = molecule?.positions.length ?? 0;

  return (
    <div className="pointer-events-auto w-56 border border-border bg-card/95 backdrop-blur-sm shadow-lg">
      <div className="flex items-center gap-1.5 border-b border-border px-3 py-2">
        <Atom size={13} className="text-accent" />
        <span className="text-xs font-medium tracking-tight text-text">Atomic structures</span>
        {molecule && (
          <button
            className="ml-auto p-0.5 text-text-muted hover:text-text cursor-pointer"
            onClick={clear}
            title="Clear molecule"
            aria-label="Clear molecule"
          >
            <X size={13} />
          </button>
        )}
      </div>

      <div className="flex flex-col gap-0.5 p-1.5">
        {MOLECULE_DEMOS.map((demo) => (
          <button
            key={demo.id}
            onClick={() => loadDemo(demo.id)}
            aria-pressed={activeDemoId === demo.id}
            className={
              "flex flex-col items-start px-2 py-1.5 text-left cursor-pointer border border-transparent transition-colors " +
              (activeDemoId === demo.id
                ? "bg-accent/10 border-accent/40"
                : "hover:bg-border/40")
            }
          >
            <span className="text-xs text-text">{demo.name}</span>
            <span className="text-[10px] text-text-muted leading-tight">{demo.blurb}</span>
          </button>
        ))}
      </div>

      {molecule && (
        <div className="border-t border-border p-1.5">
          <div className="flex border border-border">
            {REPS.map((r) => (
              <button
                key={r.id}
                onClick={() => setRepresentation(r.id)}
                aria-pressed={representation === r.id}
                className={
                  "flex-1 px-1 py-1 text-[10px] cursor-pointer border-r border-border last:border-r-0 transition-colors " +
                  (representation === r.id
                    ? "bg-accent/15 text-text"
                    : "text-text-muted hover:text-text")
                }
              >
                {r.label}
              </button>
            ))}
          </div>
          <div className="mt-1.5 px-0.5 font-mono text-[10px] text-text-muted tabular-nums">
            {atomCount.toLocaleString()} atoms · {molecule.bonds?.length ?? 0} bonds
          </div>
        </div>
      )}
    </div>
  );
}
