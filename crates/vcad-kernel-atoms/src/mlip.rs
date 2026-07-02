//! ML interatomic potential (MLIP) adapter — M3.
//!
//! The credible-fidelity force engine: a pretrained equivariant GNN potential
//! (MACE-MP, Orb, NequIP/Allegro) evaluated as a [`ForceField`] via
//! `tang-infer` / `tang-onnx`. This module defines the seam — featurization and
//! the inference boundary — behind the same `energy_forces` contract every
//! classical term uses, so the integrator, minimizer, and inverse-design loop
//! are agnostic to which engine produces forces.
//!
//! The actual model weights are loaded at runtime (from a Hugging Face
//! foundation potential); this crate owns the graph construction and the
//! adapter, not the network. Until a backend is wired, [`MlipPotential`] runs
//! against a pluggable [`MlipBackend`] so the whole pipeline is exercisable
//! with a stub and swappable for `tang-infer` without touching callers.

use crate::potential::ForceField;
use crate::system::AtomSystem;

/// A neighbor-graph featurization of a system, handed to an MLIP backend.
///
/// This is the model-agnostic input: for each atom, its neighbors within the
/// model cutoff and the corresponding displacement vectors (already
/// minimum-imaged). Equivariant potentials consume exactly this.
#[derive(Debug, Clone)]
pub struct AtomGraph {
    /// Atomic numbers per node.
    pub numbers: Vec<u32>,
    /// Directed edges `(src, dst)`.
    pub edges: Vec<(u32, u32)>,
    /// Displacement `r_dst - r_src` (Å) per edge.
    pub edge_vectors: Vec<[f64; 3]>,
    /// Model cutoff used to build the graph (Å).
    pub cutoff: f64,
}

impl AtomGraph {
    /// Build the neighbor graph for `sys` at the given cutoff (O(N²); a cell
    /// list should replace this for large systems).
    pub fn build(sys: &AtomSystem, cutoff: f64) -> Self {
        use crate::potential::min_image;
        use crate::vec3;
        let n = sys.len();
        let rc2 = cutoff * cutoff;
        let mut edges = Vec::new();
        let mut edge_vectors = Vec::new();
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                let d = min_image(vec3::sub(sys.positions[j], sys.positions[i]), &sys.cell);
                if vec3::norm2(d) <= rc2 {
                    edges.push((i as u32, j as u32));
                    edge_vectors.push(d);
                }
            }
        }
        Self {
            numbers: sys.numbers.clone(),
            edges,
            edge_vectors,
            cutoff,
        }
    }
}

/// Inference backend for an MLIP: consumes an [`AtomGraph`], returns total
/// energy (eV) and per-atom forces (eV/Å). A `tang-infer`/ONNX implementation
/// slots in here; the trait keeps the rest of the crate backend-agnostic.
pub trait MlipBackend {
    /// Evaluate energy and forces for a graph.
    fn evaluate(&self, graph: &AtomGraph) -> (f64, Vec<[f64; 3]>);
    /// The model's interaction cutoff in Å.
    fn cutoff(&self) -> f64;
}

/// A [`ForceField`] backed by an [`MlipBackend`].
pub struct MlipPotential<B: MlipBackend> {
    /// The inference backend.
    pub backend: B,
}

impl<B: MlipBackend> MlipPotential<B> {
    /// Wrap a backend as a force field.
    pub fn new(backend: B) -> Self {
        Self { backend }
    }
}

impl<B: MlipBackend> ForceField for MlipPotential<B> {
    fn energy_forces(&self, sys: &AtomSystem) -> (f64, Vec<[f64; 3]>) {
        let graph = AtomGraph::build(sys, self.backend.cutoff());
        self.backend.evaluate(&graph)
    }
}

/// A physically-motivated stub backend: a smooth pairwise Morse-like potential
/// over graph edges. It is **not** a trained model — it exists so the MLIP
/// pipeline (graph build → backend → integrator/minimizer) is fully runnable
/// and testable before real weights land, and it satisfies the `energy_forces`
/// contract (validated by the FD oracle like any other term).
pub struct PairwiseStubBackend {
    /// Well depth (eV).
    pub depth: f64,
    /// Equilibrium distance (Å).
    pub r0: f64,
    /// Width parameter (1/Å).
    pub alpha: f64,
    /// Graph cutoff (Å).
    pub cutoff: f64,
}

impl Default for PairwiseStubBackend {
    fn default() -> Self {
        Self {
            depth: 0.1,
            r0: 2.5,
            alpha: 1.5,
            cutoff: 6.0,
        }
    }
}

impl MlipBackend for PairwiseStubBackend {
    fn evaluate(&self, graph: &AtomGraph) -> (f64, Vec<[f64; 3]>) {
        use crate::vec3;
        let n = graph.numbers.len();
        let mut forces = vec![[0.0; 3]; n];
        let mut energy = 0.0;
        // Edges are directed and symmetric; count each pair once (src < dst).
        for (e, &(s, t)) in graph.edges.iter().enumerate() {
            if s >= t {
                continue;
            }
            let d = graph.edge_vectors[e]; // r_t - r_s
            let r = vec3::norm(d);
            if r < 1e-9 {
                continue;
            }
            // Morse: E = depth (1 - e^{-a (r-r0)})^2 - depth
            let ex = (-self.alpha * (r - self.r0)).exp();
            let term = 1.0 - ex;
            energy += self.depth * (term * term - 1.0);
            // dE/dr = 2 depth (1 - e) * (a e)
            let de_dr = 2.0 * self.depth * term * (self.alpha * ex);
            // F on src = -dE/dr_s ; r = |r_t - r_s|, d/dr_s r = -(d)/r
            let fmag_on_s = de_dr / r; // F_s = de_dr * d/r
            let f_s = vec3::scale(d, fmag_on_s);
            vec3::add_assign(&mut forces[s as usize], f_s);
            vec3::add_assign(&mut forces[t as usize], vec3::scale(f_s, -1.0));
        }
        (energy, forces)
    }

    fn cutoff(&self) -> f64 {
        self.cutoff
    }
}
