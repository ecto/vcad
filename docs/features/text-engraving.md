# Text/Engraving IR Support

Add text geometry to the IR for personalized parts (embossing and engraving).

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `l` |

> Verified against repo 2026-07-30. Corrected `in-progress` → `shipped`: `crates/vcad-kernel-text` exists, `Text2D` variant in `crates/vcad-ir/src/lib.rs`, `textExtrude` WASM binding, and `Text2D` handling in `packages/engine/src/evaluate.ts`.

## Problem

Users cannot add text to their CAD models for:
- Name tags and personalization
- Labels and part numbers
- Serial numbers and branding
- Technical markings

Text must currently be designed in external software and imported, breaking the parametric workflow.

## Solution

Add `Text2D` operation to the IR that converts text strings into sketch profiles, which can then be extruded and booleaned with other geometry.

### IR Schema

**Rust** (`crates/vcad-ir/src/lib.rs`):
```rust
/// Text alignment options.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextAlignment {
    Left,
    Center,
    Right,
}

/// 2D text that can be extruded into 3D geometry.
Text2D {
    /// Origin point in 3D space.
    origin: Vec3,
    /// X direction of the text plane (text flows along this axis).
    x_dir: Vec3,
    /// Y direction of the text plane (text height along this axis).
    y_dir: Vec3,
    /// The text string to render.
    text: String,
    /// Font name (e.g., "sans-serif", "monospace", or custom registered font).
    font: String,
    /// Text height in mm.
    height: f64,
    /// Letter spacing multiplier (1.0 = normal, optional).
    letter_spacing: Option<f64>,
    /// Line spacing multiplier for multi-line text (1.0 = normal, optional).
    line_spacing: Option<f64>,
    /// Text alignment (left, center, right).
    alignment: TextAlignment,
}
```

**TypeScript** (`packages/ir/src/index.ts`):
```typescript
export type TextAlignment = "left" | "center" | "right";

export interface Text2DOp {
  type: "Text2D";
  origin: Vec3;
  x_dir: Vec3;
  y_dir: Vec3;
  text: string;
  font: string;
  height: number;
  letter_spacing?: number;
  line_spacing?: number;
  alignment?: TextAlignment;
}
```

### Usage Patterns

**Emboss** (raised text):
```
Text2D → Extrude (positive direction) → Union with base
```

**Engrave** (cut text):
```
Text2D → Extrude (negative direction) → Difference from base
```

**Not included:** Curved text along paths, 3D text effects (bevels), font selection UI.

## UX Details

### Default Behavior

| Parameter | Default |
|-----------|---------|
| `font` | `"sans-serif"` (built-in Open Sans) |
| `height` | Required, in mm |
| `letter_spacing` | `1.0` |
| `line_spacing` | `1.2` |
| `alignment` | `"left"` |

### Built-in Fonts

| Name | Font |
|------|------|
| `sans-serif` | Open Sans (embedded subset) |
| `monospace` | JetBrains Mono (future) |

### Edge Cases

- **Empty text**: Return empty geometry (no error)
- **Unsupported characters**: Skip character, log warning
- **Very small height**: Minimum 0.1mm enforced
- **Multi-line text**: Split on `\n`, stack vertically

## Implementation

### New Crate: `vcad-kernel-text`

```
crates/vcad-kernel-text/
├── Cargo.toml
├── src/
│   ├── lib.rs       # Public API: text_to_profiles(), text_bounds()
│   ├── font.rs      # FontRegistry, ttf-parser integration
│   ├── glyph.rs     # Glyph outline extraction
│   ├── profile.rs   # Contours → SketchProfile conversion
│   └── builtin.rs   # Embedded Open Sans subset
```

**Dependencies**:
- `ttf-parser = "0.25"` — TTF/OTF parsing
- `thiserror = "2"` — Error handling

### Key Functions

```rust
// lib.rs

/// Convert text to a list of sketch profiles.
/// Each profile represents one connected contour (glyph outline or hole).
pub fn text_to_profiles(
    text: &str,
    font: &Font,
    height: f64,
    letter_spacing: f64,
    line_spacing: f64,
    alignment: TextAlignment,
) -> Vec<SketchProfile>

/// Get the bounding box of rendered text.
pub fn text_bounds(
    text: &str,
    font: &Font,
    height: f64,
    letter_spacing: f64,
) -> (f64, f64)  // (width, height)

/// Font registry for managing loaded fonts.
pub struct FontRegistry {
    fonts: HashMap<String, Font>,
}

impl FontRegistry {
    pub fn new() -> Self;
    pub fn register(&mut self, name: &str, data: &[u8]) -> Result<(), FontError>;
    pub fn get(&self, name: &str) -> Option<&Font>;
    pub fn builtin_sans() -> &'static Font;
}
```

### WASM Bindings

```typescript
// vcad-kernel-wasm/src/lib.rs additions

impl Solid {
    /// Create text geometry and extrude it.
    pub fn textExtrude(
        text: &str,
        origin: Vec3,
        x_dir: Vec3,
        y_dir: Vec3,
        direction: Vec3,
        height: f64,
        font: Option<&str>,
        alignment: Option<&str>,
        letter_spacing: Option<f64>,
    ) -> Result<Solid, JsValue>;
}

/// Register a custom font for text operations.
pub fn registerFont(name: &str, data: &[u8]) -> Result<(), JsValue>;

/// Get the bounding box of rendered text.
pub fn textBounds(
    text: &str,
    height: f64,
    font: Option<&str>,
) -> TextBounds;
```

### Files to Modify

| File | Changes |
|------|---------|
| `crates/vcad-kernel-text/` | NEW — entire crate |
| `crates/vcad-kernel/Cargo.toml` | Add `vcad-kernel-text` dependency |
| `crates/vcad-kernel/src/lib.rs` | Re-export text module, add facade methods |
| `crates/vcad-ir/src/lib.rs` | Add `Text2D` variant and `TextAlignment` enum |
| `packages/ir/src/index.ts` | Mirror `Text2DOp` and `TextAlignment` types |
| `crates/vcad-kernel-wasm/src/lib.rs` | Add `textExtrude`, `registerFont`, `textBounds` |
| `packages/engine/src/evaluate.ts` | Handle `Text2D` in Extrude case |

## Tasks

### Phase 1: IR Types (`xs`)

- [x] Add `TextAlignment` enum to `vcad-ir`
- [x] Add `Text2D` variant to `CsgOp` in `vcad-ir`
- [x] Mirror types in `packages/ir/src/index.ts`

### Phase 2: Kernel Crate (`m`)

- [x] Create `vcad-kernel-text` crate structure
- [ ] Implement `FontRegistry` with embedded Open Sans
- [ ] Implement glyph outline extraction using `ttf-parser`
- [ ] Implement contour → `SketchProfile` conversion
- [ ] Add `text_to_profiles()` and `text_bounds()` functions
- [ ] Write unit tests for text rendering

### Phase 3: Integration (`m`)

- [x] Add `vcad-kernel-text` to `vcad-kernel` workspace
- [x] Add WASM bindings (`textExtrude`, `registerFont`, `textBounds`)
- [x] Update `evaluate.ts` to handle `Text2D` in Extrude
- [ ] Test emboss workflow (Text2D → Extrude → Union)
- [ ] Test engrave workflow (Text2D → Extrude → Difference)

### Phase 4: Polish (`s`)

- [ ] Add to VCode format (new `TXT` opcode)
- [ ] Update MCP tools to support text operations
- [ ] Test multi-line text
- [ ] Test custom font registration in browser

## Acceptance Criteria

- [x] `Text2D` IR node can be created and serialized
- [x] Text renders correctly with built-in sans-serif font
- [x] Extruding text produces valid mesh geometry
- [x] Boolean operations work with text solids (emboss/engrave)
- [ ] Custom fonts can be registered and used in browser
- [ ] `textBounds()` returns accurate dimensions

## Future Enhancements

- [ ] Font selection UI in app
- [ ] Additional built-in fonts (monospace, serif)
- [ ] Curved text along sketch paths
- [ ] Text on faces (project onto curved surfaces)
- [ ] Bold/italic variants
- [ ] Outline-only text (stroke instead of fill)
