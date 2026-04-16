import { useEffect, useRef, useMemo, lazy, Suspense } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { CaretLeft } from "@phosphor-icons/react/dist/ssr/CaretLeft";

function BackButton() {
  const setSidebarPane = useUiStore((s) => s.setSidebarPane);
  return (
    <button
      onClick={() => setSidebarPane("tree")}
      className="flex h-6 w-6 -ml-1 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
      aria-label="Back to tree"
      title="Back to tree"
    >
      <CaretLeft size={14} />
    </button>
  );
}
import { Tooltip } from "@/components/ui/tooltip";
import { ScrubInput } from "@/components/ui/scrub-input";
import { useDocumentStore, useUiStore, isPrimitivePart, isBooleanPart, isSweepPart, isEmbroideryPatternPart, isStitchPart, isPcbBoardPart, isExtrudePart, isRevolvePart, isFilletPart, isChamferPart, isShellPart, isLinearPatternPart, isCircularPatternPart, isLoftPart, isTextPart, isMirrorPart, f64, vec3, bool } from "@vcad/core";
import { useElectronicsStore } from "@/stores/electronics-store";
import { useEmbroideryStore } from "@/stores/embroidery-store";
import type { PartInfo, PrimitivePartInfo, BooleanPartInfo, BooleanType, SweepPartInfo, ExtrudePartInfo, RevolvePartInfo, FilletPartInfo, ChamferPartInfo, ShellPartInfo, LinearPatternPartInfo, CircularPatternPartInfo, LoftPartInfo, TextPartInfo, MirrorPartInfo } from "@vcad/core";
import type { Vec3, PartInstance, Joint, JointKind } from "@vcad/ir";
import { identityTransform } from "@vcad/ir";
import { cn } from "@/lib/utils";
import { MaterialSelector, InstanceMaterialSelector } from "@/components/materials";
import { useVolumeCalculation } from "@/hooks/useVolumeCalculation";

const EmbroideryProperties = lazy(() =>
  import("@/components/embroidery/EmbroideryProperties").then((m) => ({
    default: m.EmbroideryProperties,
  }))
);


function SectionHeader({ children, tooltip }: { children: string; tooltip?: string }) {
  const content = (
    <div className="text-[10px] font-medium uppercase tracking-wider text-text-muted pt-2 pb-1">
      {children}
    </div>
  );

  if (tooltip) {
    return (
      <Tooltip content={tooltip} side="left">
        <div className="cursor-help">{content}</div>
      </Tooltip>
    );
  }
  return content;
}

function PartTypeBadge({ kind }: { kind: string }) {
  return (
    <span className="text-[10px] px-1.5 py-0.5 bg-hover border border-border/50 text-text-muted uppercase tracking-wide">
      {kind}
    </span>
  );
}

function MaterialPicker({ partId }: { partId: string }) {
  const document = useDocumentStore((s) => s.document);
  const parts = useDocumentStore((s) => s.parts);
  const part = parts.find((p) => p.id === partId);
  const volumeMm3 = useVolumeCalculation(partId);

  if (!part) return null;

  const rootEntry = document.roots.find((r) => r.root === part.translateNodeId);
  const currentMaterial = rootEntry?.material ?? "default";

  return (
    <div>
      <SectionHeader tooltip="Assign a material to this part">Material</SectionHeader>
      <MaterialSelector
        partId={partId}
        currentMaterialKey={currentMaterial}
        volumeMm3={volumeMm3}
      />
    </div>
  );
}

function PositionSection({
  part,
  offset,
}: {
  part: PartInfo;
  offset: Vec3;
}) {
  const setTranslation = useDocumentStore((s) => s.setTranslation);

  return (
    <div>
      <SectionHeader tooltip="Position offset from origin (mm)">Position</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="X"
          value={offset.x}
          onChange={(v) => setTranslation(part.id, { ...offset, x: v })}
          unit="mm"
        />
        <ScrubInput
          label="Y"
          value={offset.y}
          onChange={(v) => setTranslation(part.id, { ...offset, y: v })}
          unit="mm"
        />
        <ScrubInput
          label="Z"
          value={offset.z}
          onChange={(v) => setTranslation(part.id, { ...offset, z: v })}
          unit="mm"
        />
      </div>
    </div>
  );
}

function RotationSection({
  part,
  angles,
}: {
  part: PartInfo;
  angles: Vec3;
}) {
  const setRotation = useDocumentStore((s) => s.setRotation);

  return (
    <div>
      <SectionHeader tooltip="Rotation angles around each axis (degrees)">Rotation</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="X"
          value={angles.x}
          step={1}
          onChange={(v) => setRotation(part.id, { ...angles, x: v })}
          unit="°"
        />
        <ScrubInput
          label="Y"
          value={angles.y}
          step={1}
          onChange={(v) => setRotation(part.id, { ...angles, y: v })}
          unit="°"
        />
        <ScrubInput
          label="Z"
          value={angles.z}
          step={1}
          onChange={(v) => setRotation(part.id, { ...angles, z: v })}
          unit="°"
        />
      </div>
    </div>
  );
}

function ScaleSection({
  part,
  factor,
}: {
  part: PartInfo;
  factor: Vec3;
}) {
  const setScale = useDocumentStore((s) => s.setScale);

  return (
    <div>
      <SectionHeader tooltip="Scale factor along each axis">Scale</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="X"
          value={factor.x}
          step={0.1}
          onChange={(v) => setScale(part.id, { ...factor, x: v })}
        />
        <ScrubInput
          label="Y"
          value={factor.y}
          step={0.1}
          onChange={(v) => setScale(part.id, { ...factor, y: v })}
        />
        <ScrubInput
          label="Z"
          value={factor.z}
          step={0.1}
          onChange={(v) => setScale(part.id, { ...factor, z: v })}
        />
      </div>
    </div>
  );
}

function CubeDimensions({ part }: { part: PrimitivePartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);

  const node = document.nodes[String(part.primitiveNodeId)];
  if (!node || node.op.type !== "Cube") return null;

  const { size } = node.op;

  return (
    <div>
      <SectionHeader tooltip="Width, height, and depth of the box (mm)">Dimensions</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="W"
          value={size.x}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(part.id, { type: "Cube", size: { ...size, x: v } })
          }
          unit="mm"
        />
        <ScrubInput
          label="H"
          value={size.y}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(part.id, { type: "Cube", size: { ...size, y: v } })
          }
          unit="mm"
        />
        <ScrubInput
          label="D"
          value={size.z}
          min={0.1}
          onChange={(v) =>
            updatePrimitiveOp(part.id, { type: "Cube", size: { ...size, z: v } })
          }
          unit="mm"
        />
      </div>
    </div>
  );
}

function CylinderDimensions({ part }: { part: PrimitivePartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);

  const node = document.nodes[String(part.primitiveNodeId)];
  if (!node || node.op.type !== "Cylinder") return null;

  const op = node.op;

  return (
    <div>
      <SectionHeader tooltip="Radius and height of the cylinder (mm)">Dimensions</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="R"
          value={op.radius}
          min={0.1}
          onChange={(v) => updatePrimitiveOp(part.id, { ...op, radius: v })}
          unit="mm"
        />
        <ScrubInput
          label="H"
          value={op.height}
          min={0.1}
          onChange={(v) => updatePrimitiveOp(part.id, { ...op, height: v })}
          unit="mm"
        />
      </div>
    </div>
  );
}

function SphereDimensions({ part }: { part: PrimitivePartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const updatePrimitiveOp = useDocumentStore((s) => s.updatePrimitiveOp);

  const node = document.nodes[String(part.primitiveNodeId)];
  if (!node || node.op.type !== "Sphere") return null;

  const op = node.op;

  return (
    <div>
      <SectionHeader tooltip="Radius of the sphere (mm)">Dimensions</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="R"
          value={op.radius}
          min={0.1}
          onChange={(v) => updatePrimitiveOp(part.id, { ...op, radius: v })}
          unit="mm"
        />
      </div>
    </div>
  );
}

function SweepProperties({ part }: { part: SweepPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const updateSweepOp = useDocumentStore((s) => s.updateSweepOp);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.sweepNodeId)];
  if (!node || node.op.type !== "Sweep") return null;

  const op = node.op;
  const helixPath = op.path.type === "Helix" ? op.path : null;

  return (
    <div className="space-y-3">
      {/* Sketch reference */}
      <div>
        <SectionHeader tooltip="Profile sketch used for sweep">Profile</SectionHeader>
        <ReadOnlyParam label="" value={`Sketch #${op.sketch}`} tooltip="Sketch reference — editing not yet supported" />
      </div>

      {/* Path info */}
      <div>
        <SectionHeader tooltip="Path curve for sweep">Path</SectionHeader>
        <ReadOnlyParam label="" value={op.path.type === "Helix" ? "Helix" : "Line"} tooltip="Path type — editing not yet supported" />
      </div>

      {/* Helix path parameters */}
      {helixPath && (
        <div>
          <SectionHeader tooltip="Helix path parameters">Helix Path</SectionHeader>
          <div className="space-y-0.5">
            <ScrubInput
              label="Radius"
              value={helixPath.radius}
              min={0.1}
              step={0.5}
              onChange={(v) =>
                updateSweepOp(part.id, { path: { ...helixPath, radius: v } })
              }
              unit="mm"
              {...scrub}
            />
            <ScrubInput
              label="Pitch"
              value={helixPath.pitch}
              min={0.1}
              step={0.5}
              onChange={(v) =>
                updateSweepOp(part.id, { path: { ...helixPath, pitch: v } })
              }
              unit="mm"
              {...scrub}
            />
            <ScrubInput
              label="Height"
              value={helixPath.height}
              min={0.1}
              step={1}
              onChange={(v) =>
                updateSweepOp(part.id, { path: { ...helixPath, height: v } })
              }
              unit="mm"
              {...scrub}
            />
            <ScrubInput
              label="Turns"
              value={helixPath.turns}
              min={0.25}
              step={0.25}
              onChange={(v) =>
                updateSweepOp(part.id, { path: { ...helixPath, turns: v } })
              }
              {...scrub}
            />
          </div>
        </div>
      )}

      {/* Orientation - initial profile rotation */}
      <div>
        <SectionHeader tooltip="Initial rotation of the profile around the path">
          Orientation
        </SectionHeader>
        <ScrubInput
          label="Angle"
          value={(op.orientation ?? 0) * (180 / Math.PI)}
          step={5}
          onChange={(v) =>
            updateSweepOp(part.id, { orientation: v * (Math.PI / 180) })
          }
          unit="°"
          {...scrub}
        />
      </div>

      {/* Sweep options */}
      <div>
        <SectionHeader tooltip="Twist angle along the sweep path">Twist</SectionHeader>
        <ScrubInput
          label="Angle"
          value={(op.twist_angle ?? 0) * (180 / Math.PI)}
          step={5}
          onChange={(v) =>
            updateSweepOp(part.id, { twist_angle: v * (Math.PI / 180) })
          }
          unit="°"
          {...scrub}
        />
      </div>

      {/* Scale variation */}
      <div>
        <SectionHeader tooltip="Scale factor at start and end of sweep">Scale</SectionHeader>
        <div className="space-y-0.5">
          <ScrubInput
            label="Start"
            value={op.scale_start ?? 1}
            min={0.1}
            step={0.1}
            onChange={(v) => updateSweepOp(part.id, { scale_start: v })}
            {...scrub}
          />
          <ScrubInput
            label="End"
            value={op.scale_end ?? 1}
            min={0.1}
            step={0.1}
            onChange={(v) => updateSweepOp(part.id, { scale_end: v })}
            {...scrub}
          />
        </div>
      </div>

      {/* Quality settings */}
      <div>
        <SectionHeader tooltip="Higher values = smoother but more polygons">Quality</SectionHeader>
        <div className="space-y-0.5">
          <ScrubInput
            label="Path Segments"
            value={op.path_segments ?? 0}
            min={0}
            max={500}
            step={10}
            onChange={(v) => updateSweepOp(part.id, { path_segments: v })}
            {...scrub}
          />
          <div className="text-[10px] text-text-muted pl-1 pb-1">0 = auto</div>
          <ScrubInput
            label="Arc Segments"
            value={op.arc_segments ?? 8}
            min={1}
            max={32}
            step={1}
            onChange={(v) => updateSweepOp(part.id, { arc_segments: v })}
            {...scrub}
          />
        </div>
      </div>
    </div>
  );
}

function ReadOnlyParam({ label, value, tooltip }: { label: string; value: string; tooltip?: string }) {
  const content = (
    <div className="flex items-center gap-1.5 text-xs">
      <span className="shrink-0 text-[10px] w-4 text-text-muted font-medium">{label}</span>
      <span className="flex-1 min-w-0 bg-card border border-border px-2 py-1 text-xs text-text-muted truncate opacity-60 cursor-not-allowed">
        {value}
      </span>
    </div>
  );
  if (tooltip) {
    return <Tooltip content={tooltip} side="top"><div>{content}</div></Tooltip>;
  }
  return content;
}

function useScrubDragging() {
  const setParameterDragging = useDocumentStore((s) => s.setParameterDragging);
  return useMemo(() => ({
    onScrubStart: () => setParameterDragging(true),
    onScrubEnd: () => setParameterDragging(false),
  }), [setParameterDragging]);
}

function BooleanProperties({ part }: { part: BooleanPartInfo }) {
  const updateBooleanType = useDocumentStore((s) => s.updateBooleanType);

  return (
    <div>
      <SectionHeader>Operation</SectionHeader>
      <select
        value={part.booleanType}
        onChange={(e) => updateBooleanType(part.id, e.target.value as BooleanType)}
        className="w-full text-xs bg-hover border border-border rounded px-2 py-1 text-text focus:outline-none focus:border-brand"
      >
        <option value="union">Union</option>
        <option value="difference">Difference</option>
        <option value="intersection">Intersection</option>
      </select>
    </div>
  );
}

function ExtrudeProperties({ part }: { part: ExtrudePartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.extrudeNodeId)];
  if (!node || node.op.type !== "Extrude") return null;

  const op = node.op;
  const dir = op.direction;
  const depth = Math.sqrt(dir.x * dir.x + dir.y * dir.y + dir.z * dir.z);

  return (
    <div className="space-y-3">
      <div>
        <SectionHeader tooltip="Extrusion depth along direction vector">Depth</SectionHeader>
        <ScrubInput
          label="Depth"
          value={depth}
          min={0.01}
          onChange={(v) => setFeatureParam(part.id, "depth", f64(v))}
          unit="mm"
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader tooltip="Twist the profile along the extrusion axis">Twist</SectionHeader>
        <ScrubInput
          label="Angle"
          value={(op.twist_angle ?? 0) * (180 / Math.PI)}
          step={5}
          onChange={(v) => setFeatureParam(part.id, "twist_angle", f64(v * (Math.PI / 180)))}
          unit="°"
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader tooltip="Scale factor at end of extrusion (1.0 = no taper)">Taper</SectionHeader>
        <ScrubInput
          label="Scale"
          value={op.scale_end ?? 1}
          min={0.01}
          step={0.1}
          onChange={(v) => setFeatureParam(part.id, "scale_end", f64(v))}
          {...scrub}
        />
      </div>
    </div>
  );
}

function RevolveProperties({ part }: { part: RevolvePartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.revolveNodeId)];
  if (!node || node.op.type !== "Revolve") return null;

  const op = node.op;

  return (
    <div className="space-y-3">
      <div>
        <SectionHeader tooltip="Angle of revolution (degrees)">Angle</SectionHeader>
        <ScrubInput
          label="Angle"
          value={op.angle_deg}
          min={0.1}
          max={360}
          step={5}
          onChange={(v) => setFeatureParam(part.id, "angle_deg", f64(v))}
          unit="°"
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader tooltip="Editing not yet supported">Axis</SectionHeader>
        <div className="space-y-0.5">
          <ReadOnlyParam label="X" value={op.axis_dir.x.toFixed(2)} tooltip="Editing not yet supported" />
          <ReadOnlyParam label="Y" value={op.axis_dir.y.toFixed(2)} tooltip="Editing not yet supported" />
          <ReadOnlyParam label="Z" value={op.axis_dir.z.toFixed(2)} tooltip="Editing not yet supported" />
        </div>
      </div>
    </div>
  );
}

function FilletProperties({ part }: { part: FilletPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.filletNodeId)];
  if (!node || node.op.type !== "Fillet") return null;

  return (
    <div>
      <SectionHeader tooltip="Fillet radius (mm)">Radius</SectionHeader>
      <ScrubInput
        label="R"
        value={node.op.radius}
        min={0.1}
        step={0.5}
        onChange={(v) => setFeatureParam(part.id, "radius", f64(v))}
        unit="mm"
        {...scrub}
      />
    </div>
  );
}

function ChamferProperties({ part }: { part: ChamferPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.chamferNodeId)];
  if (!node || node.op.type !== "Chamfer") return null;

  return (
    <div>
      <SectionHeader tooltip="Chamfer distance (mm)">Distance</SectionHeader>
      <ScrubInput
        label="D"
        value={node.op.distance}
        min={0.1}
        step={0.5}
        onChange={(v) => setFeatureParam(part.id, "distance", f64(v))}
        unit="mm"
        {...scrub}
      />
    </div>
  );
}

function ShellProperties({ part }: { part: ShellPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.shellNodeId)];
  if (!node || node.op.type !== "Shell") return null;

  return (
    <div>
      <SectionHeader tooltip="Wall thickness (mm)">Thickness</SectionHeader>
      <ScrubInput
        label="T"
        value={node.op.thickness}
        min={0.1}
        step={0.5}
        onChange={(v) => setFeatureParam(part.id, "thickness", f64(v))}
        unit="mm"
        {...scrub}
      />
    </div>
  );
}

function LinearPatternProperties({ part }: { part: LinearPatternPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.patternNodeId)];
  if (!node || node.op.type !== "LinearPattern") return null;

  const op = node.op;
  const dir = op.direction;

  return (
    <div className="space-y-3">
      <div>
        <SectionHeader tooltip="Direction vector for the pattern">Direction</SectionHeader>
        <div className="space-y-0.5">
          <ScrubInput
            label="X"
            value={dir.x}
            onChange={(v) => setFeatureParam(part.id, "direction", vec3(v, dir.y, dir.z))}
            unit="mm"
            {...scrub}
          />
          <ScrubInput
            label="Y"
            value={dir.y}
            onChange={(v) => setFeatureParam(part.id, "direction", vec3(dir.x, v, dir.z))}
            unit="mm"
            {...scrub}
          />
          <ScrubInput
            label="Z"
            value={dir.z}
            onChange={(v) => setFeatureParam(part.id, "direction", vec3(dir.x, dir.y, v))}
            unit="mm"
            {...scrub}
          />
        </div>
      </div>

      <div>
        <SectionHeader tooltip="Number of copies in the pattern">Count</SectionHeader>
        <ScrubInput
          label="N"
          value={op.count}
          min={1}
          max={100}
          step={1}
          onChange={(v) => setFeatureParam(part.id, "count", f64(Math.round(v)))}
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader tooltip="Distance between copies (mm)">Spacing</SectionHeader>
        <ScrubInput
          label="S"
          value={op.spacing}
          min={0.1}
          step={1}
          onChange={(v) => setFeatureParam(part.id, "spacing", f64(v))}
          unit="mm"
          {...scrub}
        />
      </div>
    </div>
  );
}

function CircularPatternProperties({ part }: { part: CircularPatternPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const node = document.nodes[String(part.patternNodeId)];
  if (!node || node.op.type !== "CircularPattern") return null;

  const op = node.op;

  return (
    <div className="space-y-3">
      <div>
        <SectionHeader tooltip="Number of copies around the axis">Count</SectionHeader>
        <ScrubInput
          label="N"
          value={op.count}
          min={1}
          max={100}
          step={1}
          onChange={(v) => setFeatureParam(part.id, "count", f64(Math.round(v)))}
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader tooltip="Total angle of the circular pattern (degrees)">Angle</SectionHeader>
        <ScrubInput
          label="Angle"
          value={op.angle_deg}
          min={1}
          max={360}
          step={5}
          onChange={(v) => setFeatureParam(part.id, "angle_deg", f64(v))}
          unit="°"
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader tooltip="Point on the rotation axis">Axis Origin</SectionHeader>
        <div className="space-y-0.5">
          <ScrubInput
            label="X"
            value={op.axis_origin.x}
            onChange={(v) => setFeatureParam(part.id, "axis_origin", vec3(v, op.axis_origin.y, op.axis_origin.z))}
            unit="mm"
            {...scrub}
          />
          <ScrubInput
            label="Y"
            value={op.axis_origin.y}
            onChange={(v) => setFeatureParam(part.id, "axis_origin", vec3(op.axis_origin.x, v, op.axis_origin.z))}
            unit="mm"
            {...scrub}
          />
          <ScrubInput
            label="Z"
            value={op.axis_origin.z}
            onChange={(v) => setFeatureParam(part.id, "axis_origin", vec3(op.axis_origin.x, op.axis_origin.y, v))}
            unit="mm"
            {...scrub}
          />
        </div>
      </div>

      <div>
        <SectionHeader tooltip="Direction of the rotation axis">Axis Direction</SectionHeader>
        <div className="space-y-0.5">
          <ScrubInput
            label="X"
            value={op.axis_dir.x}
            onChange={(v) => setFeatureParam(part.id, "axis_dir", vec3(v, op.axis_dir.y, op.axis_dir.z))}
            step={0.1}
            {...scrub}
          />
          <ScrubInput
            label="Y"
            value={op.axis_dir.y}
            onChange={(v) => setFeatureParam(part.id, "axis_dir", vec3(op.axis_dir.x, v, op.axis_dir.z))}
            step={0.1}
            {...scrub}
          />
          <ScrubInput
            label="Z"
            value={op.axis_dir.z}
            onChange={(v) => setFeatureParam(part.id, "axis_dir", vec3(op.axis_dir.x, op.axis_dir.y, v))}
            step={0.1}
            {...scrub}
          />
        </div>
      </div>
    </div>
  );
}

function LoftProperties({ part }: { part: LoftPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);

  const node = document.nodes[String(part.loftNodeId)];
  if (!node || node.op.type !== "Loft") return null;

  const op = node.op;

  return (
    <div className="space-y-3">
      {/* Profile sketch references */}
      <div>
        <SectionHeader tooltip="Loft connects multiple sketch profiles">Profiles</SectionHeader>
        <div className="space-y-0.5">
          {op.sketches.map((sketchId, i) => (
            <ReadOnlyParam
              key={i}
              label={`${i + 1}`}
              value={`Sketch #${sketchId}`}
              tooltip="Sketch reference — editing not yet supported"
            />
          ))}
        </div>
      </div>

      {/* Closed toggle */}
      <div>
        <SectionHeader tooltip="Connect last profile back to first">Closed</SectionHeader>
        <label className="flex items-center gap-2 text-xs text-text cursor-pointer">
          <input
            type="checkbox"
            checked={op.closed ?? false}
            onChange={(e) =>
              setFeatureParam(part.id, "closed", bool(e.target.checked))
            }
            className="accent-brand"
          />
          <span>{op.closed ? "Closed loop" : "Open"}</span>
        </label>
      </div>
    </div>
  );
}

function TextProperties({ part }: { part: TextPartInfo }) {
  const document = useDocumentStore((s) => s.document);
  const setFeatureParam = useDocumentStore((s) => s.setFeatureParam);
  const scrub = useScrubDragging();

  const textNode = document.nodes[String(part.textNodeId)];
  if (!textNode || textNode.op.type !== "Text2D") return null;

  const op = textNode.op;

  // Extrude node for 3D depth
  const extrudeNode = document.nodes[String(part.extrudeNodeId)];
  const extrudeOp = extrudeNode?.op.type === "Extrude" ? extrudeNode.op : null;

  // Compute extrude depth from direction vector magnitude
  const depth = extrudeOp
    ? Math.sqrt(extrudeOp.direction.x ** 2 + extrudeOp.direction.y ** 2 + extrudeOp.direction.z ** 2)
    : 0;

  return (
    <div className="space-y-3">
      <div>
        <SectionHeader tooltip="The text content">Text</SectionHeader>
        <ReadOnlyParam label="" value={op.text} tooltip="Editing not yet supported" />
      </div>

      <div>
        <SectionHeader tooltip="Text height in mm">Size</SectionHeader>
        <ScrubInput
          label="H"
          value={op.height}
          min={0.5}
          step={1}
          onChange={(v) => setFeatureParam(part.id, "height", f64(v))}
          unit="mm"
          {...scrub}
        />
      </div>

      {extrudeOp && (
        <div>
          <SectionHeader tooltip="Extrusion depth (mm)">Depth</SectionHeader>
          <ScrubInput
            label="D"
            value={depth}
            min={0.1}
            step={0.5}
            onChange={(v) => setFeatureParam(part.id, "depth", f64(v))}
            unit="mm"
            {...scrub}
          />
        </div>
      )}

      <div>
        <SectionHeader tooltip="Spacing between letters">Spacing</SectionHeader>
        <ScrubInput
          label="Letter"
          value={op.letter_spacing ?? 1}
          min={0.1}
          step={0.1}
          onChange={(v) => setFeatureParam(part.id, "letter_spacing", f64(v))}
          {...scrub}
        />
      </div>

      <div>
        <SectionHeader>Font</SectionHeader>
        <ReadOnlyParam label="" value={op.font} tooltip="Editing not yet supported" />
      </div>
    </div>
  );
}

function MirrorProperties(props: { part: MirrorPartInfo }) {
  void props.part;
  return (
    <div>
      <SectionHeader tooltip="Editing not yet supported">Mirror</SectionHeader>
      <div className="text-xs text-text-muted">Mirror plane parameters are read-only</div>
    </div>
  );
}

function Divider() {
  return <div className="border-t border-border/40 my-2" />;
}

// --- Instance Properties ---

function InstanceMaterialPicker({ instanceId }: { instanceId: string }) {
  const document = useDocumentStore((s) => s.document);

  const instance = document.instances?.find((i) => i.id === instanceId);
  const partDef = instance ? document.partDefs?.[instance.partDefId] : undefined;
  const currentMaterial = instance?.material ?? partDef?.defaultMaterial ?? "default";

  // For instances, we use instanceId as the partId for the material selector
  // The selector will call setInstanceMaterial internally when detecting an instance ID pattern
  return (
    <div>
      <SectionHeader tooltip="Assign a material to this instance">Material</SectionHeader>
      <InstanceMaterialSelector
        instanceId={instanceId}
        currentMaterialKey={currentMaterial}
      />
    </div>
  );
}

function InstancePositionSection({ instance }: { instance: PartInstance }) {
  const setInstanceTransform = useDocumentStore((s) => s.setInstanceTransform);
  const t = instance.transform ?? identityTransform();

  return (
    <div>
      <SectionHeader tooltip="Position in world space (mm)">Position</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="X"
          value={t.translation.x}
          onChange={(v) => setInstanceTransform(instance.id, { ...t, translation: { ...t.translation, x: v } })}
          unit="mm"
        />
        <ScrubInput
          label="Y"
          value={t.translation.y}
          onChange={(v) => setInstanceTransform(instance.id, { ...t, translation: { ...t.translation, y: v } })}
          unit="mm"
        />
        <ScrubInput
          label="Z"
          value={t.translation.z}
          onChange={(v) => setInstanceTransform(instance.id, { ...t, translation: { ...t.translation, z: v } })}
          unit="mm"
        />
      </div>
    </div>
  );
}

function InstanceRotationSection({ instance }: { instance: PartInstance }) {
  const setInstanceTransform = useDocumentStore((s) => s.setInstanceTransform);
  const t = instance.transform ?? identityTransform();

  return (
    <div>
      <SectionHeader tooltip="Rotation angles in world space (degrees)">Rotation</SectionHeader>
      <div className="space-y-0.5">
        <ScrubInput
          label="X"
          value={t.rotation.x}
          step={1}
          onChange={(v) => setInstanceTransform(instance.id, { ...t, rotation: { ...t.rotation, x: v } })}
          unit="°"
        />
        <ScrubInput
          label="Y"
          value={t.rotation.y}
          step={1}
          onChange={(v) => setInstanceTransform(instance.id, { ...t, rotation: { ...t.rotation, y: v } })}
          unit="°"
        />
        <ScrubInput
          label="Z"
          value={t.rotation.z}
          step={1}
          onChange={(v) => setInstanceTransform(instance.id, { ...t, rotation: { ...t.rotation, z: v } })}
          unit="°"
        />
      </div>
    </div>
  );
}

function InstancePropertiesPanel({ instance }: { instance: PartInstance }) {
  const document = useDocumentStore((s) => s.document);
  const clearSelection = useUiStore((s) => s.clearSelection);

  const partDef = document.partDefs?.[instance.partDefId];
  const displayName = instance.name ?? partDef?.name ?? instance.partDefId;

  return (
    <div
      className={cn(
        "w-full flex flex-col bg-surface",
        "h-full",
      )}
    >
      {/* Mobile drag handle */}

      {/* Header */}
      <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-border/40 px-3">
        <div className="flex items-center gap-2 min-w-0">
          <BackButton />
          <span className="text-xs font-medium text-text truncate">
            {displayName}
          </span>
          <PartTypeBadge kind="instance" />
        </div>
        <button
          onClick={clearSelection}
          className="flex h-8 w-8 sm:h-6 sm:w-6 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
        >
          <X size={14} />
        </button>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-3 space-y-1 scrollbar-thin">
        {/* Part definition (read-only) */}
        <div>
          <SectionHeader>Part Definition</SectionHeader>
          <div className="text-xs text-text">{partDef?.name ?? instance.partDefId}</div>
        </div>
        <Divider />

        <InstancePositionSection instance={instance} />
        <Divider />
        <InstanceRotationSection instance={instance} />
        <Divider />
        <InstanceMaterialPicker instanceId={instance.id} />
      </div>
    </div>
  );
}

// --- Joint Properties ---

function getJointTypeLabel(kind: JointKind): string {
  switch (kind.type) {
    case "Fixed": return "Fixed";
    case "Revolute": return "Revolute";
    case "Slider": return "Slider";
    case "Cylindrical": return "Cylindrical";
    case "Ball": return "Ball";
  }
}

function JointStateSlider({ joint }: { joint: Joint }) {
  const setJointState = useDocumentStore((s) => s.setJointState);
  const kind = joint.kind;

  // Get limits and labels based on joint type
  let min = -180;
  let max = 180;
  let step = 1;
  let unit = "°";

  if (kind.type === "Revolute") {
    if (kind.limits) {
      [min, max] = kind.limits;
    }
    unit = "°";
  } else if (kind.type === "Slider") {
    if (kind.limits) {
      [min, max] = kind.limits;
    } else {
      min = 0;
      max = 100;
    }
    step = 0.5;
    unit = "mm";
  } else if (kind.type === "Cylindrical") {
    // Cylindrical uses rotation for state
    unit = "°";
  } else if (kind.type === "Ball") {
    unit = "°";
  } else {
    // Fixed joint has no state
    return null;
  }

  return (
    <div>
      <SectionHeader tooltip="Current joint state value">State</SectionHeader>
      <div className="flex items-center gap-2">
        <input
          type="range"
          min={min}
          max={max}
          step={step}
          value={joint.state}
          onChange={(e) => setJointState(joint.id, Number(e.target.value))}
          className="flex-1 h-1 bg-border rounded-full appearance-none cursor-pointer
            [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:h-3
            [&::-webkit-slider-thumb]:bg-brand [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:cursor-pointer"
        />
        <span className="text-xs text-text tabular-nums w-16 text-right">
          {joint.state.toFixed(kind.type === "Slider" ? 1 : 0)}{unit}
        </span>
      </div>
      <div className="flex justify-between text-[10px] text-text-muted mt-1">
        <span>{min}{unit}</span>
        <span>{max}{unit}</span>
      </div>
    </div>
  );
}

function JointPropertiesPanel({ joint }: { joint: Joint }) {
  const document = useDocumentStore((s) => s.document);
  const clearSelection = useUiStore((s) => s.clearSelection);

  const instancesById = useMemo(
    () => new Map(document.instances?.map((i) => [i.id, i]) ?? []),
    [document.instances]
  );

  const parentName = joint.parentInstanceId
    ? instancesById.get(joint.parentInstanceId)?.name ?? joint.parentInstanceId
    : "Ground";
  const childInstance = instancesById.get(joint.childInstanceId);
  const childName = childInstance?.name ?? joint.childInstanceId;
  const displayName = joint.name ?? `${getJointTypeLabel(joint.kind)} Joint`;

  return (
    <div
      className={cn(
        "w-full flex flex-col bg-surface",
        "h-full",
      )}
    >
      {/* Mobile drag handle */}

      {/* Header */}
      <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-border/40 px-3">
        <div className="flex items-center gap-2 min-w-0">
          <BackButton />
          <span className="text-xs font-medium text-text truncate">
            {displayName}
          </span>
          <PartTypeBadge kind="joint" />
        </div>
        <button
          onClick={clearSelection}
          className="flex h-8 w-8 sm:h-6 sm:w-6 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
        >
          <X size={14} />
        </button>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-3 space-y-1 scrollbar-thin">
        {/* Joint type (read-only) */}
        <div>
          <SectionHeader>Type</SectionHeader>
          <div className="text-xs text-text">{getJointTypeLabel(joint.kind)}</div>
        </div>
        <Divider />

        {/* Connection info */}
        <div>
          <SectionHeader>Connection</SectionHeader>
          <div className="text-xs text-text">
            <span className="text-text-muted">Parent:</span> {parentName}
          </div>
          <div className="text-xs text-text">
            <span className="text-text-muted">Child:</span> {childName}
          </div>
        </div>
        <Divider />

        {/* State slider for non-fixed joints */}
        {joint.kind.type !== "Fixed" && (
          <>
            <JointStateSlider joint={joint} />
          </>
        )}
      </div>
    </div>
  );
}

export function PropertyPanel() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const clearSelection = useUiStore((s) => s.clearSelection);
  const parts = useDocumentStore((s) => s.parts);
  const document = useDocumentStore((s) => s.document);
  const panelRef = useRef<HTMLDivElement>(null);

  // Close panel on Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && selectedPartIds.size > 0) {
        clearSelection();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [selectedPartIds.size, clearSelection]);

  if (selectedPartIds.size === 0) return null;

  // Check if the selection is an instance or joint (assembly mode)
  if (selectedPartIds.size === 1) {
    const singleId = Array.from(selectedPartIds)[0]!;

    // Check for joint selection (prefixed with "joint:")
    if (singleId.startsWith("joint:")) {
      const jointId = singleId.slice(6); // Remove "joint:" prefix
      const joint = document.joints?.find((j) => j.id === jointId);
      if (joint) {
        return <JointPropertiesPanel joint={joint} />;
      }
    }

    // Check for instance selection
    const instance = document.instances?.find((i) => i.id === singleId);
    if (instance) {
      return <InstancePropertiesPanel instance={instance} />;
    }
  }

  if (selectedPartIds.size > 1) {
    return (
      <div
        ref={panelRef}
        className={cn(
          "w-full flex flex-col bg-surface",
          "h-full",
        )}
      >
        {/* Mobile drag handle */}
          <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-border/40 px-3">
          <div className="flex items-center gap-2 min-w-0">
            <BackButton />
            <span className="text-xs font-medium text-text">
              {selectedPartIds.size} parts selected
            </span>
          </div>
          <button
            onClick={clearSelection}
            className="flex h-8 w-8 sm:h-6 sm:w-6 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
          >
            <X size={14} />
          </button>
        </div>
        <div className="p-3 text-[10px] text-text-muted">
          Select a single part to edit properties
        </div>
      </div>
    );
  }

  const singleId = Array.from(selectedPartIds)[0]!;
  const part = parts.find((p) => p.id === singleId);
  if (!part) return null;

  const translateNode = document.nodes[String(part.translateNodeId)];
  const rotateNode = document.nodes[String(part.rotateNodeId)];
  const scaleNode = document.nodes[String(part.scaleNodeId)];

  const offset =
    translateNode?.op.type === "Translate"
      ? translateNode.op.offset
      : { x: 0, y: 0, z: 0 };

  const angles =
    rotateNode?.op.type === "Rotate"
      ? rotateNode.op.angles
      : { x: 0, y: 0, z: 0 };

  const factor =
    scaleNode?.op.type === "Scale"
      ? scaleNode.op.factor
      : { x: 1, y: 1, z: 1 };

  return (
    <div
      ref={panelRef}
      className={cn(
        "w-full flex flex-col bg-surface",
        "h-full",
      )}
    >
      {/* Mobile drag handle */}

      {/* Header */}
      <div className="flex h-10 shrink-0 items-center justify-between gap-2 border-b border-border/40 px-3">
        <div className="flex items-center gap-2 min-w-0">
          <BackButton />
          <span className="text-xs font-medium text-text truncate">
            {part.name}
          </span>
          <PartTypeBadge kind={part.kind} />
        </div>
        <button
          onClick={clearSelection}
          className="flex h-8 w-8 sm:h-6 sm:w-6 shrink-0 items-center justify-center text-text-muted hover:text-text hover:bg-hover"
        >
          <X size={14} />
        </button>
      </div>

      {/* Body */}
      <div className="flex-1 overflow-y-auto p-3 space-y-1 scrollbar-thin">
        {/* Dimensions by type (primitives only) */}
        {isPrimitivePart(part) && part.kind === "cube" && (
          <>
            <CubeDimensions part={part} />
            <Divider />
          </>
        )}
        {isPrimitivePart(part) && part.kind === "cylinder" && (
          <>
            <CylinderDimensions part={part} />
            <Divider />
          </>
        )}
        {isPrimitivePart(part) && part.kind === "sphere" && (
          <>
            <SphereDimensions part={part} />
            <Divider />
          </>
        )}

        {/* Boolean type selector */}
        {isBooleanPart(part) && (
          <>
            <BooleanProperties part={part} />
            <Divider />
          </>
        )}

        {/* Sweep properties */}
        {isSweepPart(part) && (
          <>
            <SweepProperties part={part} />
            <Divider />
          </>
        )}

        {/* Extrude properties */}
        {isExtrudePart(part) && (
          <>
            <ExtrudeProperties part={part} />
            <Divider />
          </>
        )}

        {/* Revolve properties */}
        {isRevolvePart(part) && (
          <>
            <RevolveProperties part={part} />
            <Divider />
          </>
        )}

        {/* Fillet properties */}
        {isFilletPart(part) && (
          <>
            <FilletProperties part={part} />
            <Divider />
          </>
        )}

        {/* Chamfer properties */}
        {isChamferPart(part) && (
          <>
            <ChamferProperties part={part} />
            <Divider />
          </>
        )}

        {/* Shell properties */}
        {isShellPart(part) && (
          <>
            <ShellProperties part={part} />
            <Divider />
          </>
        )}

        {/* Linear pattern properties */}
        {isLinearPatternPart(part) && (
          <>
            <LinearPatternProperties part={part} />
            <Divider />
          </>
        )}

        {/* Circular pattern properties */}
        {isCircularPatternPart(part) && (
          <>
            <CircularPatternProperties part={part} />
            <Divider />
          </>
        )}

        {/* Loft properties */}
        {isLoftPart(part) && (
          <>
            <LoftProperties part={part} />
            <Divider />
          </>
        )}

        {/* Text properties */}
        {isTextPart(part) && (
          <>
            <TextProperties part={part} />
            <Divider />
          </>
        )}

        {/* Mirror properties */}
        {isMirrorPart(part) && (
          <>
            <MirrorProperties part={part} />
            <Divider />
          </>
        )}

        {/* PCB Board workspace entry */}
        {isPcbBoardPart(part) && (
          <>
            <button
              type="button"
              onClick={() => useElectronicsStore.getState().enter()}
              className="w-full mt-2 px-3 py-2 text-xs font-medium rounded
                bg-brand/10 text-brand border border-brand/30
                hover:bg-brand/20 transition-colors"
            >
              Edit Circuit
            </button>
            <Divider />
          </>
        )}

        {/* Embroidery properties */}
        {(isEmbroideryPatternPart(part) || isStitchPart(part)) && (
          <Suspense fallback={null}>
            <button
              type="button"
              onClick={() => useEmbroideryStore.getState().openPanel()}
              className="w-full mt-2 px-3 py-2 text-xs font-medium rounded
                bg-brand/10 text-brand border border-brand/30
                hover:bg-brand/20 transition-colors"
            >
              Open Embroidery Panel
            </button>
            <EmbroideryProperties part={part} />
            <Divider />
          </Suspense>
        )}

        <PositionSection part={part} offset={offset} />
        <Divider />
        <RotationSection part={part} angles={angles} />
        <Divider />
        <ScaleSection part={part} factor={factor} />
        <Divider />
        <MaterialPicker partId={part.id} />
      </div>
    </div>
  );
}
