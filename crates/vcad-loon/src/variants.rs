//! Parameter tables with variant overlays — design families in one file.
//!
//! A design family (rana-100 → 100b → 100c → 60c) is one design at several
//! sizes, not four designs. Written as four generator sets, every fix has to
//! be re-derived by hand for each variant, and the split between what scales
//! (radii, heights) and what must not (gear module, M3 hardware, minimum
//! wall) lives in prose. This module makes that split a property of the
//! parameter itself.
//!
//! The forms extend the declaration vocabulary [`crate::params`] already
//! speaks — `defparam` with trailing keyword options — with a table to hold
//! them and a variant to overlay them:
//!
//! ```text
//! [deftable rana
//!   [defparam envelope_d      100.0 :unit "mm"]
//!   [defparam gear_module       0.5 :unit "mm" :scale_with_envelope false
//!                                   :description "m0.5 is the PLA floor"]
//!   [defparam pocket_clearance  0.2 :unit "mm"]
//!   [defparam bore_r "envelope_d / 8"]]
//!
//! [defvariant rana_60c :from rana :scale 0.6
//!   [override pocket_clearance 0.4 :why "60c mule print: pocket ran tight"]]
//! ```
//!
//! Three rules give the overlay its meaning:
//!
//! 1. **Scale applies to literals, not formulas.** A derived parameter
//!    recomputes from its (already scaled) inputs, so it is never scaled
//!    twice.
//! 2. **`:scale_with_envelope false` holds a value through a scale.** m0.5
//!    stays m0.5 at 0.6×. Asking for that parameter to be scaled *directly*
//!    — `[scale gear_module 0.6]` — is an error naming the flag, not a
//!    silent shrink.
//! 3. **A variant's scale applies before its own overrides.** An override is
//!    a measurement taken at the variant's own size (the mule's +0.2 mm
//!    pocket clearance), so it lands on the scaled table verbatim.
//!
//! Every resolved value carries its [`Source`]: which table or variant it
//! came from and why it has the value it has. That is what
//! `vcad params resolve` prints and what `vcad params diff` compares.

use std::collections::HashMap;

use loon_lang::ast::{Expr as LExpr, ExprKind};
use serde::Serialize;
use vcad_ir::{Expr as IrExpr, Parameter};

/// Statement heads this module owns, for programs that mix tables with
/// geometry.
pub const TABLE_HEADS: &[&str] = &["deftable", "defvariant"];

// ============================================================================
// Declarations
// ============================================================================

/// One row of a base parameter table.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TableParam {
    /// Literal value or formula over other parameters in the table.
    pub value: IrExpr,
    /// Whether a variant's envelope scale multiplies this value. `false` for
    /// the classes that are set by something other than the envelope: gear
    /// module (print process), COTS hardware, minimum wall.
    pub scale_with_envelope: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl TableParam {
    fn literal(&self) -> Option<f64> {
        self.value.as_number()
    }
}

/// A named base table: an ordered set of parameters.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Table {
    pub name: String,
    pub params: HashMap<String, TableParam>,
    /// Declaration order, so output is stable and readable.
    pub order: Vec<String>,
}

/// One overlay entry in a variant.
#[derive(Debug, Clone, Serialize)]
pub enum Overlay {
    /// `[override name value :why "..."]` — replace the value outright.
    Override {
        name: String,
        value: IrExpr,
        why: Option<String>,
    },
    /// `[scale name factor]` — scale one parameter explicitly. Rejected for
    /// a parameter flagged `:scale_with_envelope false`.
    Scale {
        name: String,
        factor: f64,
        why: Option<String>,
    },
}

impl Overlay {
    fn name(&self) -> &str {
        match self {
            Overlay::Override { name, .. } | Overlay::Scale { name, .. } => name,
        }
    }
}

/// A variant: a parent (table or another variant), an optional envelope
/// scale, and overlays.
#[derive(Debug, Clone, Serialize)]
pub struct Variant {
    pub name: String,
    pub parent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub overlays: Vec<Overlay>,
}

/// Everything one parameter-table document declares.
#[derive(Debug, Clone, Default, Serialize)]
pub struct VariantSet {
    pub tables: HashMap<String, Table>,
    pub variants: HashMap<String, Variant>,
}

// ============================================================================
// Provenance
// ============================================================================

/// Where a resolved value came from, and why it has the value it has.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Source {
    /// A literal straight out of the base table, untouched.
    Base { table: String },
    /// A literal the base table set that a scale deliberately left alone
    /// because it is flagged `scale_with_envelope: false`.
    Held {
        table: String,
        /// The scale factor that did *not* apply.
        skipped_factor: f64,
        flag: &'static str,
    },
    /// A value a variant set outright.
    Override {
        variant: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        why: Option<String>,
    },
    /// A literal a variant's scale multiplied.
    ScaleDerived {
        variant: String,
        factor: f64,
        from: f64,
        /// True when the factor came from `[scale name f]` rather than the
        /// variant-wide `:scale`.
        explicit: bool,
    },
    /// A formula, recomputed from the resolved table.
    Derived { formula: String },
}

impl Source {
    /// One line explaining the value, for `params diff` and error messages.
    pub fn explain(&self) -> String {
        match self {
            Source::Base { table } => format!("base table '{table}'"),
            Source::Held {
                table,
                skipped_factor,
                flag,
            } => format!(
                "held at the '{table}' base value — {flag}: false, so the {skipped_factor}× \
                 envelope scale does not apply"
            ),
            Source::Override { variant, why } => match why {
                Some(w) => format!("own override in '{variant}' ({w})"),
                None => format!("own override in '{variant}'"),
            },
            Source::ScaleDerived {
                variant,
                factor,
                from,
                explicit,
            } => {
                let how = if *explicit {
                    "explicit [scale]"
                } else {
                    "envelope scale"
                };
                format!("{how} in '{variant}': {from} × {factor}")
            }
            Source::Derived { formula } => format!("derived: {formula}"),
        }
    }
}

/// One resolved parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolvedParam {
    pub name: String,
    pub value: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    pub scale_with_envelope: bool,
    pub source: Source,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A flat resolved table for one variant.
#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    /// The variant (or bare table) that was resolved.
    pub name: String,
    /// Inheritance chain, base table first.
    pub chain: Vec<String>,
    /// Product of every envelope scale in the chain.
    pub effective_scale: f64,
    /// Parameters in base-table declaration order.
    pub params: Vec<ResolvedParam>,
}

impl Resolved {
    pub fn get(&self, name: &str) -> Option<&ResolvedParam> {
        self.params.iter().find(|p| p.name == name)
    }

    pub fn value(&self, name: &str) -> Option<f64> {
        self.get(name).map(|p| p.value)
    }
}

// ============================================================================
// Diff
// ============================================================================

/// One differing parameter between two resolved variants.
#[derive(Debug, Clone, Serialize)]
pub struct DiffEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a: Option<ResolvedParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub b: Option<ResolvedParam>,
    /// Why they differ, in one line.
    pub reason: String,
}

/// The difference between two variants.
#[derive(Debug, Clone, Serialize)]
pub struct Diff {
    pub a: String,
    pub b: String,
    pub entries: Vec<DiffEntry>,
}

impl Diff {
    /// Names of the parameters that differ.
    pub fn names(&self) -> Vec<&str> {
        self.entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// Human-readable rendering, the CLI's default output.
    pub fn render(&self) -> String {
        let mut s = format!("{} → {}\n", self.a, self.b);
        if self.entries.is_empty() {
            s.push_str("  (no differences)\n");
            return s;
        }
        for e in &self.entries {
            let show = |p: &Option<ResolvedParam>| match p {
                Some(p) => format!("{}", p.value),
                None => "—".to_string(),
            };
            s.push_str(&format!(
                "  {}: {} → {}\n      {}\n",
                e.name,
                show(&e.a),
                show(&e.b),
                e.reason
            ));
        }
        s
    }
}

// ============================================================================
// Parsing
// ============================================================================

fn head_sym(e: &LExpr) -> Option<(&str, &[LExpr])> {
    let ExprKind::List(items) = &e.kind else {
        return None;
    };
    let ExprKind::Symbol(head) = &items.first()?.kind else {
        return None;
    };
    Some((head.as_str(), &items[1..]))
}

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

fn as_bool(e: &LExpr) -> Option<bool> {
    match &e.kind {
        ExprKind::Bool(b) => Some(*b),
        _ => None,
    }
}

/// A name argument: `[defparam envelope_d ...]` or `[override "envelope_d" ...]`
/// — symbol or string, since both read naturally here.
fn as_name(e: &LExpr) -> Option<&str> {
    as_sym(e).or_else(|| as_str(e))
}

fn as_value(e: &LExpr) -> Option<IrExpr> {
    if let Some(n) = as_number(e) {
        return Some(IrExpr::Number(n));
    }
    as_str(e).map(IrExpr::formula)
}

/// Trailing `:key value` options as an association list.
fn keyword_opts<'a>(
    opts: &'a [LExpr],
    allowed: &[&str],
) -> Result<Vec<(String, &'a LExpr)>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < opts.len() {
        let ExprKind::Keyword(k) = &opts[i].kind else {
            return Err(format!(
                "expected a keyword option ({}), got {}",
                allowed
                    .iter()
                    .map(|a| format!(":{a}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                opts[i]
            ));
        };
        if !allowed.contains(&k.as_str()) {
            return Err(format!(
                "unknown option :{k} — expected one of {}",
                allowed
                    .iter()
                    .map(|a| format!(":{a}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let arg = opts
            .get(i + 1)
            .ok_or_else(|| format!("option :{k} has no value"))?;
        out.push((k.clone(), arg));
        i += 2;
    }
    Ok(out)
}

/// Parse a parameter-table document.
pub fn parse(source: &str) -> Result<VariantSet, String> {
    let exprs = loon_lang::parser::parse(source).map_err(|e| e.message.clone())?;
    let mut set = VariantSet::default();
    for e in &exprs {
        let Some((head, args)) = head_sym(e) else {
            continue;
        };
        match head {
            "deftable" => {
                let table = parse_table(args)?;
                if set.tables.contains_key(&table.name) {
                    return Err(format!("table '{}' declared twice", table.name));
                }
                set.tables.insert(table.name.clone(), table);
            }
            "defvariant" => {
                let variant = parse_variant(args)?;
                if set.variants.contains_key(&variant.name) {
                    return Err(format!("variant '{}' declared twice", variant.name));
                }
                set.variants.insert(variant.name.clone(), variant);
            }
            _ => {}
        }
    }
    if set.tables.is_empty() {
        return Err(
            "no [deftable ...] found — a parameter-table document needs \
                    at least one base table"
                .to_string(),
        );
    }
    for v in set.variants.values() {
        if !set.tables.contains_key(&v.parent) && !set.variants.contains_key(&v.parent) {
            return Err(format!(
                "variant '{}' inherits from '{}', which is neither a table nor a variant",
                v.name, v.parent
            ));
        }
    }
    Ok(set)
}

fn parse_table(args: &[LExpr]) -> Result<Table, String> {
    let name = args
        .first()
        .and_then(as_name)
        .ok_or_else(|| "deftable takes a name, e.g. [deftable rana ...]".to_string())?;
    let mut table = Table {
        name: name.to_string(),
        ..Default::default()
    };
    for row in &args[1..] {
        let Some((head, rargs)) = head_sym(row) else {
            return Err(format!(
                "table '{name}' contains {row}, which is not a [defparam ...] row"
            ));
        };
        if head != "defparam" {
            return Err(format!(
                "table '{name}' contains a [{head} ...] form — a table holds \
                 [defparam ...] rows only"
            ));
        }
        let (Some(pname), Some(value)) = (
            rargs.first().and_then(as_name),
            rargs.get(1).and_then(as_value),
        ) else {
            return Err("defparam takes a name and a literal or formula string, \
                 e.g. [defparam envelope_d 100.0]"
                .to_string());
        };
        let mut p = TableParam {
            value,
            scale_with_envelope: true,
            unit: None,
            description: None,
        };
        for (k, arg) in keyword_opts(&rargs[2..], &["unit", "description", "scale_with_envelope"])?
        {
            match k.as_str() {
                "unit" => p.unit = as_str(arg).map(str::to_string),
                "description" => p.description = as_str(arg).map(str::to_string),
                "scale_with_envelope" => {
                    p.scale_with_envelope = as_bool(arg).ok_or_else(|| {
                        format!("option :scale_with_envelope takes true or false, got {arg}")
                    })?;
                }
                _ => unreachable!("keyword_opts filtered"),
            }
        }
        if !p.scale_with_envelope && p.value.is_formula() {
            return Err(format!(
                "parameter '{pname}' is a formula and cannot be flagged \
                 :scale_with_envelope false — a derived value follows its inputs. \
                 Flag the inputs instead."
            ));
        }
        if table.params.insert(pname.to_string(), p).is_some() {
            return Err(format!(
                "parameter '{pname}' declared twice in table '{name}'"
            ));
        }
        table.order.push(pname.to_string());
    }
    Ok(table)
}

fn parse_variant(args: &[LExpr]) -> Result<Variant, String> {
    let name = args
        .first()
        .and_then(as_name)
        .ok_or_else(|| {
            "defvariant takes a name, e.g. [defvariant rana_60c :from rana :scale 0.6 ...]"
                .to_string()
        })?
        .to_string();

    // Leading keyword options, then overlay rows.
    let mut i = 1;
    let mut parent = None;
    let mut scale = None;
    let mut description = None;
    while i < args.len() {
        let ExprKind::Keyword(k) = &args[i].kind else {
            break;
        };
        let arg = args
            .get(i + 1)
            .ok_or_else(|| format!("option :{k} has no value"))?;
        match k.as_str() {
            "from" => {
                parent = Some(
                    as_name(arg)
                        .ok_or_else(|| format!("option :from takes a name, got {arg}"))?
                        .to_string(),
                )
            }
            "scale" => {
                let f = as_number(arg)
                    .ok_or_else(|| format!("option :scale takes a number, got {arg}"))?;
                if f <= 0.0 {
                    return Err(format!(
                        "variant '{name}': :scale must be positive, got {f}"
                    ));
                }
                scale = Some(f);
            }
            "description" => description = as_str(arg).map(str::to_string),
            other => {
                return Err(format!(
                    "variant '{name}': unknown option :{other} — expected :from, :scale, \
                     or :description"
                ))
            }
        }
        i += 2;
    }
    let parent = parent.ok_or_else(|| {
        format!(
            "variant '{name}' has no :from — a variant inherits from a table or another variant"
        )
    })?;

    let mut overlays = Vec::new();
    for row in &args[i..] {
        let Some((head, rargs)) = head_sym(row) else {
            return Err(format!(
                "variant '{name}' contains {row}, which is not an overlay row"
            ));
        };
        match head {
            "override" => {
                let (Some(pname), Some(value)) = (
                    rargs.first().and_then(as_name),
                    rargs.get(1).and_then(as_value),
                ) else {
                    return Err(format!(
                        "variant '{name}': override takes a name and a value, \
                         e.g. [override pocket_clearance 0.4]"
                    ));
                };
                let mut why = None;
                for (k, arg) in keyword_opts(&rargs[2..], &["why"])? {
                    if k == "why" {
                        why = as_str(arg).map(str::to_string);
                    }
                }
                overlays.push(Overlay::Override {
                    name: pname.to_string(),
                    value,
                    why,
                });
            }
            "scale" => {
                let (Some(pname), Some(factor)) = (
                    rargs.first().and_then(as_name),
                    rargs.get(1).and_then(as_number),
                ) else {
                    return Err(format!(
                        "variant '{name}': scale takes a name and a factor, \
                         e.g. [scale flange_r 0.5]"
                    ));
                };
                let mut why = None;
                for (k, arg) in keyword_opts(&rargs[2..], &["why"])? {
                    if k == "why" {
                        why = as_str(arg).map(str::to_string);
                    }
                }
                overlays.push(Overlay::Scale {
                    name: pname.to_string(),
                    factor,
                    why,
                });
            }
            other => {
                return Err(format!(
                    "variant '{name}': unknown overlay form [{other} ...] — \
                     expected [override ...] or [scale ...]"
                ))
            }
        }
    }

    Ok(Variant {
        name,
        parent,
        scale,
        description,
        overlays,
    })
}

// ============================================================================
// Resolution
// ============================================================================

/// A parameter mid-resolution: its current value plus where that came from.
#[derive(Debug, Clone)]
struct Entry {
    param: TableParam,
    source: Source,
}

impl VariantSet {
    /// Every table and variant name, sorted — for error messages and listings.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .tables
            .keys()
            .chain(self.variants.keys())
            .cloned()
            .collect();
        v.sort();
        v
    }

    /// The inheritance chain for a name, base table first.
    pub fn chain(&self, name: &str) -> Result<Vec<String>, String> {
        let mut chain = Vec::new();
        let mut cur = name.to_string();
        loop {
            if chain.contains(&cur) {
                chain.push(cur.clone());
                return Err(format!(
                    "cycle in variant inheritance: {}",
                    chain.join(" → ")
                ));
            }
            chain.push(cur.clone());
            if self.tables.contains_key(&cur) {
                break;
            }
            match self.variants.get(&cur) {
                Some(v) => cur = v.parent.clone(),
                None => {
                    return Err(format!(
                        "no table or variant named '{name}' — have {}",
                        self.names().join(", ")
                    ))
                }
            }
        }
        chain.reverse();
        Ok(chain)
    }

    /// Resolve a variant (or a bare base table) to a flat table of values,
    /// each carrying its provenance.
    pub fn resolve(&self, name: &str) -> Result<Resolved, String> {
        let chain = self.chain(name)?;
        let table = &self.tables[&chain[0]];

        let mut entries: HashMap<String, Entry> = HashMap::new();
        for pname in &table.order {
            let param = table.params[pname].clone();
            let source = match &param.value {
                IrExpr::Formula(f) => Source::Derived { formula: f.clone() },
                IrExpr::Number(_) => Source::Base {
                    table: table.name.clone(),
                },
            };
            entries.insert(pname.clone(), Entry { param, source });
        }

        let mut effective_scale = 1.0;
        for step in &chain[1..] {
            let variant = &self.variants[step];

            // 1. The variant-wide envelope scale, applied to literals only.
            if let Some(factor) = variant.scale {
                effective_scale *= factor;
                for pname in &table.order {
                    let entry = entries.get_mut(pname).expect("declared");
                    let Some(current) = entry.param.literal() else {
                        continue; // a formula recomputes from its inputs
                    };
                    if !entry.param.scale_with_envelope {
                        entry.source = Source::Held {
                            table: table.name.clone(),
                            skipped_factor: factor,
                            flag: "scale_with_envelope",
                        };
                        continue;
                    }
                    entry.param.value = IrExpr::Number(current * factor);
                    entry.source = Source::ScaleDerived {
                        variant: variant.name.clone(),
                        factor,
                        from: current,
                        explicit: false,
                    };
                }
            }

            // 2. Then the variant's own overlays — a measurement taken at
            //    this variant's size lands on the scaled table verbatim.
            for overlay in &variant.overlays {
                let pname = overlay.name();
                let entry = entries.get_mut(pname).ok_or_else(|| {
                    format!(
                        "variant '{}' overlays '{pname}', which the base table '{}' \
                         does not declare",
                        variant.name, table.name
                    )
                })?;
                match overlay {
                    Overlay::Override { value, why, .. } => {
                        entry.param.value = value.clone();
                        entry.source = Source::Override {
                            variant: variant.name.clone(),
                            why: why.clone(),
                        };
                    }
                    Overlay::Scale { factor, .. } => {
                        if !entry.param.scale_with_envelope {
                            return Err(format!(
                                "variant '{}' scales '{pname}' by {factor}, but '{pname}' is \
                                 declared :scale_with_envelope false in table '{}'{}. \
                                 A value in that class is set by something other than the \
                                 envelope — override it outright with \
                                 [override {pname} <value> :why \"...\"] if it really must \
                                 change, or drop the flag if it really does scale.",
                                variant.name,
                                table.name,
                                match &entry.param.description {
                                    Some(d) => format!(" ({d})"),
                                    None => String::new(),
                                }
                            ));
                        }
                        let Some(current) = entry.param.literal() else {
                            return Err(format!(
                                "variant '{}' scales '{pname}', which is a derived formula — \
                                 a derived value follows its inputs; scale those instead",
                                variant.name
                            ));
                        };
                        entry.param.value = IrExpr::Number(current * factor);
                        entry.source = Source::ScaleDerived {
                            variant: variant.name.clone(),
                            factor: *factor,
                            from: current,
                            explicit: true,
                        };
                    }
                }
            }
        }

        // Evaluate formulas against the overlaid table.
        let ir: HashMap<String, Parameter> = entries
            .iter()
            .map(|(k, e)| {
                (
                    k.clone(),
                    Parameter {
                        value: e.param.value.clone(),
                        unit: e.param.unit.clone(),
                        min: None,
                        max: None,
                        description: e.param.description.clone(),
                    },
                )
            })
            .collect();
        let env = vcad_ir::resolve_parameters(&ir).map_err(|e| e.to_string())?;

        let params = table
            .order
            .iter()
            .map(|pname| {
                let e = &entries[pname];
                ResolvedParam {
                    name: pname.clone(),
                    value: env[pname],
                    unit: e.param.unit.clone(),
                    scale_with_envelope: e.param.scale_with_envelope,
                    source: e.source.clone(),
                    description: e.param.description.clone(),
                }
            })
            .collect();

        Ok(Resolved {
            name: name.to_string(),
            chain,
            effective_scale,
            params,
        })
    }

    /// What differs between two variants, and why.
    pub fn diff(&self, a: &str, b: &str) -> Result<Diff, String> {
        let ra = self.resolve(a)?;
        let rb = self.resolve(b)?;
        let mut entries = Vec::new();

        let mut names: Vec<String> = ra.params.iter().map(|p| p.name.clone()).collect();
        for p in &rb.params {
            if !names.contains(&p.name) {
                names.push(p.name.clone());
            }
        }

        for name in names {
            let pa = ra.get(&name).cloned();
            let pb = rb.get(&name).cloned();
            let same = match (&pa, &pb) {
                (Some(x), Some(y)) => (x.value - y.value).abs() <= 1e-9 * x.value.abs().max(1.0),
                _ => false,
            };
            if same {
                continue;
            }
            let reason = match (&pa, &pb) {
                (Some(x), Some(y)) if x.source == y.source => {
                    // Same provenance on both sides: a derived value whose
                    // inputs moved, or an inherited literal seen through two
                    // different scales.
                    format!("{} — same rule, different inputs", y.source.explain())
                }
                (Some(x), Some(y)) => {
                    format!("{} — was {}", y.source.explain(), x.source.explain())
                }
                (None, Some(_)) => format!("only in '{b}'"),
                (Some(_), None) => format!("only in '{a}'"),
                (None, None) => unreachable!(),
            };
            entries.push(DiffEntry {
                name,
                a: pa,
                b: pb,
                reason,
            });
        }

        Ok(Diff {
            a: a.to_string(),
            b: b.to_string(),
            entries,
        })
    }
}
