import { useState, useRef, useEffect } from "react";
import * as THREE from "three";
import { Html } from "@react-three/drei";
import { useUiStore, useDocumentStore, useEngineStore, isPrimitivePart, isTextPart } from "@vcad/core";
import { ScrubInput } from "@/components/ui/scrub-input";
import type { CsgOp } from "@vcad/ir";

function InlineRenameInput({
  partId,
  currentName,
  onDone,
}: {
  partId: string;
  currentName: string;
  onDone: () => void;
}) {
  const [text, setText] = useState(currentName);
  const inputRef = useRef<HTMLInputElement>(null);
  const renamePart = useDocumentStore((s) => s.renamePart);

  useEffect(() => {
    inputRef.current?.select();
  }, []);

  function commit() {
    const trimmed = text.trim();
    if (trimmed && trimmed !== currentName) {
      renamePart(partId, trimmed);
    }
    onDone();
  }

  return (
    <input
      ref={inputRef}
      type="text"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
        if (e.key === "Escape") onDone();
      }}
      className="w-full  border border-brand bg-surface px-1.5 py-0.5 text-xs text-text outline-none"
      autoFocus
    />
  );
}

function CubeSizeInputs({
  partId,
  op,
}: {
  partId: string;
  op: Extract<CsgOp, { type: "Cube" }>;
}) {
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);
  const { size } = op;

  return (
    <div className="mb-2 flex flex-col gap-1">
      <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted">
        Size
      </div>
      <div className="grid grid-cols-3 gap-1">
        <ScrubInput
          label="W"
          value={size.x}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(partId, {
              type: "Cube",
              size: { ...size, x: v },
            })
          }
          unit="mm"
          className="text-[10px]"
        />
        <ScrubInput
          label="H"
          value={size.y}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(partId, {
              type: "Cube",
              size: { ...size, y: v },
            })
          }
          unit="mm"
          className="text-[10px]"
        />
        <ScrubInput
          label="D"
          value={size.z}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(partId, {
              type: "Cube",
              size: { ...size, z: v },
            })
          }
          unit="mm"
          className="text-[10px]"
        />
      </div>
    </div>
  );
}

function CylinderSizeInputs({
  partId,
  op,
}: {
  partId: string;
  op: Extract<CsgOp, { type: "Cylinder" }>;
}) {
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);

  return (
    <div className="mb-2 flex flex-col gap-1">
      <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted">
        Size
      </div>
      <div className="grid grid-cols-2 gap-1">
        <ScrubInput
          label="R"
          value={op.radius}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(partId, { ...op, radius: v })
          }
          unit="mm"
          className="text-[10px]"
        />
        <ScrubInput
          label="H"
          value={op.height}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(partId, { ...op, height: v })
          }
          unit="mm"
          className="text-[10px]"
        />
      </div>
    </div>
  );
}

function SphereSizeInputs({
  partId,
  op,
}: {
  partId: string;
  op: Extract<CsgOp, { type: "Sphere" }>;
}) {
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);

  return (
    <div className="mb-2 flex flex-col gap-1">
      <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted">
        Size
      </div>
      <ScrubInput
        label="R"
        value={op.radius}
        min={0.1}
        onChange={(v) =>
          updatePrimitiveOp(partId, { ...op, radius: v })
        }
        unit="mm"
        className="text-[10px]"
      />
    </div>
  );
}

export function InlineProperties() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const featureTreeOpen = useUiStore((s) => s.featureTreeOpen);
  const parts = useDocumentStore((s) => s.parts);
  const document = useDocumentStore((s) => s.document);
  const setTranslation = useDocumentStore((s) => s.setTranslation);
  const scene = useEngineStore((s) => s.scene);

  const [isRenaming, setIsRenaming] = useState(false);

  // Only show for single selection
  if (selectedPartIds.size !== 1 || isDraggingGizmo) return null;

  const partId = Array.from(selectedPartIds)[0]!;
  const part = parts.find((p) => p.id === partId);
  if (!part) return null;

  const partIndex = parts.findIndex((p) => p.id === partId);
  const evalPart = scene?.parts[partIndex];
  if (!evalPart) return null;

  // Compute bounding box center for positioning
  const mesh = evalPart.mesh;
  if (!mesh.positions.length) return null;

  const box = new THREE.Box3();
  const pos = new THREE.Vector3();
  for (let i = 0; i < mesh.positions.length; i += 3) {
    pos.set(mesh.positions[i]!, mesh.positions[i + 1]!, mesh.positions[i + 2]!);
    box.expandByPoint(pos);
  }

  // Position to the right of the bounding box
  const anchorPosition = new THREE.Vector3(
    box.max.x + 5,
    (box.min.y + box.max.y) / 2,
    box.max.z,
  );

  // Get translation offset
  const translateNode = document.nodes[String(part.translateNodeId)];
  const offset =
    translateNode?.op.type === "Translate"
      ? translateNode.op.offset
      : { x: 0, y: 0, z: 0 };

  // Get primitive dimensions
  const primNode = isPrimitivePart(part) ? document.nodes[String(part.primitiveNodeId)] : null;
  const primOp = primNode?.op;

  // Get text info
  const textNode = isTextPart(part) ? document.nodes[String(part.textNodeId)] : null;
  const textOp = textNode?.op.type === "Text2D" ? textNode.op : null;

  // If the full PropertyPanel is open, just show the name
  const showCompact = featureTreeOpen;

  return (
    <Html position={anchorPosition} style={{ pointerEvents: "auto" }}>
      <div className=" border border-border bg-card p-2 shadow-lg min-w-[140px]">
        {/* Part name */}
        <div className="mb-2">
          {isRenaming ? (
            <InlineRenameInput
              partId={part.id}
              currentName={part.name}
              onDone={() => setIsRenaming(false)}
            />
          ) : (
            <button
              onClick={() => setIsRenaming(true)}
              className="w-full text-left text-xs font-medium text-text hover:text-brand truncate"
            >
              {part.name}
            </button>
          )}
        </div>

        {!showCompact && (
          <>
            {/* Dimensions for primitives */}
            {isPrimitivePart(part) && primOp?.type === "Cube" && (
              <CubeSizeInputs partId={part.id} op={primOp} />
            )}

            {isPrimitivePart(part) && primOp?.type === "Cylinder" && (
              <CylinderSizeInputs partId={part.id} op={primOp} />
            )}

            {isPrimitivePart(part) && primOp?.type === "Sphere" && (
              <SphereSizeInputs partId={part.id} op={primOp} />
            )}

            {/* Text info (read-only preview) */}
            {isTextPart(part) && textOp && (
              <div className="mb-2 flex flex-col gap-1">
                <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted">
                  Text
                </div>
                <div className="text-xs text-text truncate max-w-[120px]" title={textOp.text}>
                  "{textOp.text.length > 15 ? textOp.text.slice(0, 15) + "…" : textOp.text}"
                </div>
                <div className="grid grid-cols-2 gap-1 text-[10px] text-text-muted">
                  <span>H: {textOp.height}mm</span>
                </div>
              </div>
            )}

            {/* Position */}
            <div className="flex flex-col gap-1">
              <div className="text-[10px] font-bold uppercase tracking-wider text-text-muted">
                Position
              </div>
              <div className="grid grid-cols-3 gap-1">
                <ScrubInput
                  label="X"
                  value={offset.x}
                  onChange={(v) =>
                    setTranslation(part.id, { ...offset, x: v })
                  }
                  unit="mm"
                  className="text-[10px]"
                />
                <ScrubInput
                  label="Y"
                  value={offset.y}
                  onChange={(v) =>
                    setTranslation(part.id, { ...offset, y: v })
                  }
                  unit="mm"
                  className="text-[10px]"
                />
                <ScrubInput
                  label="Z"
                  value={offset.z}
                  onChange={(v) =>
                    setTranslation(part.id, { ...offset, z: v })
                  }
                  unit="mm"
                  className="text-[10px]"
                />
              </div>
            </div>
          </>
        )}
      </div>
    </Html>
  );
}
