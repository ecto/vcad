# Rich Tool Call Chips Design

Upgrade the AI chat tool call chips from anonymous icon+name buttons to rich, informative, part-linked summaries with expandable detail.

## Problem

The current `ToolCallCard` in `ChatSidebar.tsx` shows every tool call as a generic monospace name (e.g. `create`, `update`, `difference`) with a spinner/check/x icon. Users have no idea what the AI actually did without expanding every chip, and the expanded view is raw JSON. Multiple tool calls from the same message are visually indistinguishable.

## Goals

1. **Informative at a glance**: show a natural-language action sentence with the affected shape(s) and key params
2. **Part linking**: clicking a part reference in the chip selects that part in the viewport
3. **Rich detail on expand**: human-readable field list, optional raw JSON toggle, optional timing
4. **Message-level summary**: aggregate footer after multi-step operations
5. **Single source of truth**: the executor writes the display info in the same place it writes the AI-facing result — no parallel formatter module

## Non-Goals

- Inline 3D thumbnails per chip (too expensive for v1)
- Per-chip undo button
- Hover tooltips with mini-previews
- MCP/command-palette consuming the display format (future — shape supports it)

## Architecture

The design keeps everything co-located with the executor:

```
packages/core/src/commands/
  types.ts           — adds SummarySegment, ExecutionDisplay, extended ExecutionResult
  executors.ts       — each successful return adds a `display` field
  ...
packages/core/src/stores/
  chat-store.ts      — ToolCallInfo extended with display + duration
packages/app/src/hooks/
  useChatHandler.ts  — propagates display/duration from exec result → ToolCallInfo
packages/app/src/components/
  ChatSidebar.tsx    — removes inline ToolCallCard, adds message footer summary
  chat/ToolCallCard.tsx  — NEW: extracted chip component with subcomponents
```

**Single source of truth principle:** The executor author writes the sentence once, as structured `SummarySegment[]`. The AI-facing `result` string is a sibling field derived from or written alongside the segments. No downstream formatting layer means the display and AI feedback cannot drift.

## 1. New Types

File: `packages/core/src/commands/types.ts`

```typescript
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

export interface ExecutionResult {
  status: "success" | "error";
  /** Human-readable result returned to the AI as tool_result content. */
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
```

## 2. Executor Changes

File: `packages/core/src/commands/executors.ts`

### 2.1 Timing wrapper

The top-level `executeCrud` measures duration once per dispatch, so individual case branches don't need to track it:

```typescript
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
```

### 2.2 partLink helper

```typescript
function partLink(id: string, docStore: DocStore): { partId: string; name: string } {
  const part = docStore.partIndex.get(id);
  return {
    partId: id,
    name: part?.name ?? id.slice(-4),
  };
}

function text(s: string): SummarySegment {
  return { type: "text", text: s };
}

function link(id: string, docStore: DocStore): SummarySegment {
  const p = partLink(id, docStore);
  return { type: "partLink", partId: p.partId, name: p.name };
}
```

### 2.3 Per-tool display payloads

Each successful return in the existing executor gets a `display` field. Error returns are unchanged (UI falls back to the `result` string).

**Primitives (cube/cylinder/sphere):**
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

  const fields: Array<{ label: string; value: string }> = [];
  if (type === "cube" && params.size) {
    const s = params.size as { x: number; y: number; z: number };
    fields.push({ label: "size", value: `${s.x}×${s.y}×${s.z} mm` });
  }
  if ((type === "cylinder" || type === "sphere") && params.radius) {
    fields.push({ label: "radius", value: `${params.radius} mm` });
  }
  if (type === "cylinder" && params.height) {
    fields.push({ label: "height", value: `${params.height} mm` });
  }

  const sizeSuffix = fields.length > 0
    ? ` ${fields.map((f) => f.value).join(" × ")}`
    : "";

  return {
    status: "success",
    result: `Created ${type} with id: ${partId}`,
    partId,
    display: {
      summary: [
        text(`+ ${type.charAt(0).toUpperCase() + type.slice(1)}${sizeSuffix} `),
        link(partId, docStore),
      ],
      fields,
      affectedPartIds: [partId],
    },
  };
}
```

**Translate/rotate/scale:**
```typescript
case "translate": {
  // ... existing validation
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
```

(Similar shape for `rotate` and `scale`.)

**Booleans:**
```typescript
case "union":
case "difference":
case "intersection": {
  // ... existing validation
  const resultId = docStore.applyBoolean(type, left, right);
  if (!resultId) return { status: "error", result: `${type} failed` };
  const verb = type === "union" ? "Join" : type === "difference" ? "Cut" : "Intersect";
  return {
    status: "success",
    result: `Applied ${type} → new part id: ${resultId}`,
    partId: resultId,
    display: {
      summary: [
        text(`⊕ ${verb} `),
        link(left, docStore),
        text(" with "),
        link(right, docStore),
        text(" → "),
        link(resultId, docStore),
      ],
      fields: [
        { label: "operation", value: type },
      ],
      affectedPartIds: [left, right, resultId],
    },
  };
}
```

**Modifiers (fillet/chamfer/shell):**
```typescript
case "fillet": {
  // ... existing validation
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
```

**Extrude (sketch ops):**
```typescript
case "extrude": {
  // ... existing validation + sketch validation
  const partId = docStore.addExtrude(plane, s.origin, s.segments as never[], direction, options);
  if (!partId) return { status: "error", result: "Extrude failed — check sketch segments form a closed loop" };
  const depth = Math.sqrt(direction.x ** 2 + direction.y ** 2 + direction.z ** 2);
  return {
    status: "success",
    result: `Extruded sketch → new part id: ${partId}`,
    partId,
    display: {
      summary: [
        text(`▲ Extrude sketch (${s.segments.length} segs, depth ${depth.toFixed(1)}mm) → `),
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
}
```

**update/delete/set_material** follow the same pattern.

### 2.4 Error case

Error returns deliberately skip `display` — the chip renders the plain `result` string with red styling in place of a summary sentence. The chip remains expandable (same caret); the expanded view shows the raw args under "Input" and the error message under "Error". No special handling needed in the executor for errors — they return the same `{status: "error", result: "..."}` shape as today.

## 3. ToolCallInfo Extension

File: `packages/core/src/stores/chat-store.ts`

```typescript
export interface ToolCallInfo {
  id: string;
  name: string;
  args: Record<string, unknown>;
  result?: unknown;
  status: "pending" | "success" | "error";
  display?: ExecutionDisplay;   // NEW
  duration?: number;             // NEW (ms)
}
```

## 4. Chat Handler Propagation

File: `packages/app/src/hooks/useChatHandler.ts`

When the tool result comes back from `executeCrud`, the handler copies `display` and `duration` onto the `ToolCallInfo`:

```typescript
const exec = executeCrud(tool.name, tool.args, docStore, uiStore);
toolResults.push({ id: tool.id, result: exec.result, status: exec.status });
const entry = accumulatedToolCalls.find((t) => t.id === tool.id);
if (entry) {
  entry.result = exec.result;
  entry.status = exec.status;
  entry.display = exec.display;    // NEW
  entry.duration = exec.duration;  // NEW
}
```

## 5. ToolCallCard Extraction

New file: `packages/app/src/components/chat/ToolCallCard.tsx`

### 5.1 Component tree

```
<ToolCallCard call>
  <ChipSummary call onTogglePartSelect />   ← always visible
  {expanded && <ChipDetail call />}         ← expandable
```

### 5.2 ChipSummary

Renders: status icon + summary segments (text spans + `PartLink` components) + caret chevron.

**Status icons** (unchanged from current):
- pending → spinning `SpinnerGap`
- success → green `Check`
- error → red `XCircle`

**Summary rendering:**
```typescript
function ChipSummary({ call }: { call: ToolCallInfo }) {
  if (call.display?.summary) {
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
  // Fallback: monospace tool name (current behavior)
  return <span className="font-mono text-text-muted truncate flex-1">{call.name}</span>;
}
```

**Error styling:** When `call.status === "error"`, the whole summary gets a `text-error` class. The plain `call.result` string is shown inline below the tool name (since errors have no `display`).

### 5.3 PartLink

```typescript
function PartLink({ partId, name }: { partId: string; name: string }) {
  const handleClick = (e: React.MouseEvent) => {
    e.stopPropagation(); // don't toggle chip expansion
    useUiStore.getState().select(partId);
  };
  return (
    <button
      onClick={handleClick}
      className="inline-flex items-center rounded px-1 py-0 bg-accent/10 text-accent hover:bg-accent/20 transition-colors font-medium"
    >
      {name}
    </button>
  );
}
```

### 5.4 ChipDetail

Expanded view contents, in order:

1. **Fields** section (if `display.fields` present):
   ```
   size:    50×30×10 mm
   radius:  8 mm
   ```
   Two-column: label in muted color, value in text color.

2. **Error** (error chips only): the `call.result` string in an `Error:` labeled block with red accent.

3. **Raw JSON toggle**: a small `<button>` labeled "raw" that flips a local state to show args/result JSON. Collapsed by default. Available on both success and error chips.

4. **Timing**: `{duration}ms` in muted text at the bottom (if `call.duration` present).

### 5.5 File size

Target ~160 lines. Uses existing Phosphor icons. No new dependencies.

## 6. Message Footer Summary

Added inline to `MessageRow` in `ChatSidebar.tsx`. Small helper function counts categories from `msg.parts`:

```typescript
function summarizeToolParts(parts: MessagePart[]): string | null {
  const tallies = { created: 0, cut: 0, joined: 0, modified: 0, moved: 0, finished: 0, deleted: 0, colored: 0 };
  for (const p of parts) {
    if (p.type !== "tool") continue;
    const tool = p.tool;
    if (tool.status !== "success") continue;
    const argType = (tool.args.type as string) ?? "";
    if (tool.name === "create") {
      if (["cube", "cylinder", "sphere", "cone", "extrude", "revolve", "sweep", "loft", "sketch_2d", "text_2d"].includes(argType)) tallies.created++;
      else if (argType === "difference") tallies.cut++;
      else if (argType === "union") tallies.joined++;
      else if (argType === "intersection") tallies.joined++;
      else if (["translate", "rotate", "scale"].includes(argType)) tallies.moved++;
      else if (["fillet", "chamfer", "shell"].includes(argType)) tallies.finished++;
      else if (["linear_pattern", "circular_pattern"].includes(argType)) tallies.finished++;
    } else if (tool.name === "update") tallies.modified++;
    else if (tool.name === "delete") tallies.deleted++;
    else if (tool.name === "set_material") tallies.colored++;
  }
  const nonZero = Object.entries(tallies).filter(([, v]) => v > 0);
  if (nonZero.length === 0) return null;
  return nonZero.map(([k, v]) => `${v} ${k}`).join(" · ");
}
```

Displayed in a small muted footer below the tool chips only when there are **2 or more** successful tool parts:

```jsx
{toolPartCount >= 2 && (
  <p className="pl-5 text-[9px] text-text-muted italic mt-1">
    {summarizeToolParts(msg.parts)}
  </p>
)}
```

## 7. Tests

Extend `packages/core/src/__tests__/command-registry.test.ts` with a new describe block `"ExecutionResult display"`:

```typescript
describe("ExecutionResult display", () => {
  // mock minimal docStore/uiStore for each case
  it("cube create returns summary with part link and size field");
  it("cylinder create returns summary with r/h fields");
  it("translate returns summary with part link and offset field");
  it("difference returns summary with two input part links and result link");
  it("fillet returns summary with target link and radius field");
  it("extrude returns summary with segment count and depth field");
  it("update returns summary with node id and changed fields");
  it("delete returns summary with deleted part link");
  it("set_material returns summary with part link and material field");
  it("executeCrud populates duration on all successful results");
  it("error results have no display field");
});
```

Visual/component tests for `ToolCallCard.tsx` are skipped — the rendering is straightforward and exercised in the running app.

## 8. Files Modified / Created

**Modified:**
- `packages/core/src/commands/types.ts` — new types
- `packages/core/src/commands/executors.ts` — display fields on success returns + timing wrapper
- `packages/core/src/stores/chat-store.ts` — ToolCallInfo extension
- `packages/core/src/__tests__/command-registry.test.ts` — new tests
- `packages/app/src/hooks/useChatHandler.ts` — propagate display/duration
- `packages/app/src/components/ChatSidebar.tsx` — remove inline ToolCallCard, add message footer

**Created:**
- `packages/app/src/components/chat/ToolCallCard.tsx` — extracted rich chip
- `packages/app/src/components/chat/PartLink.tsx` — inline clickable part reference (optional — could stay in ToolCallCard.tsx)

## 9. Migration & Back-Compat

The `display` field is optional. Existing code that doesn't populate it (or future tools that don't bother) falls back to the current behavior — monospace tool name, JSON blob in the detail view. No breaking changes to the `ExecutionResult` / `ToolCallInfo` consumers.

## 10. Future Extensions Enabled by This Design

- **Inline 3D previews**: the `affectedPartIds` field gives the UI exactly what it needs to render a mini viewport of just those parts
- **Per-chip undo**: expose a `rewindTo: UndoSnapshot` on the display payload
- **Hover tooltips**: the `fields` array is already structured for tooltip rendering
- **MCP / palette consumption**: serialize `display.summary` to a plain string for non-chat surfaces
- **Chat-first editing**: clicking a field in the expanded view could open an update dialog
