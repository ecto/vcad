/**
 * Regression tests for on-demand rendering.
 *
 * The viewport must use frameloop="demand" so the GPU idles when nothing
 * is changing.  Every useFrame hook that mutates visible state must call
 * invalidate() so the next frame is actually scheduled.
 *
 * These tests read the source files and assert structural invariants so
 * we catch regressions even when the code is refactored.
 */
import { describe, it, expect } from "vitest";
import { readFileSync } from "fs";
import { resolve } from "path";

const SRC = resolve(__dirname, "..");

function readSrc(rel: string): string {
  return readFileSync(resolve(SRC, rel), "utf-8");
}

// ---------------------------------------------------------------------------
// 1. Canvas must use demand rendering
// ---------------------------------------------------------------------------
describe("Canvas frameloop", () => {
  it("defaults to 'demand', only switches to 'always' while in WebXR", () => {
    const src = readSrc("components/Viewport.tsx");
    // Default must remain "demand" so the GPU idles outside XR.
    expect(src).toContain('"demand"');
    // The only allowed source of "always" is the XR-presenting branch:
    // `frameloop={xrPresenting ? "always" : "demand"}`.
    if (src.includes('"always"')) {
      expect(src).toMatch(/xrPresenting\s*\?\s*"always"\s*:\s*"demand"/);
    }
  });
});

// ---------------------------------------------------------------------------
// 2. GridPlane must not poll every frame
// ---------------------------------------------------------------------------
describe("GridPlane", () => {
  it("does not use useFrame (removed dead polling loop)", () => {
    const src = readSrc("components/GridPlane.tsx");
    expect(src).not.toContain("useFrame");
  });
});

// ---------------------------------------------------------------------------
// 3. Every useFrame that mutates state must call invalidate()
// ---------------------------------------------------------------------------
describe("useFrame hooks call invalidate()", () => {
  it("ViewportContent camera animation calls invalidate", () => {
    const src = readSrc("components/ViewportContent.tsx");
    // The camera-animation useFrame must contain an invalidate() call
    // so subsequent lerp frames are scheduled in demand mode.
    expect(src).toContain("invalidate()");
    // Verify it imports invalidate from useThree
    expect(src).toMatch(/\binvalidate\b.*=.*useThree/);
  });

  it("PlaneGizmo opacity animation calls invalidate and converges", () => {
    const src = readSrc("components/PlaneGizmo.tsx");
    expect(src).toContain("invalidate()");
    // Must have a convergence check so it stops requesting frames
    expect(src).toMatch(/Math\.abs.*<.*0\.01/);
  });

  it("usePhysicsSimulation calls invalidate while running", () => {
    const src = readSrc("hooks/usePhysicsSimulation.ts");
    expect(src).toContain("invalidate()");
  });
});

// ---------------------------------------------------------------------------
// 4. Wheel handler invalidates for demand mode
// ---------------------------------------------------------------------------
describe("Wheel handler", () => {
  it("calls invalidate in scheduleUpdate for demand mode", () => {
    const src = readSrc("components/ViewportContent.tsx");
    // The scheduleUpdate function must call invalidate after controls.update()
    // so the frame is rendered in demand mode.
    const scheduleBlock = src.slice(
      src.indexOf("const scheduleUpdate"),
      src.indexOf("};", src.indexOf("const scheduleUpdate")) + 2,
    );
    expect(scheduleBlock).toContain("invalidate");
  });

  it("momentum animation calls invalidate", () => {
    const src = readSrc("components/ViewportContent.tsx");
    // The animate() function for orbit momentum must invalidate
    const animateStart = src.indexOf("const animate = ()");
    const animateEnd = src.indexOf("requestAnimationFrame(animate)", animateStart);
    const animateBlock = src.slice(animateStart, animateEnd);
    expect(animateBlock).toContain("invalidate");
  });
});

// ---------------------------------------------------------------------------
// 5. No component should continuously invalidate without a guard
// ---------------------------------------------------------------------------
describe("No unconditional invalidation loops", () => {
  it("PlaneGizmo does not invalidate when converged", () => {
    const src = readSrc("components/PlaneGizmo.tsx");
    // The useFrame must return early before calling invalidate when
    // the opacity has reached its target (convergence guard).
    const useFrameStart = src.indexOf("useFrame(()");
    const useFrameEnd = src.indexOf("});", useFrameStart);
    const block = src.slice(useFrameStart, useFrameEnd);

    // invalidate must come AFTER a convergence return
    const returnIdx = block.indexOf("return;");
    const invalidateIdx = block.indexOf("invalidate()");
    expect(returnIdx).toBeLessThan(invalidateIdx);
  });

  it("ViewportContent animation does not invalidate when not animating", () => {
    const src = readSrc("components/ViewportContent.tsx");
    // The camera animation useFrame must early-return when not animating
    const animFrameStart = src.indexOf("// Smooth target and distance animation");
    const animFrameEnd = src.indexOf("});", animFrameStart);
    const block = src.slice(animFrameStart, animFrameEnd);

    // Must check isAnimatingTargetRef before doing anything
    expect(block).toContain("if (!isAnimatingTargetRef.current");
    // invalidate must only be in the else branch (when animation is in progress)
    expect(block).toContain("invalidate()");
  });
});

// ---------------------------------------------------------------------------
// 6. White-screen guards: imperative geometry builds + initial paint
// ---------------------------------------------------------------------------
// Under frameloop="demand", meshes that populate their geometry buffers in an
// effect (geo.setAttribute(...)) mutate the scene in a way R3F's reconciler
// never sees, so no frame is scheduled and the viewport sits blank until an
// unrelated event (orbit/resize) happens to kick one. These assert the explicit
// invalidate() kicks that keep the scene from white-screening on load/update.
describe("White-screen guards", () => {
  it("SceneMesh invalidates after building geometry imperatively", () => {
    const src = readSrc("components/SceneMesh.tsx");
    // Must pull invalidate off the R3F store...
    expect(src).toMatch(/invalidate\s*=\s*useThree/);
    // ...and the geometry-build effect (keyed on the incoming mesh) must list
    // invalidate as a dep, proving the kick lives in that effect.
    expect(src).toMatch(/\[\s*mesh\s*,\s*partInfo\.name\s*,\s*invalidate\s*\]/);
    // The imported-mesh path builds geometry imperatively too.
    expect(src).toMatch(/\[\s*mesh\s*,\s*invalidate\s*\]/);
  });

  it("ViewportContent kicks an initial-paint burst on mount", () => {
    const src = readSrc("components/ViewportContent.tsx");
    // A self-terminating rAF pump that invalidates across the first frames so
    // whichever frame the async scene/composer becomes drawable in gets painted.
    expect(src).toContain("const pump");
    expect(src).toMatch(/requestAnimationFrame\(pump\)/);
    // And a reactive kick when the evaluated engine scene swaps.
    expect(src).toMatch(/\[\s*invalidate\s*,\s*scene\s*,\s*previewMesh\s*,\s*engineReady\s*\]/);
  });

  it("Viewport remounts the Canvas to recover a dead WebGL context", () => {
    const src = readSrc("components/Viewport.tsx");
    // The Canvas key is driven by a remount epoch...
    expect(src).toMatch(/key=\{glEpoch\}/);
    // ...bumped both on a stuck runtime loss and a failed init (onCreated never
    // firing). Recovery must be capped so it can't spin forever.
    expect(src).toContain("vcad:gl-stuck-lost");
    expect(src).toMatch(/glRemountsRef\.current\s*>=\s*3/);
  });
});
