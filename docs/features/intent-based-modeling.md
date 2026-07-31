# Intent-Based Modeling

**Score: 84/100** | **Priority: #6**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |

> Verified against repo 2026-07-30. No intent-inference pipeline, LLM intent parser, or confirmation UI exists in the app. (AI authoring does exist, but through the MCP server's explicit tool calls — a different mechanism than the in-app natural-language intent UX described here.)

## Overview

Users express intent, not instructions. Instead of navigating menus and specifying parameters, users describe what they want: "I need a hinge here" or "This should swing open like a door." The system infers the appropriate joint type, axis, limits, and mounting points automatically.

This inverts the traditional CAD paradigm. Rather than learning the tool's language, users speak their own language and the tool adapts.

## Why It Matters

**Traditional CAD requires learning a complex command language.** Users must know:
- That a door hinge is a "revolute joint"
- How to select the correct axis
- What angular limits are appropriate
- How to position the joint origin

This creates a high barrier to entry that excludes hobbyists, students, and domain experts who aren't CAD specialists. Even experienced users waste time on mechanical details that could be inferred from context.

**AI can bridge the gap between intent and execution.** Large language models understand natural descriptions. Geometric analysis can infer appropriate parameters. Together, they transform vague intent into precise operations.

## Technical Implementation

### Natural Language Understanding

An LLM interprets user input to extract:
- **Operation type**: joint, constraint, feature, modification
- **Target geometry**: referenced faces, edges, bodies
- **Behavioral intent**: motion type, limits, purpose

The model is fine-tuned on CAD-specific language patterns and mechanical engineering terminology.

### Geometric Analysis

Once intent is parsed, geometric analysis infers parameters:

| Analysis | Inference |
|----------|-----------|
| Face normals | Joint axis direction |
| Face adjacency | Mounting points |
| Body bounding boxes | Collision-based limits |
| Symmetry detection | Constraint relationships |
| Edge curvature | Fillet/chamfer radius |
| Part relationships | Assembly mates |

### Inference Pipeline

```
User Input + Selection → LLM Intent Parser → Geometric Analyzer → Parameter Inference → Confirmation UI
```

1. **Parse**: Extract operation type and targets from natural language
2. **Analyze**: Examine selected geometry for constraints
3. **Infer**: Propose specific parameters based on analysis
4. **Confirm**: Present preview with option to refine

### Confirmation UI

Before executing, the system shows:
- Visual preview of the proposed operation
- Inferred parameters with editable fields
- Natural language summary: "I'll create a revolute joint with axis Z, limits 0-110°. OK?"

One click confirms. Users can also:
- Adjust individual parameters
- Request alternative interpretations
- Cancel and try again

### Fallback Behavior

When intent is ambiguous:
1. Ask clarifying questions: "Do you want this to rotate or slide?"
2. Offer multiple interpretations ranked by likelihood
3. Fall back to explicit mode with pre-filled parameters

## Examples

| User Says | Selection | System Infers |
|-----------|-----------|---------------|
| "Hinge here" | Two adjacent faces | Revolute joint, axis perpendicular to face intersection |
| "This slides" | A body | Slider joint along longest axis |
| "Connect these" | Two parts | Fixed joint at closest faces |
| "Make it spin" | Cylindrical face | Revolute joint, no angular limits |
| "Round this edge" | Sharp edge | Fillet with radius proportional to adjacent faces |
| "This needs to open like a door" | Panel body | Revolute joint at edge, 0-110° limits |
| "Put a hole here" | Flat face | Through-hole, diameter inferred from context |
| "Make these the same" | Two edges | Equal length constraint |

## User Experience

### The Happy Path

1. User clicks geometry and types intent in natural language
2. System shows preview with highlighted affected geometry
3. User confirms with Enter or clicks checkmark
4. Operation executes with undo available

### Progressive Disclosure

- **Beginners**: Use natural language exclusively
- **Intermediate**: See inferred parameters, learn vocabulary
- **Advanced**: Use explicit commands when precision required

Users naturally learn explicit commands by seeing what the system infers. Intent-based modeling becomes a teaching tool.

### Voice Support (Future)

Natural language input enables voice control. Users can describe operations while keeping hands on the model, useful for:
- VR/AR CAD environments
- Accessibility
- Rapid prototyping sessions

## Success Metrics

| Metric | Target |
|--------|--------|
| Simple operations completable via intent | >80% |
| Inference accuracy for common patterns | >90% |
| Time-to-operation vs explicit commands | 50% reduction |
| User preference (after learning both modes) | >70% prefer intent |
| Fallback rate (ambiguous intent) | <15% |

### Measurement Approach

- A/B testing: intent mode vs explicit mode for identical tasks
- User studies: task completion time, error rate, satisfaction
- Telemetry: inference acceptance rate, parameter adjustment frequency

## Competitive Advantage

**No CAD tool understands intent.** Every existing system—Fusion 360, Onshape, SolidWorks, FreeCAD—requires users to:

1. Know the exact command name
2. Select geometry in the correct order
3. Specify all parameters explicitly

This is the fundamental barrier that keeps CAD inaccessible. Intent-based modeling eliminates it.

### Defensibility

- Training data: vcad-specific patterns improve over time
- Geometric inference: domain-specific algorithms for CAD
- Feedback loop: user corrections improve the model

## Implementation Phases

### Phase 1: Joint Intent
- Revolute, slider, fixed joint inference
- Face selection → axis inference
- Basic limit inference from geometry

### Phase 2: Feature Intent
- Fillet/chamfer from edge selection
- Hole placement from face clicks
- Pattern inference from repeated selections

### Phase 3: Assembly Intent
- "Connect these parts" → appropriate mate
- "Make this mechanism" → joint chain
- Kinematic intent from motion descriptions

### Phase 4: Sketch Intent
- "Draw a rectangle here" → constrained sketch
- "Make these parallel" → constraint inference
- Dimension inference from proportions

## Risks and Mitigations

| Risk | Mitigation |
|------|------------|
| LLM latency | Local model for common patterns, cloud for complex |
| Inference errors | Always preview, never auto-execute |
| User frustration with wrong guesses | Quick refinement UI, learn from corrections |
| Over-reliance on AI | Expose explicit commands, encourage learning |

## Related Features

- **Conversational CAD**: Extended dialogue for complex operations
- **Smart Constraints**: Automatic constraint inference in sketches
- **Assembly Assistant**: Guided assembly from part descriptions
