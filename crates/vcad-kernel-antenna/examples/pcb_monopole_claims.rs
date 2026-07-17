//! The measurement-pack artifact: predicted claims for the 915 MHz PCB
//! monopole, as JSON on stdout (summary on stderr).
//!
//! ```text
//! cargo run --release -p vcad-kernel-antenna --example pcb_monopole_claims > claims.json
//! ```
//!
//! Bind a NanoVNA sweep to it with `nanovna::parse_s1p` +
//! `nanovna::measurements_from_s1p` + `receipt::compare` — see
//! `docs/antenna-measurement-pack.md` for the board and the procedure.

use vcad_kernel_antenna::ecad::add_trace_as_wire;
use vcad_kernel_antenna::receipt::{predicted_claims, FrequencyBand};
use vcad_kernel_antenna::{Mesh, SolveOptions, WireGrid};

fn main() {
    let mut g = WireGrid::new();
    g.set_ground_plane(true);
    add_trace_as_wire(&mut g, &[[0.0, 0.0, 0.0], [0.0, 0.0, 78.0]], 1.6, &[12]).unwrap();
    let mesh = Mesh::build(&g).unwrap();
    let feed = mesh.nearest_basis([0.0, 0.0, 0.0]).unwrap();

    let claims = predicted_claims(
        &mesh,
        feed,
        FrequencyBand {
            f_lo_hz: 700e6,
            f_hi_hz: 1100e6,
            points: 81,
        },
        50.0,
        &SolveOptions::default(),
    )
    .expect("claims");

    for c in &claims.claims {
        eprintln!("{:>22}  {:14.6e} {}", c.name, c.value, c.unit);
    }
    println!("{}", serde_json::to_string_pretty(&claims).expect("json"));
}
