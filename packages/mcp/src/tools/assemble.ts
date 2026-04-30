/**
 * assemble (v2) — add or modify part instances + joints in a doc.
 *
 * Phase-1 covers: instance create/update, joint create with the joint
 * kinds the IR supports today (Fixed/Revolute/Slider/Cylindrical/Ball).
 * Interference detection is best-effort: AABB overlap on evaluated
 * scene parts when the kernel is reachable, otherwise we report none.
 * Higher-fidelity boolean-based interference is a follow-up.
 */

import {
  createDocument,
  type Document,
  type Instance,
  type Joint,
  type JointKind,
  type Vec3,
} from "@vcad/ir";
import type { Engine } from "@vcad/engine";
import type { ToolResult } from "../envelope.js";
import { fail, ok } from "../envelope.js";
import { commitDoc, resolveRef } from "../handles.js";
import type { DocHandle, DocRef } from "../types.js";

export const assembleSchema = {
  type: "object" as const,
  properties: {
    doc: { description: "Doc handle or inline IR (omit for a fresh assembly)." },
    instances: { type: "array" as const, description: "Instance specs to add or update." },
    joints: { type: "array" as const, description: "Joint specs to add." },
    ground: { type: "string" as const, description: "Instance name to ground (fix in world frame)." },
  },
};

interface InstanceSpec {
  name: string;
  part: string;
  transform?: { translate?: Vec3; rotate?: Vec3; scale?: Vec3 } | { matrix4: number[] };
  parameters?: Record<string, number | string>;
  material?: string;
  tags?: string[];
}

interface JointSpec {
  name: string;
  parent: string;
  child: string;
  kind: "revolute" | "prismatic" | "cylindrical" | "ball" | "fixed";
  anchor_parent: Vec3;
  anchor_child: Vec3;
  axis?: Vec3;
  limits?: { min: number; max: number };
  initial?: number;
}

interface AssembleInput {
  doc?: DocRef;
  instances?: InstanceSpec[];
  joints?: JointSpec[];
  ground?: string;
}

export function assemble(input: unknown, engine: Engine): ToolResult {
  const startedAt = performance.now();
  const args = (input ?? {}) as AssembleInput;

  const baseHandle = typeof args.doc === "string" ? args.doc : undefined;
  const { doc: source } = args.doc ? resolveRef(args.doc) : { doc: createDocument() };
  const doc: Document = JSON.parse(JSON.stringify(source));

  doc.partDefs = doc.partDefs ?? {};
  doc.instances = doc.instances ?? [];
  doc.joints = doc.joints ?? [];

  const addedInstances: string[] = [];
  const addedJoints: string[] = [];

  // Instances.
  if (args.instances) {
    for (const spec of args.instances) {
      // Resolve `part` to a partDef id. If it matches a node name, hoist
      // that node into a partDef on the fly.
      let partDefId = spec.part;
      if (!doc.partDefs[partDefId]) {
        const node = Object.values(doc.nodes).find((n) => n.name === spec.part);
        if (node) {
          partDefId = spec.part;
          doc.partDefs[partDefId] = {
            id: partDefId,
            name: spec.part,
            root: node.id,
          };
        } else {
          return fail("unknown_part", `assemble: no partDef or node named "${spec.part}"`);
        }
      }

      const transform =
        spec.transform && "translate" in spec.transform
          ? {
              translate: spec.transform.translate ?? { x: 0, y: 0, z: 0 },
              rotate: spec.transform.rotate ?? { x: 0, y: 0, z: 0 },
              scale: spec.transform.scale ?? { x: 1, y: 1, z: 1 },
            }
          : undefined;

      const existing = doc.instances.findIndex((i) => i.id === spec.name);
      const inst: Instance = {
        id: spec.name,
        partDefId,
        name: spec.name,
        transform,
        material: spec.material,
        tags: spec.tags,
      };
      if (existing >= 0) doc.instances[existing] = inst;
      else doc.instances.push(inst);
      addedInstances.push(spec.name);
    }
  }

  // Joints.
  if (args.joints) {
    for (const spec of args.joints) {
      const kind = mapJointKind(spec);
      const joint: Joint = {
        id: spec.name,
        name: spec.name,
        parentInstanceId: spec.parent,
        childInstanceId: spec.child,
        parentAnchor: spec.anchor_parent,
        childAnchor: spec.anchor_child,
        kind,
        state: spec.initial ?? 0,
      };
      const existing = doc.joints.findIndex((j) => j.id === spec.name);
      if (existing >= 0) doc.joints[existing] = joint;
      else doc.joints.push(joint);
      addedJoints.push(spec.name);
    }
  }

  if (args.ground) doc.groundInstanceId = args.ground;

  // Best-effort interference: AABB overlap on evaluated parts.
  const interferences = computeInterferences(doc, engine);

  const handle = commitDoc(doc, baseHandle as DocHandle | undefined);
  return ok({
    result: {
      added_instances: addedInstances,
      added_joints: addedJoints,
      ground: doc.groundInstanceId ?? null,
      interferences,
    },
    handle,
    doc,
    engine,
    startedAt,
  });
}

function mapJointKind(spec: JointSpec): JointKind {
  switch (spec.kind) {
    case "fixed":
      return { type: "Fixed" };
    case "revolute":
      return {
        type: "Revolute",
        axis: spec.axis ?? { x: 0, y: 0, z: 1 },
        limits: spec.limits ? [spec.limits.min, spec.limits.max] : undefined,
      };
    case "prismatic":
      return {
        type: "Slider",
        axis: spec.axis ?? { x: 0, y: 0, z: 1 },
        limits: spec.limits ? [spec.limits.min, spec.limits.max] : undefined,
      };
    case "cylindrical":
      return { type: "Cylindrical", axis: spec.axis ?? { x: 0, y: 0, z: 1 } };
    case "ball":
      return { type: "Ball" };
    default:
      throw new Error(`Unknown joint kind: ${spec.kind}`);
  }
}

function computeInterferences(
  doc: Document,
  engine: Engine,
): Array<{ a: string; b: string; volume_estimate: number; centroid: Vec3 }> {
  if (!doc.instances || doc.instances.length < 2) return [];
  // Evaluate without joints/instances to avoid the kinematics path,
  // which assumes a complete partDef + joint graph and noisily fails
  // when the assembly is half-assembled. We only need per-part bboxes.
  const planar: typeof doc = {
    ...doc,
    instances: undefined,
    joints: undefined,
    groundInstanceId: undefined,
  } as typeof doc;
  let scene;
  const origWarn = console.warn;
  console.warn = () => {};
  try {
    scene = engine.evaluate(planar);
  } catch {
    return [];
  } finally {
    console.warn = origWarn;
  }

  // Build per-instance AABB by mapping instance → root → mesh.
  type Box = { min: Vec3; max: Vec3 };
  const instBoxes = new Map<string, Box>();
  for (const inst of doc.instances) {
    const partDef = doc.partDefs?.[inst.partDefId];
    if (!partDef) continue;
    let partIndex = -1;
    for (let i = 0; i < doc.roots.length; i++) {
      if (doc.roots[i].root === partDef.root) {
        partIndex = i;
        break;
      }
    }
    if (partIndex < 0 || partIndex >= scene.parts.length) continue;
    const m = scene.parts[partIndex].mesh;
    let minX = Infinity,
      minY = Infinity,
      minZ = Infinity;
    let maxX = -Infinity,
      maxY = -Infinity,
      maxZ = -Infinity;
    for (let i = 0; i < m.positions.length; i += 3) {
      const x = m.positions[i],
        y = m.positions[i + 1],
        z = m.positions[i + 2];
      if (x < minX) minX = x;
      if (y < minY) minY = y;
      if (z < minZ) minZ = z;
      if (x > maxX) maxX = x;
      if (y > maxY) maxY = y;
      if (z > maxZ) maxZ = z;
    }
    if (!isFinite(minX)) continue;
    // Apply instance translate (only — full transform is more involved).
    const t = inst.transform?.translate ?? { x: 0, y: 0, z: 0 };
    instBoxes.set(inst.id, {
      min: { x: minX + t.x, y: minY + t.y, z: minZ + t.z },
      max: { x: maxX + t.x, y: maxY + t.y, z: maxZ + t.z },
    });
  }

  const ids = [...instBoxes.keys()];
  const out: Array<{ a: string; b: string; volume_estimate: number; centroid: Vec3 }> = [];
  for (let i = 0; i < ids.length; i++) {
    for (let j = i + 1; j < ids.length; j++) {
      const A = instBoxes.get(ids[i])!;
      const B = instBoxes.get(ids[j])!;
      const ox = Math.max(0, Math.min(A.max.x, B.max.x) - Math.max(A.min.x, B.min.x));
      const oy = Math.max(0, Math.min(A.max.y, B.max.y) - Math.max(A.min.y, B.min.y));
      const oz = Math.max(0, Math.min(A.max.z, B.max.z) - Math.max(A.min.z, B.min.z));
      const vol = ox * oy * oz;
      if (vol > 1e-6) {
        out.push({
          a: ids[i],
          b: ids[j],
          volume_estimate: Math.round(vol * 1000) / 1000,
          centroid: {
            x: (Math.max(A.min.x, B.min.x) + Math.min(A.max.x, B.max.x)) / 2,
            y: (Math.max(A.min.y, B.min.y) + Math.min(A.max.y, B.max.y)) / 2,
            z: (Math.max(A.min.z, B.min.z) + Math.min(A.max.z, B.max.z)) / 2,
          },
        });
      }
    }
  }
  return out;
}
