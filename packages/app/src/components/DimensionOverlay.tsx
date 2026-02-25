import { useMemo, useState, useRef, useEffect } from "react";
import * as THREE from "three";
import { Line, Html } from "@react-three/drei";
import { useUiStore, useDocumentStore, useEngineStore, isPrimitivePart } from "@vcad/core";
import type { CsgOp } from "@vcad/ir";

const DIM_COLOR = "#94a3b8"; // muted accent

type ParamKey = "size.x" | "size.y" | "size.z" | "radius" | "height";

interface DimensionInfo {
  start: THREE.Vector3;
  end: THREE.Vector3;
  label: string;
  value: number;
  paramKey: ParamKey;
}

function applyValueToOp(op: CsgOp, paramKey: ParamKey, value: number): CsgOp {
  const newOp = structuredClone(op);
  if (newOp.type === "Cube") {
    if (paramKey === "size.x") newOp.size.x = value;
    else if (paramKey === "size.y") newOp.size.y = value;
    else if (paramKey === "size.z") newOp.size.z = value;
  } else if (newOp.type === "Cylinder") {
    if (paramKey === "radius") newOp.radius = value;
    else if (paramKey === "height") newOp.height = value;
  } else if (newOp.type === "Sphere") {
    if (paramKey === "radius") newOp.radius = value;
  }
  return newOp;
}

interface EditableLabelProps {
  value: number;
  paramKey: ParamKey;
  partId: string;
  primitiveNodeId: number;
}

function EditableDimensionLabel({ value, paramKey, partId, primitiveNodeId }: EditableLabelProps) {
  const [isEditing, setIsEditing] = useState(false);
  const [text, setText] = useState(value.toFixed(1));
  const inputRef = useRef<HTMLInputElement>(null);
  const document = useDocumentStore((s) => s.document);
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);

  // Update text when value changes externally
  useEffect(() => {
    if (!isEditing) {
      setText(value.toFixed(1));
    }
  }, [value, isEditing]);

  // Focus and select on edit start
  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  const commit = () => {
    const parsed = parseFloat(text);
    if (!isNaN(parsed) && parsed > 0) {
      const primNode = document.nodes[String(primitiveNodeId)];
      if (primNode) {
        const newOp = applyValueToOp(primNode.op, paramKey, parsed);
        updatePrimitiveOp(partId, newOp);
      }
    } else {
      // Invalid input, revert
      setText(value.toFixed(1));
    }
    setIsEditing(false);
  };

  const cancel = () => {
    setText(value.toFixed(1));
    setIsEditing(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    e.stopPropagation();
    if (e.key === "Enter") {
      commit();
    } else if (e.key === "Escape") {
      cancel();
    }
  };

  const isRadius = paramKey === "radius";
  const prefix = isRadius ? "r=" : "";
  const suffix = " mm";

  if (isEditing) {
    return (
      <div
        className="bg-surface border border-accent px-1 py-0.5 text-[10px] text-text whitespace-nowrap flex items-center rounded"
        onClick={(e) => e.stopPropagation()}
        onPointerDown={(e) => e.stopPropagation()}
      >
        {prefix}
        <input
          ref={inputRef}
          type="text"
          value={text}
          onChange={(e) => setText(e.target.value)}
          onBlur={commit}
          onKeyDown={handleKeyDown}
          className="w-12 bg-transparent outline-none text-[10px] text-text"
          onClick={(e) => e.stopPropagation()}
          onPointerDown={(e) => e.stopPropagation()}
        />
        {suffix}
      </div>
    );
  }

  return (
    <div
      className="bg-surface border border-border px-1 py-0.5 text-[10px] text-text-muted whitespace-nowrap cursor-pointer hover:border-accent hover:text-text transition-colors rounded"
      onClick={(e) => {
        e.stopPropagation();
        setIsEditing(true);
      }}
      onPointerDown={(e) => e.stopPropagation()}
    >
      {prefix}{value.toFixed(1)}{suffix}
    </div>
  );
}

interface DimensionLineProps {
  start: THREE.Vector3;
  end: THREE.Vector3;
  label: string;
  offset?: THREE.Vector3;
  // Edit props
  editable?: boolean;
  value?: number;
  paramKey?: ParamKey;
  partId?: string;
  primitiveNodeId?: number;
}

function DimensionLine({
  start,
  end,
  label,
  offset = new THREE.Vector3(),
  editable,
  value,
  paramKey,
  partId,
  primitiveNodeId,
}: DimensionLineProps) {
  const mid = new THREE.Vector3().lerpVectors(start, end, 0.5).add(offset);
  const tickSize = 1;
  const dir = new THREE.Vector3().subVectors(end, start).normalize();

  // Perpendicular direction for ticks (in XZ plane for most cases)
  const up = new THREE.Vector3(0, 1, 0);
  const perp = new THREE.Vector3().crossVectors(dir, up).normalize();
  if (perp.length() < 0.1) {
    perp.set(1, 0, 0);
  }

  const startTick1 = start.clone().add(perp.clone().multiplyScalar(tickSize));
  const startTick2 = start.clone().add(perp.clone().multiplyScalar(-tickSize));
  const endTick1 = end.clone().add(perp.clone().multiplyScalar(tickSize));
  const endTick2 = end.clone().add(perp.clone().multiplyScalar(-tickSize));

  const showEditable = editable && value !== undefined && paramKey && partId && primitiveNodeId !== undefined;

  return (
    <>
      {/* Main dimension line */}
      <Line
        points={[start, end]}
        color={DIM_COLOR}
        lineWidth={1}
        transparent
        opacity={0.5}
      />
      {/* Start tick */}
      <Line
        points={[startTick1, startTick2]}
        color={DIM_COLOR}
        lineWidth={1}
        transparent
        opacity={0.5}
      />
      {/* End tick */}
      <Line
        points={[endTick1, endTick2]}
        color={DIM_COLOR}
        lineWidth={1}
        transparent
        opacity={0.5}
      />
      {/* Label */}
      <Html position={mid} center style={{ pointerEvents: showEditable ? "auto" : "none", zIndex: 10 }}>
        {showEditable ? (
          <EditableDimensionLabel
            value={value}
            paramKey={paramKey}
            partId={partId}
            primitiveNodeId={primitiveNodeId}
          />
        ) : (
          <div className="bg-surface border border-border px-1 py-0.5 text-[10px] text-text-muted whitespace-nowrap rounded">
            {label}
          </div>
        )}
      </Html>
    </>
  );
}

export function DimensionOverlay() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const parts = useDocumentStore((s) => s.parts);
  const document = useDocumentStore((s) => s.document);
  const scene = useEngineStore((s) => s.scene);

  const dimensionData = useMemo(() => {
    // Only show for single selected primitive
    if (selectedPartIds.size !== 1) return null;

    const partId = Array.from(selectedPartIds)[0]!;
    const part = parts.find((p) => p.id === partId);
    if (!part || !isPrimitivePart(part)) return null;

    const partIndex = parts.findIndex((p) => p.id === partId);
    const evalPart = scene?.parts[partIndex];
    if (!evalPart) return null;

    // Get primitive dimensions from document
    const primNode = document.nodes[String(part.primitiveNodeId)];
    if (!primNode) return null;

    // Get translation offset
    const translateNode = document.nodes[String(part.translateNodeId)];
    const offset =
      translateNode?.op.type === "Translate"
        ? translateNode.op.offset
        : { x: 0, y: 0, z: 0 };

    const center = new THREE.Vector3(offset.x, offset.y, offset.z);

    let dimensions: DimensionInfo[] | null = null;

    if (primNode.op.type === "Cube") {
      const { size } = primNode.op;
      const halfW = size.x / 2;
      const halfH = size.y / 2;
      const halfD = size.z / 2;

      dimensions = [
        // Width (X)
        {
          start: new THREE.Vector3(center.x - halfW, center.y - halfH - 3, center.z + halfD),
          end: new THREE.Vector3(center.x + halfW, center.y - halfH - 3, center.z + halfD),
          label: `${size.x.toFixed(1)} mm`,
          value: size.x,
          paramKey: "size.x" as const,
        },
        // Height (Y)
        {
          start: new THREE.Vector3(center.x + halfW + 3, center.y - halfH, center.z + halfD),
          end: new THREE.Vector3(center.x + halfW + 3, center.y + halfH, center.z + halfD),
          label: `${size.y.toFixed(1)} mm`,
          value: size.y,
          paramKey: "size.y" as const,
        },
        // Depth (Z)
        {
          start: new THREE.Vector3(center.x + halfW + 3, center.y - halfH - 3, center.z - halfD),
          end: new THREE.Vector3(center.x + halfW + 3, center.y - halfH - 3, center.z + halfD),
          label: `${size.z.toFixed(1)} mm`,
          value: size.z,
          paramKey: "size.z" as const,
        },
      ];
    }

    if (primNode.op.type === "Cylinder") {
      const { radius, height } = primNode.op;
      const halfH = height / 2;

      dimensions = [
        // Radius
        {
          start: new THREE.Vector3(center.x, center.y - halfH - 3, center.z),
          end: new THREE.Vector3(center.x + radius, center.y - halfH - 3, center.z),
          label: `r=${radius.toFixed(1)} mm`,
          value: radius,
          paramKey: "radius" as const,
        },
        // Height
        {
          start: new THREE.Vector3(center.x + radius + 3, center.y - halfH, center.z),
          end: new THREE.Vector3(center.x + radius + 3, center.y + halfH, center.z),
          label: `${height.toFixed(1)} mm`,
          value: height,
          paramKey: "height" as const,
        },
      ];
    }

    if (primNode.op.type === "Sphere") {
      const { radius } = primNode.op;

      dimensions = [
        // Radius
        {
          start: new THREE.Vector3(center.x, center.y, center.z),
          end: new THREE.Vector3(center.x + radius, center.y, center.z),
          label: `r=${radius.toFixed(1)} mm`,
          value: radius,
          paramKey: "radius" as const,
        },
      ];
    }

    return dimensions ? { dimensions, partId, primitiveNodeId: part.primitiveNodeId } : null;
  }, [selectedPartIds, parts, document, scene]);

  if (!dimensionData || isDraggingGizmo) return null;

  const { dimensions, partId, primitiveNodeId } = dimensionData;

  return (
    <>
      {dimensions.map((dim, i) => (
        <DimensionLine
          key={i}
          start={dim.start}
          end={dim.end}
          label={dim.label}
          editable
          value={dim.value}
          paramKey={dim.paramKey}
          partId={partId}
          primitiveNodeId={primitiveNodeId}
        />
      ))}
    </>
  );
}
