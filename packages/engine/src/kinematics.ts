/**
 * Forward kinematics solver for assembly joints.
 *
 * Given a document with joints, computes the world transforms for each instance
 * by traversing the joint tree from the ground (fixed) instance.
 */

import type { Document, Joint, Transform3D, Vec3 } from "@vcad/ir";
import { identityTransform, vec3Add, vec3Sub, vec3Scale, vec3Normalize } from "@vcad/ir";

/** Convert degrees to radians. */
function degToRad(deg: number): number {
  return (deg * Math.PI) / 180;
}

/** Create a rotation matrix from Euler angles (XYZ order, degrees). */
function eulerToMatrix(angles: Vec3): number[][] {
  const rx = degToRad(angles.x);
  const ry = degToRad(angles.y);
  const rz = degToRad(angles.z);

  const cx = Math.cos(rx),
    sx = Math.sin(rx);
  const cy = Math.cos(ry),
    sy = Math.sin(ry);
  const cz = Math.cos(rz),
    sz = Math.sin(rz);

  // Rotation matrix = Rz * Ry * Rx (applied in X, Y, Z order)
  return [
    [cy * cz, sx * sy * cz - cx * sz, cx * sy * cz + sx * sz],
    [cy * sz, sx * sy * sz + cx * cz, cx * sy * sz - sx * cz],
    [-sy, sx * cy, cx * cy],
  ];
}

/** Multiply a 3x3 matrix by a Vec3. */
function matrixVec3(m: number[][], v: Vec3): Vec3 {
  return {
    x: m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
    y: m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
    z: m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
  };
}

/** Multiply two 3x3 matrices. */
function matrixMul(a: number[][], b: number[][]): number[][] {
  const result: number[][] = [
    [0, 0, 0],
    [0, 0, 0],
    [0, 0, 0],
  ];
  for (let i = 0; i < 3; i++) {
    for (let j = 0; j < 3; j++) {
      result[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
    }
  }
  return result;
}

/** Extract Euler angles from rotation matrix (XYZ order). */
function matrixToEuler(m: number[][]): Vec3 {
  const sy = -m[2][0];
  const cy = Math.sqrt(m[0][0] * m[0][0] + m[1][0] * m[1][0]);

  if (cy > 1e-6) {
    return {
      x: (Math.atan2(m[2][1], m[2][2]) * 180) / Math.PI,
      y: (Math.atan2(sy, cy) * 180) / Math.PI,
      z: (Math.atan2(m[1][0], m[0][0]) * 180) / Math.PI,
    };
  } else {
    // Gimbal lock
    return {
      x: (Math.atan2(-m[1][2], m[1][1]) * 180) / Math.PI,
      y: (Math.atan2(sy, cy) * 180) / Math.PI,
      z: 0,
    };
  }
}

/** Create rotation matrix from axis-angle (axis should be normalized). */
function axisAngleToMatrix(axis: Vec3, angleDeg: number): number[][] {
  const angle = degToRad(angleDeg);
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const t = 1 - c;
  const x = axis.x,
    y = axis.y,
    z = axis.z;

  return [
    [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
    [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
    [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
  ];
}

/** Apply a transform to a point (scale, then rotate, then translate). */
function applyTransform(transform: Transform3D, point: Vec3): Vec3 {
  // Scale
  const scaled: Vec3 = {
    x: point.x * transform.scale.x,
    y: point.y * transform.scale.y,
    z: point.z * transform.scale.z,
  };
  // Rotate
  const rotMatrix = eulerToMatrix(transform.rotation);
  const rotated = matrixVec3(rotMatrix, scaled);
  // Translate
  return vec3Add(rotated, transform.translation);
}

/** Compose two transforms: result = outer * inner (apply inner first, then outer). */
function composeTransforms(outer: Transform3D, inner: Transform3D): Transform3D {
  // Compose scales
  const scale: Vec3 = {
    x: outer.scale.x * inner.scale.x,
    y: outer.scale.y * inner.scale.y,
    z: outer.scale.z * inner.scale.z,
  };

  // Compose rotations
  const outerRot = eulerToMatrix(outer.rotation);
  const innerRot = eulerToMatrix(inner.rotation);
  const composedRot = matrixMul(outerRot, innerRot);
  const rotation = matrixToEuler(composedRot);

  // Compose translations: outer.translation + outer.rotation * (outer.scale * inner.translation)
  const scaledInnerTrans: Vec3 = {
    x: outer.scale.x * inner.translation.x,
    y: outer.scale.y * inner.translation.y,
    z: outer.scale.z * inner.translation.z,
  };
  const rotatedInnerTrans = matrixVec3(outerRot, scaledInnerTrans);
  const translation = vec3Add(outer.translation, rotatedInnerTrans);

  return { translation, rotation, scale };
}

/**
 * Compute the transform induced by a joint at a given state.
 *
 * The joint connects parent anchor (in parent frame) to child anchor (in child frame).
 * The returned transform places the child relative to the parent.
 */
function computeJointTransform(joint: Joint): Transform3D {
  const kind = joint.kind;

  switch (kind.type) {
    case "Fixed": {
      // Fixed joint: child anchor aligns with parent anchor
      // Translation: parentAnchor - childAnchor (in parent frame)
      return {
        translation: vec3Sub(joint.parentAnchor, joint.childAnchor),
        rotation: { x: 0, y: 0, z: 0 },
        scale: { x: 1, y: 1, z: 1 },
      };
    }

    case "Revolute": {
      // Revolute joint: rotate around axis by joint.state degrees
      const axis = vec3Normalize(kind.axis);
      const rotMatrix = axisAngleToMatrix(axis, joint.state);
      const rotation = matrixToEuler(rotMatrix);

      // The child anchor rotates around the parent anchor
      // Translation: parentAnchor - rotatedChildAnchor
      const rotatedChildAnchor = matrixVec3(rotMatrix, joint.childAnchor);
      const translation = vec3Sub(joint.parentAnchor, rotatedChildAnchor);

      return {
        translation,
        rotation,
        scale: { x: 1, y: 1, z: 1 },
      };
    }

    case "Slider": {
      // Slider joint: translate along axis by joint.state distance
      const axis = vec3Normalize(kind.axis);
      const slideOffset = vec3Scale(axis, joint.state);

      return {
        translation: vec3Add(
          vec3Sub(joint.parentAnchor, joint.childAnchor),
          slideOffset
        ),
        rotation: { x: 0, y: 0, z: 0 },
        scale: { x: 1, y: 1, z: 1 },
      };
    }

    case "Cylindrical": {
      // Cylindrical joint: combination of revolute and slider (2 DOF)
      // For simplicity, use joint.state as rotation angle only
      // A full implementation would need two state values
      const axis = vec3Normalize(kind.axis);
      const rotMatrix = axisAngleToMatrix(axis, joint.state);
      const rotation = matrixToEuler(rotMatrix);

      const rotatedChildAnchor = matrixVec3(rotMatrix, joint.childAnchor);
      const translation = vec3Sub(joint.parentAnchor, rotatedChildAnchor);

      return {
        translation,
        rotation,
        scale: { x: 1, y: 1, z: 1 },
      };
    }

    case "Ball": {
      // Ball joint: 3 DOF rotation
      // For simplicity, interpret joint.state as rotation around Z axis
      // A full implementation would need Euler angles or quaternion state
      const rotMatrix = axisAngleToMatrix({ x: 0, y: 0, z: 1 }, joint.state);
      const rotation = matrixToEuler(rotMatrix);

      const rotatedChildAnchor = matrixVec3(rotMatrix, joint.childAnchor);
      const translation = vec3Sub(joint.parentAnchor, rotatedChildAnchor);

      return {
        translation,
        rotation,
        scale: { x: 1, y: 1, z: 1 },
      };
    }
  }
}

/**
 * Build a map of instance ID -> joints where instance is the child.
 */
function buildJointTree(
  joints: Joint[]
): Map<string, { joint: Joint; parentId: string | null }> {
  const tree = new Map<string, { joint: Joint; parentId: string | null }>();

  for (const joint of joints) {
    tree.set(joint.childInstanceId, {
      joint,
      parentId: joint.parentInstanceId ?? null,
    });
  }

  return tree;
}

/**
 * Solve forward kinematics for an assembly document.
 *
 * Starting from the ground instance (or instances without parent joints),
 * traverses the joint tree and computes world transforms for each instance.
 *
 * @returns Map from instance ID to world transform
 */
export function solveForwardKinematics(
  doc: Document
): Map<string, Transform3D> {
  const results = new Map<string, Transform3D>();

  if (!doc.instances || doc.instances.length === 0) {
    return results;
  }

  const joints = doc.joints ?? [];
  const instances = doc.instances;

  // Build lookup maps
  const instanceById = new Map(instances.map((i) => [i.id, i]));
  const jointTree = buildJointTree(joints);

  // Find instances connected to each parent (for BFS traversal)
  const childrenByParent = new Map<string | null, string[]>();
  childrenByParent.set(null, []); // Ground children

  for (const joint of joints) {
    const parentId = joint.parentInstanceId ?? null;
    const children = childrenByParent.get(parentId);
    if (children) {
      children.push(joint.childInstanceId);
    } else {
      childrenByParent.set(parentId, [joint.childInstanceId]);
    }
  }

  // Find all instances that are not children of any joint (root instances)
  const childIds = new Set(joints.map((j) => j.childInstanceId));
  const rootInstances = instances.filter((i) => !childIds.has(i.id));

  // Initialize root instances with their base transforms
  for (const instance of rootInstances) {
    results.set(instance.id, instance.transform ?? identityTransform());
  }

  // BFS from ground (null) and all root instances
  const queue: (string | null)[] = [null, ...rootInstances.map((i) => i.id)];
  const visited = new Set<string | null>([null]);

  while (queue.length > 0) {
    const parentId = queue.shift();
    if (parentId === undefined) break;
    const children = childrenByParent.get(parentId) ?? [];

    for (const childId of children) {
      if (visited.has(childId)) continue;
      visited.add(childId);

      const entry = jointTree.get(childId);
      if (!entry) continue;

      const instance = instanceById.get(childId);
      if (!instance) continue;

      // Get parent world transform
      const parentWorldTransform =
        parentId !== null
          ? results.get(parentId) ?? identityTransform()
          : identityTransform();

      // Compute joint-induced transform
      const jointTransform = computeJointTransform(entry.joint);

      // Compose: parentWorld * joint * instanceLocal
      const instanceLocalTransform = instance.transform ?? identityTransform();
      const jointedTransform = composeTransforms(jointTransform, instanceLocalTransform);
      const worldTransform = composeTransforms(parentWorldTransform, jointedTransform);

      results.set(childId, worldTransform);
      queue.push(childId);
    }
  }

  return results;
}

/**
 * Apply forward kinematics to update instance transforms in place.
 *
 * This is a convenience function that modifies the document's instance
 * transforms based on joint states.
 */
export function applyForwardKinematics(doc: Document): void {
  const worldTransforms = solveForwardKinematics(doc);

  if (!doc.instances) return;

  for (const instance of doc.instances) {
    const worldTransform = worldTransforms.get(instance.id);
    if (worldTransform) {
      instance.transform = worldTransform;
    }
  }
}
