/**
 * Feature-tree section for the atomic/molecular domain.
 *
 * Renders the molecule as a small DAG — the system, its species (with counts),
 * bonds, and the current selection — plus a representation toggle. Reads the
 * dedicated {@link useMoleculeStore}; selecting a species or the selected-atom
 * row drives the same atom selection the viewport uses.
 */

import type { ReactNode } from "react";
import { Atom, CircleDot, Link2, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useMoleculeStore } from "@/stores/molecule-store";
import type { AtomRepresentation } from "./AtomInstances";

const REPS: { id: AtomRepresentation; label: string }[] = [
  { id: "ball_and_stick", label: "Ball" },
  { id: "space_filling", label: "Fill" },
  { id: "wireframe", label: "Wire" },
];

function Row({
  depth,
  icon,
  label,
  suffix,
  selected,
  onClick,
}: {
  depth: number;
  icon: ReactNode;
  label: ReactNode;
  suffix?: ReactNode;
  selected?: boolean;
  onClick?: () => void;
}) {
  return (
    <div
      className={cn(
        "group flex items-center gap-1 px-2 py-1 text-xs rounded",
        onClick && "cursor-pointer",
        selected
          ? "bg-brand/20 text-brand backdrop-blur-sm"
          : "text-text-muted/90 hover:bg-surface/60 hover:text-text hover:backdrop-blur-sm",
      )}
      style={{ paddingLeft: `${8 + depth * 12}px` }}
      onClick={onClick}
    >
      <span className="shrink-0">{icon}</span>
      <span className="flex-1 overflow-hidden whitespace-nowrap">{label}</span>
      {suffix && <span className="shrink-0 text-text-muted/60 tabular-nums">{suffix}</span>}
    </div>
  );
}

export function MoleculeTreeSection() {
  const molecule = useMoleculeStore((s) => s.molecule);
  const representation = useMoleculeStore((s) => s.representation);
  const setRepresentation = useMoleculeStore((s) => s.setRepresentation);
  const selectedAtomIndex = useMoleculeStore((s) => s.selectedAtomIndex);
  const selectAtom = useMoleculeStore((s) => s.selectAtom);
  const clear = useMoleculeStore((s) => s.clear);

  if (!molecule || molecule.positions.length === 0) return null;

  // Per-species atom counts, keyed by species index.
  const counts = new Map<number, number>();
  for (const s of molecule.speciesIdx) counts.set(s, (counts.get(s) ?? 0) + 1);

  const bondCount = molecule.bonds?.length ?? 0;
  const selectedSpecies =
    selectedAtomIndex != null ? molecule.species[molecule.speciesIdx[selectedAtomIndex]!] : null;

  // Select the first atom of a given species (viewport highlights it).
  const selectFirstOfSpecies = (speciesIdx: number) => {
    const i = molecule.speciesIdx.findIndex((s) => s === speciesIdx);
    if (i >= 0) selectAtom(i);
  };

  return (
    <div className="space-y-0.5">
      <div className="flex items-center gap-1 px-2 pt-1">
        <span className="text-[10px] font-medium uppercase tracking-wider text-text-muted/70">
          Molecule
        </span>
        <button
          className="ml-auto p-0.5 text-text-muted/60 hover:text-text cursor-pointer"
          title="Remove molecule"
          aria-label="Remove molecule"
          onClick={clear}
        >
          <X size={11} />
        </button>
      </div>

      {/* System root */}
      <Row
        depth={0}
        icon={<Atom size={14} />}
        label={molecule.name ?? "Molecule"}
        suffix={`${molecule.positions.length} atoms`}
      />

      {/* Species */}
      {molecule.species.map((sp, idx) => (
        <Row
          key={idx}
          depth={1}
          icon={
            <span
              className="inline-block h-2.5 w-2.5 rounded-full ring-1 ring-black/20"
              style={{
                background: sp.color
                  ? `rgb(${sp.color.map((c) => Math.round(c * 255)).join(",")})`
                  : "#8a8a8a",
              }}
            />
          }
          label={sp.element}
          suffix={`×${counts.get(idx) ?? 0}`}
          selected={selectedSpecies === sp}
          onClick={() => selectFirstOfSpecies(idx)}
        />
      ))}

      {/* Bonds */}
      <Row depth={1} icon={<Link2 size={13} />} label="Bonds" suffix={bondCount} />

      {/* Current selection */}
      {selectedAtomIndex != null && (
        <Row
          depth={1}
          icon={<CircleDot size={13} />}
          label={`Atom #${selectedAtomIndex} · ${molecule.species[molecule.speciesIdx[selectedAtomIndex]!]?.element ?? "?"}`}
          selected
          onClick={() => selectAtom(null)}
        />
      )}

      {/* Representation toggle */}
      <div className="flex gap-1 px-2 py-1" style={{ paddingLeft: "20px" }}>
        {REPS.map((r) => (
          <button
            key={r.id}
            onClick={() => setRepresentation(r.id)}
            className={cn(
              "flex-1 rounded px-1 py-0.5 text-[10px] cursor-pointer transition-colors",
              representation === r.id
                ? "bg-brand/20 text-brand"
                : "text-text-muted/70 hover:bg-surface/60 hover:text-text",
            )}
          >
            {r.label}
          </button>
        ))}
      </div>
    </div>
  );
}
