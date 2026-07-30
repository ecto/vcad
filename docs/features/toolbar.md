# Bottom Toolbar

The tab-based bottom toolbar organizes vcad's tools into logical groups, auto-switches based on context, and supports expand/collapse for both learning and efficient workflows.

**Status:** `shipped` | **Priority:** p0

> Verified against repo 2026-07-30. Note: the component is now `ToolPalette.tsx` (plus `MobileToolPalette.tsx`), not `BottomToolbar.tsx`, and the tab set has grown beyond this doc (e.g. `sketch`, `build`, `circuit` tabs in `ui-store.ts`).

---

## Design

```
┌─────────────────────────────────────────────────────────────────────┐
│ [Create] [Transform] [Combine] [Modify] [Assembly] [Simulate] [View]│  ← Tabs
├─────────────────────────────────────────────────────────────────────┤
│     [ Box ]  [ Cylinder ]  [ Sphere ]  [ Sketch ]          [⊞][🖨] │  ← Tools + toggle + print
└─────────────────────────────────────────────────────────────────────┘
```

---

## Tabs

### Create

Add new geometry to the scene.

| Tool | Description | Shortcut |
|------|-------------|----------|
| Box | Add a box primitive | - |
| Cylinder | Add a cylinder primitive | - |
| Sphere | Add a sphere primitive | - |
| Sketch | Start a new sketch on a plane or face | S |

**Auto-activates:** When nothing is selected

### Transform

Move, rotate, or scale selected parts.

| Tool | Description | Shortcut |
|------|-------------|----------|
| Move | Translate selected parts | M |
| Rotate | Rotate selected parts | R |
| Scale | Scale selected parts | S |

**Auto-activates:** When 1+ parts are selected

### Combine

Boolean operations on two parts.

| Tool | Description | Shortcut |
|------|-------------|----------|
| Union | Merge two parts together | ⌘⇧U |
| Difference | Subtract second part from first | ⌘⇧D |
| Intersection | Keep only overlapping volume | ⌘⇧I |

**Auto-activates:** When exactly 2 parts are selected

**Disabled state:** Tools disabled unless 2 parts are selected. Tooltip shows "select 2 parts".

### Modify

Apply operations to a single part.

| Tool | Description |
|------|-------------|
| Fillet | Round edges |
| Chamfer | Bevel edges |
| Shell | Hollow out the part |
| Pattern | Create linear or circular arrays |
| Mirror | Mirror across a plane |

**Auto-activates:** Never (manual only)

**Disabled state:** Tools disabled unless 1 part is selected. Tooltip shows "select a part".

### Assembly

Create multi-part assemblies with joints.

| Tool | Description |
|------|-------------|
| Create Part | Convert selected part to a reusable part definition |
| Insert | Insert an instance of a part definition |
| Joint | Add a joint between two instances |

**Auto-activates:** When an instance is selected

**Disabled states:**
- Create Part: disabled unless 1 part is selected
- Insert: disabled if no part definitions exist
- Joint: disabled unless 2 instances are selected

### Simulate

Physics simulation controls.

| Tool | Description |
|------|-------------|
| Play/Pause | Start or pause the simulation |
| Stop | Reset simulation to initial state |
| Step | Advance simulation by one frame |
| Speed | Adjust playback speed (0.1x - 2.0x) |

**Auto-activates:** Never (manual only)

**Disabled state:** All tools disabled if no joints exist. Tooltip shows "add joints to simulate".

### View

Toggle between 3D modeling and 2D drafting views.

| Tool | Description |
|------|-------------|
| 3D | Standard 3D modeling view |
| 2D | Orthographic drafting view |

**2D-only tools (shown when in 2D mode):**

| Tool | Description |
|------|-------------|
| Direction | Select view direction (front, top, right, etc.) |
| Hidden Lines | Toggle hidden line visibility |
| Dimensions | Toggle automatic dimensions |
| Detail | Create detail view of selected region |
| Clear | Remove all detail views |
| Export | Export view to DXF |

**Auto-activates:** When entering 2D mode

---

## Auto-Switch Behavior

The toolbar automatically switches tabs based on your current context to show relevant tools:

1. **Nothing selected** → Create tab (add new geometry)
2. **1+ parts selected** → Transform tab (move/rotate/scale)
3. **2 parts selected** → Combine tab (boolean operations)
4. **Instance selected** → Assembly tab (joints and instances)
5. **2D mode** → View tab (drafting tools)

**Manual tabs:** The Modify and Simulate tabs never auto-activate to avoid jarring switches during complex workflows.

You can always manually click any tab to override the auto-switch behavior.

---

## Expand/Collapse

The toolbar has two density modes:

### Expanded (default)
- Tab names visible
- Tool buttons show icons + labels
- Keyboard shortcuts shown
- Easier for learning

### Collapsed
- Tab icons only
- Tool buttons show icons only
- More compact
- Better for experts

Toggle using the chevron button (▼/▲) in the toolbar corner.

Your preference is saved to localStorage and persists across sessions.

---

## Always Visible

These elements remain visible regardless of active tab:

- **Expand/collapse toggle**: Control toolbar density
- **Print button**: Access 3D print settings

---

## Mobile

The toolbar is optimized for mobile:

- Same tab structure, no horizontal scrolling on tool area
- Tabs may show icons only on small screens
- 44px touch targets (iOS minimum)
- Full-width at screen bottom with safe area padding

---

## Keyboard Shortcuts

### Tab Navigation

No direct keyboard shortcuts for tab switching (use mouse/touch).

### Tool Shortcuts

| Shortcut | Action |
|----------|--------|
| S | New sketch |
| M | Move mode |
| R | Rotate mode |
| S | Scale mode (when in transform) |
| ⌘⇧U | Union |
| ⌘⇧D | Difference |
| ⌘⇧I | Intersection |

---

## Implementation

- **Component:** `packages/app/src/components/ToolPalette.tsx` (mobile: `MobileToolPalette.tsx`)
- **State:** `toolbarExpanded`, `toolbarTab` in `packages/core/src/stores/ui-store.ts`
- **Persistence:** `vcad:toolbarExpanded` in localStorage
