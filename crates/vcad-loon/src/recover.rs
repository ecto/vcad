//! Recovering which document fields a parameter drives.
//!
//! A declared parameter reaches geometry through arbitrary loon code —
//! arithmetic, function calls, list comprehensions — and by the time the
//! evaluator has produced a `Document` every field is a bare `f64`. Rather
//! than trying to thread symbolic values through the interpreter, this module
//! recovers the relationship *empirically*: evaluate the program at the
//! nominal parameter values, then again with one parameter nudged, and diff
//! the two documents field by field.
//!
//! Two evaluations give a slope; a third at twice the nudge tells us whether
//! the field is genuinely affine in that parameter or merely looked linear at
//! one point. Affine covers what CAD layout arithmetic actually does — lane
//! stacks, offsets, gaps, mirrored coordinates — and everything else is
//! rejected rather than approximated.
//!
//! **The pass is fail-closed.** Nothing is trusted on the strength of the
//! fit alone. Once candidate bindings are built they are installed on a
//! document and re-resolved at a *fresh* test point that was not used in
//! fitting, and the result is compared against a ground-truth evaluation at
//! the same point. Any field that disagrees — a non-affine dependence, a
//! field path the resolver does not support, a topology change — loses its
//! binding and stays the literal it always was, with a warning. A recovered
//! binding is therefore a checked claim: the document reproduces the loon
//! program's own answer, or the binding is not there.

use std::collections::{BTreeMap, HashMap};

use vcad_ir::{BindingKey, Bindings, Document, Expr as IrExpr, Parameter};

use crate::params::Decls;

/// Relative nudge applied to a parameter when probing its influence.
const PROBE_SCALE: f64 = 1e-3;
/// A field whose value moves by less than this (relative) does not depend on
/// the parameter being probed.
const DEAD_ZONE: f64 = 1e-9;
/// Tolerance for the "is this actually affine?" third-point check.
const AFFINE_TOL: f64 = 1e-7;
/// Tolerance for the final round-trip verification.
const VERIFY_TOL: f64 = 1e-6;
/// How close a fitted coefficient must be to a simple rational to be snapped.
const SNAP_TOL: f64 = 1e-9;

/// Outcome of the recovery pass.
#[derive(Debug, Clone, Default)]
pub struct Recovery {
    /// Bindings that survived verification.
    pub bindings: Bindings,
    /// Human-readable notes about what could *not* be recovered. These are
    /// not errors — the document is still correct, just less parametric than
    /// it might have been — but they are the diagnostic that explains why a
    /// parameter does not move a part the author expected it to move.
    pub warnings: Vec<String>,
}

/// Probe step for a parameter at nominal value `v`. Large enough that the
/// slope is not dominated by floating-point noise, small enough to stay
/// within the same topology.
fn probe_step(v: f64) -> f64 {
    v.abs().max(1.0) * PROBE_SCALE
}

/// Recover bindings for every base parameter in `decls`.
///
/// `eval` maps a resolved parameter environment to the document that
/// environment produces — for the loon bridge, "rewrite the AST with these
/// values and run it".
pub fn recover<F>(decls: &Decls, nominal: &Document, mut eval: F) -> Recovery
where
    F: FnMut(&HashMap<String, f64>) -> Result<Document, String>,
{
    let mut out = Recovery::default();
    if decls.base.is_empty() {
        return out;
    }
    let Ok(env0) = decls.env() else {
        return out;
    };
    let f0 = extract(nominal);

    // field → (parameter → slope)
    let mut slopes: BTreeMap<BindingKey, BTreeMap<String, f64>> = BTreeMap::new();
    // Fields that visibly moved with a parameter but not affinely — they keep
    // their literals, and the author should hear about it by name.
    let mut non_affine: std::collections::BTreeSet<BindingKey> = Default::default();

    for name in &decls.base {
        let v = env0[name];
        let h = probe_step(v);
        let sample = |eval: &mut F, delta: f64| -> Result<BTreeMap<BindingKey, f64>, String> {
            let over: HashMap<String, f64> = [(name.clone(), v + delta)].into_iter().collect();
            let env = decls.env_with(&over)?;
            eval(&env).map(|d| extract(&d))
        };
        let (f1, f2) = match (sample(&mut eval, h), sample(&mut eval, 2.0 * h)) {
            (Ok(a), Ok(b)) => (a, b),
            _ => {
                out.warnings.push(format!(
                    "'{name}': the model does not evaluate at nearby values, so nothing \
                     could be bound to it — it stays declared but inert"
                ));
                continue;
            }
        };
        // A changed field set means the parameter changes the model's
        // *structure*, not just its numbers. Slopes are meaningless there.
        if !f1.keys().eq(f0.keys()) || !f2.keys().eq(f0.keys()) {
            out.warnings.push(format!(
                "'{name}': changes the shape of the model (fields appear or disappear), \
                 so no field could be bound to it"
            ));
            continue;
        }
        for (key, base) in &f0 {
            let (a, b) = (f1[key], f2[key]);
            let scale = 1.0 + base.abs();
            if (a - base).abs() <= DEAD_ZONE * scale {
                continue;
            }
            // Affine ⇒ the third point is the linear extrapolation of the first two.
            if (b - (2.0 * a - base)).abs() > AFFINE_TOL * scale {
                non_affine.insert(key.clone());
                continue;
            }
            slopes
                .entry(key.clone())
                .or_default()
                .insert(name.clone(), (a - base) / h);
        }
    }

    // Build a formula per field: constant + Σ slope × parameter. Coefficients
    // are snapped to nearby simple rationals so a plane that is plainly "the
    // stack origin plus 2" reads that way instead of carrying the last bits of
    // the difference quotient. Snapping is safe because the formula still has
    // to reproduce a ground-truth evaluation below.
    let mut candidates: Vec<(BindingKey, IrExpr)> = Vec::new();
    for (key, terms) in &slopes {
        let base = f0[key];
        let terms: BTreeMap<&String, f64> = terms.iter().map(|(n, a)| (n, snap(*a))).collect();
        let constant = snap(base - terms.iter().map(|(n, a)| a * env0[*n]).sum::<f64>());
        let mut formula = format!("({})", num(constant));
        for (n, a) in &terms {
            formula.push_str(&format!(" + ({}) * {n}", num(*a)));
        }
        candidates.push((key.clone(), IrExpr::formula(formula)));
    }
    report_non_affine(&non_affine, &mut out);
    if candidates.is_empty() {
        report_inert(decls, &slopes, &mut out);
        return out;
    }

    // ---- Verification -----------------------------------------------------
    // Re-resolve the candidate bindings at a point none of the fits used, and
    // compare against what the loon program itself produces there.
    let test: HashMap<String, f64> = decls
        .base
        .iter()
        .map(|n| (n.clone(), env0[n] + 0.61803 * probe_step(env0[n])))
        .collect();
    let truth = decls
        .env_with(&test)
        .and_then(|env| eval(&env))
        .map(|d| extract(&d));
    let Ok(truth) = truth else {
        out.warnings.push(
            "could not verify recovered parameter bindings (the model does not evaluate at \
             the test point), so none were kept"
                .to_string(),
        );
        return out;
    };

    // Dropping one binding cannot affect another — each patches its own field
    // — so a single verification pass is enough to separate good from bad.
    let mut probe = nominal.clone();
    probe.parameters = overridden(&decls.params, &test);
    probe.bindings = Bindings(candidates.iter().cloned().collect());
    let mut rejected: Vec<BindingKey> = Vec::new();
    match vcad_ir::resolve_document(&mut probe) {
        Ok(_) => {
            let got = extract(&probe);
            for (key, _) in &candidates {
                if !agrees(truth.get(key), got.get(key)) {
                    rejected.push(key.clone());
                }
            }
        }
        Err(e) => {
            // One unsupported field path aborts the whole resolve, so fall
            // back to checking each candidate on its own.
            out.warnings.push(format!(
                "verifying parameter bindings one at a time after: {e}"
            ));
            for (key, expr) in &candidates {
                let mut one = nominal.clone();
                one.parameters = overridden(&decls.params, &test);
                one.bindings = Bindings([(key.clone(), expr.clone())].into_iter().collect());
                let ok = vcad_ir::resolve_document(&mut one).is_ok()
                    && agrees(truth.get(key), extract(&one).get(key));
                if !ok {
                    rejected.push(key.clone());
                }
            }
        }
    }

    if !rejected.is_empty() {
        out.warnings.push(format!(
            "{} field(s) depend on parameters in a way that could not be expressed as a \
             checked formula and stay literal (first: {})",
            rejected.len(),
            rejected[0]
        ));
    }
    out.bindings = Bindings(
        candidates
            .into_iter()
            .filter(|(k, _)| !rejected.contains(k))
            .collect(),
    );

    report_inert(decls, &slopes, &mut out);
    out
}

/// Fields that move with a parameter but not affinely — a squared term, a
/// rounded count. They keep their literals: an affine formula would be a
/// linear approximation of something that is not linear, and silently wrong
/// away from the authored point.
fn report_non_affine(fields: &std::collections::BTreeSet<BindingKey>, out: &mut Recovery) {
    if fields.is_empty() {
        return;
    }
    let shown: Vec<String> = fields.iter().take(3).map(|k| k.to_string()).collect();
    let more = fields.len().saturating_sub(shown.len());
    out.warnings.push(format!(
        "{} field(s) depend on a parameter non-linearly and keep their literals: {}{}",
        fields.len(),
        shown.join(", "),
        if more > 0 {
            format!(", +{more} more")
        } else {
            String::new()
        },
    ));
}

/// The most useful diagnostic of all: a parameter the author declared that
/// does not, in the end, move anything. Either it is genuinely unused, or it
/// reaches geometry through a relationship no checked formula captures — and
/// in both cases the author would otherwise find out by turning the knob and
/// watching nothing happen.
fn report_inert(
    decls: &Decls,
    slopes: &BTreeMap<BindingKey, BTreeMap<String, f64>>,
    out: &mut Recovery,
) {
    let driven: std::collections::HashSet<&String> = slopes
        .iter()
        .filter(|(k, _)| out.bindings.0.contains_key(*k))
        .flat_map(|(_, terms)| terms.keys())
        .collect();
    for name in &decls.base {
        if !driven.contains(name) {
            out.warnings.push(format!(
                "'{name}' drives no geometry — it is either unused, or the fields that \
                 depend on it do so non-linearly and keep their literals"
            ));
        }
    }
}

/// Whether a resolved field reproduces the ground-truth evaluation.
fn agrees(want: Option<&f64>, have: Option<&f64>) -> bool {
    matches!((want, have), (Some(w), Some(g)) if (w - g).abs() <= VERIFY_TOL * (1.0 + w.abs()))
}

/// The parameter table with base values replaced by a test point.
fn overridden(
    params: &HashMap<String, Parameter>,
    point: &HashMap<String, f64>,
) -> HashMap<String, Parameter> {
    let mut out = params.clone();
    for (name, v) in point {
        if let Some(p) = out.get_mut(name) {
            p.value = IrExpr::Number(*v);
        }
    }
    out
}

/// Snap a fitted coefficient to a nearby simple rational. A difference
/// quotient of an exactly-linear relationship lands a few ulps off the whole
/// number it should be; leaving that in would make every recovered formula
/// look like a numerical artifact and would drift the geometry by an ulp on
/// every `set_parameters` round trip.
fn snap(v: f64) -> f64 {
    for d in 1..=16u32 {
        let candidate = (v * d as f64).round() / d as f64;
        if (v - candidate).abs() <= SNAP_TOL * (1.0 + v.abs()) {
            return candidate;
        }
    }
    v
}

/// Format a coefficient so the expression parser reads back exactly what we
/// computed. Rust's `Display` for `f64` is shortest-round-trip and never uses
/// exponent notation, which the expression grammar does not accept.
fn num(v: f64) -> String {
    format!("{v}")
}

// ============================================================================
// Field extraction
// ============================================================================

/// Field names that hold node references rather than measurements. They are
/// numbers in JSON but binding them would rewire the graph, so they are never
/// candidates.
const REFERENCE_FIELDS: &[&str] = &[
    "child", "left", "right", "base", "tool", "solid", "sketch", "children", "sketches",
    "profiles", "id", "node", "target",
];

/// Every numeric, bindable field in a document, keyed exactly as
/// `Document::bindings` keys it.
fn extract(doc: &Document) -> BTreeMap<BindingKey, f64> {
    let mut out = BTreeMap::new();
    for (id, node) in &doc.nodes {
        let Ok(json) = serde_json::to_value(&node.op) else {
            continue;
        };
        walk(&json, &mut String::new(), &mut |path, v| {
            out.insert(BindingKey::new(*id, path), v);
        });
    }
    // Assembly instance placements and PCB footprint positions live outside
    // the node graph but are bindable through the same sidecar.
    for inst in doc.instances.iter().flatten() {
        let Some(xf) = &inst.transform else { continue };
        for (channel, v) in [
            ("translation", &xf.translation),
            ("rotation", &xf.rotation),
            ("scale", &xf.scale),
        ] {
            for (axis, value) in [("x", v.x), ("y", v.y), ("z", v.z)] {
                out.insert(
                    BindingKey::new(0, format!("instance.{}.{channel}.{axis}", inst.id)),
                    value,
                );
            }
        }
    }
    if let Some(pcb) = &doc.pcb {
        for fp in &pcb.footprints {
            for (axis, value) in [("x", fp.position.x), ("y", fp.position.y)] {
                out.insert(
                    BindingKey::new(0, format!("pcb.{}.position.{axis}", fp.reference)),
                    value,
                );
            }
        }
    }
    out
}

/// Collect numeric leaves of a JSON object as dotted field paths.
fn walk(value: &serde_json::Value, path: &mut String, sink: &mut impl FnMut(&str, f64)) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k == "type" || REFERENCE_FIELDS.contains(&k.as_str()) {
                    continue;
                }
                let mark = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(k);
                walk(v, path, sink);
                path.truncate(mark);
            }
        }
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if !path.is_empty() {
                    sink(path, f);
                }
            }
        }
        // Arrays (sketch segment lists, loft profiles) have no binding path
        // grammar, so their contents are not candidates.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{CsgOp, Node, Vec3};

    fn doc_with(op: CsgOp) -> Document {
        let mut d = Document::new();
        d.nodes.insert(
            1,
            Node {
                id: 1,
                name: None,
                op,
            },
        );
        d
    }

    #[test]
    fn extract_finds_vector_components_but_not_child_refs() {
        let d = doc_with(CsgOp::Translate {
            child: 7,
            offset: Vec3::new(1.0, 2.0, 3.0),
        });
        let f = extract(&d);
        assert_eq!(f[&BindingKey::new(1, "offset.y")], 2.0);
        assert!(!f.contains_key(&BindingKey::new(1, "child")));
    }

    #[test]
    fn coefficients_round_trip_through_the_expression_parser() {
        for v in [131.0, -1.0, 0.5, 1.0 / 3.0, -0.000125] {
            let formula = format!("({}) * a", num(v));
            let env: HashMap<String, f64> = [("a".to_string(), 1.0)].into_iter().collect();
            let got = vcad_ir::resolve_binding(
                &BindingKey::new(0, "t"),
                &IrExpr::formula(&formula),
                &env,
            )
            .unwrap_or_else(|e| panic!("{formula}: {e}"));
            assert_eq!(got, v, "{formula}");
        }
    }
}
