//! Small-signal AC analysis — complex MNA.
//!
//! Linearizes the circuit about its DC operating point (diodes become their
//! small-signal conductance `g_d = Is/(n·Vt)·exp(v_d/(n·Vt))`) and solves the
//! complex phasor system at each requested angular frequency ω:
//!
//! - R → conductance `1/R`
//! - C → admittance `jωC`
//! - L → an MNA branch enforcing `v(p) − v(n) − jωL·i = 0` (branch form, so
//!   ω = 0 degrades gracefully to the DC short instead of a singular 1/jωL)
//! - the designated source drives with unit amplitude, zero phase; every
//!   other independent source is zeroed (V shorts, I opens) — superposition,
//!   the textbook small-signal setup
//!
//! Complex arithmetic is hand-rolled as an `(re, im)` pair — no external
//! dependency, and the dense complex LU is a line-for-line sibling of the
//! real one in [`super::linalg`].

use super::dc::{operating_point, DcError};
use super::devices::VT;
use super::{Circuit, Device};

/// A complex number as an explicit `(re, im)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Cplx {
    /// Real part.
    pub re: f64,
    /// Imaginary part.
    pub im: f64,
}

impl Cplx {
    /// Zero.
    pub const ZERO: Cplx = Cplx { re: 0.0, im: 0.0 };
    /// One.
    pub const ONE: Cplx = Cplx { re: 1.0, im: 0.0 };

    /// Build from real and imaginary parts.
    pub fn new(re: f64, im: f64) -> Self {
        Cplx { re, im }
    }

    /// A purely real value.
    pub fn real(re: f64) -> Self {
        Cplx { re, im: 0.0 }
    }

    /// A purely imaginary value.
    pub fn imag(im: f64) -> Self {
        Cplx { re: 0.0, im }
    }

    /// Magnitude |z|.
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }

    /// Argument (phase) in radians.
    pub fn arg(self) -> f64 {
        self.im.atan2(self.re)
    }

    /// Complex conjugate.
    pub fn conj(self) -> Self {
        Cplx {
            re: self.re,
            im: -self.im,
        }
    }
}

impl std::ops::Add for Cplx {
    type Output = Cplx;
    fn add(self, o: Cplx) -> Cplx {
        Cplx::new(self.re + o.re, self.im + o.im)
    }
}

impl std::ops::Sub for Cplx {
    type Output = Cplx;
    fn sub(self, o: Cplx) -> Cplx {
        Cplx::new(self.re - o.re, self.im - o.im)
    }
}

impl std::ops::Mul for Cplx {
    type Output = Cplx;
    fn mul(self, o: Cplx) -> Cplx {
        Cplx::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
}

impl std::ops::Div for Cplx {
    type Output = Cplx;
    fn div(self, o: Cplx) -> Cplx {
        // Smith's algorithm — avoids overflow for extreme magnitudes.
        if o.re.abs() >= o.im.abs() {
            let r = o.im / o.re;
            let d = o.re + o.im * r;
            Cplx::new((self.re + self.im * r) / d, (self.im - self.re * r) / d)
        } else {
            let r = o.re / o.im;
            let d = o.re * r + o.im;
            Cplx::new((self.re * r + self.im) / d, (self.im * r - self.re) / d)
        }
    }
}

impl std::ops::Neg for Cplx {
    type Output = Cplx;
    fn neg(self) -> Cplx {
        Cplx::new(-self.re, -self.im)
    }
}

impl std::ops::AddAssign for Cplx {
    fn add_assign(&mut self, o: Cplx) {
        *self = *self + o;
    }
}

impl std::ops::SubAssign for Cplx {
    fn sub_assign(&mut self, o: Cplx) {
        *self = *self - o;
    }
}

/// Solve the complex system `a · x = b` (row-major `n×n`) by Gaussian
/// elimination with partial pivoting on |pivot|. Consumes both. Returns
/// `None` when singular.
pub fn solve_dense_c(a: &mut [Cplx], b: &mut [Cplx], n: usize) -> Option<Vec<Cplx>> {
    debug_assert_eq!(a.len(), n * n);
    debug_assert_eq!(b.len(), n);
    if n == 0 {
        return Some(Vec::new());
    }

    for col in 0..n {
        let mut pivot_row = col;
        let mut pivot_mag = a[col * n + col].abs();
        for r in (col + 1)..n {
            let mag = a[r * n + col].abs();
            if mag > pivot_mag {
                pivot_mag = mag;
                pivot_row = r;
            }
        }
        if pivot_mag < 1e-14 {
            return None;
        }
        if pivot_row != col {
            for c in 0..n {
                a.swap(pivot_row * n + c, col * n + c);
            }
            b.swap(pivot_row, col);
        }

        let pivot = a[col * n + col];
        for r in (col + 1)..n {
            let factor = a[r * n + col] / pivot;
            if factor != Cplx::ZERO {
                for c in col..n {
                    let sub = factor * a[col * n + c];
                    a[r * n + c] -= sub;
                }
                let sub = factor * b[col];
                b[r] -= sub;
            }
        }
    }

    let mut x = vec![Cplx::ZERO; n];
    for r in (0..n).rev() {
        let mut sum = b[r];
        for c in (r + 1)..n {
            sum -= a[r * n + c] * x[c];
        }
        x[r] = sum / a[r * n + r];
    }
    Some(x)
}

/// Why an AC solve failed.
#[derive(Debug, Clone, PartialEq)]
pub enum AcError {
    /// The underlying DC operating-point solve failed.
    Dc(DcError),
    /// The complex MNA matrix was singular at this frequency.
    Singular,
    /// `source` is not a `VSource`/`ISource` device id.
    BadSource,
    /// A device kind AC analysis does not support.
    Unsupported(&'static str),
}

impl std::fmt::Display for AcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcError::Dc(e) => write!(f, "DC operating point failed: {e}"),
            AcError::Singular => write!(f, "singular complex MNA system"),
            AcError::BadSource => write!(f, "AC source id is not an independent source"),
            AcError::Unsupported(what) => write!(f, "unsupported device in AC analysis: {what}"),
        }
    }
}

impl std::error::Error for AcError {}

impl From<DcError> for AcError {
    fn from(e: DcError) -> Self {
        AcError::Dc(e)
    }
}

/// Phasor node voltages at one frequency, driven by a unit-amplitude source.
#[derive(Debug, Clone, PartialEq)]
pub struct AcSolution {
    /// Angular frequency (rad/s) this solve ran at.
    pub omega: f64,
    /// Complex node voltages (`[0]` = ground = 0).
    pub node_voltages: Vec<Cplx>,
    /// Complex branch currents, one per branch device in device order
    /// (`VSource` and `Inductor` at AC).
    pub branch_currents: Vec<Cplx>,
}

/// The assembled complex MNA system plus its solution — shared between the
/// forward AC solve and the adjoint (which needs A, x, and the layout).
pub(crate) struct AcSystem {
    pub m: usize,
    pub nn: usize,
    /// Row-major complex MNA matrix (before the solve consumed a copy).
    pub a: Vec<Cplx>,
    /// Full solution vector (node voltages then branch currents).
    pub x: Vec<Cplx>,
    /// Branch index (into the branch block) per device id, for branch devices.
    pub branch_of: Vec<Option<usize>>,
}

/// Number of AC branch unknowns: voltage sources and inductors.
fn ac_branches(circuit: &Circuit) -> usize {
    circuit
        .devices
        .iter()
        .filter(|d| matches!(d, Device::VSource { .. } | Device::Inductor { .. }))
        .count()
}

/// Assemble and solve the complex MNA system at `omega`, driving device
/// `source` with unit amplitude.
pub(crate) fn build_and_solve(
    circuit: &Circuit,
    source: usize,
    omega: f64,
) -> Result<AcSystem, AcError> {
    if circuit
        .devices
        .iter()
        .any(|d| matches!(d, Device::Motor { .. }))
    {
        return Err(AcError::Unsupported("Motor"));
    }
    match circuit.devices.get(source) {
        Some(Device::VSource { .. }) | Some(Device::ISource { .. }) => {}
        _ => return Err(AcError::BadSource),
    }

    // Diode small-signal conductances need the DC operating point.
    let mut diode_g = vec![0.0f64; circuit.devices.len()];
    if circuit
        .devices
        .iter()
        .any(|d| matches!(d, Device::Diode { .. }))
    {
        let dc = operating_point(circuit)?;
        for (id, dev) in circuit.devices.iter().enumerate() {
            if let Device::Diode { p, n, model } = *dev {
                let vte = model.n * VT;
                let vd = dc.node_voltages[p] - dc.node_voltages[n];
                diode_g[id] = (model.is / vte) * (vd / vte).min(60.0).exp();
            }
        }
    }

    let nn = circuit.num_nodes;
    let nb = ac_branches(circuit);
    let m = (nn - 1) + nb;

    let mut a = vec![Cplx::ZERO; m * m];
    let mut b = vec![Cplx::ZERO; m];
    let mut branch_of = vec![None; circuit.devices.len()];
    let mut branch = 0usize;

    let stamp_g = |a: &mut [Cplx], p: usize, n: usize, g: Cplx| {
        let mut add = |i: usize, j: usize, v: Cplx| {
            if i != 0 && j != 0 {
                a[(i - 1) * m + (j - 1)] += v;
            }
        };
        add(p, p, g);
        add(p, n, -g);
        add(n, p, -g);
        add(n, n, g);
    };

    for (id, dev) in circuit.devices.iter().enumerate() {
        match *dev {
            Device::Resistor { p, n, r } => stamp_g(&mut a, p, n, Cplx::real(1.0 / r)),
            Device::Capacitor { p, n, c } => stamp_g(&mut a, p, n, Cplx::imag(omega * c)),
            Device::Diode { p, n, .. } => stamp_g(&mut a, p, n, Cplx::real(diode_g[id])),
            Device::Inductor { p, n, l } => {
                let br = (nn - 1) + branch;
                branch_of[id] = Some(branch);
                branch += 1;
                if p != 0 {
                    a[(p - 1) * m + br] += Cplx::ONE;
                    a[br * m + (p - 1)] += Cplx::ONE;
                }
                if n != 0 {
                    a[(n - 1) * m + br] -= Cplx::ONE;
                    a[br * m + (n - 1)] -= Cplx::ONE;
                }
                a[br * m + br] -= Cplx::imag(omega * l);
            }
            Device::VSource { p, n, .. } => {
                let br = (nn - 1) + branch;
                branch_of[id] = Some(branch);
                branch += 1;
                if p != 0 {
                    a[(p - 1) * m + br] += Cplx::ONE;
                    a[br * m + (p - 1)] += Cplx::ONE;
                }
                if n != 0 {
                    a[(n - 1) * m + br] -= Cplx::ONE;
                    a[br * m + (n - 1)] -= Cplx::ONE;
                }
                if id == source {
                    b[br] += Cplx::ONE;
                }
            }
            Device::ISource { p, n, .. } => {
                if id == source {
                    if p != 0 {
                        b[p - 1] += Cplx::ONE;
                    }
                    if n != 0 {
                        b[n - 1] -= Cplx::ONE;
                    }
                }
            }
            Device::Motor { .. } => unreachable!("rejected above"),
        }
    }

    let mut a_work = a.clone();
    let mut b_work = b.clone();
    let x = solve_dense_c(&mut a_work, &mut b_work, m).ok_or(AcError::Singular)?;

    Ok(AcSystem {
        m,
        nn,
        a,
        x,
        branch_of,
    })
}

/// Solve the small-signal response at angular frequency `omega` (rad/s),
/// driving device `source` (a `VSource` or `ISource`) with unit amplitude.
pub fn ac_response(circuit: &Circuit, source: usize, omega: f64) -> Result<AcSolution, AcError> {
    let sys = build_and_solve(circuit, source, omega)?;
    let nn = sys.nn;
    let mut node_voltages = vec![Cplx::ZERO; nn];
    node_voltages[1..nn].copy_from_slice(&sys.x[..(nn - 1)]);
    Ok(AcSolution {
        omega,
        node_voltages,
        branch_currents: sys.x[(nn - 1)..].to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rc_lowpass(r: f64, c: f64) -> (Circuit, usize, usize) {
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let out = ckt.node();
        let src = ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: 0.0,
        });
        ckt.add(Device::Resistor { p: vin, n: out, r });
        ckt.add(Device::Capacitor { p: out, n: 0, c });
        (ckt, src, out)
    }

    #[test]
    fn rc_lowpass_matches_analytic_transfer() {
        // H(jω) = 1/(1 + jωRC), checked at ω = 1/RC: |H| = 1/√2, phase −45°.
        let (ckt, src, out) = rc_lowpass(1_000.0, 1e-6);
        let omega = 1.0 / (1_000.0 * 1e-6);
        let sol = ac_response(&ckt, src, omega).unwrap();
        let h = sol.node_voltages[out];
        assert!((h.abs() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-12);
        assert!((h.arg() + std::f64::consts::FRAC_PI_4).abs() < 1e-12);
    }

    #[test]
    fn inductor_branch_handles_dc_limit() {
        // At ω = 0 the inductor is a short: out follows vin exactly.
        let mut ckt = Circuit::new();
        let vin = ckt.node();
        let out = ckt.node();
        let src = ckt.add(Device::VSource {
            p: vin,
            n: 0,
            v: 0.0,
        });
        ckt.add(Device::Inductor {
            p: vin,
            n: out,
            l: 1e-3,
        });
        ckt.add(Device::Resistor {
            p: out,
            n: 0,
            r: 50.0,
        });
        let sol = ac_response(&ckt, src, 0.0).unwrap();
        assert!((sol.node_voltages[out].abs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn complex_solver_agrees_with_real_solver_on_real_input() {
        let mut a = vec![
            Cplx::real(2.0),
            Cplx::real(1.0),
            Cplx::real(1.0),
            Cplx::real(3.0),
        ];
        let mut b = vec![Cplx::real(5.0), Cplx::real(10.0)];
        let x = solve_dense_c(&mut a, &mut b, 2).unwrap();
        assert!((x[0].re - 1.0).abs() < 1e-12 && x[0].im.abs() < 1e-15);
        assert!((x[1].re - 3.0).abs() < 1e-12 && x[1].im.abs() < 1e-15);
    }
}
