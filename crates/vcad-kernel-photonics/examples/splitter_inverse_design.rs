//! THE flagship: inverse-design a 1×2 power splitter, ship it as GDS.
//!
//! A 2×2 (length-unit)² design box between an input waveguide and two
//! output arms. Per-arm adjoint gradients (one forward + two adjoint
//! FDTD runs per iteration) chained through the density → filter →
//! projection parameterization drive projected gradient ascent with a
//! β binarization schedule. The design is then **hard-thresholded** —
//! claims are made on the binary twin (what the fab receives), with the
//! gray-vs-binary gap reported as an honesty metric. Characterization
//! normalizes by a same-run input monitor (net delivered power, so
//! T_a + T_b ≤ 1 by energy conservation — reference-run normalization
//! is broken here by soft-source back-action from the nearby box) and
//! measures reflection by Meep-style incident-phasor subtraction.
//!
//! Run: `cargo run --release -p vcad-kernel-photonics --example splitter_inverse_design`

use vcad_kernel_photonics::{
    design_to_gds, maximize_split, objectives_and_gradients, solve_slab_mode_even, splitter_claims,
    Claim, CpmlSpec, DesignRegion, FluxSpec, GridSpec, ModeOverlap, OptimizeOptions, Polarization,
    Shape2, Simulation, SolverProvenance, Source, SplitEval, SplitterMeasurement, TopologyParam,
    Waveform,
};

const LAMBDA0: f64 = 1.55;
const N_CORE: f64 = 3.48;
const N_CLAD: f64 = 1.44;
const HALF_W: f64 = 0.11;
const RES: usize = 40;
const NX: usize = 160;
const NY: usize = 140;
const JC: usize = 70; // input-guide axis row
const ARM_OFF: usize = 18; // output-arm offset in rows (±0.7 units)
const I_IN: usize = 40; // input monitor column (source at 20, box at 56)
const I_OUT: usize = 130; // output monitor column
const STEPS: usize = 5000; // optimization window (short windows breed resonance exploitation)
const CHAR_STEPS: usize = 12000; // ring-down: resonant designs need the full decay

fn delta() -> f64 {
    LAMBDA0 / RES as f64
}

fn region() -> DesignRegion {
    DesignRegion {
        i0: 56,
        i1: 107,
        j0: 44,
        j1: 95,
    }
}

fn mode() -> vcad_kernel_photonics::SlabMode {
    solve_slab_mode_even(N_CORE, N_CLAD, HALF_W, LAMBDA0, Polarization::Tm).unwrap()
}

fn source_span() -> (usize, usize) {
    (JC - 20, JC + 20)
}

/// Build the splitter simulation: input guide → design box → two output
/// arms, mode-TF/SF source, optional design stamped in.
fn build_sim(topo: Option<&TopologyParam>, with_sources: bool) -> Simulation {
    let d = delta();
    let yc = JC as f64 * d;
    let r = region();
    let box_x0 = r.i0 as f64 * d - 0.5 * d;
    let box_x1 = r.i1 as f64 * d + 0.5 * d;
    let mut sim = Simulation::new(GridSpec::new(NX, NY, d), Polarization::Tm);
    sim.set_cpml(CpmlSpec::uniform(12));
    sim.fill_epsilon(N_CLAD * N_CLAD);
    sim.paint(
        &Shape2::rect(-1.0, yc - HALF_W, box_x0, yc + HALF_W),
        N_CORE * N_CORE,
    );
    for arm in [JC + ARM_OFF, JC - ARM_OFF] {
        let ya = arm as f64 * d;
        sim.paint(
            &Shape2::rect(box_x1, ya - HALF_W, 1e9, ya + HALF_W),
            N_CORE * N_CORE,
        );
    }
    if let Some(t) = topo {
        t.apply(&mut sim);
    }
    if with_sources {
        let m = mode();
        let (j0, j1) = source_span();
        let profile: Vec<f64> = (j0..=j1)
            .map(|j| m.profile((j as f64 - JC as f64) * d))
            .collect();
        sim.add_source(Source::mode_tfsf(
            20,
            j0,
            profile,
            m.n_eff,
            Waveform::gaussian(1.0 / LAMBDA0, 1.0 / LAMBDA0 / 5.0),
        ));
    }
    sim
}

fn arm_objective(arm_row: usize) -> ModeOverlap {
    let d = delta();
    let m = mode();
    let (j0, j1) = (arm_row - 14, arm_row + 14);
    ModeOverlap {
        i: 126,
        j0,
        weights: (j0..=j1)
            .map(|j| m.profile((j as f64 - arm_row as f64) * d))
            .collect(),
        freq: 1.0 / LAMBDA0,
    }
}

struct Characterization {
    meas: Vec<SplitterMeasurement>,
    reflection: Vec<(f64, f64)>,
}

/// Run a design with input/output flux monitors. Transmission is
/// normalized by the same-run **net input power** (delivered − reflected
/// at the input plane); reflection comes from incident-phasor
/// subtraction against a straight-guide reference.
fn characterize(topo: &TopologyParam, freqs: &[f64]) -> Characterization {
    let (j0s, j1s) = source_span();
    // Reference: straight guide, incident phasors + power at the input plane.
    let (ref_e, ref_h, p_inc) = {
        let d = delta();
        let yc = JC as f64 * d;
        let mut sim = Simulation::new(GridSpec::new(NX, NY, d), Polarization::Tm);
        sim.set_cpml(CpmlSpec::uniform(12));
        sim.fill_epsilon(N_CLAD * N_CLAD);
        sim.paint(
            &Shape2::rect(-1.0, yc - HALF_W, 1e9, yc + HALF_W),
            N_CORE * N_CORE,
        );
        let m = mode();
        let profile: Vec<f64> = (j0s..=j1s)
            .map(|j| m.profile((j as f64 - JC as f64) * d))
            .collect();
        sim.add_source(Source::mode_tfsf(
            20,
            j0s,
            profile,
            m.n_eff,
            Waveform::gaussian(1.0 / LAMBDA0, 1.0 / LAMBDA0 / 5.0),
        ));
        let fl = sim.add_flux(FluxSpec::Vertical {
            i: I_IN,
            j0: j0s,
            j1: j1s,
            freqs: freqs.to_vec(),
        });
        sim.run(CHAR_STEPS);
        let (e, h) = sim.flux_phasors(fl);
        (e, h, sim.flux_power(fl))
    };

    let mut sim = build_sim(Some(topo), true);
    let f_in = sim.add_flux(FluxSpec::Vertical {
        i: I_IN,
        j0: j0s,
        j1: j1s,
        freqs: freqs.to_vec(),
    });
    let f_refl = sim.add_flux(FluxSpec::Vertical {
        i: I_IN,
        j0: j0s,
        j1: j1s,
        freqs: freqs.to_vec(),
    });
    sim.subtract_flux_phasors(f_refl, &ref_e, &ref_h);
    let (ja, jb) = (JC + ARM_OFF, JC - ARM_OFF);
    let fa = sim.add_flux(FluxSpec::Vertical {
        i: I_OUT,
        j0: ja - 14,
        j1: ja + 14,
        freqs: freqs.to_vec(),
    });
    let fb = sim.add_flux(FluxSpec::Vertical {
        i: I_OUT,
        j0: jb - 14,
        j1: jb + 14,
        freqs: freqs.to_vec(),
    });
    // Full-interior-height witnesses for exact energy accounting.
    let f_in_full = sim.add_flux(FluxSpec::Vertical {
        i: I_IN,
        j0: 13,
        j1: NY - 13,
        freqs: freqs.to_vec(),
    });
    let f_out_full = sim.add_flux(FluxSpec::Vertical {
        i: I_OUT,
        j0: 13,
        j1: NY - 13,
        freqs: freqs.to_vec(),
    });
    sim.run(CHAR_STEPS);
    let p_in = sim.flux_power(f_in);
    let p_in_full = sim.flux_power(f_in_full);
    let p_out_full = sim.flux_power(f_out_full);
    let p_sub = sim.flux_power(f_refl);
    let pa = sim.flux_power(fa);
    let pb = sim.flux_power(fb);
    println!("  energy accounting (per f): P_inc_ref | net_in(win) | net_in(full) | fwd_out(full) | P_a+P_b");
    for (k, &f) in freqs.iter().enumerate() {
        println!(
            "    f {f:.4}:  {:.4e} | {:.4e} | {:.4e} | {:.4e} | {:.4e}",
            p_inc[k].1,
            p_in[k].1,
            p_in_full[k].1,
            p_out_full[k].1,
            pa[k].1 + pb[k].1
        );
    }
    let meas = freqs
        .iter()
        .enumerate()
        .map(|(k, &f)| SplitterMeasurement {
            freq: f,
            p_in: p_in[k].1,
            p_arm_a: pa[k].1,
            p_arm_b: pb[k].1,
        })
        .collect();
    // Scattered-field flux at the input plane is backward ⇒ negative;
    // R = −P_sub / P_incident(reference).
    let reflection = freqs
        .iter()
        .enumerate()
        .map(|(k, &f)| (f, -p_sub[k].1 / p_inc[k].1))
        .collect();
    Characterization { meas, reflection }
}

fn main() {
    let f0 = 1.0 / LAMBDA0;
    let m = mode();
    println!("vcad-kernel-photonics M5 — inverse-designed 1×2 splitter");
    println!("units: c = ε₀ = μ₀ = 1; 1 length unit = 1 µm by convention");
    println!(
        "guide n_eff = {:.5}, design box 2×2 over {} cells, Δ = λ/{}\n",
        m.n_eff,
        region().len(),
        RES
    );

    // ---- Optimize (objective = per-arm mode overlap; normalization-free).
    let r = region();
    let mut topo = TopologyParam::uniform(r, 0.5, N_CLAD * N_CLAD, N_CORE * N_CORE);
    topo.filter_radius_cells = 3.0; // ≈232 nm minimum feature diameter
                                    // Seed: a coarse Y-taper (input fanning to both arms) — converges to
                                    // cleaner topologies than an all-gray start.
    {
        let (rx, ry) = (r.ns_x(), r.ns_y());
        let jc_loc = JC as f64 - r.j0 as f64;
        for di in 0..rx {
            let frac = di as f64 / rx as f64;
            let spread = 2.0 + frac * (ARM_OFF as f64 + 4.0 - 2.0);
            for dj in 0..ry {
                let y = dj as f64 - jc_loc;
                let in_fan = y.abs() <= spread
                    && (frac < 0.55 || (y.abs() - ARM_OFF as f64 * frac).abs() <= 5.0);
                topo.rho[di * ry + dj] = if in_fan { 0.9 } else { 0.12 };
            }
        }
    }
    let objs = [arm_objective(JC + ARM_OFF), arm_objective(JC - ARM_OFF)];
    let n_runs = std::cell::Cell::new(0usize);
    let mut eval = |t: &TopologyParam| -> SplitEval {
        let t2 = t.clone();
        let mut build = move |ws: bool| build_sim(Some(&t2), ws);
        let res = objectives_and_gradients(&mut build, &r, &objs, STEPS);
        n_runs.set(n_runs.get() + 3);
        SplitEval {
            j_a: res[0].objective,
            j_b: res[1].objective,
            grad_a: t.chain_gradient(&res[0].grad),
            grad_b: t.chain_gradient(&res[1].grad),
        }
    };
    let opts = OptimizeOptions {
        betas: vec![4.0, 8.0, 16.0, 32.0, 64.0, 128.0],
        iters_per_beta: 10,
        balance_gamma: 2.0,
        ..OptimizeOptions::default()
    };
    let spec_path = std::env::temp_dir().join("vcad_splitter_spec.json");
    let reuse = std::env::var("VCAD_SPLITTER_REUSE").is_ok() && spec_path.exists();
    let mut trace = Vec::new();
    if reuse {
        // Post-mortem mode: reload the optimized densities via the spec
        // seam instead of re-running the optimization.
        let json = std::fs::read_to_string(&spec_path).unwrap();
        let spec: vcad_kernel_photonics::TopologyProblemSpec = serde_json::from_str(&json).unwrap();
        let resolved = spec
            .resolve(&std::collections::BTreeMap::new(), 128.0)
            .unwrap();
        topo = resolved.param;
        topo.filter_radius_cells = 3.0;
        println!("(reusing optimized design from {})", spec_path.display());
    } else {
        println!(
            "optimizing: β schedule {:?}, ≤{} iters each",
            opts.betas, opts.iters_per_beta
        );
        trace = maximize_split(&mut eval, &mut topo, &opts);
        println!("  iter  β     J_a          J_b          FoM         step    ok");
        for rec in trace.iter().filter(|t| t.accepted) {
            println!(
                "  {:>4}  {:>4}  {:.5e}  {:.5e}  {:.5e}  {:.4}  +",
                rec.iter, rec.beta, rec.j_a, rec.j_b, rec.fom, rec.step
            );
        }
        println!("  ({} FDTD runs total)", n_runs.get());
        let mut spec =
            vcad_kernel_photonics::TopologyProblemSpec::new(LAMBDA0, N_CORE, N_CLAD, RES, r);
        spec.filter_radius_cells = topo.filter_radius_cells.into();
        spec.set_rho(topo.rho.clone());
        std::fs::write(&spec_path, serde_json::to_string(&spec).unwrap()).unwrap();
        println!("  design spec → {}", spec_path.display());
    }

    // ---- Binarize: claims are made on what the fab receives.
    let binary = topo.binarized();
    let gray_fom = trace.iter().rfind(|t| t.accepted).map(|t| t.fom);
    let bin_eval = eval(&binary);
    println!(
        "\nbinarization: gray FoM {:.5e} → binary FoM {:.5e} (gap = honesty metric)",
        gray_fom.unwrap_or(f64::NAN),
        bin_eval.j_a + bin_eval.j_b
            - (bin_eval.j_a - bin_eval.j_b).powi(2) / (bin_eval.j_a + bin_eval.j_b)
    );
    let solid = binary.rho.iter().filter(|&&v| v >= 0.5).count();
    println!("binary design: {solid}/{} solid pixels\n", binary.rho.len());

    // ---- Characterize the binary design over a wavelength sweep.
    let lambdas = [1.50, 1.525, 1.55, 1.575, 1.60];
    let freqs: Vec<f64> = lambdas.iter().map(|l| 1.0 / l).collect();
    let ch = characterize(&binary, &freqs);
    println!("  λ        T_a      T_b      total    ratio    R_in");
    for (k, mrow) in ch.meas.iter().enumerate() {
        let (ta, tb) = (mrow.p_arm_a / mrow.p_in, mrow.p_arm_b / mrow.p_in);
        println!(
            "  {:.3}    {ta:.4}   {tb:.4}   {:.4}   {:.4}   {:.4}",
            lambdas[k],
            ta + tb,
            ta / (ta + tb),
            ch.reflection[k].1
        );
        if ta + tb > 1.0 + 1e-3 {
            println!(
                "    !! window-normalized total {} exceeds 1 — see accounting above",
                ta + tb
            );
        }
    }

    // ---- Claims (binary design, net-input normalization).
    let sim_for_prov = build_sim(Some(&binary), true);
    let mut prov = SolverProvenance::from_sim(&sim_for_prov, LAMBDA0, N_CORE, CHAR_STEPS);
    prov.monitor_freqs = freqs.clone();
    let mut claims = splitter_claims(&ch.meas, f0, prov, Some(&topo)).expect("claims");
    let r_center = ch
        .reflection
        .iter()
        .find(|(f, _)| (f - f0).abs() < 1e-12)
        .map(|(_, r2)| *r2)
        .unwrap();
    claims.claims.push(Claim {
        name: "reflection_input".to_string(),
        value: r_center,
        unit: "1".to_string(),
        basis: "predicted".to_string(),
        note: "backward power at the input plane / incident power, via \
               incident-phasor subtraction against a straight-guide \
               reference (exact for co-propagating scatter)"
            .to_string(),
    });
    println!("\nclaims ({}):", claims.schema);
    for c in &claims.claims {
        println!("  {:<22} {:>10.4} {}", c.name, c.value, c.unit);
    }
    let json = serde_json::to_string_pretty(&claims).unwrap();
    let claims_path = std::env::temp_dir().join("vcad_splitter_claims.json");
    std::fs::write(&claims_path, &json).unwrap();
    println!("  claims JSON → {}", claims_path.display());

    // ---- GDS (same threshold as the binary twin).
    let d = delta();
    let yc = JC as f64 * d;
    let box_x0 = r.i0 as f64 * d - 0.5 * d;
    let box_x1 = r.i1 as f64 * d + 0.5 * d;
    let mut guides: Vec<(f64, f64, f64, f64)> = vec![(0.0, yc - HALF_W, box_x0, yc + HALF_W)];
    for arm in [JC + ARM_OFF, JC - ARM_OFF] {
        let ya = arm as f64 * d;
        guides.push((box_x1, ya - HALF_W, NX as f64 * d, ya + HALF_W));
    }
    let lib = design_to_gds(&binary, d, &guides, 1, "vcad_splitter_1x2");
    let bytes = vcad_gdsii::write_library(&lib).unwrap();
    let gds_path = std::env::temp_dir().join("vcad_splitter_1x2.gds");
    std::fs::write(&gds_path, &bytes).unwrap();
    println!(
        "  GDS ({} boundaries, µm/nm units) → {}",
        lib.cells[0].elements.len(),
        gds_path.display()
    );

    // ---- Convergence: re-simulate the binary pixel geometry at finer
    // grids (the λ/40 row must reproduce the characterization — printed
    // as a self-consistency check).
    println!("\nconvergence of the binary geometry (pixel shapes re-painted):");
    println!("  res      T_a      T_b      total");
    for res in [40usize, 60, 80] {
        let (ta, tb) = resimulate_at(&binary, res);
        println!("  λ/{res}    {ta:.4}   {tb:.4}   {:.4}", ta + tb);
    }
    println!("\nλ/40 re-paints the native pixels (must match the table above);");
    println!("finer grids re-discretize the same shapes — drift is O(Δ²).");
}

/// Re-simulate the binary pixel geometry at another resolution with a
/// same-run input monitor, returning (T_a, T_b).
fn resimulate_at(binary: &TopologyParam, res: usize) -> (f64, f64) {
    let scale = res as f64 / RES as f64;
    let d = LAMBDA0 / res as f64;
    let (nx, ny) = (
        (NX as f64 * scale).round() as usize,
        (NY as f64 * scale).round() as usize,
    );
    let jc = (JC as f64 * scale).round() as usize;
    let arm = (ARM_OFF as f64 * scale).round() as usize;
    let yc = jc as f64 * d;
    let f0 = 1.0 / LAMBDA0;
    let m = mode();
    let r = region();
    let d0 = delta();
    let box_x0 = r.i0 as f64 * d0 - 0.5 * d0;
    let box_x1 = r.i1 as f64 * d0 + 0.5 * d0;

    let mut sim = Simulation::new(GridSpec::new(nx, ny, d), Polarization::Tm);
    sim.set_cpml(CpmlSpec::uniform((12.0 * scale).round() as usize));
    sim.fill_epsilon(N_CLAD * N_CLAD);
    sim.paint(
        &Shape2::rect(-1.0, yc - HALF_W, box_x0, yc + HALF_W),
        N_CORE * N_CORE,
    );
    for a in [jc + arm, jc - arm] {
        let ya = a as f64 * d;
        sim.paint(
            &Shape2::rect(box_x1, ya - HALF_W, 1e9, ya + HALF_W),
            N_CORE * N_CORE,
        );
    }
    // The binary pixel geometry as physical rects (edges preserved).
    let (rx, ry) = (r.ns_x(), r.ns_y());
    let solid: Vec<bool> = binary.projected().iter().map(|&p| p >= 0.5).collect();
    let ox = r.i0 as f64 * d0 - 0.5 * d0;
    let oy = r.j0 as f64 * d0 - 0.5 * d0;
    for (i0, j0, i1, j1) in vcad_kernel_photonics::decompose_rects(&solid, rx, ry) {
        sim.paint(
            &Shape2::rect(
                ox + i0 as f64 * d0,
                oy + j0 as f64 * d0,
                ox + i1 as f64 * d0,
                oy + j1 as f64 * d0,
            ),
            N_CORE * N_CORE,
        );
    }
    let sc = |v: usize| (v as f64 * scale).round() as usize;
    let (j0s, j1s) = (jc - sc(20), jc + sc(20));
    let profile: Vec<f64> = (j0s..=j1s)
        .map(|j| m.profile((j as f64 - jc as f64) * d))
        .collect();
    sim.add_source(Source::mode_tfsf(
        sc(20),
        j0s,
        profile,
        m.n_eff,
        Waveform::gaussian(f0, f0 / 5.0),
    ));
    let f_in = sim.add_flux(FluxSpec::Vertical {
        i: sc(I_IN),
        j0: j0s,
        j1: j1s,
        freqs: vec![f0],
    });
    let span = sc(14);
    let fa = sim.add_flux(FluxSpec::Vertical {
        i: sc(I_OUT),
        j0: jc + arm - span,
        j1: jc + arm + span,
        freqs: vec![f0],
    });
    let fb = sim.add_flux(FluxSpec::Vertical {
        i: sc(I_OUT),
        j0: jc - arm - span,
        j1: jc - arm + span,
        freqs: vec![f0],
    });
    sim.run((CHAR_STEPS as f64 * scale) as usize);
    let p_in = sim.flux_power(f_in)[0].1;
    (
        sim.flux_power(fa)[0].1 / p_in,
        sim.flux_power(fb)[0].1 / p_in,
    )
}
