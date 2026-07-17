# Heat-conduction FEA M3: the seam — named parameters, voxelized parts, MaterialCards

Fourth rung of the `vcad-kernel-thermal` ladder. The crate becomes
addressable from `.vcad` documents.

## ThermalSpec: the serde contract

`spec::ThermalSpec` mirrors the model with every physical number a
`ParamValue` — a literal or the **name** of a document parameter bound at
`resolve()` time. Resolution is fail-closed: an unbound name is an error,
never a default (the particle `DeviceSpec` contract verbatim; the vcad
side already speaks this pattern through
`document_parameter_gradient`). `from_model()` gives the literal spec for
unparameterized documents; JSON round-trips are tested including the
tagged shape and boundary enums.

## parameter_roles: who gets the adjoint

`parameter_roles()` classifies every named parameter by how its gradient
is obtained, and the honesty rule from M2 carries into the classification:

- **Adjoint**: conductivity components, film coefficients, source powers —
  exactly what `adjoint::smooth_max_gradient` computes.
- **FiniteDifference**: geometry (it moves the discrete material mask — no
  smooth adjoint covers that), heat capacity (transient-only), and
  **temperatures** (ambients, reservoirs, reference). Temperatures are
  adjoint-*capable* — they enter the right-hand side linearly — but the
  path is not wired, so they are classified by what is implemented, not
  what is possible. When the wiring lands they flip.
- A name shared between an adjoint field and anything else is
  conservatively FD (tested).

## The tessellated-part seam

`VoxelMaterials` carries an externally produced per-voxel material index
(−1 = void) that **overrides region painting**. The voxelizer itself
deliberately lands on the vcad side of the seam — sample voxel centers
against the part with the kernel's point-in-solid machinery, emit indices
into the material table — the same division of labor as the particle
crate's BRep-to-rings extraction. Indices are data, not parameters, and
pass through the spec verbatim. Fail-closed: wrong length and
out-of-range indices are named errors; the end-to-end test drives a
voxelized bar (copper|copper|FR4|void) through spec → JSON → resolve →
solve, exposed-face convection and all.

## MaterialCard hookup — documented, honestly

`vcad-kernel-atoms::homogenize::MaterialCard` today carries **density and
elastic constants only**: there is no thermal conductivity and no
specific heat in the card. The intended mapping when the atoms side grows
them:

- `k_w_mk` ← phonon/Green–Kubo lattice conductivity (a new atoms
  milestone; not derivable from the static elastic homogenization),
- `heat_capacity_j_m3k` ← `density_kg_m3` × c_p (c_p from a phonon DOS or
  tabulated).

Until then, thermal properties come from handbooks. Wiring a card field
that does not exist would be a silent default, and this crate does not do
those.

All previous caveats stand: conduction only, h supplied, no radiation.
