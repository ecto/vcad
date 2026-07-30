# Fork Reality

**Score: 66/100** | **Priority: #24**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |
| Owner | `unassigned` |
| Priority | `p3` |
| Effort | `xl` |

> Verified against repo 2026-07-30. Added missing Status table; no image-to-model vision pipeline exists in the codebase (URDF import exists, but nothing image-driven).

## Overview

See a cool robot on Twitter? Paste the image. vcad generates a parametric model. Go from inspiration to simulation in 30 seconds.

## Why It Matters

Inspiration is everywhere—Twitter, papers, videos, the real world. But the traditional path from inspiration to creation has a massive drop-off:

**Before:** See thing → wish you could build it → realize you'd need hours of CAD work → give up

**With Fork Reality:** See thing → paste image → have editable model → iterate → simulate

This lowers the barrier from "expert who can model from scratch" to "anyone with an image." It's the difference between "I could never make that" and "let me tweak this."

## Technical Implementation

### Vision Analysis Pipeline

The vision model analyzes the input image for:

1. **Robot topology**
   - Number of links
   - Joint connectivity graph
   - Base/end-effector identification

2. **Approximate dimensions**
   - Extracted from context clues (human hands, common objects, scale references)
   - Proportional relationships between links
   - Standard component recognition (motors, bearings, brackets)

3. **Joint types**
   - Revolute (rotational)
   - Prismatic (linear)
   - Spherical
   - Fixed connections

4. **End effector classification**
   - Parallel gripper
   - Vacuum/suction
   - Tool holder
   - Custom geometry

### Output Generation

- Generate parametric vcad document with full feature tree
- Mark uncertain dimensions as "estimated" for user refinement
- Preserve topology as constraints (joints can't be accidentally broken)
- Support multiple input views for improved accuracy

### Multi-View Enhancement

When multiple images are provided:
- Cross-reference dimensions between views
- Resolve ambiguities in joint axes
- Improve depth estimation
- Validate topology consistency

## User Flow

```
1. User pastes image (or drags file, or pastes URL)

2. AI: "I see a 6-DOF arm with parallel gripper. Analyzing..."
   [Progress indicator shows vision pipeline stages]

3. AI: "Estimated dimensions:
   - Base: 100mm (high confidence)
   - Link 1: 250mm (medium confidence)
   - Link 2: 200mm (medium confidence)
   - Gripper span: 80mm (low confidence - partially occluded)"

4. [Viewport shows generated model in same pose as image]
   [Side-by-side comparison mode available]

5. AI: "This is approximate. Refine dimensions or add another view?"

6. User adjusts sliders, model updates in real-time
   [Changes propagate through parametric relationships]

7. User: "Add physics"
   → Robot simulates with estimated mass properties
   → Ready for control experiments
```

## Input Sources

| Source | How | Notes |
|--------|-----|-------|
| Image paste | Ctrl+V / Cmd+V | Most common path |
| File drop | Drag & drop | Supports PNG, JPG, WebP |
| URL | Paste image link | Auto-fetches and processes |
| Screenshot | System tool | Direct from screen capture |
| Video frame | Paste video, select frame | Frame picker UI |
| Paper figure | Technical drawings | Benefits from cleaner lines |
| Multiple images | Batch paste/drop | Improves accuracy |

## Accuracy Expectations

| Aspect | Expected Accuracy | Notes |
|--------|-------------------|-------|
| Topology | >90% correct | Link count, joint types, connectivity |
| Dimensions | ±30% | Requires user refinement for precision |
| Joint limits | Estimated from typical ranges | Based on joint type heuristics |
| Mass properties | ±50% | Estimated from geometry + material guess |

Each estimate displays explicit **confidence indicators**:
- **High**: Clear visual evidence, multiple cues agree
- **Medium**: Single source of evidence, plausible
- **Low**: Inferred or partially occluded, needs verification

## Success Metrics

| Metric | Target |
|--------|--------|
| Image → editable model | <60 seconds |
| Topology correctness | >90% |
| User successfully refines to accurate model | >80% |
| User proceeds to simulation | >50% |

## Viral Potential

The shareable moment: *"I recreated this robot from a tweet in 30 seconds"*

- Before/after comparison is visually compelling
- Low barrier to try (just paste an image)
- Results are immediately useful (not just a static render)
- Natural progression to "watch me simulate it" follow-up content

## Competitive Advantage

**No CAD tool can create models from images.**

Traditional workflow requires:
1. Manual measurement or estimation
2. Sketch creation
3. Feature-by-feature modeling
4. Assembly of components
5. Joint definition

Fork Reality compresses this to a single paste operation, with refinement as optional follow-up rather than required prerequisite.

## Future Enhancements

- **Real-time video analysis**: Point phone camera at robot, get live model
- **URDF/SDF import fusion**: Combine image analysis with existing robot descriptions
- **Part library matching**: Identify standard components (Dynamixel servos, OpenBuilds rails)
- **Inverse kinematics from motion**: Analyze video to extract joint limits and velocities
