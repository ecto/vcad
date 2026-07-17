//! Emit the M0 validation-ladder numbers as JSON (doc + chart source).
use vcad_kernel_em::analytic;
use vcad_kernel_em::axisym::{Annulus, AxisymMagnetostatics, Coil};
use vcad_kernel_em::grid::{Bc, SolveOptions};

fn thin_loop(r_mm: f64, z_mm: f64, turns: f64, current_a: f64) -> Coil {
    Coil {
        region: Annulus {
            r_inner_mm: r_mm - 0.5,
            r_outer_mm: r_mm + 0.5,
            z_min_mm: z_mm - 0.5,
            z_max_mm: z_mm + 0.5,
        },
        turns,
        current_a,
    }
}

fn main() {
    let opts = SolveOptions::default();

    // Infinite solenoid exact anchor: error vs radial resolution.
    println!("{{\"solenoid_inf\": [");
    let expect = {
        let bore = analytic::solenoid_inductance_per_m(10_000.0, 0.020, 0.0, 1.0);
        let t: f64 = 0.002;
        let mut wind = 0.0;
        let m = 4000;
        for p in 0..m {
            let r = 0.020 + (p as f64 + 0.5) * t / m as f64;
            let h = (0.022 - r) / t;
            wind += h * h * 2.0 * std::f64::consts::PI * r * (t / m as f64);
        }
        bore + vcad_kernel_em::constants::MU_0 * 1e8 * wind
    };
    let mut first = true;
    for nr in [21usize, 41, 81, 161, 321] {
        let mut dev = AxisymMagnetostatics::new(40.0, 0.0, 100.0);
        dev.bc_r_outer = Bc::Neumann;
        dev.bc_z_low = Bc::Neumann;
        dev.bc_z_high = Bc::Neumann;
        dev.coils.push(Coil {
            region: Annulus {
                r_inner_mm: 20.0,
                r_outer_mm: 22.0,
                z_min_mm: 0.0,
                z_max_mm: 100.0,
            },
            turns: 1000.0,
            current_a: 1.0,
        });
        let sol = dev.solve(nr, 11, &opts).unwrap();
        let l = 2.0 * sol.energy().source / 0.1;
        if !first {
            println!(",");
        }
        first = false;
        print!(
            "  {{\"nr\": {nr}, \"h_mm\": {:.4}, \"rel_err\": {:.6e}}}",
            40.0 / (nr as f64 - 1.0),
            (l - expect).abs() / expect
        );
    }
    println!("\n],");

    // Wheeler
    let mut dev = AxisymMagnetostatics::new(150.0, -150.0, 150.0);
    dev.coils.push(Coil {
        region: Annulus {
            r_inner_mm: 9.9,
            r_outer_mm: 10.1,
            z_min_mm: -20.0,
            z_max_mm: 20.0,
        },
        turns: 200.0,
        current_a: 1.0,
    });
    let sol = dev.solve(121, 241, &opts).unwrap();
    let wheeler = analytic::wheeler_solenoid_inductance(0.010, 0.040, 200.0);
    println!(
        "\"wheeler\": {{\"l_solved\": {:.6e}, \"l_wheeler\": {:.6e}, \"rel\": {:.4e}}},",
        sol.self_inductance(0),
        wheeler,
        (sol.self_inductance(0) - wheeler).abs() / wheeler
    );

    // Mutual
    let mut dev = AxisymMagnetostatics::new(120.0, -120.0, 120.0);
    dev.coils.push(thin_loop(30.0, -10.0, 1.0, 100.0));
    dev.coils.push(thin_loop(30.0, 10.0, 1.0, 0.0));
    let sol = dev.solve(121, 241, &opts).unwrap();
    let m_s = sol.flux_linkage(1) / 100.0;
    let m_a = analytic::loop_mutual_inductance(0.030, 0.030, 0.020);
    println!(
        "\"mutual\": {{\"m_solved\": {:.6e}, \"m_maxwell\": {:.6e}, \"rel\": {:.4e}}},",
        m_s,
        m_a,
        (m_s - m_a).abs() / m_a
    );

    // Force three ways
    let mut dev = AxisymMagnetostatics::new(120.0, -120.0, 120.0);
    dev.coils.push(thin_loop(30.0, -10.0, 1.0, 100.0));
    dev.coils.push(thin_loop(30.0, 10.0, 1.0, 100.0));
    let sol = dev.solve(121, 241, &opts).unwrap();
    let f_a = analytic::loop_axial_force(0.030, 0.030, 0.020, 100.0, 100.0);
    let f_j = sol.axial_force_on_coil(1);
    let f_s = sol.axial_force_stress(45.0, 2.0, 30.0, 600);
    println!("\"force\": {{\"analytic\": {:.6e}, \"jxb\": {:.6e}, \"stress\": {:.6e}, \"rel_jxb\": {:.4e}, \"rel_stress_vs_jxb\": {:.4e}}},", f_a, f_j, f_s, (f_j-f_a).abs()/f_a.abs(), (f_s-f_j).abs()/f_j.abs());

    // Loop-field cross-check points vs b_ring
    use vcad_kernel_particle::field::{b_ring, RingCoil};
    let mut dev = AxisymMagnetostatics::new(150.0, -150.0, 150.0);
    dev.coils.push(thin_loop(30.0, 0.0, 1.0, 100.0));
    let sol = dev.solve(121, 241, &opts).unwrap();
    let fil = RingCoil {
        radius_m: 0.030,
        z_m: 0.0,
        ampere_turns: 100.0,
        wire_radius_m: 5e-4,
    };
    let g = sol.system.grid.clone();
    println!("\"b_ring_points\": [");
    let mut first = true;
    for (r_t, z_t) in [
        (0.000, 0.010),
        (0.010, 0.005),
        (0.015, 0.025),
        (0.045, 0.000),
        (0.040, 0.010),
        (0.025, 0.040),
    ] {
        let r = if r_t == 0.0 {
            0.0
        } else {
            ((r_t / g.dx).floor() + 0.5) * g.dx
        };
        let z = g.y0 + (((z_t - g.y0) / g.dy).floor() + 0.5) * g.dy;
        let (br_s, bz_s) = sol.b_at(r, z);
        let (br_a, bz_a) = b_ring(&fil, r, z);
        let mag = (br_a * br_a + bz_a * bz_a).sqrt();
        let err = ((br_s - br_a).powi(2) + (bz_s - bz_a).powi(2)).sqrt() / mag;
        if !first {
            println!(",");
        }
        first = false;
        print!(
            "  {{\"r_mm\": {:.1}, \"z_mm\": {:.1}, \"rel_err\": {:.4e}}}",
            r * 1e3,
            z * 1e3,
            err
        );
    }
    println!("\n]}}");
}
