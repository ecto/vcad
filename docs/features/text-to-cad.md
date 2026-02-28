# Text-to-CAD (cad0)

Generate parametric CAD models from natural language descriptions.

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p0` |
| Effort | `xl` (7 weeks) |
| Target | Q2 2025 |

## Problem

Creating CAD models requires learning complex tools. Users must:

1. Understand parametric modeling concepts (sketches, extrusions, booleans)
2. Learn specific software UI (dozens of toolbar buttons, modal dialogs)
3. Translate mental design → precise geometry (constraints, dimensions)
4. Debug failed operations (non-manifold, self-intersection, thin walls)

This friction excludes:
- **Hobbyists** who just want to 3D print a simple bracket
- **Engineers** who think faster than they can click
- **Designers** iterating on concepts before committing to CAD
- **Non-experts** who can describe what they want but can't model it

AI should understand "design a bracket with 4 mounting holes" and produce valid parametric geometry.

## Solution

**cad0** — a family of fine-tuned LLMs that output vcad VCode from natural language.

### Model Variants

| Model | Parameters | Use Case | Deployment |
|-------|------------|----------|------------|
| **cad0** | 7B | Server/CLI inference, highest quality | Rust inference via Candle |
| **cad0-mini** | 1.5B | Browser offline fallback | WASM + WebGPU |

### VCode Format

Efficient token representation for model output:

```
# Bracket with mounting holes
box 100 50 10 @base
cylinder r=3 h=15 @hole_template
pattern @hole_template linear x=80 y=40 nx=2 ny=2 @holes
difference @base @holes @bracket
fillet @bracket r=2
```

Key properties:
- **Dense** — No verbose JSON, minimal tokens per operation
- **Unambiguous** — Each line is one operation
- **Parametric** — Named references enable downstream editing
- **Ordered** — Dependency order matches line order

### Why Not Existing Formats?

| Format | Problem |
|--------|---------|
| JSON IR | 10x more tokens, expensive inference |
| Python/CadQuery | Requires code execution, security risk |
| OpenSCAD | Imperative, hard to edit |
| STEP | Not parametric, huge files |

VCode is designed specifically for LLM output efficiency.

## UX Details

### Text Input

```
┌─────────────────────────────────────────────────────────┐
│ 🔮 Describe what you want to create...                  │
│                                                         │
│ [Design a phone stand with adjustable angle]     [⏎]   │
└─────────────────────────────────────────────────────────┘
```

### Generation Flow

1. User types natural language description
2. Loading indicator with "Generating CAD model..."
3. VCode appears in side panel (collapsible)
4. 3D preview renders as model generates (streaming)
5. On completion, model is added to document as parametric feature

### Interaction States

| State | Behavior |
|-------|----------|
| Empty | Placeholder text, example prompts below |
| Typing | Debounce 500ms before showing suggestions |
| Generating | Spinner, disable input, show progress |
| Success | Preview in viewport, "Add to document" button |
| Error | Red border, error message, retry button |
| Offline (cad0-mini) | Badge indicating local inference |

### Edge Cases

- **Ambiguous input**: Ask clarifying questions ("How many holes? What diameter?")
- **Impossible geometry**: Explain why and suggest alternatives
- **Very complex**: Warn about generation time, offer to simplify
- **Network failure**: Fall back to cad0-mini if available

## Architecture

### Training Pipeline

```
┌──────────────┐    ┌──────────────┐    ┌──────────────┐
│ Part Family  │───▶│  Synthetic   │───▶│   Training   │
│  Generators  │    │   Dataset    │    │   Pipeline   │
└──────────────┘    └──────────────┘    └──────────────┘
       │                   │                    │
       ▼                   ▼                    ▼
   Parametric         500K pairs:           Qwen 2.5
   Templates          prompt → IR          Coder 7B
   (brackets,                              + LoRA
   enclosures,
   gears, etc.)
```

### Part Family Generators

Procedural generators for common part types:

| Family | Parameters | Variations |
|--------|------------|------------|
| Brackets | holes, thickness, angle, material | L, Z, U shapes |
| Enclosures | dimensions, wall thickness, lid type | Box, rounded, vented |
| Gears | teeth, module, bore, hub | Spur, helical, bevel |
| Flanges | bolt circle, holes, thickness | Blind, through, raised |
| Standoffs | height, thread, base | Round, hex, square |
| Hinges | pin diameter, leaves, angle | Butt, piano, living |

Each generator produces:
- Randomized valid geometry
- Natural language description (template + variation)
- VCode representation

### Training Data Generation

```python
# Pseudocode for synthetic data generation
for family in part_families:
    for _ in range(samples_per_family):
        params = family.random_params()
        ir = family.generate_ir(params)
        prompt = family.generate_prompt(params)

        # Validate geometry
        if kernel.validate(ir):
            dataset.add(prompt, ir)
```

### Model Architecture

**Base model:** Qwen 2.5-Coder 7B
- Strong code generation capabilities
- Efficient tokenizer for structured output
- Open weights, permissive license

**Fine-tuning:** LoRA (Low-Rank Adaptation)
- Rank 64, alpha 128
- ~100M trainable parameters
- 4-bit quantization for inference

**Distillation to 1.5B:**
- Teacher-student training from 7B → 1.5B
- Quantization-aware training for browser
- Target: <1GB model size for WASM

### Inference Engine

Rust-native inference via Candle:

```rust
// crates/cad0/src/lib.rs
pub struct Cad0 {
    model: QwenModel,
    tokenizer: Tokenizer,
}

impl Cad0 {
    pub fn generate(&self, prompt: &str) -> Result<VCode> {
        let tokens = self.tokenizer.encode(prompt)?;
        let output = self.model.forward(&tokens)?;
        let ir_text = self.tokenizer.decode(&output)?;
        VCode::parse(&ir_text)
    }
}
```

Browser inference via WebGPU:
- WASM-compiled Candle
- WebGPU for matrix operations
- Progressive loading (show partial results)

## Implementation

### Files to Create

| Path | Purpose |
|------|---------|
| `crates/vcad-ir/src/vcode.rs` | VCode parser/serializer |
| `crates/cad0/` | Inference engine crate |
| `crates/cad0/src/lib.rs` | Model loading and generation |
| `crates/cad0/src/tokenizer.rs` | Tokenizer wrapper |
| `crates/cad0/src/model.rs` | Qwen model implementation |
| `packages/training/` | Training pipeline (Python) |
| `packages/training/generators/` | Part family generators |
| `packages/training/train.py` | Fine-tuning script |
| `packages/training/distill.py` | Distillation script |
| `packages/app/src/components/TextToCad.tsx` | UI component |
| `packages/engine/src/cad0.ts` | WASM binding for inference |

### Files to Modify

| File | Changes |
|------|---------|
| `crates/vcad-ir/src/lib.rs` | Add `VCode` type |
| `crates/vcad-kernel-wasm/src/lib.rs` | Expose VCode parsing |
| `packages/core/src/stores/ui-store.ts` | Add text-to-cad panel state |
| `packages/app/src/App.tsx` | Add TextToCad component |
| `packages/engine/src/evaluate.ts` | Support VCode evaluation |

### VCode Grammar

```ebnf
program     = { statement } ;
statement   = operation [ "@" identifier ] ;
operation   = primitive | transform | boolean | modifier ;

primitive   = "box" number number number
            | "cylinder" "r=" number "h=" number
            | "sphere" "r=" number
            | "cone" "r1=" number "r2=" number "h=" number ;

transform   = "translate" ref number number number
            | "rotate" ref axis number
            | "scale" ref number [ number number ]
            | "mirror" ref axis ;

boolean     = "union" ref ref
            | "difference" ref ref
            | "intersection" ref ref ;

modifier    = "fillet" ref "r=" number [ "edges=" edge_list ]
            | "chamfer" ref "d=" number
            | "shell" ref "t=" number
            | "pattern" ref pattern_type params ;

pattern_type = "linear" | "circular" ;
ref         = "@" identifier ;
axis        = "x" | "y" | "z" ;
number      = float | int ;
identifier  = letter { letter | digit | "_" } ;
```

### Validation Pipeline

Every generated model must pass:

1. **Parse validation** — VCode syntax correct
2. **Reference validation** — All @refs resolve
3. **Geometry validation** — Kernel produces valid solid
4. **Manifold check** — No self-intersection, watertight
5. **Dimension check** — Reasonable sizes (not 0, not huge)

## Tasks

### Phase 1: VCode Parser/Serializer (Week 1)

- [ ] Define VCode grammar specification (`m`)
- [ ] Implement lexer in `crates/vcad-ir/src/vcode/lexer.rs` (`s`)
- [ ] Implement parser in `crates/vcad-ir/src/vcode/parser.rs` (`m`)
- [ ] Implement serializer (IR → VCode text) (`s`)
- [ ] Add VCode ↔ JSON IR conversion (`s`)
- [ ] Write unit tests for all operations (`s`)
- [ ] WASM bindings for VCode parsing (`xs`)

### Phase 2: Synthetic Data Generation (Week 2)

- [ ] Set up `packages/training/` Python package (`xs`)
- [ ] Implement bracket family generator (`s`)
- [ ] Implement enclosure family generator (`s`)
- [ ] Implement gear family generator (`m`)
- [ ] Implement flange family generator (`s`)
- [ ] Implement standoff family generator (`xs`)
- [ ] Implement hinge family generator (`s`)
- [ ] Prompt template system with variations (`s`)
- [ ] Validation pipeline (run kernel, check manifold) (`m`)
- [ ] Generate 500K training pairs (`s`)
- [ ] Train/val/test split with stratification (`xs`)

### Phase 3: Training Pipeline (Week 3)

- [ ] Set up training infrastructure (GPU cluster or cloud) (`m`)
- [ ] Implement data loader for prompt/IR pairs (`s`)
- [ ] Configure Qwen 2.5-Coder 7B with LoRA (`m`)
- [ ] Implement training loop with validation (`m`)
- [ ] Add metrics: parse rate, valid geometry rate, dimension accuracy (`s`)
- [ ] Hyperparameter sweep (rank, alpha, lr) (`m`)
- [ ] Train cad0-7B model (`l`)
- [ ] Checkpoint selection based on validation metrics (`xs`)

### Phase 4: Distillation to cad0-mini (Week 4)

- [ ] Implement teacher-student training framework (`m`)
- [ ] Configure Qwen 2.5-Coder 1.5B as student (`s`)
- [ ] Knowledge distillation training (`l`)
- [ ] Quantization-aware fine-tuning (`m`)
- [ ] Export to ONNX format (`s`)
- [ ] Validate cad0-mini quality vs cad0 (`s`)

### Phase 5: Validation & Quality (Week 5)

- [ ] Create evaluation benchmark (100 diverse prompts) (`m`)
- [ ] Measure parse rate (target: >90%) (`s`)
- [ ] Measure valid geometry rate (target: >85%) (`s`)
- [ ] Measure dimension accuracy (target: within 10%) (`s`)
- [ ] Ablation study on training data size (`m`)
- [ ] Error analysis and failure modes (`m`)
- [ ] Iterate on training data based on failures (`m`)

### Phase 6: Rust Inference Engine (Week 6)

- [ ] Create `crates/cad0/` crate (`xs`)
- [ ] Integrate Candle for model loading (`m`)
- [ ] Implement tokenizer (use HuggingFace tokenizers) (`s`)
- [ ] Implement Qwen forward pass in Candle (`l`)
- [ ] Load LoRA weights (`s`)
- [ ] Implement streaming generation (`m`)
- [ ] 4-bit quantization for efficient inference (`m`)
- [ ] Benchmark inference speed (target: <5s for typical prompt) (`s`)
- [ ] WASM compilation for browser (`m`)
- [ ] WebGPU acceleration for browser inference (`l`)

### Phase 7: Integration & Release (Week 7)

- [ ] Create `TextToCad.tsx` component (`m`)
- [ ] Implement prompt input with suggestions (`s`)
- [ ] Streaming preview during generation (`m`)
- [ ] Error handling and retry UI (`s`)
- [ ] Offline indicator for cad0-mini (`xs`)
- [ ] Add to command palette (`xs`)
- [ ] Write user documentation (`s`)
- [ ] Create demo video (`s`)
- [ ] Release blog post (`m`)
- [ ] Publish model weights to HuggingFace (`s`)

## Acceptance Criteria

- [ ] VCode parser handles all primitive, transform, boolean, and modifier operations
- [ ] cad0 achieves >90% parse rate on evaluation benchmark
- [ ] cad0 achieves >85% valid geometry rate (kernel produces manifold solid)
- [ ] Dimensions within 10% of specified values in prompts
- [ ] Inference completes in <5 seconds for typical prompts (7B model, GPU)
- [ ] cad0-mini runs in browser without network (offline-capable)
- [ ] cad0-mini model size <1GB for reasonable download
- [ ] UI shows streaming preview during generation
- [ ] Generated models are fully parametric (editable in vcad)
- [ ] Graceful fallback from cad0 → cad0-mini on network failure

## Competitive Advantage

No existing tool offers this:

| Aspect | vcad cad0 | Competitors |
|--------|-----------|-------------|
| **Output format** | Parametric IR (editable) | Mesh only (dead geometry) |
| **Offline capable** | cad0-mini runs in browser | Cloud-only |
| **Open source** | Weights + code released | Proprietary |
| **Integration** | Native vcad feature | Separate tool/API |
| **Efficiency** | VCode, minimal tokens | Verbose output |

### Why This Matters

1. **Democratization** — Anyone can create CAD models by describing them
2. **Speed** — Rough concept in seconds, not hours
3. **Iteration** — "Make it taller" works naturally
4. **Accessibility** — No CAD training required
5. **Offline** — Works on planes, in factories, without internet

## Future Enhancements

- [ ] Multi-turn refinement ("make the holes larger")
- [ ] Image-to-CAD (sketch photo → parametric model)
- [ ] Assembly generation ("design a hinge mechanism")
- [ ] Manufacturing constraints ("make this 3D printable")
- [ ] Constraint inference ("add constraints so holes stay centered")
- [ ] Fine-tune on user's designs (personalization)
- [ ] CAD-to-text (explain existing model in natural language)
- [ ] Integration with MCP for Claude/GPT agents

## Research References

| Paper | ID | Relevance |
|-------|-----|-----------|
| CAD-MLLM | 2411.04954 | Multimodal CAD generation |
| CAD-Recode | 2412.14042 | Point cloud → CAD code |
| SketchAgent | 2411.17673 | Language-driven sketching |
| CurveGen | 2104.09621 | Sketch completion |
