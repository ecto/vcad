//! Minimal periodic-table reference data: atomic number, mass (amu), covalent
//! radius (Å), van der Waals radius (Å), and a CPK-style sRGB color.
//!
//! Covers H–Kr plus a handful of common heavy elements — enough for organic,
//! materials, and biomolecular work. Unknown symbols fall back to carbon-like
//! defaults so importers never fail on an exotic element.

/// Reference data for one element.
#[derive(Debug, Clone, Copy)]
pub struct ElementData {
    /// Element symbol.
    pub symbol: &'static str,
    /// Atomic number.
    pub number: u32,
    /// Standard atomic mass in amu.
    pub mass: f64,
    /// Covalent radius in Å (used for bond perception).
    pub covalent_radius: f64,
    /// Van der Waals radius in Å (used for space-filling display).
    pub vdw_radius: f64,
    /// CPK-style color as sRGB in 0..1.
    pub color: [f64; 3],
}

const fn e(
    symbol: &'static str,
    number: u32,
    mass: f64,
    covalent_radius: f64,
    vdw_radius: f64,
    color: [f64; 3],
) -> ElementData {
    ElementData {
        symbol,
        number,
        mass,
        covalent_radius,
        vdw_radius,
        color,
    }
}

// CPK colors follow the common Jmol convention (normalized to 0..1).
const TABLE: &[ElementData] = &[
    e("H", 1, 1.008, 0.31, 1.20, [1.00, 1.00, 1.00]),
    e("He", 2, 4.0026, 0.28, 1.40, [0.85, 1.00, 1.00]),
    e("Li", 3, 6.94, 1.28, 1.82, [0.80, 0.50, 1.00]),
    e("Be", 4, 9.0122, 0.96, 1.53, [0.76, 1.00, 0.00]),
    e("B", 5, 10.81, 0.84, 1.92, [1.00, 0.71, 0.71]),
    e("C", 6, 12.011, 0.76, 1.70, [0.56, 0.56, 0.56]),
    e("N", 7, 14.007, 0.71, 1.55, [0.19, 0.31, 0.97]),
    e("O", 8, 15.999, 0.66, 1.52, [1.00, 0.05, 0.05]),
    e("F", 9, 18.998, 0.57, 1.47, [0.56, 0.88, 0.31]),
    e("Ne", 10, 20.180, 0.58, 1.54, [0.70, 0.89, 0.96]),
    e("Na", 11, 22.990, 1.66, 2.27, [0.67, 0.36, 0.95]),
    e("Mg", 12, 24.305, 1.41, 1.73, [0.54, 1.00, 0.00]),
    e("Al", 13, 26.982, 1.21, 1.84, [0.75, 0.65, 0.65]),
    e("Si", 14, 28.085, 1.11, 2.10, [0.94, 0.78, 0.63]),
    e("P", 15, 30.974, 1.07, 1.80, [1.00, 0.50, 0.00]),
    e("S", 16, 32.06, 1.05, 1.80, [1.00, 1.00, 0.19]),
    e("Cl", 17, 35.45, 1.02, 1.75, [0.12, 0.94, 0.12]),
    e("Ar", 18, 39.948, 1.06, 1.88, [0.50, 0.82, 0.89]),
    e("K", 19, 39.098, 2.03, 2.75, [0.56, 0.25, 0.83]),
    e("Ca", 20, 40.078, 1.76, 2.31, [0.24, 1.00, 0.00]),
    e("Sc", 21, 44.956, 1.70, 2.11, [0.90, 0.90, 0.90]),
    e("Ti", 22, 47.867, 1.60, 2.00, [0.75, 0.76, 0.78]),
    e("V", 23, 50.942, 1.53, 2.00, [0.65, 0.65, 0.67]),
    e("Cr", 24, 51.996, 1.39, 2.00, [0.54, 0.60, 0.78]),
    e("Mn", 25, 54.938, 1.39, 2.00, [0.61, 0.48, 0.78]),
    e("Fe", 26, 55.845, 1.32, 2.00, [0.88, 0.40, 0.20]),
    e("Co", 27, 58.933, 1.26, 2.00, [0.94, 0.56, 0.63]),
    e("Ni", 28, 58.693, 1.24, 1.63, [0.31, 0.82, 0.31]),
    e("Cu", 29, 63.546, 1.32, 1.40, [0.78, 0.50, 0.20]),
    e("Zn", 30, 65.38, 1.22, 1.39, [0.49, 0.50, 0.69]),
    e("Ga", 31, 69.723, 1.22, 1.87, [0.76, 0.56, 0.56]),
    e("Ge", 32, 72.630, 1.20, 2.11, [0.40, 0.56, 0.56]),
    e("As", 33, 74.922, 1.19, 1.85, [0.74, 0.50, 0.89]),
    e("Se", 34, 78.971, 1.20, 1.90, [1.00, 0.63, 0.00]),
    e("Br", 35, 79.904, 1.20, 1.85, [0.65, 0.16, 0.16]),
    e("Kr", 36, 83.798, 1.16, 2.02, [0.36, 0.72, 0.82]),
    // Common heavier elements for biomolecular / materials work.
    e("Mo", 42, 95.95, 1.54, 2.00, [0.33, 0.71, 0.71]),
    e("Ag", 47, 107.87, 1.45, 1.72, [0.75, 0.75, 0.75]),
    e("Cd", 48, 112.41, 1.44, 1.58, [1.00, 0.85, 0.56]),
    e("Sn", 50, 118.71, 1.39, 2.17, [0.40, 0.50, 0.50]),
    e("I", 53, 126.90, 1.39, 1.98, [0.58, 0.00, 0.58]),
    e("Pt", 78, 195.08, 1.36, 1.75, [0.82, 0.82, 0.88]),
    e("Au", 79, 196.97, 1.36, 1.66, [1.00, 0.82, 0.14]),
    e("Hg", 80, 200.59, 1.32, 1.55, [0.72, 0.72, 0.82]),
    e("Pb", 82, 207.2, 1.46, 2.02, [0.34, 0.35, 0.38]),
    e("U", 92, 238.03, 1.96, 1.86, [0.00, 0.56, 0.00]),
];

/// Fallback used when a symbol is unknown (carbon-like).
const FALLBACK: ElementData = e("X", 0, 12.011, 0.76, 1.70, [0.90, 0.40, 0.90]);

/// Look up element data by symbol (case-insensitive on the first letter's
/// convention, e.g. "fe", "FE", and "Fe" all resolve to iron). Returns a
/// carbon-like fallback for unknown symbols.
pub fn lookup(symbol: &str) -> ElementData {
    let norm = normalize_symbol(symbol);
    TABLE
        .iter()
        .find(|d| d.symbol == norm)
        .copied()
        .unwrap_or(FALLBACK)
}

/// Look up element data by atomic number.
pub fn lookup_number(number: u32) -> Option<ElementData> {
    TABLE.iter().find(|d| d.number == number).copied()
}

/// Normalize an element symbol to canonical case: first letter uppercase, rest
/// lowercase (e.g. "NA" -> "Na", "cl" -> "Cl"). Strips trailing digits/labels
/// (e.g. "C1", "OW" -> "O" only when the two-letter form is unknown).
pub fn normalize_symbol(symbol: &str) -> String {
    let trimmed: String = symbol
        .chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut chars = trimmed.chars();
    let first = chars.next().unwrap().to_ascii_uppercase();
    let rest: String = chars.map(|c| c.to_ascii_lowercase()).collect();
    let two = format!("{first}{rest}");
    // Prefer the full two-letter symbol if it's a real element; otherwise fall
    // back to just the first letter (handles PDB atom names like "CA", "OW").
    if TABLE.iter().any(|d| d.symbol == two) {
        two
    } else {
        first.to_string()
    }
}
