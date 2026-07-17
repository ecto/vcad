//! Reproduce Meep's canonical `bend-flux` benchmark configuration.
//!
//! The exact setup of Meep's published waveguide-bend tutorial
//! (`python/examples/bend-flux.py`, Oskooi et al., Comp. Phys. Comm. 181,
//! 687 (2010)): ε = 12 waveguide, w = 1, cell 16×32, resolution 10
//! px/unit, PML 1.0, Ez (our TM) line source at fcen = 0.15, df = 0.1.
//! Two runs — straight reference and bend — with reflection measured by
//! **incident-phasor subtraction** (our port of `load_minus_flux`) and
//! transmission from the port monitors, printed as R/T/loss spectra.
//!
//! The Meep docs publish these results as curves, not numbers, so the
//! quantitative A/B is a script away: `docs/photonics-m0.md` carries the
//! 25-line Meep script matching this geometry line for line — run both
//! and diff the tables. What must hold here without Meep installed:
//! R + T + loss = 1 (it does, by construction of the subtraction), R and
//! loss small but nonzero, T dipping at long wavelengths — the shape of
//! the published figure.
//!
//! Run: `cargo run --release -p vcad-kernel-photonics --example meep_bend_benchmark`

use vcad_kernel_photonics::{
    CpmlSpec, FluxSpec, GridSpec, Polarization, Shape2, Simulation, Source, Waveform,
};

// Meep's bend-flux parameters, verbatim (lengths in Meep units).
const SX: f64 = 16.0;
const SY: f64 = 32.0;
const W: f64 = 1.0;
const PAD: f64 = 4.0;
const DPML: f64 = 1.0;
const EPS: f64 = 12.0;
const FCEN: f64 = 0.15;
const DF: f64 = 0.1;
const RESOLUTION: usize = 10;

fn main() {
    println!("vcad-kernel-photonics — Meep bend-flux benchmark configuration");
    println!(
        "ε = {EPS}, w = {W}, cell {SX}×{SY}, resolution {RESOLUTION}/unit, \
         PML {DPML}, fcen {FCEN}, df {DF} (Ez ≡ our TM)\n"
    );

    let d = 1.0 / RESOLUTION as f64;
    let (nx, ny) = ((SX / d) as usize, (SY / d) as usize);
    let pml = (DPML / d) as usize;
    // Meep coordinates are centered; ours start at the corner.
    let xc = |x: f64| ((x + 0.5 * SX) / d).round() as usize;
    let yc = |y: f64| ((y + 0.5 * SY) / d).round() as usize;
    let wvg_ycen = -0.5 * (SY - W - 2.0 * PAD); // -11.5
    let wvg_xcen = 0.5 * (SX - W - 2.0 * PAD); // 3.5

    let nfreq = 11;
    let freqs: Vec<f64> = (0..nfreq)
        .map(|k| FCEN - 0.5 * DF + DF * k as f64 / (nfreq - 1) as f64)
        .collect();

    // Source: Ez line of width w at x = −sx/2 + dpml.
    let src_i = pml;
    let (src_j0, src_j1) = (yc(wvg_ycen - 0.5 * W), yc(wvg_ycen + 0.5 * W));
    // Monitors (Meep positions): refl at x = −sx/2 + dpml + 0.5, span 2w;
    // straight trans at x = sx/2 − dpml; bend trans at y = sy/2 − dpml − 0.5.
    let refl_i = xc(-0.5 * SX + DPML + 0.5);
    let (mon_j0, mon_j1) = (yc(wvg_ycen - W), yc(wvg_ycen + W));
    let tran_i = xc(0.5 * SX - DPML);
    let bend_tran_j = yc(0.5 * SY - DPML - 0.5);
    let (bend_i0, bend_i1) = (xc(wvg_xcen - W), xc(wvg_xcen + W));

    // Run time: generous fixed window (Meep stops on 1e−3 field decay).
    let steps = (400.0 / (0.5 * d / 2f64.sqrt())) as usize;

    let build = |bend: bool| -> Simulation {
        let mut sim = Simulation::new(GridSpec::new(nx, ny, d), Polarization::Tm);
        sim.set_cpml(CpmlSpec::uniform(pml));
        if bend {
            // Horizontal block (sx − pad, w) centered (−pad/2, wvg_ycen);
            // vertical block (w, sy − pad) centered (wvg_xcen, pad/2).
            // Horizontal block: Meep center (−pad/2, wvg_ycen), size
            // (sx − pad, w) → corner x ∈ [0, sx/2 − pad/2 + (sx−pad)/2].
            sim.paint(
                &Shape2::rect(
                    -1.0,
                    0.5 * SY + wvg_ycen - 0.5 * W,
                    0.5 * SX - 0.5 * PAD + 0.5 * (SX - PAD),
                    0.5 * SY + wvg_ycen + 0.5 * W,
                ),
                EPS,
            );
            sim.paint(
                &Shape2::rect(
                    0.5 * SX + wvg_xcen - 0.5 * W,
                    0.5 * SY + wvg_ycen - 0.5 * W,
                    0.5 * SX + wvg_xcen + 0.5 * W,
                    SY + 1.0,
                ),
                EPS,
            );
        } else {
            sim.paint(
                &Shape2::rect(
                    -1.0,
                    0.5 * SY + wvg_ycen - 0.5 * W,
                    SX + 1.0,
                    0.5 * SY + wvg_ycen + 0.5 * W,
                ),
                EPS,
            );
        }
        sim.add_source(Source::line_uniform(
            src_i,
            src_j0,
            src_j1,
            Waveform::gaussian(FCEN, DF / 2.0),
        ));
        sim
    };

    // ---- Straight reference: incident phasors + normalization fluxes.
    let mut straight = build(false);
    let s_refl = straight.add_flux(FluxSpec::Vertical {
        i: refl_i,
        j0: mon_j0,
        j1: mon_j1,
        freqs: freqs.clone(),
    });
    let s_tran = straight.add_flux(FluxSpec::Vertical {
        i: tran_i,
        j0: mon_j0,
        j1: mon_j1,
        freqs: freqs.clone(),
    });
    straight.run(steps);
    let (ref_e, ref_h) = straight.flux_phasors(s_refl);
    let p_inc_refl = straight.flux_power(s_refl);
    let p_inc_tran = straight.flux_power(s_tran);

    // ---- Bend run: subtracted reflection + bend-arm transmission.
    let mut bend = build(true);
    let b_refl = bend.add_flux(FluxSpec::Vertical {
        i: refl_i,
        j0: mon_j0,
        j1: mon_j1,
        freqs: freqs.clone(),
    });
    bend.subtract_flux_phasors(b_refl, &ref_e, &ref_h);
    let b_tran = bend.add_flux(FluxSpec::Horizontal {
        j: bend_tran_j,
        i0: bend_i0,
        i1: bend_i1,
        freqs: freqs.clone(),
    });
    bend.run(steps);
    let p_refl = bend.flux_power(b_refl);
    let p_tran = bend.flux_power(b_tran);

    println!("  λ         f       R         T         loss      R+T+loss");
    for k in 0..nfreq {
        let f = freqs[k];
        let r = -p_refl[k].1 / p_inc_refl[k].1;
        let t = p_tran[k].1 / p_inc_tran[k].1;
        let loss = 1.0 - r - t;
        println!(
            "  {:.3}    {f:.4}  {r:.5}   {t:.5}   {loss:.5}   {:.5}",
            1.0 / f,
            r + t + loss
        );
    }
    println!(
        "\nA/B check: run the matching Meep script in docs/photonics-m0.md \
         (same geometry,\nsame monitors, same normalization) and diff this table."
    );
}
