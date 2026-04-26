import { useEffect, useMemo, useRef, type ReactNode } from "react";
import * as THREE from "three";
import { Line } from "@react-three/drei";
import { useFrame, useThree } from "@react-three/fiber";
import {
  useUiStore,
  useDocumentStore,
  useEngineStore,
  useParticipantStore,
  selectionItemsEqual,
  type SelectionItem,
  LOCAL_PARTICIPANT_ID,
} from "@vcad/core";
import { useTheme } from "@/hooks/useTheme";
import {
  buildFaceHighlightGeometry,
  findCoplanarTriangles,
  getEdgeEndpoints,
  getVertex,
} from "@/lib/sub-feature-geometry";

const SUB_HOVER_OPACITY = 0.4;
const SUB_SELECTED_OPACITY = 0.85;
const FACE_HOVER_COLOR = new THREE.Color(0x00d4ff);
const FACE_SELECTED_COLOR = new THREE.Color(0xf92672);
const SUB_HOVER_LINE_COLOR = "#00d4ff";
const SUB_SELECTED_LINE_COLOR = "#f92672";

const ACCENT_DARK = "#6b8fa3";
const ACCENT_LIGHT = "#4a7080";

function BoundingBoxLines({
  box,
  color,
  opacity,
}: {
  box: THREE.Box3;
  color: string;
  opacity: number;
}) {
  const min = box.min;
  const max = box.max;

  // 12 edges of a box as line segments
  const edges: [THREE.Vector3, THREE.Vector3][] = [
    // Bottom face
    [
      new THREE.Vector3(min.x, min.y, min.z),
      new THREE.Vector3(max.x, min.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, min.z),
      new THREE.Vector3(max.x, min.y, max.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, max.z),
      new THREE.Vector3(min.x, min.y, max.z),
    ],
    [
      new THREE.Vector3(min.x, min.y, max.z),
      new THREE.Vector3(min.x, min.y, min.z),
    ],
    // Top face
    [
      new THREE.Vector3(min.x, max.y, min.z),
      new THREE.Vector3(max.x, max.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, max.y, min.z),
      new THREE.Vector3(max.x, max.y, max.z),
    ],
    [
      new THREE.Vector3(max.x, max.y, max.z),
      new THREE.Vector3(min.x, max.y, max.z),
    ],
    [
      new THREE.Vector3(min.x, max.y, max.z),
      new THREE.Vector3(min.x, max.y, min.z),
    ],
    // Vertical edges
    [
      new THREE.Vector3(min.x, min.y, min.z),
      new THREE.Vector3(min.x, max.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, min.z),
      new THREE.Vector3(max.x, max.y, min.z),
    ],
    [
      new THREE.Vector3(max.x, min.y, max.z),
      new THREE.Vector3(max.x, max.y, max.z),
    ],
    [
      new THREE.Vector3(min.x, min.y, max.z),
      new THREE.Vector3(min.x, max.y, max.z),
    ],
  ];

  return (
    <>
      {edges.map((edge, i) => (
        <Line
          key={i}
          points={edge}
          color={color}
          lineWidth={1}
          dashed
          dashSize={1}
          gapSize={0.8}
          transparent
          opacity={opacity}
        />
      ))}
    </>
  );
}

/** Compute the combined bbox (kernel Z-up) of a set of selected part ids. */
function useSelectionBox(partIdSet: Set<string>): THREE.Box3 | null {
  const parts = useDocumentStore((s) => s.parts);
  const scene = useEngineStore((s) => s.scene);

  return useMemo(() => {
    if (partIdSet.size === 0 || !scene) return null;

    const combinedBox = new THREE.Box3();
    let hasValidBox = false;

    partIdSet.forEach((partId) => {
      const partIndex = parts.findIndex((p) => p.id === partId);
      if (partIndex === -1) return;

      const evalPart = scene.parts[partIndex];
      if (!evalPart) return;

      const mesh = evalPart.mesh;
      if (!mesh.positions.length) return;

      const partBox = new THREE.Box3();
      const pos = new THREE.Vector3();
      for (let i = 0; i < mesh.positions.length; i += 3) {
        pos.set(
          mesh.positions[i]!,
          mesh.positions[i + 1]!,
          mesh.positions[i + 2]!,
        );
        partBox.expandByPoint(pos);
      }

      combinedBox.union(partBox);
      hasValidBox = true;
    });

    return hasValidBox ? combinedBox : null;
  }, [partIdSet, parts, scene]);
}

/** Local user's selection box — dashed accent outline. */
function LocalSelection() {
  const selectedPartIds = useUiStore((s) => s.selectedPartIds);
  const box = useSelectionBox(selectedPartIds);
  const { isDark } = useTheme();
  if (!box) return null;
  const accentColor = isDark ? ACCENT_DARK : ACCENT_LIGHT;
  return <BoundingBoxLines box={box} color={accentColor} opacity={0.5} />;
}

/**
 * Every non-local participant's selection, drawn in their own color so
 * the user can see at a glance what the AI (or a peer) is looking at.
 */
function ParticipantAttention() {
  const participants = useParticipantStore((s) => s.participants);
  const followMode = useUiStore((s) => s.followMode);
  // Free mode hides all non-local presence cues.
  if (followMode === "free") return null;
  const nodes: ReactNode[] = [];
  participants.forEach((p) => {
    if (p.id === LOCAL_PARTICIPANT_ID) return;
    if (p.selectedPartIds.size === 0) return;
    nodes.push(<ParticipantSelection key={p.id} ids={p.selectedPartIds} color={p.color} />);
  });
  return <>{nodes}</>;
}

function ParticipantSelection({ ids, color }: { ids: Set<string>; color: string }) {
  const box = useSelectionBox(ids);
  if (!box) return null;
  // AI/peer attention is slightly brighter than local selection so it
  // reads as an event drawing your eye.
  return <BoundingBoxLines box={box} color={color} opacity={0.7} />;
}

/** Labeled axis gnomon at the world origin, only visible during
 *  ai-screenshot.ts's capture window. Gives the AI an unambiguous reference
 *  for "which way is +X / +Y / +Z" in the captured image, so it doesn't have
 *  to guess from scene context whether the bike is laid out along X or Y.
 *  The scene is kernel Z-up but gets wrapped in a -90° X rotation by the
 *  viewport geometry group; this axes helper lives inside the same group so
 *  the labels match the kernel frame (X red, Y green, Z blue). */
function CaptureAxesGnomon() {
  const scene = useEngineStore((s) => s.scene);
  // Scale the gnomon with the scene so it's readable regardless of zoom.
  const size = useMemo(() => {
    let maxDim = 100;
    if (scene?.parts) {
      for (const p of scene.parts) {
        const pos = p.mesh.positions;
        for (let i = 0; i < pos.length; i += 3) {
          maxDim = Math.max(maxDim, Math.abs(pos[i]!), Math.abs(pos[i + 1]!), Math.abs(pos[i + 2]!));
        }
      }
    }
    return maxDim * 0.25;
  }, [scene]);

  return (
    <group>
      <Line points={[[0, 0, 0], [size, 0, 0]]} color="#ff3b30" lineWidth={3} />
      <Line points={[[0, 0, 0], [0, size, 0]]} color="#34c759" lineWidth={3} />
      <Line points={[[0, 0, 0], [0, 0, size]]} color="#0a84ff" lineWidth={3} />
    </group>
  );
}

/** Look up a part's mesh from the engine scene. */
function usePartMesh(partId: string) {
  const scene = useEngineStore((s) => s.scene);
  const parts = useDocumentStore((s) => s.parts);
  return useMemo(() => {
    if (!scene) return null;
    const idx = parts.findIndex((p) => p.id === partId);
    if (idx < 0) return null;
    return scene.parts[idx]?.mesh ?? null;
  }, [scene, parts, partId]);
}

function FaceHighlight({
  item,
  variant,
}: {
  item: Extract<SelectionItem, { kind: "face" }>;
  variant: "hover" | "selected";
}) {
  const mesh = usePartMesh(item.partId);
  const geometry = useMemo(() => {
    if (!mesh) return null;
    const tris = findCoplanarTriangles(mesh, item.faceIndex);
    if (tris.length === 0) return null;
    return buildFaceHighlightGeometry(mesh, tris);
  }, [mesh, item.faceIndex]);

  useEffect(() => {
    return () => {
      geometry?.dispose();
    };
  }, [geometry]);

  if (!geometry) return null;
  const color = variant === "selected" ? FACE_SELECTED_COLOR : FACE_HOVER_COLOR;
  const opacity = variant === "selected" ? SUB_SELECTED_OPACITY : SUB_HOVER_OPACITY;
  return (
    <mesh geometry={geometry} renderOrder={998}>
      <meshBasicMaterial
        color={color}
        transparent
        opacity={opacity}
        depthWrite={false}
        depthTest={false}
        side={THREE.DoubleSide}
      />
    </mesh>
  );
}

function EdgeHighlight({
  item,
  variant,
}: {
  item: Extract<SelectionItem, { kind: "edge" }>;
  variant: "hover" | "selected";
}) {
  const mesh = usePartMesh(item.partId);
  const points = useMemo(() => {
    if (!mesh) return null;
    const { a, b } = getEdgeEndpoints(mesh, item.edgeId);
    return [
      [a.x, a.y, a.z],
      [b.x, b.y, b.z],
    ] as [number, number, number][];
  }, [mesh, item.edgeId]);
  if (!points) return null;
  const color =
    variant === "selected" ? SUB_SELECTED_LINE_COLOR : SUB_HOVER_LINE_COLOR;
  return (
    <Line
      points={points}
      color={color}
      lineWidth={variant === "selected" ? 3 : 2}
      transparent
      opacity={variant === "selected" ? 1.0 : 0.7}
      depthWrite={false}
      depthTest={false}
      renderOrder={998}
    />
  );
}

function VertexHighlight({
  item,
  variant,
}: {
  item: Extract<SelectionItem, { kind: "vertex" }>;
  variant: "hover" | "selected";
}) {
  const mesh = usePartMesh(item.partId);
  const position = useMemo(() => {
    if (!mesh) return null;
    return getVertex(mesh, item.vertexId);
  }, [mesh, item.vertexId]);
  const meshRef = useRef<THREE.Mesh>(null);
  const { camera, size } = useThree();

  useFrame(() => {
    if (!meshRef.current || !position) return;
    const cam = camera as THREE.PerspectiveCamera;
    // Vertex is in kernel space; the parent rotation group applies -90°X,
    // so the camera-distance read uses the rotated (display) position.
    const wp = new THREE.Vector3(position.x, position.z, -position.y);
    const dist = cam.position.distanceTo(wp);
    const fovRad = ((cam.fov ?? 50) * Math.PI) / 180;
    const worldPerPx = (2 * dist * Math.tan(fovRad / 2)) / size.height;
    const screenPx = variant === "selected" ? 6 : 5;
    meshRef.current.scale.setScalar(Math.max(1e-4, screenPx * worldPerPx));
  });

  if (!position) return null;
  const color =
    variant === "selected" ? SUB_SELECTED_LINE_COLOR : SUB_HOVER_LINE_COLOR;
  return (
    <mesh ref={meshRef} position={position} renderOrder={999}>
      <sphereGeometry args={[1, 12, 12]} />
      <meshBasicMaterial
        color={color}
        transparent
        opacity={variant === "selected" ? 1.0 : 0.85}
        depthWrite={false}
        depthTest={false}
      />
    </mesh>
  );
}

/** Per-item dispatcher — picks the right highlight component per kind. */
function ItemHighlight({
  item,
  variant,
}: {
  item: SelectionItem;
  variant: "hover" | "selected";
}) {
  if (item.kind === "face") return <FaceHighlight item={item} variant={variant} />;
  if (item.kind === "edge") return <EdgeHighlight item={item} variant={variant} />;
  if (item.kind === "vertex") return <VertexHighlight item={item} variant={variant} />;
  // part / segment / constraint are handled by other renderers.
  return null;
}

/** Renders all face / edge / vertex highlights for the current selection
 *  + hover. Mounted alongside the part-bbox outlines below. */
function SubFeatureHighlights() {
  const selection = useUiStore((s) => s.selection);
  const hoveredItem = useUiStore((s) => s.hoveredItem);

  // Skip the hover render if the same item is already selected, to avoid
  // drawing the highlight twice.
  const hovered = useMemo(() => {
    if (!hoveredItem || hoveredItem.kind === "part") return null;
    return selection.some((it) => selectionItemsEqual(it, hoveredItem))
      ? null
      : hoveredItem;
  }, [hoveredItem, selection]);

  return (
    <>
      {selection.map((item, i) => (
        <ItemHighlight key={`sel-${i}`} item={item} variant="selected" />
      ))}
      {hovered && <ItemHighlight item={hovered} variant="hover" />}
    </>
  );
}

export function SelectionOverlay() {
  const isDraggingGizmo = useUiStore((s) => s.isDraggingGizmo);
  const isOrbiting = useUiStore((s) => s.isOrbiting);
  const captureMode = useUiStore((s) => s.captureMode);

  // During an AI screenshot, swap the usual selection/attention overlays
  // for a clean axes gnomon. This is the single feedback channel the model
  // uses to verify work, so it should show material colors — not participant
  // bbox rings that happen to dominate the frame.
  if (captureMode) return <CaptureAxesGnomon />;

  // Skip rendering during orbit for performance.
  if (isOrbiting || isDraggingGizmo) return null;

  return (
    <>
      <LocalSelection />
      <ParticipantAttention />
      <SubFeatureHighlights />
    </>
  );
}
