import React, { useState, useMemo, useRef, useEffect } from "react";
import { Box, Text, useInput } from "ink";
import chalk from "chalk";
import { useEngineStore } from "@vcad/core";
import {
  computeBounds,
  meshToTriangles,
  type Triangle,
} from "../renderer/software-renderer.js";

// Convert RGBA pixel buffer to chalk-styled strings for Ink compatibility
function pixelsToChalkLines(pixels: Uint8Array, width: number, height: number): string[] {
  const lines: string[] = [];

  // Use half-block characters (▀) to get 2 pixels per character vertically
  for (let y = 0; y < height; y += 2) {
    let line = "";
    for (let x = 0; x < width; x++) {
      const topIdx = (y * width + x) * 4;
      const botIdx = ((y + 1) * width + x) * 4;

      const tr = pixels[topIdx]!;
      const tg = pixels[topIdx + 1]!;
      const tb = pixels[topIdx + 2]!;

      let br = tr, bg = tg, bb = tb;
      if (y + 1 < height) {
        br = pixels[botIdx]!;
        bg = pixels[botIdx + 1]!;
        bb = pixels[botIdx + 2]!;
      }

      // Use chalk for proper terminal color support
      line += chalk.rgb(tr, tg, tb).bgRgb(br, bg, bb)("▀");
    }
    lines.push(line);
  }

  return lines;
}

interface Props {
  width: number;
  height: number;
}

// Part colors (RGB normalized to 0-1)
const PART_COLORS: [number, number, number][] = [
  [0.39, 0.58, 0.93], // cornflower blue
  [0.56, 0.93, 0.56], // light green
  [1.00, 0.71, 0.76], // light pink
  [1.00, 0.85, 0.73], // peach
  [0.87, 0.63, 0.87], // plum
  [0.69, 0.88, 0.90], // powder blue
];

export function Viewport3D({ width, height }: Props) {
  const scene = useEngineStore((s) => s.scene);
  const engine = useEngineStore((s) => s.engine);
  const [rotation, setRotation] = useState({ x: -25, y: 45 });
  const [zoom, setZoom] = useState(1);
  const [useRayTracing, setUseRayTracing] = useState(true);

  // Cache for CPU ray tracers
  const rayTracersRef = useRef<Map<number, unknown>>(new Map());

  // Handle keyboard input for rotation
  useInput((input, key) => {
    const rotStep = 15;
    const zoomStep = 0.2;

    if (key.leftArrow || input === "h") {
      setRotation((r) => ({ ...r, y: r.y - rotStep }));
    }
    if (key.rightArrow || input === "l") {
      setRotation((r) => ({ ...r, y: r.y + rotStep }));
    }
    if (key.upArrow && !key.ctrl) {
      setRotation((r) => ({ ...r, x: Math.max(-89, r.x - rotStep) }));
    }
    if (key.downArrow && !key.ctrl) {
      setRotation((r) => ({ ...r, x: Math.min(89, r.x + rotStep) }));
    }
    if (input === "+" || input === "=") {
      setZoom((z) => Math.min(3, z + zoomStep));
    }
    if (input === "-" || input === "_") {
      setZoom((z) => Math.max(0.3, z - zoomStep));
    }
    // Reset view
    if (input === "0") {
      setRotation({ x: -25, y: 45 });
      setZoom(1);
    }
    // Toggle ray tracing
    if (input === "r") {
      setUseRayTracing((rt) => !rt);
    }
  });

  // Clear ray tracer cache when scene changes
  useEffect(() => {
    rayTracersRef.current.clear();
  }, [scene]);

  // Get bounding box from mesh triangles (for camera positioning)
  const bounds = useMemo(() => {
    if (!scene || scene.parts.length === 0) {
      return { center: { x: 0, y: 0, z: 0 }, size: 20 };
    }

    const allTriangles: Triangle[] = [];
    scene.parts.forEach((part, idx) => {
      const color = [100, 149, 237] as [number, number, number];
      const partTriangles = meshToTriangles(part.mesh.positions, part.mesh.indices, color);
      allTriangles.push(...partTriangles);
    });

    if (allTriangles.length === 0) {
      return { center: { x: 0, y: 0, z: 0 }, size: 20 };
    }

    return computeBounds(allTriangles);
  }, [scene]);

  // Render the scene using CPU ray tracing
  const renderedLines = useMemo(() => {
    const renderWidth = width;
    const renderHeight = height * 2;

    // Check if we can use ray tracing
    const CpuRayTracer = engine?.CpuRayTracer;
    const canRayTrace = useRayTracing && CpuRayTracer && scene && scene.parts.length > 0;

    // Empty scene - draw grid
    if (!scene || scene.parts.length === 0) {
      const pixels = new Uint8Array(renderWidth * renderHeight * 4);
      for (let i = 0; i < renderWidth * renderHeight; i++) {
        const x = i % renderWidth;
        const y = Math.floor(i / renderWidth);
        const isGrid = x % 10 === 0 || y % 10 === 0;
        pixels[i * 4] = isGrid ? 50 : 30;
        pixels[i * 4 + 1] = isGrid ? 52 : 32;
        pixels[i * 4 + 2] = isGrid ? 60 : 40;
        pixels[i * 4 + 3] = 255;
      }
      return pixelsToChalkLines(pixels, renderWidth, renderHeight);
    }

    // Compute camera position
    const distance = (bounds.size * 2) / zoom;
    const radX = rotation.x * Math.PI / 180;
    const radY = rotation.y * Math.PI / 180;

    const camera = [
      bounds.center.x + distance * Math.cos(radX) * Math.sin(radY),
      bounds.center.y + distance * Math.sin(radX),
      bounds.center.z + distance * Math.cos(radX) * Math.cos(radY),
    ];
    const target = [bounds.center.x, bounds.center.y, bounds.center.z];
    const up = [0, 1, 0];

    // Try CPU ray tracing for each part with a BRep solid
    if (canRayTrace) {
      try {
        // Find the first part with a solid
        for (let i = 0; i < scene.parts.length; i++) {
          const part = scene.parts[i];
          // eslint-disable-next-line @typescript-eslint/no-explicit-any
          const solid = (part as any).solid;

          if (solid && typeof solid.canRaytrace === 'function' && solid.canRaytrace()) {
            // Get or create ray tracer for this part
            let rayTracer = rayTracersRef.current.get(i);
            if (!rayTracer) {
              try {
                rayTracer = new CpuRayTracer(solid);
                // Set material color
                const color = PART_COLORS[i % PART_COLORS.length]!;
                (rayTracer as any).setMaterial(color[0], color[1], color[2]);
                rayTracersRef.current.set(i, rayTracer);
              } catch {
                continue; // Skip this solid, try next
              }
            }

            // Render
            const pixels = (rayTracer as any).render(camera, target, up, renderWidth, renderHeight, 45);
            if (pixels && pixels.length === renderWidth * renderHeight * 4) {
              return pixelsToChalkLines(new Uint8Array(pixels), renderWidth, renderHeight);
            }
          }
        }
      } catch {
        // Fall through to mesh-based rendering
      }
    }

    // Fallback: render background only (ray tracing not available or failed)
    const pixels = new Uint8Array(renderWidth * renderHeight * 4);
    for (let i = 0; i < renderWidth * renderHeight; i++) {
      pixels[i * 4] = 30;
      pixels[i * 4 + 1] = 32;
      pixels[i * 4 + 2] = 40;
      pixels[i * 4 + 3] = 255;
    }

    // Draw a simple placeholder showing the scene has geometry
    const cx = Math.floor(renderWidth / 2);
    const cy = Math.floor(renderHeight / 2);
    for (let dy = -10; dy <= 10; dy++) {
      for (let dx = -10; dx <= 10; dx++) {
        const x = cx + dx;
        const y = cy + dy;
        if (x >= 0 && x < renderWidth && y >= 0 && y < renderHeight) {
          const dist = Math.sqrt(dx * dx + dy * dy);
          if (dist <= 10) {
            const idx = (y * renderWidth + x) * 4;
            const intensity = 1 - dist / 10;
            pixels[idx] = Math.floor(100 * intensity);
            pixels[idx + 1] = Math.floor(149 * intensity);
            pixels[idx + 2] = Math.floor(237 * intensity);
          }
        }
      }
    }

    return pixelsToChalkLines(pixels, renderWidth, renderHeight);
  }, [scene, engine, rotation, zoom, width, height, bounds, useRayTracing]);

  const partCount = scene?.parts.length ?? 0;
  const hasBRep = scene?.parts.some((p: any) => p.solid?.canRaytrace?.()) ?? false;

  return (
    <Box flexDirection="column">
      {renderedLines.map((line, i) => (
        <Text key={i}>{line}</Text>
      ))}
      <Box justifyContent="space-between" paddingX={1}>
        <Text dimColor>
          ←→↑↓: rotate | +/-: zoom | 0: reset | r: toggle RT
        </Text>
        <Text dimColor>
          {partCount > 0 ? `${partCount} part${partCount > 1 ? 's' : ''}` : "empty"}
          {hasBRep && useRayTracing && " [RT]"}
          {!hasBRep && partCount > 0 && " [mesh]"}
        </Text>
      </Box>
    </Box>
  );
}
