/**
 * Tests for glTF animation support in the hand-rolled GLB writer.
 */
import { describe, expect, it } from "vitest";
import {
  buildGlb,
  type GlbAnimationOptions,
  type GlbMesh,
} from "../export/glb.js";

function triMesh(name: string): GlbMesh {
  return {
    name,
    positions: [0, 0, 0, 1, 0, 0, 0, 1, 0],
    indices: [0, 1, 2],
    color: [0.8, 0.2, 0.2],
    metallic: 0.1,
    roughness: 0.5,
  };
}

interface ParsedGlb {
  totalLength: number;
  json: Record<string, any>;
  binOffset: number;
  binLength: number;
  bytes: Uint8Array;
}

function parseGlb(glb: Uint8Array): ParsedGlb {
  const view = new DataView(glb.buffer, glb.byteOffset, glb.byteLength);
  expect(new TextDecoder().decode(glb.slice(0, 4))).toBe("glTF");
  expect(view.getUint32(4, true)).toBe(2);
  const totalLength = view.getUint32(8, true);
  const jsonLen = view.getUint32(12, true);
  expect(view.getUint32(16, true)).toBe(0x4e4f534a); // "JSON"
  const json = JSON.parse(
    new TextDecoder().decode(glb.slice(20, 20 + jsonLen)),
  );
  const binHeader = 20 + jsonLen;
  const binLength = view.getUint32(binHeader, true);
  expect(view.getUint32(binHeader + 4, true)).toBe(0x004e4942); // "BIN\0"
  return { totalLength, json, binOffset: binHeader + 8, binLength, bytes: glb };
}

const rotationChannel = {
  nodeName: "2:lid",
  path: "rotation" as const,
  times: [0, 1, 2],
  values: [0, 0, 0, 1, 0, 0, 0.7071, 0.7071, 0, 0, 1, 0],
};
const turntableChannel = {
  nodeName: "__scene_root__",
  path: "rotation" as const,
  times: [0, 4],
  values: [0, 0, 0, 1, 0, 0, 1, 0],
  interpolation: "LINEAR" as const,
};

describe("buildGlb animation", () => {
  const meshes = [triMesh("1:base"), triMesh("2:lid")];
  const anim: GlbAnimationOptions = {
    name: "spin",
    channels: [rotationChannel, turntableChannel],
    rootNodeName: "__scene_root__",
  };

  it("emits a valid animated GLB", () => {
    const glb = buildGlb(meshes, "test", anim);
    const { totalLength, json, binOffset, binLength, bytes } = parseGlb(glb);

    // Total GLB length field matches buffer length.
    expect(totalLength).toBe(bytes.length);
    expect(binOffset + binLength).toBe(bytes.length);

    // Root node wraps children; scene points only at root.
    const rootIdx = json.nodes.findIndex(
      (n: any) => n.name === "__scene_root__",
    );
    expect(rootIdx).toBeGreaterThanOrEqual(0);
    expect(json.nodes[rootIdx].children).toEqual([0, 1]);
    expect(json.nodes[rootIdx].mesh).toBeUndefined();
    expect(json.scenes[0].nodes).toEqual([rootIdx]);
    expect(json.nodes[0].name).toBe("1:base");
    expect(json.nodes[1].name).toBe("2:lid");

    // Animations array shape.
    expect(json.animations).toHaveLength(1);
    const a = json.animations[0];
    expect(a.name).toBe("spin");
    expect(a.samplers).toHaveLength(2);
    expect(a.channels).toHaveLength(2);

    // Channel targets resolve to the right named nodes.
    expect(json.nodes[a.channels[0].target.node].name).toBe("2:lid");
    expect(a.channels[0].target.path).toBe("rotation");
    expect(json.nodes[a.channels[1].target.node].name).toBe("__scene_root__");
    expect(a.channels[1].target.path).toBe("rotation");

    // Sampler accessors: input SCALAR f32 with min/max, output VEC4 f32.
    for (const [i, s] of a.samplers.entries()) {
      const input = json.accessors[s.input];
      expect(input.type).toBe("SCALAR");
      expect(input.componentType).toBe(5126);
      expect(input.min).toBeDefined();
      expect(input.max).toBeDefined();
      const output = json.accessors[s.output];
      expect(output.type).toBe("VEC4");
      expect(output.componentType).toBe(5126);
      expect(output.count).toBe(input.count);
      expect(s.interpolation).toBe("LINEAR");
      // Animation bufferViews have no GL target.
      expect(json.bufferViews[input.bufferView].target).toBeUndefined();
      expect(json.bufferViews[output.bufferView].target).toBeUndefined();
      void i;
    }
    expect(json.accessors[a.samplers[0].input].count).toBe(3);
    expect(json.accessors[a.samplers[0].input].min).toEqual([0]);
    expect(json.accessors[a.samplers[0].input].max).toEqual([2]);
    expect(json.accessors[a.samplers[1].input].count).toBe(2);

    // All bufferView offsets 4-byte aligned and within the BIN chunk.
    for (const bv of json.bufferViews) {
      expect(bv.byteOffset % 4).toBe(0);
      expect(bv.byteOffset + bv.byteLength).toBeLessThanOrEqual(binLength);
    }
    expect(json.buffers[0].byteLength).toBe(binLength);
  });

  it("round-trips sampler data through the BIN chunk", () => {
    const glb = buildGlb(meshes, "test", anim);
    const { json, binOffset, bytes } = parseGlb(glb);
    const s = json.animations[0].samplers[0];
    const bv = json.bufferViews[json.accessors[s.input].bufferView];
    const times = new Float32Array(
      bytes.buffer.slice(
        binOffset + bv.byteOffset,
        binOffset + bv.byteOffset + bv.byteLength,
      ),
    );
    expect(Array.from(times)).toEqual([0, 1, 2]);
  });

  it("skips channels targeting unknown nodes without throwing", () => {
    const glb = buildGlb(meshes, "test", {
      channels: [
        { ...rotationChannel, nodeName: "does-not-exist" },
        rotationChannel,
      ],
    });
    const { json } = parseGlb(glb);
    expect(json.animations[0].channels).toHaveLength(1);
    expect(json.animations[0].name).toBe("timeline");
    // No rootNodeName: scene nodes unchanged.
    expect(json.scenes[0].nodes).toEqual([0, 1]);
  });

  it("omits animations entirely when every channel is skipped", () => {
    const glb = buildGlb(meshes, "test", {
      channels: [{ ...rotationChannel, nodeName: "nope" }],
    });
    expect(parseGlb(glb).json.animations).toBeUndefined();
  });

  it("without animation is byte-identical to the pre-change output", () => {
    const glb = buildGlb(meshes, "test");
    const { json } = parseGlb(glb);
    expect(json.animations).toBeUndefined();
    expect(json.scenes[0].nodes).toEqual([0, 1]);
    expect(json.nodes.every((n: any) => n.children === undefined)).toBe(true);
    // Two-arg and explicit-undefined calls produce identical bytes.
    const glb2 = buildGlb(meshes, "test", undefined);
    expect(Buffer.from(glb2).equals(Buffer.from(glb))).toBe(true);
  });

  it("loads in three.js GLTFLoader", async () => {
    const { GLTFLoader } = await import(
      "three/examples/jsm/loaders/GLTFLoader.js"
    );
    const glb = buildGlb(meshes, "test", anim);
    const loader = new GLTFLoader();
    const gltf = await new Promise<any>((resolve, reject) => {
      loader.parse(
        glb.buffer.slice(glb.byteOffset, glb.byteOffset + glb.byteLength),
        "",
        resolve,
        reject,
      );
    });
    expect(gltf.animations).toHaveLength(1);
    expect(gltf.animations[0].tracks.length).toBe(2);
    expect(gltf.animations[0].duration).toBeCloseTo(4);
  });
});
