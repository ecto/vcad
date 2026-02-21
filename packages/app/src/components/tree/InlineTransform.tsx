import { useState } from "react";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { CaretDown } from "@phosphor-icons/react/dist/ssr/CaretDown";
import { ScrubInput } from "@/components/ui/scrub-input";
import { useDocumentStore } from "@vcad/core";
import type { PartInfo } from "@vcad/core";
import type { Vec3 } from "@vcad/ir";

interface CollapsibleSectionProps {
  title: string;
  children: React.ReactNode;
  defaultExpanded?: boolean;
}

function CollapsibleSection({
  title,
  children,
  defaultExpanded = false,
}: CollapsibleSectionProps) {
  const [expanded, setExpanded] = useState(defaultExpanded);

  return (
    <div className="px-2">
      <button
        onClick={() => setExpanded(!expanded)}
        className="flex items-center gap-1 py-0.5 text-[10px] text-text-muted hover:text-text w-full"
      >
        {expanded ? <CaretDown size={10} /> : <CaretRight size={10} />}
        <span>{title}</span>
      </button>
      {expanded && <div className="pt-0.5 pb-1">{children}</div>}
    </div>
  );
}

interface InlinePositionSectionProps {
  part: PartInfo;
  offset: Vec3;
}

export function InlinePositionSection({
  part,
  offset,
}: InlinePositionSectionProps) {
  const setTranslation = useDocumentStore((s) => s.setTranslation);

  return (
    <CollapsibleSection title="Position">
      <div className="grid grid-cols-3 gap-1">
        <ScrubInput
          label="X"
          value={offset.x}
          onChange={(v) => setTranslation(part.id, { ...offset, x: v })}
          unit="mm"
          compact
        />
        <ScrubInput
          label="Y"
          value={offset.y}
          onChange={(v) => setTranslation(part.id, { ...offset, y: v })}
          unit="mm"
          compact
        />
        <ScrubInput
          label="Z"
          value={offset.z}
          onChange={(v) => setTranslation(part.id, { ...offset, z: v })}
          unit="mm"
          compact
        />
      </div>
    </CollapsibleSection>
  );
}

interface InlineRotationSectionProps {
  part: PartInfo;
  angles: Vec3;
}

export function InlineRotationSection({
  part,
  angles,
}: InlineRotationSectionProps) {
  const setRotation = useDocumentStore((s) => s.setRotation);

  return (
    <CollapsibleSection title="Rotation">
      <div className="grid grid-cols-3 gap-1">
        <ScrubInput
          label="X"
          value={angles.x}
          step={1}
          onChange={(v) => setRotation(part.id, { ...angles, x: v })}
          unit="deg"
          compact
        />
        <ScrubInput
          label="Y"
          value={angles.y}
          step={1}
          onChange={(v) => setRotation(part.id, { ...angles, y: v })}
          unit="deg"
          compact
        />
        <ScrubInput
          label="Z"
          value={angles.z}
          step={1}
          onChange={(v) => setRotation(part.id, { ...angles, z: v })}
          unit="deg"
          compact
        />
      </div>
    </CollapsibleSection>
  );
}
