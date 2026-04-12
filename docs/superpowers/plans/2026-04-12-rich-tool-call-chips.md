# Rich Tool Call Chips Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace anonymous monospace tool call chips with rich, part-linked action sentences that expand into human-readable field lists, powered by display payloads attached to `ExecutionResult`.

**Architecture:** Each executor case writes a `display` payload (summary segments, fields, affectedPartIds) alongside its existing AI-facing `result` string — single source of truth. `executeCrud` wraps dispatch with a timing measurement. The chat handler propagates `display` and `duration` onto `ToolCallInfo`. A new extracted `ToolCallCard.tsx` component renders the rich summary, clickable `PartLink`s, and expanded detail pane.

**Tech Stack:** TypeScript, React, Zustand, Vitest, Phosphor icons (existing), Tailwind CSS (existing).

**Spec:** `docs/superpowers/specs/2026-04-12-rich-tool-call-chips-design.md`

---

### Task 1: Add new types (SummarySegment, ExecutionDisplay, ExecutionResult extension)

**Files:**
- Modify: `packages/core/src/commands/types.ts`

- [ ] **Step 1: Add new types to types.ts**

Replace the contents of `packages/core/src/commands/types.ts` with:

```typescript
/** Mirrors Rust ToolSchemaEntry — parsed from WASM JSON at init. */
export interface ToolSchemaEntry {
  name: string;
  description: string;
  category: string;
  ai_hint?: string;
  input_schema: Record<string, unknown>;
}

/** Renderable piece of a tool call summary sentence. */
export type SummarySegment =
  | { type: "text"; text: string }
  | { type: "partLink"; partId: string; name: string };

/** Optional rich display payload attached to a successful execution. */
export interface ExecutionDisplay {
  /** The at-rest summary sentence, as template segments (text + clickable part links). */
  summary: SummarySegment[];
  /** Human-readable parameter list for the expanded detail view. */
  fields?: Array<{ label: string; value: string }>;
  /** Part IDs touched by this call — used by the chip to highlight on hover. */
  affectedPartIds?: string[];
}

/** Result of executing a CRUD tool. */
export interface ExecutionResult {
  status: "success" | "error";
  /** Human-readable summary returned to the AI. */
  result: string;
  /** Part ID if a part was created or modified. */
  partId?: string;
  /** Node ID if a node was created or modified. */
  nodeId?: string;
  /** Optional rich display payload for UI chips. Absent = UI falls back to `result`. */
  display?: ExecutionDisplay;
  /** Duration of the execution in milliseconds, populated by executeCrud wrapper. */
  duration?: number;
}

/** Anthropic tool definition format. */
export interface AnthropicTool {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
}
```

- [ ] **Step 2: Export SummarySegment and ExecutionDisplay from @vcad/core**

In `packages/core/src/index.ts`, find the existing `// AI Tool Registry (CRUD)` block and update the type export line to include the new types:

```typescript
// AI Tool Registry (CRUD)
export { commandRegistry, executeCrud } from "./commands/index.js";
export type { ToolSchemaEntry, ExecutionResult, ExecutionDisplay, SummarySegment, AnthropicTool } from "./commands/index.js";
```

Also update `packages/core/src/commands/index.ts` to re-export the new types:

```typescript
export { CommandRegistry, commandRegistry } from "./registry.js";
export { executeCrud } from "./executors.js";
export type { ToolSchemaEntry, ExecutionResult, ExecutionDisplay, SummarySegment, AnthropicTool } from "./types.js";
```

- [ ] **Step 3: Build core and verify no type errors**

Run:
```bash
cd /home/cam/code/vcad && npx tsc -p packages/core/tsconfig.json
```

Expected: No errors. If existing tests reference `ExecutionResult` shape, they still work since new fields are optional.

- [ ] **Step 4: Commit**

```bash
git add packages/core/src/commands/types.ts packages/core/src/commands/index.ts packages/core/src/index.ts
git commit -m "feat: add SummarySegment and ExecutionDisplay types for rich tool chips"
```

---

### Task 2: Add partLink/text/link helpers and duration wrapper

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Add helpers at the top of executors.ts**

In `packages/core/src/commands/executors.ts`, add these helper functions near the top, right after the existing `validatePartId` and `validateSketch` functions (before `executeCrud`):

```typescript
import type { ExecutionResult, ExecutionDisplay, SummarySegment } from "./types.js";
```

(Update the existing `import type { ExecutionResult }` to include `ExecutionDisplay` and `SummarySegment`.)

Add the helper functions:

```typescript
/** Render a part ID as a clickable segment, falling back to last 4 chars if unknown. */
function link(id: string, docStore: DocStore): SummarySegment {
  const part = docStore.partIndex.get(id);
  return {
    type: "partLink",
    partId: id,
    name: part?.name ?? id.slice(-4),
  };
}

/** Shorthand for a text segment. */
function text(s: string): SummarySegment {
  return { type: "text", text: s };
}
```

- [ ] **Step 2: Wrap executeCrud with timing measurement**

Find the existing `executeCrud` function and rename it to `executeCrudInner`. Then add a new `executeCrud` wrapper:

```typescript
/** Execute a CRUD tool by name, measuring duration. */
export function executeCrud(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  const t0 = performance.now();
  const result = executeCrudInner(tool, args, docStore, uiStore);
  result.duration = performance.now() - t0;
  return result;
}

function executeCrudInner(
  tool: string,
  args: Record<string, unknown>,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  switch (tool) {
    // ... existing cases unchanged for now
```

The switch body stays exactly the same — just renamed from `executeCrud` to `executeCrudInner`. The public export is the wrapper.

- [ ] **Step 3: Build and verify**

Run:
```bash
cd /home/cam/code/vcad && npx tsc -p packages/core/tsconfig.json
```

Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display helpers and duration wrapper in executors"
```

---

### Task 3: Write tests for display payloads (TDD — failing)

**Files:**
- Modify: `packages/core/src/__tests__/command-registry.test.ts`

- [ ] **Step 1: Add a mock docStore/uiStore factory at the top of the test file**

At the top of `packages/core/src/__tests__/command-registry.test.ts`, after the existing imports, add:

```typescript
import { executeCrud } from "../commands/executors.js";

// Minimal mock stores for executor tests
type MockPart = { id: string; name: string; kind: string };
function makeMockDocStore(parts: MockPart[] = []) {
  const partIndex = new Map(parts.map((p) => [p.id, p]));
  const created: Array<{ kind: string; params?: unknown }> = [];
  let nextId = 0;
  return {
    partIndex,
    parts,
    document: { nodes: {} as Record<string, unknown>, roots: [] },
    addPrimitive: (kind: string) => {
      const id = `mock:${nextId++}`;
      created.push({ kind });
      partIndex.set(id, { id, name: kind, kind });
      parts.push({ id, name: kind, kind });
      return id;
    },
    updatePrimitiveOp: () => {},
    setTranslation: () => {},
    setRotation: () => {},
    setScale: () => {},
    setFeatureParam: () => {},
    setPartMaterial: () => {},
    applyBoolean: (type: string, left: string, right: string) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: type, kind: "boolean" });
      parts.push({ id, name: type, kind: "boolean" });
      return id;
    },
    addFillet: (_target: string, _radius: number) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "fillet", kind: "fillet" });
      parts.push({ id, name: "fillet", kind: "fillet" });
      return id;
    },
    addChamfer: (_t: string, _d: number) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "chamfer", kind: "chamfer" });
      parts.push({ id, name: "chamfer", kind: "chamfer" });
      return id;
    },
    addShell: (_t: string, _t2: number) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "shell", kind: "shell" });
      parts.push({ id, name: "shell", kind: "shell" });
      return id;
    },
    addExtrude: (
      _plane: unknown,
      _origin: unknown,
      _segs: unknown[],
      _dir: unknown,
      _opts: unknown,
    ) => {
      const id = `mock:${nextId++}`;
      partIndex.set(id, { id, name: "extrude", kind: "extrude" });
      parts.push({ id, name: "extrude", kind: "extrude" });
      return id;
    },
    removePart: (_partId: string) => {},
  } as never;
}

function makeMockUiStore() {
  const selectedPartIds = new Set<string>();
  return {
    select: (id: string) => selectedPartIds.add(id),
    clearSelection: () => selectedPartIds.clear(),
    selectedPartIds,
  } as never;
}
```

- [ ] **Step 2: Add the new describe block with tests**

At the bottom of the same file, inside the main `describe("CommandRegistry", ...)` block or in a new top-level describe block (your choice — put it at the bottom of the file before the final closing `});`), add:

```typescript
describe("ExecutionResult display", () => {
  it("cube create returns summary with part link and size field", () => {
    const doc = makeMockDocStore();
    const ui = makeMockUiStore();
    const result = executeCrud(
      "create",
      { type: "cube", params: { size: { x: 50, y: 30, z: 10 } } },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    expect(result.display).toBeDefined();
    const summary = result.display!.summary;
    expect(summary.some((s) => s.type === "text" && s.text.includes("Cube"))).toBe(true);
    expect(summary.some((s) => s.type === "partLink")).toBe(true);
    expect(result.display!.fields).toContainEqual({ label: "size", value: "50×30×10 mm" });
    expect(result.display!.affectedPartIds).toHaveLength(1);
  });

  it("cylinder create returns summary with radius and height fields", () => {
    const doc = makeMockDocStore();
    const ui = makeMockUiStore();
    const result = executeCrud(
      "create",
      { type: "cylinder", params: { radius: 8, height: 20 } },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    expect(result.display!.fields).toContainEqual({ label: "radius", value: "8 mm" });
    expect(result.display!.fields).toContainEqual({ label: "height", value: "20 mm" });
  });

  it("translate returns summary with part link and offset field", () => {
    const doc = makeMockDocStore([{ id: "part-1", name: "Base", kind: "cube" }]);
    const ui = makeMockUiStore();
    const result = executeCrud(
      "create",
      { type: "translate", params: { child: "part-1", offset: { x: 10, y: 0, z: 0 } } },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    const segments = result.display!.summary;
    expect(segments.some((s) => s.type === "partLink" && s.partId === "part-1")).toBe(true);
    expect(result.display!.fields).toContainEqual({
      label: "offset",
      value: "(10, 0, 0) mm",
    });
  });

  it("difference returns summary with two input part links and result link", () => {
    const doc = makeMockDocStore([
      { id: "a", name: "Base", kind: "cube" },
      { id: "b", name: "Hole", kind: "cylinder" },
    ]);
    const ui = makeMockUiStore();
    const result = executeCrud(
      "create",
      { type: "difference", params: { left: "a", right: "b" } },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    const links = result.display!.summary.filter((s) => s.type === "partLink");
    expect(links).toHaveLength(3); // left, right, result
    expect(result.display!.affectedPartIds).toEqual(expect.arrayContaining(["a", "b"]));
  });

  it("fillet returns summary with target link and radius field", () => {
    const doc = makeMockDocStore([{ id: "p1", name: "Body", kind: "cube" }]);
    const ui = makeMockUiStore();
    const result = executeCrud(
      "create",
      { type: "fillet", params: { child: "p1", radius: 3 } },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    expect(result.display!.fields).toContainEqual({ label: "radius", value: "3 mm" });
    expect(result.display!.summary.some((s) => s.type === "partLink")).toBe(true);
  });

  it("extrude returns summary with segment count and depth field", () => {
    const doc = makeMockDocStore();
    const ui = makeMockUiStore();
    const result = executeCrud(
      "create",
      {
        type: "extrude",
        params: {
          sketch: {
            origin: { x: 0, y: 0, z: 0 },
            x_dir: { x: 1, y: 0, z: 0 },
            y_dir: { x: 0, y: 1, z: 0 },
            segments: [
              { type: "Line", start: { x: 0, y: 0 }, end: { x: 10, y: 0 } },
              { type: "Line", start: { x: 10, y: 0 }, end: { x: 10, y: 10 } },
              { type: "Line", start: { x: 10, y: 10 }, end: { x: 0, y: 10 } },
              { type: "Line", start: { x: 0, y: 10 }, end: { x: 0, y: 0 } },
            ],
          },
          direction: { x: 0, y: 0, z: 5 },
        },
      },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    expect(result.display!.fields).toContainEqual({ label: "segments", value: "4" });
    expect(result.display!.fields).toContainEqual({ label: "depth", value: "5.00 mm" });
  });

  it("delete returns summary with deleted part link", () => {
    const doc = makeMockDocStore([{ id: "p1", name: "Body", kind: "cube" }]);
    const ui = makeMockUiStore();
    const result = executeCrud("delete", { part_id: "p1" }, doc, ui);
    expect(result.status).toBe("success");
    expect(result.display!.summary.some((s) => s.type === "partLink" && s.partId === "p1")).toBe(true);
  });

  it("set_material returns summary with part link and material field", () => {
    const doc = makeMockDocStore([{ id: "p1", name: "Body", kind: "cube" }]);
    const ui = makeMockUiStore();
    const result = executeCrud(
      "set_material",
      { part_id: "p1", material: "aluminum" },
      doc,
      ui,
    );
    expect(result.status).toBe("success");
    expect(result.display!.fields).toContainEqual({ label: "material", value: "aluminum" });
  });

  it("executeCrud populates duration on all successful results", () => {
    const doc = makeMockDocStore();
    const ui = makeMockUiStore();
    const result = executeCrud("create", { type: "cube", params: {} }, doc, ui);
    expect(result.duration).toBeDefined();
    expect(typeof result.duration).toBe("number");
    expect(result.duration).toBeGreaterThanOrEqual(0);
  });

  it("error results have no display field", () => {
    const doc = makeMockDocStore();
    const ui = makeMockUiStore();
    const result = executeCrud("create", { type: "cone", params: {} }, doc, ui);
    expect(result.status).toBe("error");
    expect(result.display).toBeUndefined();
  });
});
```

- [ ] **Step 3: Run tests — expect them to FAIL**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts
```

Expected: The new tests fail because `display` isn't yet attached to the executor outputs. The existing 21 tests should still pass. Duration test may pass accidentally if some default is set — not a concern.

- [ ] **Step 4: Commit failing tests**

```bash
git add packages/core/src/__tests__/command-registry.test.ts
git commit -m "test: add failing tests for tool display payloads"
```

---

### Task 4: Add display payloads to primitive executors (cube/cylinder/sphere)

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Replace the primitive case in executeCreate**

Find the `case "cube":` / `case "cylinder":` / `case "sphere":` block in `executeCreate` and replace it with:

```typescript
      case "cube":
      case "cylinder":
      case "sphere": {
        const partId = docStore.addPrimitive(type as "cube" | "cylinder" | "sphere");
        if (params && Object.keys(params).length > 0) {
          const capitalizedType = type.charAt(0).toUpperCase() + type.slice(1);
          setTimeout(() => {
            docStore.updatePrimitiveOp(partId, { type: capitalizedType, ...params });
          }, 0);
        }
        uiStore.select(partId);

        // Build display payload from the params
        const fields: Array<{ label: string; value: string }> = [];
        if (type === "cube" && params.size) {
          const s = params.size as { x: number; y: number; z: number };
          fields.push({ label: "size", value: `${s.x}×${s.y}×${s.z} mm` });
        }
        if (type === "cylinder") {
          if (params.radius != null) fields.push({ label: "radius", value: `${params.radius} mm` });
          if (params.height != null) fields.push({ label: "height", value: `${params.height} mm` });
        }
        if (type === "sphere" && params.radius != null) {
          fields.push({ label: "radius", value: `${params.radius} mm` });
        }

        const sizeSuffix = fields.length > 0 ? ` ${fields.map((f) => f.value).join(", ")}` : "";
        const capitalized = type.charAt(0).toUpperCase() + type.slice(1);

        return {
          status: "success",
          result: `Created ${type} with id: ${partId}`,
          partId,
          display: {
            summary: [text(`+ ${capitalized}${sizeSuffix} `), link(partId, docStore)],
            fields,
            affectedPartIds: [partId],
          },
        };
      }
```

- [ ] **Step 2: Run primitive tests and verify they pass**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts -t "cube create\|cylinder create"
```

Expected: Both primitive tests pass. Other tests in the new describe block still fail.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display payload to primitive (cube/cylinder/sphere) executor"
```

---

### Task 5: Add display payloads to transform executors (translate/rotate/scale)

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Replace the transform cases in executeCreate**

Find the `case "translate":` / `case "rotate":` / `case "scale":` cases and replace them with:

```typescript
      case "translate": {
        const child = params.child as string;
        const offset = params.offset as { x: number; y: number; z: number };
        if (!child || !offset) return { status: "error", result: "translate requires child and offset" };
        const err = validatePartId(child, docStore, "translate child");
        if (err) return err;
        docStore.setTranslation(child, offset);
        return {
          status: "success",
          result: `Translated ${child} by (${offset.x}, ${offset.y}, ${offset.z})`,
          partId: child,
          display: {
            summary: [
              text("↦ Translate "),
              link(child, docStore),
              text(` by (${offset.x}, ${offset.y}, ${offset.z})`),
            ],
            fields: [{ label: "offset", value: `(${offset.x}, ${offset.y}, ${offset.z}) mm` }],
            affectedPartIds: [child],
          },
        };
      }
      case "rotate": {
        const child = params.child as string;
        const angles = params.angles as { x: number; y: number; z: number };
        if (!child || !angles) return { status: "error", result: "rotate requires child and angles" };
        const err = validatePartId(child, docStore, "rotate child");
        if (err) return err;
        docStore.setRotation(child, angles);
        return {
          status: "success",
          result: `Rotated ${child}`,
          partId: child,
          display: {
            summary: [
              text("↻ Rotate "),
              link(child, docStore),
              text(` by (${angles.x}°, ${angles.y}°, ${angles.z}°)`),
            ],
            fields: [{ label: "angles", value: `(${angles.x}°, ${angles.y}°, ${angles.z}°)` }],
            affectedPartIds: [child],
          },
        };
      }
      case "scale": {
        const child = params.child as string;
        const factor = params.factor as { x: number; y: number; z: number };
        if (!child || !factor) return { status: "error", result: "scale requires child and factor" };
        const err = validatePartId(child, docStore, "scale child");
        if (err) return err;
        docStore.setScale(child, factor);
        return {
          status: "success",
          result: `Scaled ${child}`,
          partId: child,
          display: {
            summary: [
              text("⇱ Scale "),
              link(child, docStore),
              text(` by (${factor.x}, ${factor.y}, ${factor.z})`),
            ],
            fields: [{ label: "factor", value: `(${factor.x}, ${factor.y}, ${factor.z})` }],
            affectedPartIds: [child],
          },
        };
      }
```

- [ ] **Step 2: Run transform tests**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts -t "translate"
```

Expected: translate test passes.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display payloads to transform executors"
```

---

### Task 6: Add display payloads to boolean executors (union/difference/intersection)

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Replace the boolean case in executeCreate**

Find the `case "union":` / `case "difference":` / `case "intersection":` block and replace with:

```typescript
      case "union":
      case "difference":
      case "intersection": {
        let left = params.left as string;
        let right = params.right as string;
        if (!left || !right) {
          const selectedIds = Array.from(uiStore.selectedPartIds);
          if (selectedIds.length !== 2) {
            return { status: "error", result: "Boolean requires left and right part IDs, or exactly 2 parts selected" };
          }
          left = selectedIds[0]!;
          right = selectedIds[1]!;
        }
        const lerr = validatePartId(left, docStore, "boolean left");
        if (lerr) return lerr;
        const rerr = validatePartId(right, docStore, "boolean right");
        if (rerr) return rerr;
        const resultId = docStore.applyBoolean(type, left, right);
        if (!resultId) return { status: "error", result: `${type} failed` };
        const verb = type === "union" ? "Join" : type === "difference" ? "Cut" : "Intersect";
        const icon = type === "union" ? "⊕" : type === "difference" ? "⊖" : "⊗";
        return {
          status: "success",
          result: `Applied ${type} → new part id: ${resultId}`,
          partId: resultId,
          display: {
            summary: [
              text(`${icon} ${verb} `),
              link(left, docStore),
              text(" with "),
              link(right, docStore),
              text(" → "),
              link(resultId, docStore),
            ],
            fields: [{ label: "operation", value: type }],
            affectedPartIds: [left, right, resultId],
          },
        };
      }
```

- [ ] **Step 2: Run boolean test**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts -t "difference"
```

Expected: difference test passes.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display payloads to boolean executors"
```

---

### Task 7: Add display payloads to modifier executors (fillet/chamfer/shell)

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Replace the modifier cases**

Find the `case "fillet":` / `case "chamfer":` / `case "shell":` cases and replace with:

```typescript
      case "fillet": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "fillet requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "fillet target");
        if (err) return err;
        const id = docStore.addFillet(target, params.radius as number);
        if (!id) return { status: "error", result: "Fillet failed — target may not be a solid" };
        return {
          status: "success",
          result: `Applied ${params.radius}mm fillet to ${target} → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`⌒ Fillet `),
              link(target, docStore),
              text(` r=${params.radius}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "radius", value: `${params.radius} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
      case "chamfer": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "chamfer requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "chamfer target");
        if (err) return err;
        const id = docStore.addChamfer(target, params.distance as number);
        if (!id) return { status: "error", result: "Chamfer failed — target may not be a solid" };
        return {
          status: "success",
          result: `Applied ${params.distance}mm chamfer to ${target} → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`⌐ Chamfer `),
              link(target, docStore),
              text(` d=${params.distance}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "distance", value: `${params.distance} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
      case "shell": {
        const target = parentPartId || (params.child as string);
        if (!target) return { status: "error", result: "shell requires parent_part_id or child" };
        const err = validatePartId(target, docStore, "shell target");
        if (err) return err;
        const id = docStore.addShell(target, params.thickness as number);
        if (!id) return { status: "error", result: "Shell failed — target may not be a solid" };
        return {
          status: "success",
          result: `Shelled ${target} with ${params.thickness}mm walls → new part id: ${id}`,
          partId: id,
          display: {
            summary: [
              text(`□ Shell `),
              link(target, docStore),
              text(` t=${params.thickness}mm → `),
              link(id, docStore),
            ],
            fields: [{ label: "thickness", value: `${params.thickness} mm` }],
            affectedPartIds: [target, id],
          },
        };
      }
```

- [ ] **Step 2: Run fillet test**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts -t "fillet"
```

Expected: fillet test passes.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display payloads to modifier executors"
```

---

### Task 8: Add display payload to extrude executor

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Update the extrude case**

Find the `case "extrude":` block. It currently ends with:

```typescript
          return partId
            ? { status: "success", result: `Extruded sketch → new part id: ${partId}`, partId }
            : { status: "error", result: "Extrude failed — check sketch segments form a closed loop" };
```

Replace the success return with:

```typescript
          if (!partId) {
            return { status: "error", result: "Extrude failed — check sketch segments form a closed loop" };
          }
          const depth = Math.sqrt(direction.x ** 2 + direction.y ** 2 + direction.z ** 2);
          return {
            status: "success",
            result: `Extruded sketch → new part id: ${partId}`,
            partId,
            display: {
              summary: [
                text(`▲ Extrude sketch (${s.segments.length} segs, ${depth.toFixed(1)}mm) → `),
                link(partId, docStore),
              ],
              fields: [
                { label: "segments", value: `${s.segments.length}` },
                { label: "depth", value: `${depth.toFixed(2)} mm` },
                { label: "origin", value: `(${s.origin.x}, ${s.origin.y}, ${s.origin.z})` },
              ],
              affectedPartIds: [partId],
            },
          };
```

- [ ] **Step 2: Run extrude test**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts -t "extrude"
```

Expected: extrude test passes.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display payload to extrude executor"
```

---

### Task 9: Add display payloads to delete and set_material executors

**Files:**
- Modify: `packages/core/src/commands/executors.ts`

- [ ] **Step 1: Update executeDelete**

Find the `executeDelete` function at the bottom of the file and replace with:

```typescript
function executeDelete(
  partId: string,
  docStore: DocStore,
  uiStore: UiStore,
): ExecutionResult {
  try {
    const part = docStore.partIndex.get(partId);
    const name = part?.name ?? partId.slice(-4);
    docStore.removePart(partId);
    uiStore.clearSelection();
    return {
      status: "success",
      result: `Deleted part ${partId}`,
      display: {
        summary: [
          text("✕ Delete "),
          { type: "partLink", partId, name },
        ],
        fields: [{ label: "part id", value: partId }],
        affectedPartIds: [partId],
      },
    };
  } catch (err) {
    return { status: "error", result: err instanceof Error ? err.message : "Delete failed" };
  }
}
```

- [ ] **Step 2: Update executeSetMaterial**

Find the existing `executeSetMaterial` function and replace with:

```typescript
function executeSetMaterial(
  partId: string,
  materialKey: string,
  docStore: DocStore,
): ExecutionResult {
  if (!partId) return { status: "error", result: "set_material requires part_id" };
  if (!materialKey) return { status: "error", result: "set_material requires material key" };
  const err = validatePartId(partId, docStore, "set_material part_id");
  if (err) return err;
  try {
    docStore.setPartMaterial(partId, materialKey);
    return {
      status: "success",
      result: `Set ${partId} material to ${materialKey}`,
      partId,
      display: {
        summary: [
          text("⬤ Material "),
          link(partId, docStore),
          text(` = ${materialKey}`),
        ],
        fields: [{ label: "material", value: materialKey }],
        affectedPartIds: [partId],
      },
    };
  } catch (e) {
    return { status: "error", result: e instanceof Error ? e.message : "set_material failed" };
  }
}
```

- [ ] **Step 3: Run delete and set_material tests**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts -t "delete\|set_material\|duration\|error results"
```

Expected: All four tests pass.

- [ ] **Step 4: Run the full test suite to verify nothing regressed**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts
```

Expected: All tests pass (21 existing + ~10 new display tests = ~31 total).

- [ ] **Step 5: Commit**

```bash
git add packages/core/src/commands/executors.ts
git commit -m "feat: add display payloads to delete and set_material executors"
```

---

### Task 10: Extend ToolCallInfo with display and duration

**Files:**
- Modify: `packages/core/src/stores/chat-store.ts`

- [ ] **Step 1: Import ExecutionDisplay and extend the interface**

At the top of `packages/core/src/stores/chat-store.ts`, add the import:

```typescript
import type { ExecutionDisplay } from "../commands/types.js";
```

Find the `ToolCallInfo` interface and add two optional fields:

```typescript
export interface ToolCallInfo {
  id: string;
  name: string;
  args: Record<string, unknown>;
  result?: unknown;
  status: "pending" | "success" | "error";
  display?: ExecutionDisplay;
  duration?: number;
}
```

- [ ] **Step 2: Build core and verify**

Run:
```bash
cd /home/cam/code/vcad && npx tsc -p packages/core/tsconfig.json
```

Expected: No errors.

- [ ] **Step 3: Commit**

```bash
git add packages/core/src/stores/chat-store.ts
git commit -m "feat: extend ToolCallInfo with display and duration fields"
```

---

### Task 11: Propagate display/duration in useChatHandler

**Files:**
- Modify: `packages/app/src/hooks/useChatHandler.ts`

- [ ] **Step 1: Update the tool execution block**

In `packages/app/src/hooks/useChatHandler.ts`, find the tool execution loop that contains:

```typescript
          const toolResults: Array<{ id: string; result: string; status: "success" | "error" }> = [];
          for (const tool of toolCalls) {
            const { result, status } = executeTool(tool);
            toolResults.push({ id: tool.id, result, status });
            const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
            if (entry) {
              entry.result = result;
              entry.status = status;
            }
          }
```

The problem: `executeTool` currently returns only `{ result: string; status: "success" | "error" }` (discarding the full `ExecutionResult`). Replace `executeTool` to return the full result:

Find the existing `executeTool` function:

```typescript
function executeTool(tool: ToolCall): { result: string; status: "success" | "error" } {
  const docStore = useDocumentStore.getState();
  const uiStore = useUiStore.getState();
  return executeCrud(tool.name, tool.args, docStore, uiStore);
}
```

And change its return type to `ExecutionResult`:

```typescript
function executeTool(tool: ToolCall): ExecutionResult {
  const docStore = useDocumentStore.getState();
  const uiStore = useUiStore.getState();
  return executeCrud(tool.name, tool.args, docStore, uiStore);
}
```

Add the `ExecutionResult` type import at the top (in the existing `@vcad/core` import or a new `import type`):

```typescript
import type { SelectionContext, ToolCallInfo, MessagePart, ExecutionResult } from "@vcad/core";
```

Then update the tool execution loop to propagate the full result:

```typescript
          const toolResults: Array<{ id: string; result: string; status: "success" | "error" }> = [];
          for (const tool of toolCalls) {
            const exec = executeTool(tool);
            toolResults.push({ id: tool.id, result: exec.result, status: exec.status });
            const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
            if (entry) {
              entry.result = exec.result;
              entry.status = exec.status;
              entry.display = exec.display;
              entry.duration = exec.duration;
            }
          }
```

- [ ] **Step 2: Build app and verify no type errors**

Run:
```bash
cd /home/cam/code/vcad && npx tsc --noEmit -p packages/app/tsconfig.json 2>&1 | head -20
```

Expected: No new errors. (There may be pre-existing warnings unrelated to this change.)

- [ ] **Step 3: Commit**

```bash
git add packages/app/src/hooks/useChatHandler.ts
git commit -m "feat: propagate display and duration from exec result to ToolCallInfo"
```

---

### Task 12: Create ToolCallCard.tsx component file with ChipSummary and PartLink

**Files:**
- Create: `packages/app/src/components/chat/ToolCallCard.tsx`

- [ ] **Step 1: Create the chat directory and the new component file**

Create `packages/app/src/components/chat/ToolCallCard.tsx` with:

```typescript
import { useState } from "react";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { Check } from "@phosphor-icons/react/dist/ssr/Check";
import { XCircle } from "@phosphor-icons/react/dist/ssr/XCircle";
import { CaretRight } from "@phosphor-icons/react/dist/ssr/CaretRight";
import { cn } from "@/lib/utils";
import { useUiStore } from "@vcad/core";
import type { ToolCallInfo } from "@vcad/core";

// ---------------------------------------------------------------------------
// PartLink — inline clickable part reference
// ---------------------------------------------------------------------------

function PartLink({ partId, name }: { partId: string; name: string }) {
  const handleClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    useUiStore.getState().select(partId);
  };
  return (
    <button
      onClick={handleClick}
      className="inline-flex items-center rounded px-1 bg-accent/10 text-accent hover:bg-accent/20 transition-colors font-medium"
    >
      {name}
    </button>
  );
}

// ---------------------------------------------------------------------------
// ChipSummary — the at-rest summary row
// ---------------------------------------------------------------------------

function ChipSummary({ call }: { call: ToolCallInfo }) {
  if (call.display?.summary && call.status !== "error") {
    return (
      <span className="flex-1 truncate">
        {call.display.summary.map((seg, i) =>
          seg.type === "text" ? (
            <span key={i}>{seg.text}</span>
          ) : (
            <PartLink key={i} partId={seg.partId} name={seg.name} />
          ),
        )}
      </span>
    );
  }
  // Error or no display: show tool name + result inline
  if (call.status === "error" && typeof call.result === "string") {
    return (
      <span className="flex-1 truncate text-error">
        <span className="font-mono">{call.name}</span>
        <span className="ml-1 text-[9px]">{call.result}</span>
      </span>
    );
  }
  // Pending or no display: monospace tool name fallback
  return <span className="font-mono text-text-muted truncate flex-1">{call.name}</span>;
}

// ---------------------------------------------------------------------------
// ChipDetail — the expanded detail pane
// ---------------------------------------------------------------------------

function ChipDetail({ call }: { call: ToolCallInfo }) {
  const [rawOpen, setRawOpen] = useState(false);
  const fields = call.display?.fields ?? [];
  const isError = call.status === "error";

  return (
    <div className="px-2 pb-2 border-t border-border">
      {fields.length > 0 && (
        <dl className="mt-1 grid grid-cols-[auto_1fr] gap-x-2 gap-y-0.5 text-[9px]">
          {fields.map((f, i) => (
            <>
              <dt key={`l-${i}`} className="text-text-muted">{f.label}:</dt>
              <dd key={`v-${i}`} className="text-text font-mono">{f.value}</dd>
            </>
          ))}
        </dl>
      )}
      {isError && typeof call.result === "string" && (
        <div className="mt-1">
          <div className="text-[9px] text-error font-medium">Error:</div>
          <pre className="text-[9px] text-error whitespace-pre-wrap break-all font-mono leading-relaxed">
            {call.result}
          </pre>
        </div>
      )}
      <div className="mt-1 flex items-center gap-1">
        <button
          onClick={() => setRawOpen((r) => !r)}
          className="text-[9px] text-text-muted hover:text-text transition-colors"
        >
          {rawOpen ? "hide raw" : "raw"}
        </button>
        {call.duration != null && (
          <span className="ml-auto text-[9px] text-text-muted">
            {call.duration < 1 ? "<1" : call.duration.toFixed(0)}ms
          </span>
        )}
      </div>
      {rawOpen && (
        <>
          <pre className="mt-1 text-[9px] text-text-muted whitespace-pre-wrap break-all font-mono leading-relaxed">
            {JSON.stringify(call.args, null, 2)}
          </pre>
          {call.result !== undefined && !isError && (
            <>
              <div className="mt-1 text-[9px] text-text-muted font-medium">Result:</div>
              <pre className="text-[9px] text-text-muted whitespace-pre-wrap break-all font-mono leading-relaxed">
                {typeof call.result === "string" ? call.result : JSON.stringify(call.result, null, 2)}
              </pre>
            </>
          )}
        </>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// ToolCallCard — the outer chip
// ---------------------------------------------------------------------------

export function ToolCallCard({ call }: { call: ToolCallInfo }) {
  const [expanded, setExpanded] = useState(false);

  const statusIcon =
    call.status === "success" ? (
      <Check size={10} className="text-success shrink-0" />
    ) : call.status === "error" ? (
      <XCircle size={10} className="text-error shrink-0" />
    ) : (
      <SpinnerGap size={10} className="animate-spin text-text-muted shrink-0" />
    );

  return (
    <div className="mt-1 border border-border bg-bg rounded text-[10px]">
      <button
        onClick={() => setExpanded((e) => !e)}
        className="flex w-full items-center gap-1.5 px-2 py-1 text-left hover:bg-hover transition-colors"
      >
        {statusIcon}
        <ChipSummary call={call} />
        <CaretRight
          size={10}
          className={cn(
            "text-text-muted transition-transform shrink-0",
            expanded && "rotate-90",
          )}
        />
      </button>
      {expanded && <ChipDetail call={call} />}
    </div>
  );
}
```

- [ ] **Step 2: Build app and verify no type errors**

Run:
```bash
cd /home/cam/code/vcad && npx tsc --noEmit -p packages/app/tsconfig.json 2>&1 | grep -i "ToolCallCard\|chat/" | head -10
```

Expected: No errors from the new file.

- [ ] **Step 3: Commit**

```bash
git add packages/app/src/components/chat/ToolCallCard.tsx
git commit -m "feat: create extracted ToolCallCard component with rich chip rendering"
```

---

### Task 13: Wire ChatSidebar to use the new ToolCallCard

**Files:**
- Modify: `packages/app/src/components/ChatSidebar.tsx`

- [ ] **Step 1: Remove the inline ToolCallCard function from ChatSidebar.tsx**

Delete the entire `function ToolCallCard({ call }: { call: ToolCallInfo }) { ... }` block (currently lines ~56-101).

- [ ] **Step 2: Add import of the new ToolCallCard**

In the imports section near the top of `ChatSidebar.tsx`, add:

```typescript
import { ToolCallCard } from "@/components/chat/ToolCallCard";
```

Also remove the now-unused imports of `Check`, `XCircle`, `CaretRight`, `SpinnerGap` from phosphor (they were used by the inline ToolCallCard but still needed by the streaming "Thinking..." indicator — so keep `SpinnerGap`, remove `Check`, `XCircle`, `CaretRight`).

After this change, the top imports should look like:

```typescript
import { useState, useRef, useEffect, useCallback } from "react";
import { X } from "@phosphor-icons/react/dist/ssr/X";
import { PaperPlaneTilt } from "@phosphor-icons/react/dist/ssr/PaperPlaneTilt";
import { Plus } from "@phosphor-icons/react/dist/ssr/Plus";
import { SpinnerGap } from "@phosphor-icons/react/dist/ssr/SpinnerGap";
import { cn } from "@/lib/utils";
import {
  useChatStore,
  useUiStore,
  useDocumentStore,
  useEngineStore,
  parseVcadFile,
  documentToLoon,
} from "@vcad/core";
import type { SelectionContext, ChatMessage } from "@vcad/core";
import { ToolCallCard } from "@/components/chat/ToolCallCard";
```

Note: `ToolCallInfo` type import was only used by the deleted local function, so it can be removed from the `@vcad/core` type imports.

- [ ] **Step 3: Build the app and verify**

Run:
```bash
cd /home/cam/code/vcad && npx tsc --noEmit -p packages/app/tsconfig.json 2>&1 | head -20
```

Expected: No new errors.

- [ ] **Step 4: Commit**

```bash
git add packages/app/src/components/ChatSidebar.tsx
git commit -m "refactor: import extracted ToolCallCard in ChatSidebar"
```

---

### Task 14: Add message footer summary in MessageRow

**Files:**
- Modify: `packages/app/src/components/ChatSidebar.tsx`

- [ ] **Step 1: Add a summarizeToolParts helper above MessageRow**

Above the `MessageRow` function definition in `ChatSidebar.tsx`, add:

```typescript
function summarizeToolParts(parts: Array<{ type: "text"; text: string } | { type: "tool"; tool: { name: string; args: Record<string, unknown>; status: string } }> | undefined): string | null {
  if (!parts) return null;
  const tallies = {
    created: 0,
    cut: 0,
    joined: 0,
    modified: 0,
    moved: 0,
    finished: 0,
    deleted: 0,
    colored: 0,
  };
  let successToolCount = 0;
  for (const p of parts) {
    if (p.type !== "tool") continue;
    const tool = p.tool;
    if (tool.status !== "success") continue;
    successToolCount++;
    const argType = (tool.args.type as string) ?? "";
    if (tool.name === "create") {
      if (["cube", "cylinder", "sphere", "cone", "extrude", "revolve", "sweep", "loft", "sketch_2d", "text_2d"].includes(argType)) {
        tallies.created++;
      } else if (argType === "difference") {
        tallies.cut++;
      } else if (argType === "union" || argType === "intersection") {
        tallies.joined++;
      } else if (["translate", "rotate", "scale"].includes(argType)) {
        tallies.moved++;
      } else if (["fillet", "chamfer", "shell", "linear_pattern", "circular_pattern"].includes(argType)) {
        tallies.finished++;
      }
    } else if (tool.name === "update") {
      tallies.modified++;
    } else if (tool.name === "delete") {
      tallies.deleted++;
    } else if (tool.name === "set_material") {
      tallies.colored++;
    }
  }
  if (successToolCount < 2) return null;
  const nonZero = Object.entries(tallies).filter(([, v]) => v > 0);
  if (nonZero.length === 0) return null;
  return nonZero.map(([k, v]) => `${v} ${k}`).join(" · ");
}
```

- [ ] **Step 2: Render the footer after the parts list in MessageRow**

Find the section in `MessageRow` that renders parts:

```typescript
      {/* Chronological parts (assistant messages with parts) */}
      {!isUser && msg.parts && msg.parts.length > 0 ? (
        <div className="pl-5 space-y-1.5">
          {msg.parts.map((part, i) =>
            part.type === "text" ? (
              part.text.trim() ? (
                <p key={`text-${i}`} className="text-[11px] text-text leading-relaxed whitespace-pre-wrap">
                  {part.text}
                </p>
              ) : null
            ) : (
              <ToolCallCard key={part.tool.id} call={part.tool} />
            )
          )}
        </div>
      ) : (
```

Change it to include the footer summary after the `.map` but inside the same div:

```typescript
      {/* Chronological parts (assistant messages with parts) */}
      {!isUser && msg.parts && msg.parts.length > 0 ? (
        <div className="pl-5 space-y-1.5">
          {msg.parts.map((part, i) =>
            part.type === "text" ? (
              part.text.trim() ? (
                <p key={`text-${i}`} className="text-[11px] text-text leading-relaxed whitespace-pre-wrap">
                  {part.text}
                </p>
              ) : null
            ) : (
              <ToolCallCard key={part.tool.id} call={part.tool} />
            )
          )}
          {(() => {
            const summary = summarizeToolParts(msg.parts);
            return summary ? (
              <p className="text-[9px] text-text-muted italic">{summary}</p>
            ) : null;
          })()}
        </div>
      ) : (
```

- [ ] **Step 3: Build app and verify**

Run:
```bash
cd /home/cam/code/vcad && npx tsc --noEmit -p packages/app/tsconfig.json 2>&1 | head -10
```

Expected: No errors.

- [ ] **Step 4: Commit**

```bash
git add packages/app/src/components/ChatSidebar.tsx
git commit -m "feat: add message-level tool summary footer"
```

---

### Task 15: Full build and manual smoke test

**Files:**
- No file changes — verification only.

- [ ] **Step 1: Run the full core test suite**

Run:
```bash
cd /home/cam/code/vcad && npx vitest run packages/core/src/__tests__/command-registry.test.ts
```

Expected: All tests pass (21 existing + ~10 new = ~31 tests, 0 failing).

- [ ] **Step 2: Run Rust workspace tests**

Run:
```bash
cd /home/cam/code/vcad && cargo test -p vcad-ir tool_schema
```

Expected: 5 passed.

- [ ] **Step 3: Build all TypeScript packages**

Run:
```bash
cd /home/cam/code/vcad && npx tsc -p packages/core/tsconfig.json && cp CHANGELOG.json packages/core/dist/
```

Expected: No errors.

- [ ] **Step 4: Clear Vite cache and start dev server**

Run:
```bash
cd /home/cam/code/vcad && rm -rf packages/app/node_modules/.vite && npm run dev -w @vcad/app -- --host 0.0.0.0
```

Expected: Vite ready on http://localhost:5173/.

- [ ] **Step 5: Manual smoke test**

Open the dev URL. In the chat:

1. Say: "create a cube 50x30x10"
   - Expected chip: `✓ + Cube 50×30×10 mm [mock:0]` with the part name clickable
2. Say: "translate it by 20mm in x"
   - Expected chip: `✓ ↦ Translate [name] by (20, 0, 0)` — clicking the part name selects it
3. Expand the cube chip
   - Expected: `size: 50×30×10 mm` field, `raw` button, timing like `<1ms`
4. Expand raw
   - Expected: JSON args + result visible
5. Say: "create a cylinder, then cut it from the cube"
   - Expected: 2+ tool chips, message footer shows `1 created · 1 cut` (or similar)
6. Say: "do something impossible like create a cone"
   - Expected: Red error chip with inline error message

- [ ] **Step 6: Commit any final adjustments**

If manual test surfaces issues, fix and commit. Otherwise no action.

---

### Task 16: Changelog entry

**Files:**
- Modify: `CHANGELOG.json`

- [ ] **Step 1: Add entry at the top of the entries array**

In `CHANGELOG.json`, add this entry as the first item in the `entries` array (after the opening `[`):

```json
    {
      "id": "2026-04-12-rich-tool-chips",
      "version": "0.8.0",
      "date": "2026-04-12",
      "category": "feat",
      "title": "Rich AI chat tool call chips",
      "summary": "Tool call chips now show action sentences with clickable part links, human-readable field lists, and per-message operation summaries.",
      "features": ["ai", "chat", "ux"]
    },
```

- [ ] **Step 2: Copy the updated CHANGELOG to core dist**

Run:
```bash
cd /home/cam/code/vcad && cp CHANGELOG.json packages/core/dist/
```

- [ ] **Step 3: Commit**

```bash
git add CHANGELOG.json
git commit -m "docs: add changelog entry for rich tool call chips"
```
