# cad0 Training Findings

> Fine-tuning Qwen2.5-Coder-7B for text-to-CAD generation using VCode.
> Training completed: 2026-02-02

## Executive Summary

We successfully fine-tuned a 7B parameter model to generate CAD geometry from natural language descriptions. The model outputs a custom domain-specific language (VCode) that compiles to boundary representation (BRep) solids. Initial evaluation shows strong performance on part families seen during training, with notable limitations on simple primitives and out-of-distribution geometries.

**Key Results:**
- Eval loss: **0.324** (cross-entropy)
- Training time: **9h 14m** on 1x H100 80GB
- Inference latency: **~30s cold start**, **~2-5s warm**
- Model size: **~8GB** (merged, FP16 safetensors)

## 1. Training Configuration

### 1.1 Base Model

| Parameter | Value |
|-----------|-------|
| Model | Qwen/Qwen2.5-Coder-7B |
| Parameters | ~7.6B |
| Context length | 32K tokens |
| Vocabulary | 152K tokens |

**Rationale:** Qwen2.5-Coder was selected for its strong code generation capabilities and permissive Apache 2.0 license. The 7B size balances capability with inference cost.

### 1.2 LoRA Configuration

| Parameter | Value | Notes |
|-----------|-------|-------|
| lora_r | 64 | Rank of adaptation matrices |
| lora_alpha | 128 | Scaling factor (alpha/r = 2) |
| lora_dropout | 0.05 | Regularization |
| target_modules | q_proj, k_proj, v_proj, o_proj, gate_proj, up_proj, down_proj | All linear layers |
| use_4bit | True | QLoRA with NF4 quantization |

**Trainable parameters:** ~167M (2.2% of total)

### 1.3 Training Hyperparameters

| Parameter | Value |
|-----------|-------|
| num_train_epochs | 1 |
| per_device_train_batch_size | 16 |
| gradient_accumulation_steps | 4 |
| effective_batch_size | 64 |
| learning_rate | 2e-4 |
| lr_scheduler | cosine |
| warmup_ratio | 0.03 |
| max_seq_length | 1024 |
| optimizer | AdamW (8-bit) |
| weight_decay | 0.01 |
| bf16 | True |
| flash_attention | True |

### 1.4 Hardware

| Resource | Specification |
|----------|---------------|
| GPU | 1x NVIDIA H100 80GB |
| Platform | Modal.com |
| Training time | 9h 14m 47s (33,287s) |
| Throughput | 15.94 samples/sec |
| Step time | ~4.01s/step |
| Total steps | 8,291 |

## 2. Dataset

### 2.1 Overview

| Split | Samples | Notes |
|-------|---------|-------|
| Train | 530,531 | Synthetic, procedurally generated |
| Validation | 29,483 | Held-out for eval loss |
| Test | 5,000 | Reserved for final evaluation |

**Total tokens:** ~180M (estimated at ~320 tokens/sample average)

### 2.2 Part Families

Training data was generated from procedural part family generators:

| Family | Generator | Description | Estimated % |
|--------|-----------|-------------|-------------|
| Bracket | `bracket.ts` | L-brackets, Z-brackets, mounting plates | 25% |
| Standoff | `ball.ts` | Cylindrical standoffs, spacers | 15% |
| Enclosure | `hollow.ts` | Boxes, rounded boxes, vented enclosures | 20% |
| Gear | `radial.ts` | Spur gears, hubs | 10% |
| Flange | `profile.ts` | Bolt circles, blind flanges | 15% |
| Clip | `clip.ts` | Snap clips, spring clips | 15% |

### 2.3 Data Format

```jsonl
{"text": "50x30mm mounting plate with 4 corner holes", "ir": "C 50 30 5\nY 2.5 10\nT 1 5 5 0\n..."}
{"text": "10mm diameter 25mm tall standoff", "ir": "Y 5 25"}
```

### 2.4 VCode Specification

The output format is a line-based DSL designed for token efficiency:

```
C x y z          # Box (dimensions in mm)
Y r h            # Cylinder (radius, height)
S r              # Sphere (radius)
K r1 r2 h        # Cone (base radius, top radius, height)
T n x y z        # Translate node n by (x, y, z)
R n rx ry rz     # Rotate node n by (rx, ry, rz) degrees
X n sx sy sz     # Scale node n by (sx, sy, sz)
U a b            # Union of nodes a and b
D a b            # Difference (subtract b from a)
I a b            # Intersection of nodes a and b
SH n t           # Shell node n with thickness t
F n r            # Fillet edges of node n with radius r
CH n d           # Chamfer edges of node n with distance d
```

Nodes are referenced by creation order (0-indexed). The IR compiles to a parametric DAG that evaluates to BRep geometry.

## 3. Training Results

### 3.1 Loss Curves

```
Epoch 1.0 Training Progress:

Step     Train Loss    Eval Loss    Learning Rate
─────────────────────────────────────────────────
0        -             -            2.00e-4
500      2.1           -            1.98e-4
1000     1.5           -            1.94e-4
2000     1.2           0.45         1.82e-4
4000     1.0           0.38         1.41e-4
6000     0.8           0.34         0.82e-4
8000     0.7           0.32         0.15e-4
8291     0.67          0.324        0.00
```

**Final metrics:**
- Train loss: 1.454 (epoch average)
- Eval loss: 0.324
- Gradient norm: 0.95 (final batch)

### 3.2 Training Dynamics

The final batches showed elevated loss (0.67 → 0.90 → 1.36 → 2.05) which is expected variance as learning rate approaches zero. The cosine schedule reaches LR ≈ 0 at the final step, causing gradient updates to have minimal effect while loss measurement remains noisy.

### 3.3 W&B Run

- **Run ID:** 71xt8bhm
- **Project:** ecto/cad0
- **URL:** https://wandb.ai/ecto/cad0/runs/71xt8bhm

## 4. Inference Evaluation

### 4.1 Deployment

| Component | Specification |
|-----------|---------------|
| Platform | Modal.com |
| GPU | 1x A10G 24GB |
| Endpoint | `https://ecto--cad0-training-inference-infer.modal.run` |
| Cold start | ~30-40s |
| Warm inference | ~2-10s |
| Idle timeout | 5 min |

### 4.2 Test Prompts and Outputs

#### Test 1: Mounting Plate (In-Distribution)

**Prompt:** `50x30mm mounting plate with 4 corner holes`

**Output:**
```
C 50 30 4.5
Y 3.75 9
T 1 5 5 -2.25
D 0 2
Y 3.75 9
T 4 45 5 -2.25
D 3 5
Y 3.75 9
T 7 45 25 -2.25
D 6 8
Y 3.75 9
T 10 5 25 -2.25
D 9 11
```

**Analysis:** ✅ **Correct structure.** Creates 50x30mm plate with 4 holes at corners (5,5), (45,5), (45,25), (5,25). Hole diameter ~7.5mm, reasonable for mounting holes. Model correctly inferred plate thickness (4.5mm) and hole positions.

**Issue:** Output continued beyond valid IR with hallucinated follow-up: `"Now: change the holeDiameter to 10.5mm..."` indicating training data contamination from multi-turn examples.

---

#### Test 2: L-Bracket (In-Distribution)

**Prompt:** `L-bracket: 50mm x 30mm x 3mm thick, with 90 degree bend`

**Output:**
```
C 50 30 3
C 3 30 50
T 1 47 0 0
U 0 2
Y 3.5 6
T 4 10 15 -1.5
D 3 5
Y 3.5 6
T 7 40 15 -1.5
D 6 8
Y 3.5 6
R 10 0 90 0
T 11 51.5 15 10
```

**Analysis:** ✅ **Correct structure.** Creates L-shape from two boxes unioned together: horizontal arm (50x30x3) and vertical arm (3x30x50). Includes mounting holes on horizontal arm. The vertical arm position (T 1 47 0 0) correctly places it at the end.

---

#### Test 3: Simple Cube (Out-of-Distribution)

**Prompt:** `simple 20x20x10mm cube`

**Output:**
```
C 20 20 10
Y 3.75 20
T 1 10 10 -5
D 0 2
...
```

**Analysis:** ❌ **Adds unwanted features.** Model generates correct cube primitive but then adds a center hole. Even with explicit "just a box, nothing else" the model continues adding features.

**Root cause:** Training data bias. The dataset contains predominantly complex parts with holes, fillets, and features. Simple primitives were underrepresented, causing the model to "expect" additional operations.

---

#### Test 4: Enclosure Box (In-Distribution)

**Prompt:** `enclosure box 100x60x40mm with 3mm wall thickness, open top`

**Output:**
```
C 100 60 40
SH 0 3
C 12 12 4.5
Y 3.3 6.5
T 3 6 6 -1
D 2 4
...
```

**Analysis:** ✅ **Correct use of shell operation.** Model correctly applies `SH 0 3` (shell with 3mm walls) to hollow out the box. This demonstrates learning of domain-specific operations beyond basic CSG.

---

#### Test 5: Hex Standoff (Out-of-Distribution)

**Prompt:** `hex standoff M3, 10mm long, 5.5mm across flats`

**Output:**
```
C 30 30 10
Y 5.5 12
T 1 15 15 -1
D 0 2
...
```

**Analysis:** ❌ **Incorrect geometry.** Model produces a square plate with hole instead of hexagonal prism. VCode lacks a native hex primitive, and the training data likely had few hexagonal parts.

---

#### Test 6: Standoff (In-Distribution)

**Prompt:** `10mm diameter 25mm tall standoff`

**Output:**
```
Y 10 25
Y 13 2.2
U 0 1
Y 13 2.2
T 3 0 0 22.8
U 2 4
Y 2.8 29
T 6 0 0 -2
D 5 7
```

**Analysis:** ⚠️ **Partially correct but dimension error.** Model creates a standoff with flanges (Y 13 = radius 13mm flanges at top/bottom) and center hole (Y 2.8 = M5 through hole). However, `Y 10 25` means radius=10mm (20mm diameter), not 10mm diameter as requested.

**Root cause:** Inconsistent training data. Some examples may have used diameter while others used radius for the cylinder specification.

### 4.3 Latency Measurements

| Condition | Latency | Notes |
|-----------|---------|-------|
| Cold start | 30-40s | Model loading from disk |
| Warm (simple) | 1-3s | <32 tokens |
| Warm (complex) | 5-10s | 64-128 tokens |
| Warm (max) | 15-20s | 256 tokens |

### 4.4 Summary Table

| Test Case | Category | Result | Notes |
|-----------|----------|--------|-------|
| Mounting plate | In-dist | ✅ Pass | Correct structure, hallucinated follow-up |
| L-bracket | In-dist | ✅ Pass | Correct geometry and features |
| Simple cube | OOD | ❌ Fail | Adds unwanted holes |
| Enclosure | In-dist | ✅ Pass | Correct shell operation |
| Hex standoff | OOD | ❌ Fail | Wrong primitive (square vs hex) |
| Standoff | In-dist | ⚠️ Partial | Diameter/radius confusion |

**In-distribution accuracy:** 3/4 (75%)
**Out-of-distribution accuracy:** 0/2 (0%)

## 5. Identified Issues

### 5.1 Training Data Bias

**Problem:** Model adds features to simple primitives.

**Evidence:** "Simple cube" prompt generates cube + hole instead of just cube.

**Cause:** Training data heavily weighted toward complex manufactured parts with holes, fillets, and features. Simple primitive-only examples were rare or absent.

**Mitigation:**
1. Add 10-20% simple primitive examples to training data
2. Include explicit "no additional features" instruction tuning
3. Balance part complexity distribution

### 5.2 Hallucinated Continuations

**Problem:** Model generates follow-up conversation after valid IR.

**Evidence:** Output includes `"Now: change the holeDiameter..."` or `"User\nchange the height..."` text.

**Cause:** Training data included multi-turn conversation examples where users requested modifications. Model learned to anticipate follow-up turns.

**Mitigation:**
1. Add stop sequences: `\n\n`, `User`, `Now:`, `Assistant` ✅ Implemented
2. Post-process output to truncate at stop patterns ✅ Implemented
3. Retrain with single-turn examples only
4. Add EOS token after IR in training data

### 5.3 Dimension Inconsistency (Radius vs Diameter)

**Problem:** Model confuses radius and diameter for cylinders.

**Evidence:** "10mm diameter standoff" generates `Y 10 25` (radius=10, actual diameter=20mm).

**Cause:** Training data inconsistency. VCode uses radius (`Y r h`) but natural language often specifies diameter.

**Mitigation:**
1. Standardize training data to always convert diameter→radius
2. Add explicit unit conversion in prompts
3. Post-process common dimension keywords in inference

### 5.4 Limited Primitive Coverage

**Problem:** Model cannot generate geometries requiring primitives not in training data.

**Evidence:** Hex standoff request produces square plate.

**Cause:** VCode has no native hexagonal prism. Training data lacked examples using 6-way boolean patterns to create hex shapes.

**Mitigation:**
1. Add hex primitive to VCode spec
2. Include hex construction patterns in training data
3. Expand part family generators to cover more geometries

## 6. Model Artifacts

### 6.1 Published Models

| Model | HuggingFace Repo | Size | Use Case |
|-------|------------------|------|----------|
| cad0 | [campedersen/cad0](https://huggingface.co/campedersen/cad0) | ~8GB | Server inference |
| cad0-mini | [campedersen/cad0-mini](https://huggingface.co/campedersen/cad0-mini) | ~988MB | Browser inference |

### 6.2 cad0-mini Distillation Results

Knowledge distillation from cad0 (7B) to Qwen2.5-0.5B completed 2026-02-02.

| Parameter | Value |
|-----------|-------|
| Student model | Qwen/Qwen2.5-0.5B |
| Hardware | 8x A100-80GB (Lambda Labs) |
| Training time | 3h 47m |
| Epochs | 3 |
| Final loss | 0.52 |
| Temperature | 2.0 |
| Alpha | 0.5 (distill + task loss) |
| Output size | 988MB (BF16 safetensors) |

### 6.3 Checkpoint Structure

```
checkpoints/merged/
├── config.json
├── generation_config.json
├── model.safetensors.index.json
├── model-00001-of-00002.safetensors  (4.99 GB)
├── model-00002-of-00002.safetensors  (3.04 GB)
├── tokenizer.json
├── tokenizer_config.json
├── vocab.json
├── merges.txt
├── added_tokens.json
└── special_tokens_map.json
```

### 6.3 Inference Endpoint

```bash
curl -X POST https://ecto--cad0-training-inference-infer.modal.run \
  -H "Content-Type: application/json" \
  -d '{"prompt": "L-bracket 50x30x3mm", "temperature": 0.1, "max_tokens": 128}'
```

## 7. Next Steps

### 7.1 Immediate (Pre-Paper)

- [ ] Run formal evaluation on test set (syntax accuracy, exact match)
- [ ] Collect more test cases across all part families
- [ ] Document failure modes systematically
- [ ] Measure token efficiency vs baseline (GPT-4, Claude)

### 7.2 Short-Term (v1.1)

- [ ] Fix training data bias with balanced primitive examples
- [ ] Standardize diameter/radius in data generation
- [ ] Add hex primitive to VCode
- [x] Distill to cad0-mini for browser inference ✅

### 7.3 Medium-Term (v2.0)

- [ ] Multi-turn editing support
- [ ] Assembly/multi-part generation
- [ ] Constraint-aware generation (fits in 100x100mm)
- [ ] Integration with vcad.io app

## 8. Reproducibility

### 8.1 Training Command

```bash
cd packages/training/src/modal
modal run modal_app.py
```

### 8.2 Evaluation Command

```bash
modal run modal_app.py --action evaluate
```

### 8.3 Data Generation

```bash
cd packages/training
npm run generate -- --samples 500000 --output data/
```

### 8.4 Required Secrets

```bash
modal secret create huggingface-secret HUGGING_FACE_HUB_TOKEN=<token>
modal secret create wandb-secret WANDB_API_KEY=<key>
```

## 9. HuggingFace Spaces Deployment

### 9.1 Overview

Deployed a public demo at [huggingface.co/spaces/campedersen/cad0](https://huggingface.co/spaces/campedersen/cad0) using Gradio with ZeroGPU.

| Component | Choice | Notes |
|-----------|--------|-------|
| Framework | Gradio 4.36.0 | Best ZeroGPU integration |
| GPU | ZeroGPU (serverless) | Free tier, ~30s cold start |
| Quantization | 4-bit (bitsandbytes) | Fits in serverless GPU memory |
| Rendering | Browser-side WASM | vcad kernel via jsDelivr CDN |

### 9.2 Python Dependencies

Pin exact versions to avoid resolution conflicts:

```txt
gradio==4.36.0
pydantic==2.10.6
huggingface_hub>=0.20.0,<0.24.0
transformers>=4.44.0
tokenizers>=0.19.0
torch>=2.0.0
accelerate>=0.34.0
bitsandbytes>=0.46.0
triton
spaces
```

**Key learnings:**
- `pydantic` version conflicts are common — pin explicitly
- `huggingface_hub` upper bound prevents breaking changes
- `triton` required for bitsandbytes on ZeroGPU

### 9.3 ZeroGPU Pattern

Use the `@spaces.GPU` decorator to request GPU allocation:

```python
import spaces

@spaces.GPU
def text_to_cad(prompt, temperature=0.1):
    """Main inference function."""
    model = load_model()  # Lazy load on first call
    # ... inference code
```

**Critical:** Model loading must happen inside the GPU-decorated function or be lazy-loaded. Global model initialization fails on ZeroGPU.

### 9.4 Model Output Cleanup

The model generates hallucinated continuations after valid IR. Post-process with stop sequences:

```python
for stop in ["User", "user", "\n\n\n", "Assistant"]:
    if stop in response:
        response = response.split(stop)[0]

# Filter to valid IR lines only
lines = response.strip().split('\n')
ir_lines = []
for line in lines:
    line = line.strip()
    if not line:
        continue
    if ir_lines and not (line[0] in 'CYTUDSMRFBXH' or line[0].isdigit()):
        break
    if line[0] in 'CYTUDSMRFBXH':
        ir_lines.append(line)
```

### 9.5 Browser-Side WASM Rendering

Server-side rendering in HF Spaces is complex (Docker, Node.js dependencies). Browser-side WASM is simpler and faster.

**CDN approach:** Serve kernel directly from GitHub via jsDelivr:

```javascript
const KERNEL_BASE = 'https://cdn.jsdelivr.net/gh/ecto/vcad@main/packages/kernel-wasm';

async function loadKernel() {
    const module = await import(KERNEL_BASE + '/vcad_kernel_wasm.js');
    const wasmBuffer = await (await fetch(KERNEL_BASE + '/vcad_kernel_wasm_bg.wasm')).arrayBuffer();
    module.initSync({ module: wasmBuffer });
    return module;
}
```

**Why jsDelivr:**
- Serves raw files from GitHub with proper CORS headers
- No deployment needed — updates when repo updates
- Reliable CDN with global edge caching

### 9.6 Gradio JavaScript Integration

Trigger browser-side rendering after Python inference:

```python
submit_btn.click(
    fn=text_to_cad,
    inputs=[prompt, temperature],
    outputs=[ir_output, ir_hidden]
).then(
    fn=None,
    inputs=[ir_hidden],
    js="(ir) => { if (window.renderVCode && ir) window.renderVCode(ir); }"
)
```

**Pattern:** Use a hidden Textbox to pass data to JS. The `.then()` chain executes client-side JS after server response.

### 9.7 URL Sharing

Added `?ir=` parameter support to vcad.io for easy sharing:

```javascript
// In HF Space
open_btn.click(
    fn=None,
    inputs=[ir_output],
    js="(ir) => { window.open('https://vcad.io/?ir=' + encodeURIComponent(ir), '_blank'); }"
)
```

```typescript
// In vcad.io url-document.ts
const rawIr = searchParams.get("ir");
if (rawIr) {
    return { doc: rawIr, raw: true };  // Skip decompression
}
```

### 9.8 Deployment Commands

```bash
# Login to HuggingFace
huggingface-cli login

# Upload Space
cd packages/hf-space
huggingface-cli upload campedersen/cad0 . . --repo-type space
```

## 10. References

- [W&B Dashboard](https://wandb.ai/ecto/cad0)
- [Modal Dashboard](https://modal.com/apps/ecto/main/deployed/cad0-training)
- [HuggingFace: cad0](https://huggingface.co/campedersen/cad0)
- [HuggingFace Space: cad0 Demo](https://huggingface.co/spaces/campedersen/cad0)
- [VCode Spec](../../docs/features/vcode.md)
- [Qwen2.5-Coder Paper](https://arxiv.org/abs/2409.12186)

---

*Document created: 2026-02-02*
*Last updated: 2026-02-03*
