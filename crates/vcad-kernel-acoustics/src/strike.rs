//! Structural strike simulation — the solid half of the acoustics seam.
//!
//! Models a mallet strike on a flat free-free bar (a glockenspiel /
//! vibraphone bar): modal frequencies from both the closed-form
//! Euler–Bernoulli model and a hole-aware 1-D Hermite-beam FEM, strike-excited
//! modal synthesis, a WAV encoder, and an FFT round trip for the pitch
//! verdict. This is the *structure-side* solver the crate-level docs describe
//! as `simulate_strike`; the air-side field solve lives in [`crate::helmholtz`]
//! and coupling the two (surface velocity in, pressure out) is M2.
//!
//! Ported from the MCP server's TypeScript implementation; the pinned tests at
//! the bottom reproduce that implementation's physics receipts (exact
//! free-free eigenvalue roots, FEM-vs-closed-form agreement, non-harmonic
//! overtone ratios, a synth→FFT round trip accurate to a cent) so the two
//! stay interchangeable.
//!
//! The generalized symmetric eigensolve here (dense Cholesky + cyclic Jacobi)
//! is real-valued and tiny (~200 DOF), deliberately separate from the complex
//! LU in [`crate::linalg`] which serves the indefinite Helmholtz operator.

use serde::{Deserialize, Serialize};

// ─── Free-free beam modal math ────────────────────────────────────────────

/// Roots of `cosh(x)·cos(x) = 1` (free-free beam), Newton-refined.
pub fn free_free_beta_l(count: usize) -> Vec<f64> {
    let mut roots = Vec::with_capacity(count);
    for n in 1..=count {
        // Asymptotic start: (2n+1)π/2.
        let mut x = (2 * n + 1) as f64 * std::f64::consts::PI / 2.0;
        for _ in 0..50 {
            let f = x.cosh() * x.cos() - 1.0;
            let df = x.sinh() * x.cos() - x.cosh() * x.sin();
            let step = f / df;
            x -= step;
            if step.abs() < 1e-13 {
                break;
            }
        }
        roots.push(x);
    }
    roots
}

/// Free-free mode shape φₙ(ξ), ξ = x/L in `[0, 1]`, normalized to max |φ| = 1.
///
/// φ = cosh(βx) + cos(βx) − σ(sinh(βx) + sin(βx)).
pub fn mode_shape(beta_l: f64, xi: f64) -> f64 {
    let sigma = (beta_l.cosh() - beta_l.cos()) / (beta_l.sinh() - beta_l.sin());
    let raw = |t: f64| {
        (beta_l * t).cosh() + (beta_l * t).cos()
            - sigma * ((beta_l * t).sinh() + (beta_l * t).sin())
    };
    // Max |φ| is at the free ends (= 2 in this normalization).
    raw(xi) / raw(0.0).abs()
}

/// Flat free-free bar: geometry in millimeters, material in SI-adjacent units.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BarSpec {
    /// Bar length (mm).
    pub length_mm: f64,
    /// Bar width (mm).
    pub width_mm: f64,
    /// Bar thickness (mm).
    pub thickness_mm: f64,
    /// Cord-hole centers along the bar axis (mm from one end).
    #[serde(default)]
    pub holes_mm: Vec<f64>,
    /// Cord-hole diameter (mm).
    #[serde(default)]
    pub hole_dia_mm: f64,
    /// Young's modulus (GPa).
    pub modulus_gpa: f64,
    /// Density (kg/m³).
    pub density_kg_m3: f64,
}

/// Closed-form modal frequencies (Hz) for the uniform bar.
pub fn closed_form_hz(bar: &BarSpec, count: usize) -> Vec<f64> {
    let l = bar.length_mm / 1000.0;
    let t = bar.thickness_mm / 1000.0;
    let e = bar.modulus_gpa * 1e9;
    // I/A = t²/12 → √(EI/ρA) = t·√(E/12ρ).
    let c = t * (e / (12.0 * bar.density_kg_m3)).sqrt();
    free_free_beta_l(count)
        .iter()
        .map(|bl| bl * bl * c / (2.0 * std::f64::consts::PI * l * l))
        .collect()
}

// ─── Hole-aware 1-D FEM (Hermite beam elements) ───────────────────────────

/// Material width through the cord holes at axial station `x_mm` (mm).
fn effective_width_mm(bar: &BarSpec, x_mm: f64) -> f64 {
    let mut w = bar.width_mm;
    let r = bar.hole_dia_mm / 2.0;
    for &h in &bar.holes_mm {
        let d = (x_mm - h).abs();
        if d < r {
            w -= 2.0 * (r * r - d * d).sqrt();
        }
    }
    w.max(1e-6)
}

/// Lowest `count` elastic modal frequencies (Hz) from a free-free Hermite
/// beam FEM with per-Gauss-point section properties. The two rigid-body
/// modes are discarded. Dense symmetric eigensolve (Cholesky + Jacobi) —
/// fine at the ~200-DOF meshes this uses.
pub fn fem_hz(bar: &BarSpec, count: usize) -> Vec<f64> {
    fem_hz_with_mesh(bar, count, 96)
}

/// [`fem_hz`] with an explicit element count.
pub fn fem_hz_with_mesh(bar: &BarSpec, count: usize, nel: usize) -> Vec<f64> {
    let l = bar.length_mm / 1000.0;
    let t = bar.thickness_mm / 1000.0;
    let e_mod = bar.modulus_gpa * 1e9;
    let rho = bar.density_kg_m3;
    let n_dof = 2 * (nel + 1);
    let mut k_mat = vec![0.0_f64; n_dof * n_dof];
    let mut m_mat = vec![0.0_f64; n_dof * n_dof];
    let le = l / nel as f64;

    // 4-point Gauss on [-1, 1].
    const GP: [f64; 4] = [
        -0.861_136_311_594_052_6,
        -0.339_981_043_584_856_3,
        0.339_981_043_584_856_3,
        0.861_136_311_594_052_6,
    ];
    const GW: [f64; 4] = [
        0.347_854_845_137_453_8,
        0.652_145_154_862_546_1,
        0.652_145_154_862_546_1,
        0.347_854_845_137_453_8,
    ];

    for e in 0..nel {
        let x0 = e as f64 * le;
        let mut ke = [0.0_f64; 16];
        let mut me = [0.0_f64; 16];
        for g in 0..4 {
            let xi = GP[g]; // element coordinate in [-1, 1]
            let x = x0 + (xi + 1.0) / 2.0 * le; // global (m)
            let w_eff = effective_width_mm(bar, x * 1000.0) / 1000.0; // m
            let a = w_eff * t;
            let i_sec = w_eff * t * t * t / 12.0;
            // Hermite shape functions on [-1,1] (rotational DOFs scaled by le/2).
            let s = le / 2.0;
            let n = [
                0.25 * (1.0 - xi) * (1.0 - xi) * (2.0 + xi),
                s * 0.25 * (1.0 - xi) * (1.0 - xi) * (1.0 + xi),
                0.25 * (1.0 + xi) * (1.0 + xi) * (2.0 - xi),
                s * 0.25 * (1.0 + xi) * (1.0 + xi) * (xi - 1.0),
            ];
            // Second derivatives d²N/dx² = d²N/dξ² · (2/le)².
            let c2 = (2.0 / le) * (2.0 / le);
            let b = [
                1.5 * xi * c2,
                s * (1.5 * xi - 0.5) * c2,
                -1.5 * xi * c2,
                s * (1.5 * xi + 0.5) * c2,
            ];
            let w_j = GW[g] * s; // Gauss weight × Jacobian
            for i in 0..4 {
                for j in 0..4 {
                    ke[i * 4 + j] += e_mod * i_sec * b[i] * b[j] * w_j;
                    me[i * 4 + j] += rho * a * n[i] * n[j] * w_j;
                }
            }
        }
        let dof = [2 * e, 2 * e + 1, 2 * e + 2, 2 * e + 3];
        for i in 0..4 {
            for j in 0..4 {
                k_mat[dof[i] * n_dof + dof[j]] += ke[i * 4 + j];
                m_mat[dof[i] * n_dof + dof[j]] += me[i * 4 + j];
            }
        }
    }

    // Generalized symmetric eigenproblem K φ = ω² M φ via M = LLᵀ,
    // C = L⁻¹ K L⁻ᵀ (symmetric), then cyclic Jacobi.
    let l_chol = cholesky(&m_mat, n_dof);
    let c = congruence(&k_mat, &l_chol, n_dof);
    let mut lambda = jacobi_eigenvalues(c, n_dof);
    lambda.sort_by(|a, b| a.partial_cmp(b).unwrap());
    // Drop rigid-body modes (λ ≈ 0; threshold at (2π·1 Hz)²).
    let floor = (2.0 * std::f64::consts::PI).powi(2);
    lambda
        .into_iter()
        .filter(|&l| l > floor)
        .take(count)
        .map(|l| l.sqrt() / (2.0 * std::f64::consts::PI))
        .collect()
}

/// Lower-triangular Cholesky factor of an SPD matrix (row-major `n×n`).
///
/// Panics if the matrix is not positive definite — the consistent Hermite
/// mass matrix always is, so a failure here is a caller bug, not an input
/// condition.
fn cholesky(a: &[f64], n: usize) -> Vec<f64> {
    let mut l = vec![0.0_f64; n * n];
    for i in 0..n {
        for j in 0..=i {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= l[i * n + k] * l[j * n + k];
            }
            if i == j {
                assert!(s > 0.0, "mass matrix not positive definite");
                l[i * n + i] = s.sqrt();
            } else {
                l[i * n + j] = s / l[j * n + j];
            }
        }
    }
    l
}

/// C = L⁻¹ K L⁻ᵀ (forward/back substitution, keeps symmetry).
fn congruence(k: &[f64], l: &[f64], n: usize) -> Vec<f64> {
    // Y = L⁻¹ K (solve L·Y = K column-wise via forward substitution on rows).
    let mut y = vec![0.0_f64; n * n];
    for c in 0..n {
        for i in 0..n {
            let mut s = k[i * n + c];
            for kk in 0..i {
                s -= l[i * n + kk] * y[kk * n + c];
            }
            y[i * n + c] = s / l[i * n + i];
        }
    }
    // C = Y L⁻ᵀ → Cᵀ = L⁻¹ Yᵀ, and C symmetric.
    let mut c_mat = vec![0.0_f64; n * n];
    for r in 0..n {
        for i in 0..n {
            let mut s = y[r * n + i];
            for kk in 0..i {
                s -= l[i * n + kk] * c_mat[r * n + kk];
            }
            c_mat[r * n + i] = s / l[i * n + i];
        }
    }
    // Symmetrize against round-off.
    for i in 0..n {
        for j in 0..i {
            let v = 0.5 * (c_mat[i * n + j] + c_mat[j * n + i]);
            c_mat[i * n + j] = v;
            c_mat[j * n + i] = v;
        }
    }
    c_mat
}

/// Eigenvalues of a symmetric matrix by cyclic Jacobi rotations (consumes
/// the row-major `n×n` matrix).
fn jacobi_eigenvalues(mut a: Vec<f64>, n: usize) -> Vec<f64> {
    for _ in 0..30 {
        let mut off = 0.0;
        for i in 0..n {
            for j in (i + 1)..n {
                off += a[i * n + j] * a[i * n + j];
            }
        }
        if off < 1e-18 * (n * n) as f64 {
            break;
        }
        for p in 0..n.saturating_sub(1) {
            for q in (p + 1)..n {
                if a[p * n + q].abs() < 1e-30 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * a[p * n + q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let akp = a[k * n + p];
                    let akq = a[k * n + q];
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[p * n + k];
                    let aqk = a[q * n + k];
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
            }
        }
    }
    (0..n).map(|i| a[i * n + i]).collect()
}

// ─── Strike model, synthesis, spectrum ────────────────────────────────────

/// One strike-excited mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mode {
    /// Mode number (1-based).
    pub n: usize,
    /// Frequency (Hz).
    pub hz: f64,
    /// Linear amplitude, ≤ 1 (mode shape at strike × mallet spectrum).
    pub gain: f64,
    /// Quality factor (material + suspension heuristic).
    pub q: f64,
    /// Time to −60 dB (s).
    pub t60_s: f64,
}

/// Half-sine force pulse spectrum magnitude, normalized to 1 at DC.
pub fn mallet_spectrum(hz: f64, contact_ms: f64) -> f64 {
    let tc = contact_ms / 1000.0;
    let u = 2.0 * hz * tc;
    // |F̂(f)| / |F̂(0)| for a half-sine of duration tc.
    let denom = (1.0 - u * u).abs();
    if denom < 1e-9 {
        return std::f64::consts::PI / 4.0; // removable singularity at u = 1
    }
    (std::f64::consts::PI * hz * tc).cos().abs() / denom
}

/// Build the strike-excited modal set (gains normalized to the loudest).
pub fn strike_modes(
    bar: &BarSpec,
    fem_freqs: &[f64],
    strike_frac: f64,
    contact_ms: f64,
    material_q: f64,
    suspension_q0: f64,
) -> Vec<Mode> {
    let betas = free_free_beta_l(fem_freqs.len());
    let mut modes: Vec<Mode> = fem_freqs
        .iter()
        .enumerate()
        .map(|(i, &hz)| {
            let phi_strike = mode_shape(betas[i], strike_frac);
            let gain = phi_strike.abs() * mallet_spectrum(hz, contact_ms);
            // Suspension damping: cord at the holes bleeds energy ∝ φₙ² there.
            // Mode 1 has φ ≈ 0 at the nodal holes — that's why they're there.
            let mut phi_sq = 0.0;
            for &h in &bar.holes_mm {
                let phi = mode_shape(betas[i], h / bar.length_mm);
                phi_sq += phi * phi;
            }
            let q_susp = suspension_q0 / phi_sq.max(1e-9);
            let q = 1.0 / (1.0 / material_q + 1.0 / q_susp);
            Mode {
                n: i + 1,
                hz,
                gain,
                q,
                t60_s: 1000.0_f64.ln() * q / (std::f64::consts::PI * hz),
            }
        })
        .collect();
    let peak = modes.iter().map(|m| m.gain).fold(1e-12_f64, f64::max);
    for m in &mut modes {
        m.gain /= peak;
    }
    modes
}

/// Default material Q (matches the TS implementation's default).
pub const DEFAULT_MATERIAL_Q: f64 = 2500.0;
/// Default suspension Q scale (matches the TS implementation's default).
pub const DEFAULT_SUSPENSION_Q0: f64 = 150.0;

/// Sum of exponentially decaying sinusoids, peak-normalized to −1 dBFS.
pub fn synthesize(modes: &[Mode], duration_s: f64, sample_rate: f64) -> Vec<f64> {
    let n = (duration_s * sample_rate) as usize;
    let mut out = vec![0.0_f64; n];
    for m in modes {
        if m.hz >= 0.45 * sample_rate {
            continue;
        }
        let w = 2.0 * std::f64::consts::PI * m.hz / sample_rate;
        let tau = m.q / (std::f64::consts::PI * m.hz); // amplitude time constant (s)
        let decay = (-1.0 / (tau * sample_rate)).exp();
        // Recursive oscillator: amp·decayᵏ·sin(w·k).
        let mut amp = m.gain;
        for (k, o) in out.iter_mut().enumerate() {
            *o += amp * (w * k as f64).sin();
            amp *= decay;
        }
    }
    let peak = out.iter().fold(1e-12_f64, |p, &v| p.max(v.abs()));
    let norm = 0.891 / peak; // −1 dBFS
    for o in &mut out {
        *o *= norm;
    }
    out
}

/// Encode mono float samples as a 16-bit PCM WAV.
pub fn encode_wav(samples: &[f64], sample_rate: u32) -> Vec<u8> {
    let n = samples.len();
    let mut buf = Vec::with_capacity(44 + n * 2);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + n as u32 * 2).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&(n as u32 * 2).to_le_bytes());
    for &s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        buf.extend_from_slice(&v.to_le_bytes());
    }
    buf
}

/// In-place radix-2 FFT (`re`, `im` length must be a power of two).
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let wr = ang.cos();
        let wi = ang.sin();
        let mut i = 0;
        while i < n {
            let mut cr = 1.0;
            let mut ci = 0.0;
            for k in 0..len / 2 {
                let ur = re[i + k];
                let ui = im[i + k];
                let vr = re[i + k + len / 2] * cr - im[i + k + len / 2] * ci;
                let vi = re[i + k + len / 2] * ci + im[i + k + len / 2] * cr;
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// One spectral peak from the synthesized strike.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpectralPeak {
    /// Peak frequency (Hz), sub-bin via parabolic interpolation.
    pub hz: f64,
    /// Level relative to the strongest bin (dB, ≤ 0).
    pub db: f64,
}

/// Hann-windowed FFT of the first 2¹⁶ samples, top peaks by local maxima
/// with parabolic interpolation on log magnitude (sub-bin accuracy).
pub fn spectrum_peaks(samples: &[f64], sample_rate: f64, max_peaks: usize) -> Vec<SpectralPeak> {
    let n = 65536usize.min(1 << (samples.len() as f64).log2().floor() as usize);
    let mut re = vec![0.0_f64; n];
    let mut im = vec![0.0_f64; n];
    for i in 0..n {
        let w = 0.5 * (1.0 - (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos());
        re[i] = samples[i] * w;
    }
    fft(&mut re, &mut im);
    let half = n / 2;
    let mut mag = vec![0.0_f64; half];
    let mut max_mag = 1e-30_f64;
    for i in 0..half {
        mag[i] = re[i].hypot(im[i]);
        max_mag = max_mag.max(mag[i]);
    }
    let mut peaks: Vec<SpectralPeak> = Vec::new();
    let floor_db = -60.0;
    for i in 2..half - 2 {
        if mag[i] <= mag[i - 1] || mag[i] < mag[i + 1] {
            continue;
        }
        let db = 20.0 * (mag[i] / max_mag).log10();
        if db < floor_db {
            continue;
        }
        // Parabolic interpolation on log magnitude.
        let a = (mag[i - 1] + 1e-30).ln();
        let b = (mag[i] + 1e-30).ln();
        let c = (mag[i + 1] + 1e-30).ln();
        let mut denom = a - 2.0 * b + c;
        if denom == 0.0 {
            denom = 1e-30;
        }
        let delta = 0.5 * (a - c) / denom;
        peaks.push(SpectralPeak {
            hz: (i as f64 + delta) * sample_rate / n as f64,
            db,
        });
    }
    peaks.sort_by(|a, b| b.db.partial_cmp(&a.db).unwrap());
    // Suppress shoulders: keep peaks at least 3% apart in frequency.
    let mut kept: Vec<SpectralPeak> = Vec::new();
    for p in peaks {
        if kept.iter().all(|k| (p.hz - k.hz).abs() / k.hz > 0.03) {
            kept.push(p);
        }
        if kept.len() >= max_peaks {
            break;
        }
    }
    kept.sort_by(|a, b| a.hz.partial_cmp(&b.hz).unwrap());
    kept
}

/// Interval between two frequencies in cents.
pub fn cents(a: f64, b: f64) -> f64 {
    1200.0 * (a / b).log2()
}

/// `"C6"`, `"F#4"`, `"Bb3"` → Hz (equal temperament, A4 = 440).
pub fn note_to_hz(note: &str) -> Result<f64, String> {
    let s = note.trim();
    let bytes: Vec<char> = s.chars().collect();
    let err = || format!("unparseable note {s:?} — use e.g. \"C6\", \"F#4\"");
    if bytes.len() < 2 {
        return Err(err());
    }
    let letter = bytes[0].to_ascii_uppercase();
    let base = match letter {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return Err(err()),
    };
    let mut idx = 1;
    let mut semis = base;
    if bytes[idx] == '#' {
        semis += 1;
        idx += 1;
    } else if bytes[idx] == 'b' {
        semis -= 1;
        idx += 1;
    }
    let octave: i32 = s[idx..].parse().map_err(|_| err())?;
    if !(-9..=9).contains(&octave) {
        return Err(err());
    }
    let midi = (octave + 1) * 12 + semis;
    Ok(440.0 * 2.0_f64.powf((midi - 69) as f64 / 12.0))
}

// ─── One-call strike pipeline ─────────────────────────────────────────────

/// Everything `simulate_strike` needs to compute, in one call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrikeInput {
    /// The bar (geometry + material).
    pub bar: BarSpec,
    /// Strike point as a fraction of bar length, 0..1.
    pub strike_position: f64,
    /// Mallet contact time (ms).
    pub mallet_contact_ms: f64,
    /// Length of the synthesized strike (s).
    pub duration_s: f64,
    /// Synthesis sample rate (Hz).
    pub sample_rate: u32,
    /// How many modes to compute.
    pub n_modes: usize,
    /// Target fundamental (Hz) for the verdict, if any.
    #[serde(default)]
    pub expected_hz: Option<f64>,
    /// Pass/fail tolerance for the verdict (cents).
    #[serde(default = "default_tolerance_cents")]
    pub tolerance_cents: f64,
    /// Whether to encode and return the WAV bytes.
    #[serde(default)]
    pub include_wav: bool,
}

fn default_tolerance_cents() -> f64 {
    10.0
}

/// Pitch verdict: dominant spectral peak vs the expected fundamental.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrikeVerdict {
    /// Target fundamental (Hz).
    pub expected_hz: f64,
    /// Dominant FFT peak (Hz).
    pub measured_hz: f64,
    /// Measured − expected, in cents.
    pub cents_error: f64,
    /// Tolerance the verdict was judged against (cents).
    pub tolerance_cents: f64,
    /// |cents_error| ≤ tolerance.
    pub pass: bool,
}

/// Full result of the strike pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrikeResult {
    /// Closed-form (uniform-section) modal frequencies (Hz).
    pub closed_form_hz: Vec<f64>,
    /// Hole-aware FEM modal frequencies (Hz).
    pub fem_hz: Vec<f64>,
    /// Audible strike-excited modes (gain > 1e-4, below 0.45·fs), the
    /// synthesis input and the client-side Web Audio data contract.
    pub modes: Vec<Mode>,
    /// FFT peaks of the synthesized strike.
    pub spectrum_peaks: Vec<SpectralPeak>,
    /// Pitch verdict, when `expected_hz` was given.
    pub verdict: Option<StrikeVerdict>,
    /// 16-bit PCM WAV bytes, when `include_wav` was set.
    pub wav: Option<Vec<u8>>,
}

/// Run the whole strike pipeline: closed form + FEM → strike-excited modes →
/// synthesis → FFT peaks → verdict (and optionally the WAV).
pub fn simulate_strike(input: &StrikeInput) -> StrikeResult {
    let sample_rate = input.sample_rate as f64;
    let closed = closed_form_hz(&input.bar, input.n_modes);
    let fem = fem_hz(&input.bar, input.n_modes);
    let modes = strike_modes(
        &input.bar,
        &fem,
        input.strike_position,
        input.mallet_contact_ms,
        DEFAULT_MATERIAL_Q,
        DEFAULT_SUSPENSION_Q0,
    );
    let audible: Vec<Mode> = modes
        .into_iter()
        .filter(|m| m.hz < 0.45 * sample_rate && m.gain > 1e-4)
        .collect();
    let samples = synthesize(&audible, input.duration_s, sample_rate);
    let peaks = spectrum_peaks(&samples, sample_rate, 8);

    let verdict = input.expected_hz.map(|expected| {
        let dominant = peaks
            .iter()
            .fold((0.0_f64, f64::NEG_INFINITY), |best, p| {
                if p.db > best.1 {
                    (p.hz, p.db)
                } else {
                    best
                }
            })
            .0;
        let err = cents(dominant, expected);
        StrikeVerdict {
            expected_hz: expected,
            measured_hz: dominant,
            cents_error: err,
            tolerance_cents: input.tolerance_cents,
            pass: err.abs() <= input.tolerance_cents,
        }
    });

    let wav = input
        .include_wav
        .then(|| encode_wav(&samples, input.sample_rate));

    StrikeResult {
        closed_form_hz: closed,
        fem_hz: fem,
        modes: audible,
        spectrum_peaks: peaks,
        verdict,
        wav,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c6_bar() -> BarSpec {
        BarSpec {
            length_mm: 125.6,
            width_mm: 25.0,
            thickness_mm: 3.175,
            holes_mm: vec![28.16, 97.44],
            hole_dia_mm: 4.2,
            modulus_gpa: 69.0,
            density_kg_m3: 2700.0,
        }
    }

    fn uniform_bar() -> BarSpec {
        BarSpec {
            holes_mm: vec![],
            hole_dia_mm: 0.0,
            ..c6_bar()
        }
    }

    #[test]
    fn solves_the_cosh_cos_eigenvalue_roots() {
        let bl = free_free_beta_l(4);
        assert!((bl[0] - 4.730_040_74).abs() < 1e-6);
        assert!((bl[1] - 7.853_204_62).abs() < 1e-6);
        assert!((bl[2] - 10.995_607_84).abs() < 1e-6);
        assert!((bl[3] - 14.137_165_49).abs() < 1e-6);
    }

    #[test]
    fn mode_1_nodal_line_sits_at_0_2242_l() {
        let bl = free_free_beta_l(1)[0];
        // φ₁ changes sign across the node — where the cord holes go.
        assert!(mode_shape(bl, 0.2241) * mode_shape(bl, 0.2243) < 0.0);
    }

    #[test]
    fn closed_form_matches_spec_f1_for_6061() {
        // f₁ ≈ 16.50/L² for 6061 at 3.175 mm.
        let f1 = closed_form_hz(&uniform_bar(), 1)[0];
        assert!((f1 - 16.498 / (0.1256_f64 * 0.1256)).abs() < 0.5, "f1={f1}");
    }

    #[test]
    fn fem_reproduces_closed_form_on_uniform_bar_sub_cent() {
        let closed = closed_form_hz(&uniform_bar(), 3);
        let fem = fem_hz(&uniform_bar(), 3);
        for i in 0..3 {
            assert!(
                cents(fem[i], closed[i]).abs() < 0.5,
                "mode {i}: fem={} closed={}",
                fem[i],
                closed[i]
            );
        }
    }

    #[test]
    fn fem_recovers_non_harmonic_overtone_ratios() {
        let fem = fem_hz(&uniform_bar(), 3);
        assert!((fem[1] / fem[0] - 2.7565).abs() < 5e-4);
        assert!((fem[2] / fem[0] - 5.4039).abs() < 5e-4);
    }

    #[test]
    fn nodal_cord_holes_flatten_f1_slightly() {
        let plain = fem_hz(&uniform_bar(), 1)[0];
        let holed = fem_hz(&c6_bar(), 1)[0];
        let shift = cents(holed, plain);
        assert!(shift < -1.0, "shift={shift}"); // flat, not sharp
        assert!(shift > -20.0, "shift={shift}"); // and small
    }

    #[test]
    fn strike_round_trips_through_fft_within_a_cent() {
        let bar = c6_bar();
        let fem = fem_hz(&bar, 6);
        let modes = strike_modes(
            &bar,
            &fem,
            0.5,
            0.5,
            DEFAULT_MATERIAL_Q,
            DEFAULT_SUSPENSION_Q0,
        );
        let audible: Vec<Mode> = modes.into_iter().filter(|m| m.gain > 1e-4).collect();
        let samples = synthesize(&audible, 2.5, 44100.0);
        let peaks = spectrum_peaks(&samples, 44100.0, 8);
        let dominant = peaks
            .iter()
            .max_by(|a, b| a.db.partial_cmp(&b.db).unwrap())
            .unwrap();
        assert!(cents(dominant.hz, fem[0]).abs() < 1.0);
    }

    #[test]
    fn center_strike_suppresses_antisymmetric_partial() {
        let bar = c6_bar();
        let fem = fem_hz(&bar, 3);
        let modes = strike_modes(
            &bar,
            &fem,
            0.5,
            0.5,
            DEFAULT_MATERIAL_Q,
            DEFAULT_SUSPENSION_Q0,
        );
        assert!(modes[1].gain < 0.01); // node of mode 2 at center
        let off = strike_modes(
            &bar,
            &fem,
            0.3,
            0.5,
            DEFAULT_MATERIAL_Q,
            DEFAULT_SUSPENSION_Q0,
        );
        assert!(off[1].gain > 0.05);
    }

    #[test]
    fn mode_1_rings_long_higher_partials_die_fast() {
        let bar = c6_bar();
        let fem = fem_hz(&bar, 3);
        let modes = strike_modes(
            &bar,
            &fem,
            0.5,
            0.5,
            DEFAULT_MATERIAL_Q,
            DEFAULT_SUSPENSION_Q0,
        );
        assert!(modes[0].t60_s > 2.0);
        assert!(modes[2].t60_s < 0.5);
    }

    #[test]
    fn parses_note_names() {
        assert!((note_to_hz("A4").unwrap() - 440.0).abs() < 1e-6);
        assert!((note_to_hz("C6").unwrap() - 1046.502).abs() < 5e-3);
        assert!((note_to_hz("F#4").unwrap() - 369.994).abs() < 5e-3);
        assert!((note_to_hz("Bb3").unwrap() - 233.082).abs() < 5e-3);
        assert!(note_to_hz("H9").is_err());
        assert!(note_to_hz("C").is_err());
    }

    #[test]
    fn wav_encoder_writes_a_valid_header() {
        let wav = encode_wav(&[0.0, 0.5, -0.5, 1.0], 44100);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(wav.len(), 44 + 8);
        // Full-scale sample clamps to 32767.
        assert_eq!(i16::from_le_bytes([wav[50], wav[51]]), 32767);
    }

    #[test]
    fn full_pipeline_verdict_passes_for_a_c6_bar() {
        // The end-to-end precedent: the glockenspiel C6 bar, verified in TS
        // to land flat-but-inside-tolerance against 1046.5 Hz.
        let result = simulate_strike(&StrikeInput {
            bar: c6_bar(),
            strike_position: 0.5,
            mallet_contact_ms: 0.5,
            duration_s: 2.5,
            sample_rate: 44100,
            n_modes: 6,
            expected_hz: Some(note_to_hz("C6").unwrap()),
            tolerance_cents: 10.0,
            include_wav: true,
        });
        let v = result.verdict.as_ref().unwrap();
        assert!(v.pass, "cents_error={}", v.cents_error);
        assert!(v.cents_error < 0.0 && v.cents_error > -10.0);
        // Hole shift: FEM f₁ sits below the closed form.
        let shift = cents(result.fem_hz[0], result.closed_form_hz[0]);
        assert!(shift < -1.0);
        // Overtone ratio just under the uniform 2.7565.
        let ratio = result.fem_hz[1] / result.fem_hz[0];
        assert!((ratio - 2.7565).abs() < 0.05);
        let wav = result.wav.as_ref().unwrap();
        assert_eq!(wav.len(), 44 + 2 * (2.5 * 44100.0) as usize);
    }
}
