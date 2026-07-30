# Generative Design (Topology Optimization)

> **Superseded by a shipped implementation that diverged from this design.** Topology optimization shipped as `crates/vcad-kernel-topopt` (SIMP with voxel FEA + surface-nets extraction), exposed via the `topology_optimize` MCP tool — the result lands in the document as a frozen mesh part. The WebGPU/cloud-FEA architecture, MMA optimizer, manufacturing constraints, NURBS BRep reconstruction, and the `create_topology_optimization`/`run_topology_optimization`/`get_topology_result` MCP tools described below were never built.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` (as `vcad-kernel-topopt`; this design largely superseded) |
| Owner | `n/a` |
| Priority | `p2` |
| Effort | `n/a` |

> Verified against repo 2026-07-30. Added missing Status table and banner pointing at the real implementation.

AI-driven topology optimization for vcad. Define a design space and loading conditions, then optimize material distribution to minimize weight while meeting structural requirements.

## Overview

Topology optimization answers the question: "Given a design envelope and loads, where should material be placed?" Unlike traditional CAD where designers manually create geometry, generative design explores the full design space algorithmically.

**Key benefits:**
- Weight reduction of 40-70% compared to traditional designs
- Organic, load-optimized geometries
- Manufacturing-aware results (additive, casting, machining)
- Integration with vcad's parametric workflow

## Algorithm: SIMP (Solid Isotropic Material with Penalization)

SIMP is the industry-standard method for density-based topology optimization. Each element in the design space has a pseudo-density value that determines material presence.

### Mathematical Formulation

**Density field:** Each element has density ρ ∈ [0, 1]
- ρ = 0: void (no material)
- ρ = 1: solid (full material)
- Intermediate values are penalized to drive toward 0/1

**Interpolation scheme (modified SIMP):**
```
E(ρ) = E_min + ρ^p × (E_0 - E_min)
```

Where:
- `E(ρ)` = effective Young's modulus at density ρ
- `E_0` = base material Young's modulus
- `E_min` = small value to prevent singularity (typically 10⁻⁹ × E_0)
- `p` = penalization exponent (typically 3)

The penalty exponent p > 1 makes intermediate densities inefficient, driving the solution toward a clear 0/1 distribution.

### Objective Function (Compliance Minimization)

The standard formulation minimizes structural compliance (maximizes stiffness):

```
minimize:  C = U^T × K × U       (total strain energy)

subject to:
    K(ρ) × U = F                 (equilibrium equation)
    Σ ρ_e × v_e ≤ V_target       (volume constraint)
    0 < ρ_min ≤ ρ_e ≤ 1          (density bounds)
```

Where:
- `U` = global displacement vector
- `K` = global stiffness matrix (function of densities)
- `F` = applied force vector
- `v_e` = element volume
- `V_target` = target volume fraction × total volume

### Sensitivity Analysis

Gradients computed via adjoint method:

```
∂C/∂ρ_e = -p × ρ_e^(p-1) × u_e^T × K_0 × u_e
```

Where `u_e` is the element displacement vector and `K_0` is the element stiffness matrix at unit density.

### Optimizer: MMA (Method of Moving Asymptotes)

MMA is the de facto standard optimizer for topology optimization:
- Handles large numbers of design variables (millions)
- Efficiently incorporates multiple constraints
- Self-adaptive move limits via asymptote updates
- Proven convergence for SIMP problems

Alternative for MVP: Optimality Criteria (OC) method - simpler but less robust for multiple constraints.

## Filtering

Filtering prevents numerical artifacts and provides mesh-independence.

### Density Filter (Prevents Checkerboarding)

Weighted average of neighboring densities:
```
ρ̃_e = Σ_j (w_ej × ρ_j) / Σ_j w_ej

w_ej = max(0, r - dist(e,j))
```

Where `r` is the filter radius (typically 1.5-2× element size).

### Sensitivity Filter

Chain rule modification for filtered sensitivities:
```
∂C/∂ρ_e = (1/ρ_e) × Σ_j (w_ej × ρ_j × ∂C/∂ρ̃_j) / Σ_j w_ej
```

### Heaviside Projection (Optional)

Further sharpens 0/1 boundaries:
```
ρ̄ = (tanh(β×η) + tanh(β×(ρ̃-η))) / (tanh(β×η) + tanh(β×(1-η)))
```

Where β controls sharpness (increased during optimization) and η is the threshold.

## FEA Integration

### Browser-Based FEA (Small Problems)

For problems under ~64³ voxels (~260K elements):
- WASM-compiled FEM solver
- WebGPU-accelerated matrix operations
- Iterative solver (PCG with Jacobi preconditioner)
- Hex8 elements with reduced integration

### Cloud Backend (Large Problems)

For larger problems, delegate to cloud FEA:
- **FEniCS**: Python-based, flexible, good for prototyping
- **Code_Aster**: Production-grade, comprehensive capabilities
- **PETSc**: Scalable parallel solvers

Communication via vcad's existing cloud infrastructure.

## Design Space Definition

### Workflow

1. **Design envelope**: Select existing solid as maximum design space
2. **Preserved regions**: Surfaces/volumes that must remain solid
   - Mounting faces
   - Bolt holes
   - Bearing surfaces
3. **Exclusion zones**: Volumes where material cannot exist
   - Clearance for moving parts
   - Cable routing
4. **Voxelization**: Convert to regular grid
   - Default resolution: 64³
   - User-configurable up to 256³ in browser

### Voxelization Algorithm

1. Compute AABB of design envelope
2. Create regular hex grid at specified resolution
3. Point-in-solid test for each voxel center
4. Mark preserved/excluded regions
5. Generate element connectivity

## Load and Boundary Conditions

### Boundary Conditions

| Type | Application | Description |
|------|-------------|-------------|
| Fixed | Face/Edge/Vertex | Zero displacement (all DOFs) |
| Pinned | Vertex | Zero translation, free rotation |
| Roller | Face | Normal displacement = 0 |
| Symmetry | Face | Symmetric displacement |

### Loads

| Type | Application | Description |
|------|-------------|-------------|
| Point Force | Vertex | Concentrated force vector |
| Distributed Force | Face | Force per unit area |
| Pressure | Face | Normal surface pressure |
| Gravity | Volume | Body force from acceleration |
| Moment | Edge | Applied torque |

### Multiple Load Cases

Support for multiple independent load cases:
```
minimize: Σ_i w_i × C_i

subject to: K × U_i = F_i  for each load case i
```

Weights `w_i` balance importance of each load case.

## Manufacturing Constraints

### Additive Manufacturing (3D Printing)

**Overhang constraint:**
- Critical angle: typically 45° from horizontal
- Prevents unsupported overhangs that require support material

**Self-supporting filter:**
```
ρ̄_layer(x,y) = max(ρ(x,y,z), min(ρ(x±Δ,y±Δ,z-Δz)))
```

Process layer-by-layer from build plate, ensuring each layer is supported by the previous.

**Minimum feature size:**
- Controlled via filter radius
- Matches printer resolution capabilities

### Casting and Molding

**Draw direction constraint:**
- Single draw: monotonic density along parting direction
- Split draw: monotonic from parting plane in both directions

**Implementation:**
Cast rays along draw direction; enforce density ordering:
```
ρ(x,y,z) ≤ ρ(x,y,z+Δz)  for positive draw direction
```

**No undercuts:**
- Prevent features that would lock the mold

### CNC Machining

**Accessibility constraint:**
- All surfaces must be reachable by cutting tool
- Define tool geometry (diameter, length, type)
- Specify fixturing orientations

**Visibility filter:**
Project from each machining direction; occluded regions cannot be void.

### Extrusion

**2.5D constraint:**
- Constant density along extrusion direction
- Reduces to 2D optimization problem

```
ρ(x,y,z) = ρ(x,y)  for all z
```

## Result Processing: Voxel to BRep

### Pipeline Overview

```
Density Grid → Threshold → SDF → Marching Cubes → Mesh → Smooth → BRep
```

### Step 1: Threshold and SDF Generation

Convert density field to signed distance field:
```python
sdf[i,j,k] = ρ[i,j,k] - 0.5  # Simple threshold
```

For smoother results, use distance transform on thresholded field.

### Step 2: Isosurface Extraction (Marching Cubes)

- Extract triangle mesh at SDF = 0 isosurface
- WebGPU-accelerated implementation
- Output: indexed triangle mesh

### Step 3: Mesh Smoothing

**Laplacian smoothing:**
```
v_new = v + λ × (v_avg_neighbors - v)
```

**HC algorithm** (volume-preserving):
- Two-step process prevents shrinkage
- Better preserves sharp features

### Step 4: NURBS Surface Fitting (Optional)

For high-quality BRep output:
1. Segment mesh into regions of similar curvature
2. Fit B-spline surfaces to each region
3. Trim and sew into watertight solid

### Step 5: BRep Assembly

1. Create faces from fitted surfaces
2. Build half-edge topology
3. Validate solid (closed, manifold, consistent orientation)
4. Export to STEP or integrate into vcad document

## Compute Architecture

### GPU Acceleration (WebGPU)

Critical operations for GPU:

| Operation | % Time | GPU Approach |
|-----------|--------|--------------|
| FEA Solve | ~70% | SpMV kernels, PCG iteration |
| Sensitivity | ~20% | Element-parallel computation |
| Filtering | ~5% | 3D convolution kernel |
| Assembly | ~5% | Parallel element matrix assembly |

**WebGPU implementation:**
- Compute shaders for all heavy operations
- Storage buffers for large arrays
- Workgroup-optimized for coalesced memory access

### Problem Size Tiers

| Tier | Elements | Resolution | Platform | Time/Iter |
|------|----------|------------|----------|-----------|
| Small | <100K | 46³ | Browser WebGPU | <1s |
| Medium | 100K-1M | 46³-100³ | Browser + Cloud FEA | 1-10s |
| Large | >1M | >100³ | Full cloud | >10s |

### Performance Targets

| Configuration | Target |
|---------------|--------|
| 100K elements, browser | <10 seconds total (50 iterations) |
| 1M elements, GPU | ~2 minutes total |
| 10M elements, cloud | ~20 minutes total |
| 65M elements, multi-GPU | ~2 hours total |

## Open Source References

### Algorithm References

- **ToPy**: Python reference implementation of 2D/3D SIMP
  - Clean, educational codebase
  - Good for algorithm validation

- **TopOpt (DTU)**: MATLAB implementation
  - Includes MMA optimizer
  - Well-documented theory

- **TopOpt_in_PETSc**: Large-scale parallel implementation
  - Architecture reference for cloud scaling
  - Multi-GPU support

### Solver References

- **deal.II**: Finite element library with topology optimization examples
- **MFEM**: Scalable FEM with GPU support
- **Trilinos**: Sandia's parallel solver framework

## Implementation Phases

### Phase 1: MVP (4-6 weeks)

**Scope:**
- 2D SIMP demonstration
- OC optimizer (simpler than MMA)
- Browser-based FEA
- Basic visualization

**Deliverables:**
- 2D design space from sketch profile
- Point loads and fixed BCs
- Real-time density visualization
- Export optimized outline as sketch

### Phase 2: 3D Browser (6-8 weeks)

**Scope:**
- Full 3D optimization
- WebGPU-accelerated FEA
- Marching cubes output
- Integration with vcad workflow

**Deliverables:**
- 3D voxelization of design envelope
- GPU FEA solver (<64³)
- Mesh export (STL/GLB)
- Basic preserved regions

### Phase 3: Manufacturing Constraints (4-6 weeks)

**Scope:**
- Overhang constraint (additive)
- Draw direction (casting)
- Symmetry constraints

**Deliverables:**
- Manufacturing mode selector
- Constraint visualization
- Process-specific post-processing

### Phase 4: Cloud Scaling (6-8 weeks)

**Scope:**
- Cloud FEA backend integration
- Job queuing and progress tracking
- Higher resolution support

**Deliverables:**
- API for cloud FEA dispatch
- Real-time progress updates
- Results caching
- Multi-GPU support for large problems

### Phase 5: BRep Reconstruction (4-6 weeks)

**Scope:**
- High-quality surface fitting
- STEP export
- Feature recognition

**Deliverables:**
- NURBS surface fitting
- Watertight BRep output
- Integration with vcad parametric tree
- STEP AP214 export

### Phase 6: Advanced Features (Ongoing)

- **Level-set method**: Alternative to density-based, sharper boundaries
- **Multi-material**: Optimize material distribution across materials
- **Stress constraints**: Direct stress limits instead of compliance
- **Frequency constraints**: Eigenvalue optimization for dynamics
- **Thermal coupling**: Combined structural-thermal optimization

## UI/UX Considerations

### Design Space Wizard

1. Select design envelope solid
2. Pick preserved faces/regions
3. Define loads (click faces, specify vectors)
4. Set boundary conditions
5. Choose manufacturing constraints
6. Configure resolution and targets
7. Run optimization

### Progress Visualization

- Real-time density field rendering
- Convergence plot (compliance vs iteration)
- Volume fraction indicator
- Estimated time remaining

### Result Exploration

- Density threshold slider
- Section views
- Compare to original design
- Export options (mesh, BRep, both)

## Integration with vcad

### Parametric Integration

Optimized geometry can be:
- Inserted as mesh feature in part
- Converted to BRep and added to feature tree
- Linked to original design space (re-optimize on parameter change)

### MCP Tools

```typescript
// Create topology optimization problem
create_topology_optimization({
  designSpace: solidId,
  preserved: [faceId1, faceId2],
  loads: [{ face: faceId3, force: [0, 0, -1000] }],
  fixed: [faceId4, faceId5],
  volumeFraction: 0.3,
  manufacturing: "additive"
})

// Run optimization
run_topology_optimization({
  problemId: problemId,
  resolution: [64, 64, 64],
  maxIterations: 100
})

// Get result
get_topology_result({
  problemId: problemId,
  format: "mesh" | "brep"
})
```

## References

1. Bendsoe, M.P., Sigmund, O. (2003). *Topology Optimization: Theory, Methods and Applications*. Springer.

2. Svanberg, K. (1987). "The method of moving asymptotes—a new method for structural optimization." *International Journal for Numerical Methods in Engineering*, 24(2), 359-373.

3. Andreassen, E., et al. (2011). "Efficient topology optimization in MATLAB using 88 lines of code." *Structural and Multidisciplinary Optimization*, 43(1), 1-16.

4. Liu, K., Tovar, A. (2014). "An efficient 3D topology optimization code written in Matlab." *Structural and Multidisciplinary Optimization*, 50(6), 1175-1196.

5. Aage, N., et al. (2017). "Giga-voxel computational morphogenesis for structural design." *Nature*, 550(7674), 84-86.
