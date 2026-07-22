//! Suite E oracle: refdes-agnostic netlist graph isomorphism.
//!
//! The golden netlist is a grader-only task input (kind
//! `"golden_netlist"`), a small JSON file:
//!
//! ```json
//! { "components": { "R1": "R", "T1": "J" },
//!   "nets": { "VIN": ["T1.1", "R1.1"], "GND": ["R1.2"] } }
//! ```
//!
//! Golden refs and net names are labels only — the candidate is matched
//! structurally. A component's identity is its **class** (classified from
//! footprint id + value: R, C, L, LED, D, J, B, U) plus the multiset of
//! nets its pins touch; pin *order* within a component is deliberately
//! ignored (R and C are symmetric; for polarized parts the surrounding
//! topology disambiguates in practice at task sizes).
//!
//! Matching runs Weisfeiler-Lehman color refinement on the bipartite
//! component↔net graph, then verifies exactly with a backtracking net
//! bijection inside color classes — no similarity scores, iso or not.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

/// The golden netlist file format.
#[derive(Debug, Deserialize)]
pub struct GoldenNetlist {
    /// ref → class label ("R", "C", "LED", "J", "B", "U", …).
    pub components: BTreeMap<String, String>,
    /// net name → pin refs ("REF.PIN").
    pub nets: BTreeMap<String, Vec<String>>,
}

/// One side of the comparison, reduced to structure: components are
/// (class, sorted net-index list w/ multiplicity); nets are index-labeled.
#[derive(Debug, Clone)]
pub struct NetGraph {
    /// Per component: (class, sorted net indices its pins touch).
    pub comps: Vec<(String, Vec<usize>)>,
    /// Net count (indices 0..n).
    pub net_count: usize,
    /// Net display names (for diagnostics).
    pub net_names: Vec<String>,
}

/// Classify a candidate component from footprint id + value. Deterministic
/// and documented in SCHEMA.md; unknown parts fall through to "U".
pub fn classify(footprint_id: &str, value: &str) -> String {
    let f = footprint_id.to_ascii_lowercase();
    let v = value.to_ascii_lowercase();
    let hay = format!("{f} {v}");
    if hay.contains("led") {
        return "LED".into();
    }
    if f.contains("resistor") || f.starts_with("r_") || f.contains(":r_") {
        return "R".into();
    }
    if f.contains("capacitor") || f.starts_with("c_") || f.contains(":c_") {
        return "C".into();
    }
    if f.contains("inductor") || f.starts_with("l_") || f.contains(":l_") {
        return "L".into();
    }
    if hay.contains("battery") || v.starts_with("bt") {
        return "B".into();
    }
    if f.contains("pinheader")
        || f.contains("conn")
        || f.contains("header")
        || f.contains("testpoint")
    {
        return "J".into();
    }
    if f.contains("diode") || f.starts_with("d_") {
        return "D".into();
    }
    "U".into()
}

/// Build a [`NetGraph`] from (ref → class) + (net → pin refs). Pins whose
/// ref has no classified component are kept under class "U".
fn build_graph(
    classes: &BTreeMap<String, String>,
    nets: &BTreeMap<String, Vec<String>>,
) -> NetGraph {
    let mut net_names: Vec<String> = nets.keys().cloned().collect();
    net_names.sort();
    let net_idx: HashMap<&str, usize> = net_names
        .iter()
        .enumerate()
        .map(|(i, n)| (n.as_str(), i))
        .collect();

    let mut touches: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (net, pins) in nets {
        let ni = net_idx[net.as_str()];
        for pin in pins {
            let comp_ref = pin.split('.').next().unwrap_or(pin);
            touches.entry(comp_ref.to_string()).or_default().push(ni);
        }
    }
    let comps = touches
        .into_iter()
        .map(|(r, mut ns)| {
            ns.sort_unstable();
            let class = classes.get(&r).cloned().unwrap_or_else(|| "U".into());
            (class, ns)
        })
        .collect();
    NetGraph {
        comps,
        net_count: net_names.len(),
        net_names,
    }
}

/// Golden file → graph.
pub fn golden_graph(g: &GoldenNetlist) -> NetGraph {
    build_graph(&g.components, &g.nets)
}

/// Candidate schematic → graph (components classified from footprint+value).
pub fn candidate_graph(sheet: &vcad_ir::ecad::SchematicSheet) -> Option<NetGraph> {
    let nets = sheet.nets.as_ref()?;
    let classes: BTreeMap<String, String> = sheet
        .components
        .iter()
        .map(|c| (c.reference.clone(), classify(&c.footprint_id, &c.value)))
        .collect();
    Some(build_graph(&classes, nets))
}

/// WL color refinement: per-round, a net's color folds in its member
/// components' colors and vice versa. Returns (comp colors, net colors).
fn refine(g: &NetGraph) -> (Vec<u64>, Vec<u64>) {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let h = |t: &(u64, Vec<u64>)| {
        let mut s = DefaultHasher::new();
        t.hash(&mut s);
        s.finish()
    };
    let hs = |s: &str| {
        let mut d = DefaultHasher::new();
        s.hash(&mut d);
        d.finish()
    };
    let mut comp: Vec<u64> = g.comps.iter().map(|(c, _)| hs(c)).collect();
    let mut net: Vec<u64> = vec![0; g.net_count];
    for _ in 0..(g.comps.len() + g.net_count + 2) {
        let mut nn = vec![Vec::new(); g.net_count];
        for (ci, (_, ns)) in g.comps.iter().enumerate() {
            for &n in ns {
                nn[n].push(comp[ci]);
            }
        }
        let new_net: Vec<u64> = nn
            .into_iter()
            .map(|mut v| {
                v.sort_unstable();
                h(&(0x6e65, v))
            })
            .collect();
        let new_comp: Vec<u64> = g
            .comps
            .iter()
            .enumerate()
            .map(|(ci, (_, ns))| {
                let mut v: Vec<u64> = ns.iter().map(|&n| new_net[n]).collect();
                v.sort_unstable();
                h(&(comp[ci], v))
            })
            .collect();
        if new_comp == comp && new_net == net {
            break;
        }
        comp = new_comp;
        net = new_net;
    }
    (comp, net)
}

/// Component key under a net mapping: (class, mapped+sorted net list).
fn comp_keys(g: &NetGraph, net_map: &[usize]) -> Vec<(String, Vec<usize>)> {
    let mut keys: Vec<(String, Vec<usize>)> = g
        .comps
        .iter()
        .map(|(c, ns)| {
            let mut m: Vec<usize> = ns.iter().map(|&n| net_map[n]).collect();
            m.sort_unstable();
            (c.clone(), m)
        })
        .collect();
    keys.sort();
    keys
}

/// Exact isomorphism test: backtrack a candidate→golden net bijection
/// within WL color classes, then compare component multisets under it.
pub fn isomorphic(cand: &NetGraph, gold: &NetGraph) -> bool {
    if cand.comps.len() != gold.comps.len() || cand.net_count != gold.net_count {
        return false;
    }
    let (cc, cn) = refine(cand);
    let (gc, gn) = refine(gold);
    let sorted = |mut v: Vec<u64>| {
        v.sort_unstable();
        v
    };
    if sorted(cc.clone()) != sorted(gc.clone()) || sorted(cn.clone()) != sorted(gn.clone()) {
        return false;
    }
    // Backtracking net bijection restricted to equal WL colors.
    let n = cand.net_count;
    let mut map = vec![usize::MAX; n];
    let mut used = vec![false; n];
    let gold_keys = comp_keys(gold, &(0..n).collect::<Vec<_>>());
    #[allow(clippy::too_many_arguments)]
    fn bt(
        i: usize,
        n: usize,
        cn: &[u64],
        gn: &[u64],
        map: &mut Vec<usize>,
        used: &mut Vec<bool>,
        cand: &NetGraph,
        gold_keys: &[(String, Vec<usize>)],
    ) -> bool {
        if i == n {
            return comp_keys(cand, map) == *gold_keys;
        }
        for j in 0..n {
            if used[j] || cn[i] != gn[j] {
                continue;
            }
            map[i] = j;
            used[j] = true;
            if bt(i + 1, n, cn, gn, map, used, cand, gold_keys) {
                return true;
            }
            used[j] = false;
            map[i] = usize::MAX;
        }
        false
    }
    bt(0, n, &cn, &gn, &mut map, &mut used, cand, &gold_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nets(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(n, ps)| (n.to_string(), ps.iter().map(|p| p.to_string()).collect()))
            .collect()
    }
    fn classes(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(r, c)| (r.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn divider_matches_relabeled_divider() {
        let gold = build_graph(
            &classes(&[
                ("R1", "R"),
                ("R2", "R"),
                ("T1", "J"),
                ("T2", "J"),
                ("T3", "J"),
            ]),
            &nets(&[
                ("VIN", &["T1.1", "R1.1"]),
                ("VOUT", &["R1.2", "R2.1", "T2.1"]),
                ("GND", &["R2.2", "T3.1"]),
            ]),
        );
        // Same structure, every label different, pin numbers scrambled.
        let cand = build_graph(
            &classes(&[
                ("RA", "R"),
                ("RB", "R"),
                ("P1", "J"),
                ("P2", "J"),
                ("P3", "J"),
            ]),
            &nets(&[
                ("IN", &["P1.1", "RB.2"]),
                ("MID", &["RB.1", "RA.2", "P2.1"]),
                ("0V", &["RA.1", "P3.1"]),
            ]),
        );
        assert!(isomorphic(&cand, &gold));
    }

    #[test]
    fn parallel_resistors_do_not_match_series() {
        let series = build_graph(
            &classes(&[("R1", "R"), ("R2", "R"), ("T1", "J"), ("T2", "J")]),
            &nets(&[
                ("A", &["T1.1", "R1.1"]),
                ("MID", &["R1.2", "R2.1"]),
                ("B", &["R2.2", "T2.1"]),
            ]),
        );
        let parallel = build_graph(
            &classes(&[("R1", "R"), ("R2", "R"), ("T1", "J"), ("T2", "J")]),
            &nets(&[
                ("A", &["T1.1", "R1.1", "R2.1"]),
                ("B", &["R1.2", "R2.2", "T2.1"]),
                ("C", &[]),
            ]),
        );
        assert!(!isomorphic(&parallel, &series));
    }

    #[test]
    fn wrong_class_does_not_match() {
        let gold = build_graph(
            &classes(&[("R1", "R"), ("T1", "J"), ("T2", "J")]),
            &nets(&[("A", &["T1.1", "R1.1"]), ("B", &["R1.2", "T2.1"])]),
        );
        let cand = build_graph(
            &classes(&[("C1", "C"), ("T1", "J"), ("T2", "J")]),
            &nets(&[("A", &["T1.1", "C1.1"]), ("B", &["C1.2", "T2.1"])]),
        );
        assert!(!isomorphic(&cand, &gold));
    }

    #[test]
    fn classify_covers_common_footprints() {
        assert_eq!(classify("Resistor_SMD:R_0805", "330"), "R");
        assert_eq!(classify("Capacitor_SMD:C_0402", "100nF"), "C");
        assert_eq!(classify("LED_SMD:LED_0805", "LED"), "LED");
        assert_eq!(classify("PinHeader_1x02_P2.54mm", "PWR"), "J");
        assert_eq!(classify("Battery_Holder_AA", "BT1"), "B");
        assert_eq!(classify("QFP-32_7x7mm_P0.8mm", "MCU"), "U");
    }
}
