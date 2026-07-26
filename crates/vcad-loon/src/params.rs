//! Declaration scanning for parametric loon: `defparam`, datum planes/axes/
//! points, and lane stacks.
//!
//! Loon's own `let` bindings are lexical and vanish at evaluation — the
//! emitted document keeps only the arithmetic result, so design intent is
//! unrecoverable. This module adds a small set of *declaration* forms that
//! the vcad bridge resolves statically, before the program runs:
//!
//! ```text
//! [defparam pitch_axis_x 310.0]        ; a named, settable value
//! [defparam femur_len "pitch_axis_x - 40"]  ; derived from other parameters
//! [datum-plane "femur_inner" y 131.0]  ; named reference plane
//! [datum-axis  "pitch" x 0.0 0.0 310.0]
//! [stack y "leg" 131.0                 ; a declarative packing stack
//!   [lane "femur_inner" 5.0]
//!   [gap  "idler_clr"  1.0]
//!   [lane "idler_boss" 3.0]]
//! ```
//!
//! and matching *read* forms that geometry uses in place of literals:
//!
//! ```text
//! [datum "leg_idler_boss_lo"]      ; the plane's offset
//! [datum+ "leg_idler_boss_hi" 3.0] ; 3 mm outboard of that face
//! [datum-x "pitch"]                ; a component of an axis/point datum
//! ```
//!
//! Scanning is a two-pass affair. [`scan`] walks the parsed AST and collects
//! declarations into a [`Decls`] — a parameter table (literals and formulas)
//! plus datum entities. Resolving that table yields an environment, and
//! [`rewrite`] then substitutes every read form with the concrete number it
//! denotes, producing an ordinary loon program the existing evaluator runs
//! unchanged.
//!
//! That substitution is also the hook for provenance recovery: re-running
//! [`rewrite`] under a perturbed environment and diffing the two documents is
//! how `recover` learns which document fields a parameter drives. See
//! [`crate::recover`].

use std::collections::HashMap;

use loon_lang::ast::{Expr as LExpr, ExprKind};
use vcad_ir::{Datum, Expr as IrExpr, Parameter, PrincipalAxis};

/// Declaration forms. These are statements — they bind or declare rather
/// than produce a scene value — and are also listed in the source-level
/// statement heads so multi-value programs split correctly.
pub const DECL_HEADS: &[&str] = &[
    "defparam",
    "datum-plane",
    "datum-axis",
    "datum-point",
    "stack",
];

/// Everything the declaration pass learned from a program.
#[derive(Debug, Clone, Default)]
pub struct Decls {
    /// Parameter table, ready to drop into `Document::parameters`.
    pub params: HashMap<String, Parameter>,
    /// Independent (literal-valued) parameter names in declaration order.
    /// These are the knobs provenance recovery perturbs; derived parameters
    /// follow from them and are never perturbed directly.
    pub base: Vec<String>,
    /// Named reference geometry, ready to drop into `Document::datums`.
    pub datums: HashMap<String, Datum>,
    /// Whether the program contains any read form (`datum`, `datum+`, …).
    /// A program that reads without declaring is an error, but the rewrite
    /// pass has to run for it to be reported as one rather than surfacing as
    /// an unbound symbol from the interpreter.
    pub reads: bool,
}

impl Decls {
    /// Whether the program declared anything at all. When false the caller
    /// takes the plain, non-parametric evaluation path.
    pub fn is_empty(&self) -> bool {
        self.params.is_empty() && self.datums.is_empty() && !self.reads
    }

    /// Resolve the parameter table to concrete values.
    pub fn env(&self) -> Result<HashMap<String, f64>, String> {
        vcad_ir::resolve_parameters(&self.params).map_err(|e| e.to_string())
    }

    /// Resolve with the given base parameters overridden — the perturbation
    /// entry point. Derived parameters recompute from the overrides.
    pub fn env_with(
        &self,
        overrides: &HashMap<String, f64>,
    ) -> Result<HashMap<String, f64>, String> {
        let mut params = self.params.clone();
        for (name, v) in overrides {
            let Some(p) = params.get_mut(name) else {
                return Err(format!("no such parameter '{name}'"));
            };
            p.value = IrExpr::Number(*v);
        }
        vcad_ir::resolve_parameters(&params).map_err(|e| e.to_string())
    }

    fn declare(&mut self, name: &str, param: Parameter) -> Result<(), String> {
        check_name(name)?;
        let is_base = param.value.as_number().is_some();
        if let Some(prev) = self.params.get(name) {
            // A repeated declaration is fine when it says the same thing —
            // the same `[param "x" 5.0]` may appear at several call sites —
            // but two different values for one name is exactly the
            // disagreement this module exists to prevent.
            if prev.value != param.value {
                return Err(format!(
                    "parameter '{name}' declared twice with different values \
                     ({:?} then {:?}) — one name must mean one value",
                    prev.value, param.value
                ));
            }
            return Ok(());
        }
        if is_base {
            self.base.push(name.to_string());
        }
        self.params.insert(name.to_string(), param);
        Ok(())
    }

    fn declare_datum(&mut self, name: &str, datum: Datum) -> Result<(), String> {
        check_name(name)?;
        if let Some(prev) = self.datums.get(name) {
            if *prev != datum {
                return Err(format!(
                    "datum '{name}' declared twice with different geometry — \
                     one name must mean one plane"
                ));
            }
            return Ok(());
        }
        self.datums.insert(name.to_string(), datum);
        Ok(())
    }
}

/// Parameter and datum names must be valid identifiers for the expression
/// parser (`[A-Za-z_][A-Za-z0-9_]*`) — a `-` would parse as subtraction, so
/// kebab-case names are rejected with a pointed message rather than silently
/// producing a formula that means something else.
fn check_name(name: &str) -> Result<(), String> {
    let mut chars = name.chars();
    let ok = match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "'{name}' is not a valid parameter name — use letters, digits and \
             underscores (e.g. '{}')",
            name.replace('-', "_")
        ))
    }
}

// ============================================================================
// AST helpers
// ============================================================================

/// A list expression with a symbol head: returns the head and its arguments.
fn head_sym(e: &LExpr) -> Option<(&str, &[LExpr])> {
    let ExprKind::List(items) = &e.kind else {
        return None;
    };
    let ExprKind::Symbol(head) = &items.first()?.kind else {
        return None;
    };
    Some((head.as_str(), &items[1..]))
}

/// A numeric literal (loon `Int` and `Float` both count).
fn as_number(e: &LExpr) -> Option<f64> {
    match &e.kind {
        ExprKind::Float(f) => Some(*f),
        ExprKind::Int(i) => Some(*i as f64),
        _ => None,
    }
}

fn as_str(e: &LExpr) -> Option<&str> {
    match &e.kind {
        ExprKind::Str(s) => Some(s.as_str()),
        _ => None,
    }
}

fn as_sym(e: &LExpr) -> Option<&str> {
    match &e.kind {
        ExprKind::Symbol(s) => Some(s.as_str()),
        _ => None,
    }
}

/// A declaration's value argument: a literal number or a formula string.
fn as_value(e: &LExpr) -> Option<IrExpr> {
    if let Some(n) = as_number(e) {
        return Some(IrExpr::Number(n));
    }
    as_str(e).map(IrExpr::formula)
}

fn as_axis(e: &LExpr) -> Option<PrincipalAxis> {
    as_sym(e)
        .and_then(PrincipalAxis::parse)
        .or_else(|| as_str(e).and_then(PrincipalAxis::parse))
}

/// Replace an expression in place with a numeric literal, preserving its span
/// and node id so downstream error reporting still points at the source.
fn to_literal(e: &mut LExpr, v: f64) {
    e.kind = ExprKind::Float(v);
}

/// Every child expression of a node, for the generic recursive walk.
fn children_mut(e: &mut LExpr) -> Vec<&mut LExpr> {
    match &mut e.kind {
        ExprKind::List(v) | ExprKind::Vec(v) | ExprKind::Set(v) | ExprKind::Tuple(v) => {
            v.iter_mut().collect()
        }
        ExprKind::Map(pairs) => pairs.iter_mut().flat_map(|(k, v)| [k, v]).collect(),
        ExprKind::Quote(b) | ExprKind::Unquote(b) | ExprKind::UnquoteSplice(b) => vec![b.as_mut()],
        ExprKind::DotAccess(b, _) => vec![b.as_mut()],
        _ => Vec::new(),
    }
}

fn children(e: &LExpr) -> Vec<&LExpr> {
    match &e.kind {
        ExprKind::List(v) | ExprKind::Vec(v) | ExprKind::Set(v) | ExprKind::Tuple(v) => {
            v.iter().collect()
        }
        ExprKind::Map(pairs) => pairs.iter().flat_map(|(k, v)| [k, v]).collect(),
        ExprKind::Quote(b) | ExprKind::Unquote(b) | ExprKind::UnquoteSplice(b) => vec![b.as_ref()],
        ExprKind::DotAccess(b, _) => vec![b.as_ref()],
        _ => Vec::new(),
    }
}

// ============================================================================
// Pass 1 — collect declarations
// ============================================================================

/// Walk a parsed program and collect every declaration form.
///
/// Declarations may appear anywhere, including inside function bodies, but
/// their arguments must be literals: a declaration is resolved before the
/// program runs, so it cannot depend on runtime values. Formula strings are
/// the escape hatch for values derived from other parameters.
pub fn scan(exprs: &[LExpr]) -> Result<Decls, String> {
    let mut decls = Decls::default();
    for e in exprs {
        scan_expr(e, &mut decls)?;
    }
    Ok(decls)
}

fn scan_expr(e: &LExpr, decls: &mut Decls) -> Result<(), String> {
    if let Some((head, args)) = head_sym(e) {
        match head {
            "defparam" => {
                let (Some(name), Some(value)) = (
                    args.first().and_then(as_sym),
                    args.get(1).and_then(as_value),
                ) else {
                    return Err("defparam takes a name and a literal or formula string, \
                         e.g. [defparam pitch_axis_x 310.0]"
                        .to_string());
                };
                decls.declare(name, param_with_opts(value, &args[2..])?)?;
            }
            "param" => {
                // The expression form: [param "name" 5.0] evaluates to 5.0.
                if let (Some(name), Some(value)) = (
                    args.first().and_then(as_str),
                    args.get(1).and_then(as_value),
                ) {
                    decls.declare(name, param_with_opts(value, &args[2..])?)?;
                }
            }
            "datum-plane" => scan_datum_plane(args, decls)?,
            "datum-axis" | "datum-point" => scan_datum_frame(head, args, decls)?,
            "stack" => scan_stack(args, decls)?,
            "datum" | "datum+" | "datum-x" | "datum-y" | "datum-z" => decls.reads = true,
            _ => {}
        }
    }
    for c in children(e) {
        scan_expr(c, decls)?;
    }
    Ok(())
}

/// Trailing `:unit`/`:min`/`:max`/`:description` keyword options on a
/// declaration. Unknown keywords are an error rather than a silent no-op.
fn param_with_opts(value: IrExpr, opts: &[LExpr]) -> Result<Parameter, String> {
    let mut p = Parameter {
        value,
        unit: None,
        min: None,
        max: None,
        description: None,
    };
    let mut i = 0;
    while i < opts.len() {
        let ExprKind::Keyword(k) = &opts[i].kind else {
            return Err(format!(
                "expected a keyword option (:unit, :min, :max, :description), got {}",
                opts[i]
            ));
        };
        let arg = opts
            .get(i + 1)
            .ok_or_else(|| format!("option :{k} has no value"))?;
        match k.as_str() {
            "unit" => p.unit = as_str(arg).map(str::to_string),
            "description" => p.description = as_str(arg).map(str::to_string),
            "min" => p.min = as_number(arg),
            "max" => p.max = as_number(arg),
            other => return Err(format!("unknown option :{other}")),
        }
        i += 2;
    }
    Ok(p)
}

fn scan_datum_plane(args: &[LExpr], decls: &mut Decls) -> Result<(), String> {
    let (Some(name), Some(axis), Some(value)) = (
        args.first().and_then(as_str),
        args.get(1).and_then(as_axis),
        args.get(2).and_then(as_value),
    ) else {
        return Err("datum-plane takes a name, an axis (x/y/z) and an offset, \
                    e.g. [datum-plane \"femur_inner\" y 131.0]"
            .to_string());
    };
    decls.declare(name, param_with_opts(value, &args[3..])?)?;
    decls.declare_datum(name, Datum::axis_plane(axis, IrExpr::formula(name)))
}

fn scan_datum_frame(head: &str, args: &[LExpr], decls: &mut Decls) -> Result<(), String> {
    // [datum-axis "pitch" x ox oy oz] / [datum-point "hip" ox oy oz]
    let is_axis = head == "datum-axis";
    let name = args.first().and_then(as_str).ok_or_else(|| {
        format!("{head} takes a name first, e.g. [{head} \"pitch\" x 0.0 0.0 310.0]")
    })?;
    let (axis, coords) = if is_axis {
        let a = args
            .get(1)
            .and_then(as_axis)
            .ok_or_else(|| "datum-axis needs a direction axis (x/y/z)".to_string())?;
        (Some(a), &args[2..])
    } else {
        (None, &args[1..])
    };
    if coords.len() < 3 {
        return Err(format!("{head} needs three origin coordinates"));
    }
    let mut origin = [
        IrExpr::Number(0.0),
        IrExpr::Number(0.0),
        IrExpr::Number(0.0),
    ];
    for (i, comp) in ["x", "y", "z"].iter().enumerate() {
        let value = as_value(&coords[i])
            .ok_or_else(|| format!("{head} coordinate {comp} must be a literal or formula"))?;
        let comp_name = format!("{name}_{comp}");
        decls.declare(
            &comp_name,
            Parameter {
                value,
                ..Parameter::literal(0.0)
            },
        )?;
        origin[i] = IrExpr::formula(comp_name);
    }
    let datum = match axis {
        Some(a) => {
            let u = a.unit();
            Datum::Axis {
                origin,
                direction: [
                    IrExpr::Number(u[0]),
                    IrExpr::Number(u[1]),
                    IrExpr::Number(u[2]),
                ],
            }
        }
        None => Datum::Point { position: origin },
    };
    decls.declare_datum(name, datum)
}

/// `[stack AXIS "prefix" origin [lane "n" t] [gap "n" g] ...]`
///
/// Every lane boundary becomes a datum plane and a *derived* parameter, so
/// the running clearances are named values rather than arbitrary numbers and
/// widening one gap slides everything outboard of it. The declared knobs are
/// the origin, each lane thickness (`<prefix>_<lane>_t`), and each gap
/// (`<prefix>_<gap>`); the boundaries (`_lo` / `_hi`) are derived.
fn scan_stack(args: &[LExpr], decls: &mut Decls) -> Result<(), String> {
    let (Some(axis), Some(prefix), Some(origin)) = (
        args.first().and_then(as_axis),
        args.get(1).and_then(as_str),
        args.get(2).and_then(as_value),
    ) else {
        return Err(
            "stack takes an axis, a name prefix and a start coordinate, \
                    e.g. [stack y \"leg\" 131.0 [lane \"plate\" 5.0] ...]"
                .to_string(),
        );
    };
    check_name(prefix)?;
    let origin_name = format!("{prefix}_origin");
    decls.declare(
        &origin_name,
        Parameter {
            value: origin,
            ..Parameter::literal(0.0)
        },
    )?;

    // `cursor` is the formula for the current running coordinate.
    let mut cursor = origin_name.clone();
    for entry in &args[3..] {
        let Some((kind, eargs)) = head_sym(entry) else {
            return Err(format!(
                "stack entries must be [lane ...] or [gap ...], got {entry}"
            ));
        };
        let (Some(name), Some(size)) = (
            eargs.first().and_then(as_str),
            eargs.get(1).and_then(as_value),
        ) else {
            return Err(format!("[{kind} ...] takes a name and a size"));
        };
        match kind {
            "lane" => {
                let t = format!("{prefix}_{name}_t");
                let lo = format!("{prefix}_{name}_lo");
                let hi = format!("{prefix}_{name}_hi");
                decls.declare(
                    &t,
                    Parameter {
                        value: size,
                        ..Parameter::literal(0.0)
                    },
                )?;
                decls.declare(&lo, Parameter::derived(cursor.clone()))?;
                decls.declare(&hi, Parameter::derived(format!("{lo} + {t}")))?;
                decls.declare_datum(&lo, Datum::axis_plane(axis, IrExpr::formula(&lo)))?;
                decls.declare_datum(&hi, Datum::axis_plane(axis, IrExpr::formula(&hi)))?;
                cursor = hi;
            }
            "gap" => {
                let g = format!("{prefix}_{name}");
                decls.declare(
                    &g,
                    Parameter {
                        value: size,
                        ..Parameter::literal(0.0)
                    },
                )?;
                cursor = format!("{cursor} + {g}");
            }
            other => {
                return Err(format!(
                    "unknown stack entry '{other}' — expected lane or gap"
                ))
            }
        }
    }
    let end = format!("{prefix}_end");
    decls.declare(&end, Parameter::derived(cursor))?;
    decls.declare_datum(&end, Datum::axis_plane(axis, IrExpr::formula(&end)))
}

// ============================================================================
// Pass 2 — rewrite reads and declarations into a plain loon program
// ============================================================================

/// Substitute every declaration and read form with the concrete number it
/// denotes under `env`, yielding an ordinary loon program.
///
/// Declarations collapse to inert `let` bindings (they have already done
/// their work); `defparam` keeps its name bound so the rest of the program
/// can use it as an ordinary value.
pub fn rewrite(exprs: &[LExpr], env: &HashMap<String, f64>) -> Result<Vec<LExpr>, String> {
    let mut out: Vec<LExpr> = exprs.to_vec();
    for e in &mut out {
        rewrite_expr(e, env)?;
    }
    Ok(out)
}

/// Look a name up in the resolved environment.
fn lookup(env: &HashMap<String, f64>, name: &str) -> Result<f64, String> {
    env.get(name).copied().ok_or_else(|| {
        format!("'{name}' is not a declared parameter or datum — declare it before it is read")
    })
}

fn rewrite_expr(e: &mut LExpr, env: &HashMap<String, f64>) -> Result<(), String> {
    // Decide what this node becomes before recursing, so a read form's
    // arguments are not themselves rewritten as if they were geometry.
    enum Action {
        None,
        Literal(f64),
        /// `[+ <literal> <original arg>]` — used by `datum+`, whose offset
        /// may be an arbitrary runtime expression.
        AddTo(f64, usize),
        /// Collapse a declaration to `[let <name> <literal>]`.
        Bind(String, f64),
    }

    let action = match head_sym(e) {
        Some(("defparam", args)) => {
            let name = args
                .first()
                .and_then(as_sym)
                .ok_or_else(|| "defparam takes a name".to_string())?;
            Action::Bind(name.to_string(), lookup(env, name)?)
        }
        Some(("param", args)) => match args.first().and_then(as_str) {
            Some(name) => Action::Literal(lookup(env, name)?),
            None => Action::None,
        },
        Some(("datum-plane" | "datum-axis" | "datum-point" | "stack", _)) => {
            // The declaration has already done its work in `scan`; collapse it
            // to an inert binding so it neither evaluates nor becomes the
            // program's value.
            Action::Bind(format!("__vcad_decl_{}", e.id.0), 0.0)
        }
        Some(("datum", args)) => {
            let name = args
                .first()
                .and_then(as_str)
                .ok_or_else(|| "[datum \"name\"] takes a datum name".to_string())?;
            Action::Literal(datum_read(env, name, None)?)
        }
        Some((head @ ("datum-x" | "datum-y" | "datum-z"), args)) => {
            let name = args
                .first()
                .and_then(as_str)
                .ok_or_else(|| format!("[{head} \"name\"] takes a datum name"))?;
            Action::Literal(datum_read(env, name, head.strip_prefix("datum-"))?)
        }
        Some(("datum+", args)) => {
            let name = args
                .first()
                .and_then(as_str)
                .ok_or_else(|| "[datum+ \"name\" offset] takes a datum name".to_string())?;
            if args.len() < 2 {
                return Err("[datum+ \"name\" offset] takes an offset".to_string());
            }
            Action::AddTo(datum_read(env, name, None)?, 2)
        }
        _ => Action::None,
    };

    match action {
        Action::None => {}
        Action::Literal(v) => {
            to_literal(e, v);
            return Ok(());
        }
        Action::Bind(name, v) => {
            let mut span_src = e.clone();
            to_literal(&mut span_src, v);
            let mut sym = e.clone();
            sym.kind = ExprKind::Symbol(name);
            let mut let_head = e.clone();
            let_head.kind = ExprKind::Symbol("let".to_string());
            e.kind = ExprKind::List(vec![let_head, sym, span_src]);
            return Ok(());
        }
        Action::AddTo(base, arg_index) => {
            let ExprKind::List(items) = &e.kind else {
                unreachable!("datum+ matched a list");
            };
            let mut offset = items[arg_index].clone();
            rewrite_expr(&mut offset, env)?;
            let mut plus = e.clone();
            plus.kind = ExprKind::Symbol("+".to_string());
            let mut lit = e.clone();
            to_literal(&mut lit, base);
            e.kind = ExprKind::List(vec![plus, lit, offset]);
            return Ok(());
        }
    }

    for c in children_mut(e) {
        rewrite_expr(c, env)?;
    }
    Ok(())
}

/// Read a datum's offset (for a plane) or an origin component.
fn datum_read(
    env: &HashMap<String, f64>,
    name: &str,
    component: Option<&str>,
) -> Result<f64, String> {
    match component {
        None => lookup(env, name).map_err(|_| {
            format!(
                "'{name}' is not a declared plane datum — for an axis or point \
                 datum use [datum-x \"{name}\"] / -y / -z"
            )
        }),
        Some(c) => lookup(env, &format!("{name}_{c}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Vec<LExpr> {
        loon_lang::parser::parse(src).expect("parse")
    }

    #[test]
    fn declaration_heads_are_also_source_level_statement_heads() {
        // The source splitter classifies top-level forms by head symbol before
        // the AST exists, so it keeps its own copy of this list. A declaration
        // missing there would be treated as the program's scene value.
        for head in DECL_HEADS {
            assert!(
                crate::STATEMENT_HEADS.contains(head),
                "'{head}' is a declaration but not a statement head"
            );
        }
    }

    #[test]
    fn defparam_becomes_a_base_parameter() {
        let d = scan(&parse("[defparam hip_x 310.0]")).unwrap();
        assert_eq!(d.base, vec!["hip_x"]);
        assert_eq!(d.params["hip_x"].value, IrExpr::Number(310.0));
    }

    #[test]
    fn formula_parameters_are_derived_not_base() {
        let d = scan(&parse("[defparam a 10.0]\n[defparam b \"a * 2\"]")).unwrap();
        assert_eq!(d.base, vec!["a"]);
        assert_eq!(d.env().unwrap()["b"], 20.0);
    }

    #[test]
    fn kebab_names_are_rejected_with_a_suggestion() {
        let err = scan(&parse("[defparam pitch-axis 3.0]")).unwrap_err();
        assert!(err.contains("pitch_axis"), "{err}");
    }

    #[test]
    fn one_name_cannot_mean_two_values() {
        let err = scan(&parse("[defparam a 1.0]\n[defparam a 2.0]")).unwrap_err();
        assert!(err.contains("declared twice"), "{err}");
    }

    #[test]
    fn repeated_identical_declarations_are_fine() {
        scan(&parse("[param \"a\" 1.0]\n[param \"a\" 1.0]")).unwrap();
    }

    #[test]
    fn datum_plane_declares_a_parameter_and_a_plane() {
        let d = scan(&parse("[datum-plane \"femur_inner\" y 131.0]")).unwrap();
        assert_eq!(d.env().unwrap()["femur_inner"], 131.0);
        assert!(matches!(d.datums["femur_inner"], Datum::Plane { .. }));
    }

    #[test]
    fn stack_derives_boundaries_from_thicknesses_and_gaps() {
        let d = scan(&parse(
            r#"[stack y "leg" 131.0
                 [lane "femur_inner" 5.0]
                 [gap "idler_clr" 1.0]
                 [lane "idler_boss" 3.0]]"#,
        ))
        .unwrap();
        let env = d.env().unwrap();
        assert_eq!(env["leg_femur_inner_lo"], 131.0);
        assert_eq!(env["leg_femur_inner_hi"], 136.0);
        assert_eq!(env["leg_idler_boss_lo"], 137.0);
        assert_eq!(env["leg_idler_boss_hi"], 140.0);
        assert_eq!(env["leg_end"], 140.0);
        // Widening the running clearance slides everything outboard of it.
        let env2 = d
            .env_with(&[("leg_idler_clr".to_string(), 2.0)].into_iter().collect())
            .unwrap();
        assert_eq!(env2["leg_femur_inner_hi"], 136.0);
        assert_eq!(env2["leg_idler_boss_lo"], 138.0);
        assert_eq!(env2["leg_end"], 141.0);
    }

    #[test]
    fn stack_boundaries_are_datum_planes() {
        let d = scan(&parse("[stack z \"s\" 0.0 [lane \"plate\" 2.0]]")).unwrap();
        assert!(d.datums.contains_key("s_plate_lo"));
        assert!(d.datums.contains_key("s_plate_hi"));
    }

    #[test]
    fn reads_rewrite_to_literals() {
        let src = "[datum-plane \"p\" y 131.0]\n[cube [datum \"p\"] [datum+ \"p\" 3.0] 1.0]";
        let exprs = parse(src);
        let d = scan(&exprs).unwrap();
        let out = rewrite(&exprs, &d.env().unwrap()).unwrap();
        let text = format!("{}", out[1]);
        assert!(text.contains("131"), "{text}");
        assert!(text.contains('+'), "datum+ keeps a runtime add: {text}");
    }

    #[test]
    fn reading_an_undeclared_datum_is_an_error() {
        let exprs = parse("[cube [datum \"nope\"] 1.0 1.0]");
        let d = scan(&exprs).unwrap();
        assert!(rewrite(&exprs, &d.env().unwrap()).is_err());
    }

    #[test]
    fn axis_datum_components_are_readable() {
        let src = "[datum-axis \"pitch\" x 0.0 0.0 310.0]\n[cube [datum-z \"pitch\"] 1.0 1.0]";
        let exprs = parse(src);
        let d = scan(&exprs).unwrap();
        assert_eq!(d.env().unwrap()["pitch_z"], 310.0);
        let out = rewrite(&exprs, &d.env().unwrap()).unwrap();
        assert!(format!("{}", out[1]).contains("310"));
    }

    #[test]
    fn options_ride_along() {
        let d = scan(&parse(
            "[defparam w 10.0 :unit \"mm\" :min 1.0 :description \"width\"]",
        ))
        .unwrap();
        assert_eq!(d.params["w"].unit.as_deref(), Some("mm"));
        assert_eq!(d.params["w"].min, Some(1.0));
    }

    #[test]
    fn unknown_option_is_rejected() {
        assert!(scan(&parse("[defparam w 10.0 :colour \"red\"]")).is_err());
    }
}
