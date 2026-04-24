//! Document-level named parameters and expression bindings.
//!
//! A [`Document`](crate::Document) can declare named [`Parameter`]s whose
//! values are either literal numbers or expression strings. Operation fields
//! can be bound to expressions via the [`Bindings`] sidecar — a map from
//! (node id, field path) to [`Expr`]. At evaluation time, the
//! [`resolve`](Bindings::resolve) pass evaluates every parameter and binding
//! and returns concrete `f64` values.
//!
//! Why a sidecar? It keeps the kernel, mesh, and WASM pipelines oblivious
//! to expressions. They continue to consume concrete `f64` fields on the
//! existing `CsgOp` variants; only the outer-layer file format carries
//! parametric intent. Old `.vcad` files (no `parameters`, no `bindings`)
//! deserialize unchanged.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::NodeId;

// ============================================================================
// Expr
// ============================================================================

/// A value that is either a literal number or a formula string.
///
/// Serializes as a bare number (`5`) or a string (`"wheelbase * 0.5"`). The
/// untagged union means new parametric docs can embed expressions inline
/// while old numeric docs remain wire-compatible.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Expr {
    /// Literal numeric value.
    Number(f64),
    /// Expression formula (parsed lazily at resolve time).
    Formula(String),
}

impl Expr {
    /// Literal number shortcut.
    pub fn num(v: f64) -> Self {
        Self::Number(v)
    }

    /// Formula shortcut.
    pub fn formula(s: impl Into<String>) -> Self {
        Self::Formula(s.into())
    }

    /// If the expression is a literal, return it.
    pub fn as_number(&self) -> Option<f64> {
        if let Self::Number(v) = self {
            Some(*v)
        } else {
            None
        }
    }

    /// Whether this is a non-literal formula.
    pub fn is_formula(&self) -> bool {
        matches!(self, Self::Formula(_))
    }
}

impl From<f64> for Expr {
    fn from(v: f64) -> Self {
        Self::Number(v)
    }
}

impl From<String> for Expr {
    fn from(s: String) -> Self {
        Self::Formula(s)
    }
}

impl From<&str> for Expr {
    fn from(s: &str) -> Self {
        Self::Formula(s.to_string())
    }
}

impl Default for Expr {
    fn default() -> Self {
        Self::Number(0.0)
    }
}

// ============================================================================
// Parameter
// ============================================================================

/// A named document-level parameter.
///
/// The `value` is an [`Expr`]. Simple parameters use `Expr::Number(...)`;
/// derived parameters use `Expr::Formula(...)` and may reference other
/// parameters. Evaluation order is determined by topological sort.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    /// Literal value or formula referencing other parameters.
    pub value: Expr,
    /// Optional unit string for display (e.g. "mm", "deg"). Not used for
    /// dimensional analysis in v1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Optional lower bound for the scrub input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Optional upper bound for the scrub input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Optional description shown in the parameters panel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl Parameter {
    /// Create a literal parameter.
    pub fn literal(value: f64) -> Self {
        Self {
            value: Expr::Number(value),
            unit: None,
            min: None,
            max: None,
            description: None,
        }
    }

    /// Create a derived parameter from a formula string.
    pub fn derived(formula: impl Into<String>) -> Self {
        Self {
            value: Expr::Formula(formula.into()),
            unit: None,
            min: None,
            max: None,
            description: None,
        }
    }
}

// ============================================================================
// Binding key
// ============================================================================

/// Key for a single bound field: a node id plus a dotted field path.
///
/// Examples of `field_path`:
/// - `"size.x"` for `Cube.size.x`
/// - `"radius"` for `Cylinder.radius`
/// - `"offset.z"` for `Translate.offset.z`
///
/// Serialized as `"{node_id}:{field_path}"` for compact JSON and easy
/// MCP authoring.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BindingKey {
    /// Target node identifier.
    pub node_id: NodeId,
    /// Dotted path into the node's fields.
    pub field_path: String,
}

impl BindingKey {
    /// Construct a binding key.
    pub fn new(node_id: NodeId, field_path: impl Into<String>) -> Self {
        Self {
            node_id,
            field_path: field_path.into(),
        }
    }
}

impl fmt::Display for BindingKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.node_id, self.field_path)
    }
}

impl Serialize for BindingKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for BindingKey {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let (id, path) = s.split_once(':').ok_or_else(|| {
            serde::de::Error::custom(format!("expected 'node_id:field_path', got {:?}", s))
        })?;
        let node_id: NodeId = id
            .parse()
            .map_err(|_| serde::de::Error::custom(format!("invalid node id {:?}", id)))?;
        Ok(Self {
            node_id,
            field_path: path.to_string(),
        })
    }
}

// ============================================================================
// Bindings
// ============================================================================

/// Sidecar map from (node, field path) → expression.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bindings(pub HashMap<BindingKey, Expr>);

impl Bindings {
    /// Create an empty binding map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether any bindings are present.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Bind a field path on a node to an expression.
    pub fn bind(&mut self, key: BindingKey, expr: Expr) {
        self.0.insert(key, expr);
    }

    /// Remove a binding, returning the previous expression if any.
    pub fn unbind(&mut self, key: &BindingKey) -> Option<Expr> {
        self.0.remove(key)
    }

    /// Look up a binding.
    pub fn get(&self, key: &BindingKey) -> Option<&Expr> {
        self.0.get(key)
    }

    /// Iterate all bindings.
    pub fn iter(&self) -> impl Iterator<Item = (&BindingKey, &Expr)> {
        self.0.iter()
    }
}

// ============================================================================
// Resolution
// ============================================================================

/// An error produced while resolving parameters or bindings.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveError {
    /// Parse error on a parameter's formula.
    ParameterParse {
        /// Parameter name.
        name: String,
        /// Underlying message.
        message: String,
    },
    /// Parse error on a binding's formula.
    BindingParse {
        /// Binding key.
        key: BindingKey,
        /// Underlying message.
        message: String,
    },
    /// Evaluation error on a parameter's formula.
    ParameterEval {
        /// Parameter name.
        name: String,
        /// Underlying message.
        message: String,
    },
    /// Evaluation error on a binding's formula.
    BindingEval {
        /// Binding key.
        key: BindingKey,
        /// Underlying message.
        message: String,
    },
    /// Circular dependency among parameters.
    Cycle {
        /// The cycle path.
        path: Vec<String>,
    },
    /// A binding referenced a variable that isn't a parameter.
    UnknownVariable {
        /// Binding key.
        key: BindingKey,
        /// Missing variable name.
        var: String,
    },
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParameterParse { name, message } => {
                write!(f, "parameter '{}': {}", name, message)
            }
            Self::BindingParse { key, message } => {
                write!(f, "binding '{}': {}", key, message)
            }
            Self::ParameterEval { name, message } => {
                write!(f, "parameter '{}': {}", name, message)
            }
            Self::BindingEval { key, message } => {
                write!(f, "binding '{}': {}", key, message)
            }
            Self::Cycle { path } => {
                write!(f, "cycle in parameter dependencies: {}", path.join(" → "))
            }
            Self::UnknownVariable { key, var } => {
                write!(f, "binding '{}' references unknown variable '{}'", key, var)
            }
        }
    }
}

impl std::error::Error for ResolveError {}

/// Evaluate every parameter in dependency order, returning the environment
/// (name → concrete value). Propagates parse / eval / cycle errors.
pub fn resolve_parameters(
    params: &HashMap<String, Parameter>,
) -> Result<HashMap<String, f64>, ResolveError> {
    // Parse each parameter's formula once so free-var analysis is possible.
    let mut asts: HashMap<String, Option<tang_expr_parser::Ast>> = HashMap::new();
    for (name, p) in params {
        match &p.value {
            Expr::Number(_) => {
                asts.insert(name.clone(), None);
            }
            Expr::Formula(s) => {
                let ast = tang_expr_parser::parse(s).map_err(|e| ResolveError::ParameterParse {
                    name: name.clone(),
                    message: e.to_string(),
                })?;
                asts.insert(name.clone(), Some(ast));
            }
        }
    }

    // Build dependency graph: name → set of referenced parameter names.
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for (name, maybe_ast) in &asts {
        let mut free = Vec::new();
        if let Some(ast) = maybe_ast {
            for v in tang_expr_parser::free_vars(ast) {
                if params.contains_key(&v) {
                    free.push(v);
                }
            }
        }
        deps.insert(name.clone(), free);
    }

    // Topological sort with cycle reporting (DFS with color marks).
    #[derive(Clone, Copy)]
    enum Mark {
        White,
        Gray,
        Black,
    }
    let mut marks: HashMap<String, Mark> =
        params.keys().map(|k| (k.clone(), Mark::White)).collect();
    let mut order: Vec<String> = Vec::with_capacity(params.len());
    let mut stack: Vec<String> = Vec::new();

    fn visit(
        node: &str,
        deps: &HashMap<String, Vec<String>>,
        marks: &mut HashMap<String, Mark>,
        order: &mut Vec<String>,
        stack: &mut Vec<String>,
    ) -> Result<(), ResolveError> {
        match marks.get(node).copied() {
            Some(Mark::Black) => return Ok(()),
            Some(Mark::Gray) => {
                // Cycle: rebuild the path from the stack.
                let idx = stack.iter().position(|n| n == node).unwrap_or(0);
                let mut path: Vec<String> = stack[idx..].to_vec();
                path.push(node.to_string());
                return Err(ResolveError::Cycle { path });
            }
            _ => {}
        }
        marks.insert(node.to_string(), Mark::Gray);
        stack.push(node.to_string());
        if let Some(children) = deps.get(node) {
            for child in children {
                visit(child, deps, marks, order, stack)?;
            }
        }
        stack.pop();
        marks.insert(node.to_string(), Mark::Black);
        order.push(node.to_string());
        Ok(())
    }

    let names: Vec<String> = params.keys().cloned().collect();
    for n in &names {
        visit(n, &deps, &mut marks, &mut order, &mut stack)?;
    }

    // Evaluate in dependency order.
    let mut env: HashMap<String, f64> = HashMap::new();
    for name in &order {
        let param = &params[name];
        let value = match &param.value {
            Expr::Number(v) => *v,
            Expr::Formula(_) => {
                let ast = asts[name].as_ref().expect("parsed above");
                tang_expr_parser::eval(ast, &env).map_err(|e| ResolveError::ParameterEval {
                    name: name.clone(),
                    message: e.to_string(),
                })?
            }
        };
        env.insert(name.clone(), value);
    }
    Ok(env)
}

/// Evaluate a single binding against a resolved parameter environment.
pub fn resolve_binding(
    key: &BindingKey,
    expr: &Expr,
    env: &HashMap<String, f64>,
) -> Result<f64, ResolveError> {
    match expr {
        Expr::Number(v) => Ok(*v),
        Expr::Formula(s) => {
            let ast = tang_expr_parser::parse(s).map_err(|e| ResolveError::BindingParse {
                key: key.clone(),
                message: e.to_string(),
            })?;
            for v in tang_expr_parser::free_vars(&ast) {
                if !matches!(v.as_str(), "pi" | "tau" | "e") && !env.contains_key(&v) {
                    return Err(ResolveError::UnknownVariable {
                        key: key.clone(),
                        var: v,
                    });
                }
            }
            tang_expr_parser::eval(&ast, env).map_err(|e| ResolveError::BindingEval {
                key: key.clone(),
                message: e.to_string(),
            })
        }
    }
}

/// Validate that every binding's referenced variables exist in the env
/// and every formula parses. Returns the set of keys that would fail.
pub fn validate_bindings(
    bindings: &Bindings,
    env: &HashMap<String, f64>,
) -> Result<(), Vec<ResolveError>> {
    let mut errors = Vec::new();
    for (k, e) in bindings.iter() {
        if let Err(err) = resolve_binding(k, e, env) {
            errors.push(err);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Build a HashSet of parameter names referenced by any binding formula.
/// Used by the UI to highlight parameters that have live references.
pub fn referenced_parameter_names(bindings: &Bindings) -> HashSet<String> {
    let mut out = HashSet::new();
    for (_k, expr) in bindings.iter() {
        if let Expr::Formula(s) = expr {
            if let Ok(ast) = tang_expr_parser::parse(s) {
                for v in tang_expr_parser::free_vars(&ast) {
                    out.insert(v);
                }
            }
        }
    }
    out
}

// Local alias so parameter resolution code stays readable.
mod tang_expr_parser {
    pub use crate::expr_parser::{eval, free_vars, parse, Ast};
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn param_literal(v: f64) -> Parameter {
        Parameter::literal(v)
    }

    fn param_derived(s: &str) -> Parameter {
        Parameter::derived(s)
    }

    #[test]
    fn expr_serde_is_untagged() {
        let n: Expr = serde_json::from_str("5").unwrap();
        assert_eq!(n, Expr::Number(5.0));
        let f: Expr = serde_json::from_str(r#""wheelbase * 0.5""#).unwrap();
        assert_eq!(f, Expr::Formula("wheelbase * 0.5".to_string()));
        assert_eq!(serde_json::to_string(&Expr::Number(3.0)).unwrap(), "3.0");
        assert_eq!(
            serde_json::to_string(&Expr::Formula("a".into())).unwrap(),
            r#""a""#
        );
    }

    #[test]
    fn parameters_simple_and_derived() {
        let mut params = HashMap::new();
        params.insert("wheelbase".to_string(), param_literal(1000.0));
        params.insert("half".to_string(), param_derived("wheelbase * 0.5"));
        let env = resolve_parameters(&params).unwrap();
        assert_eq!(env["wheelbase"], 1000.0);
        assert_eq!(env["half"], 500.0);
    }

    #[test]
    fn parameters_cycle_detected() {
        let mut params = HashMap::new();
        params.insert("a".to_string(), param_derived("b + 1"));
        params.insert("b".to_string(), param_derived("a + 1"));
        let err = resolve_parameters(&params).unwrap_err();
        assert!(matches!(err, ResolveError::Cycle { .. }));
    }

    #[test]
    fn parameters_chained_order_independent() {
        // Inserted in "leaves last" order on purpose.
        let mut params = HashMap::new();
        params.insert("c".to_string(), param_derived("b * 2"));
        params.insert("b".to_string(), param_derived("a + 10"));
        params.insert("a".to_string(), param_literal(5.0));
        let env = resolve_parameters(&params).unwrap();
        assert_eq!(env["a"], 5.0);
        assert_eq!(env["b"], 15.0);
        assert_eq!(env["c"], 30.0);
    }

    #[test]
    fn binding_key_roundtrip() {
        let k = BindingKey::new(42, "size.x");
        let s = serde_json::to_string(&k).unwrap();
        assert_eq!(s, r#""42:size.x""#);
        let back: BindingKey = serde_json::from_str(&s).unwrap();
        assert_eq!(back, k);
    }

    #[test]
    fn bindings_resolve_against_env() {
        let mut env = HashMap::new();
        env.insert("wheelbase".to_string(), 1000.0);
        let k = BindingKey::new(1, "radius");
        assert_eq!(
            resolve_binding(&k, &Expr::formula("wheelbase * 0.05"), &env).unwrap(),
            50.0
        );
        assert_eq!(resolve_binding(&k, &Expr::num(7.0), &env).unwrap(), 7.0);
    }

    #[test]
    fn bindings_unknown_variable() {
        let env = HashMap::new();
        let k = BindingKey::new(1, "x");
        let err = resolve_binding(&k, &Expr::formula("missing_param"), &env).unwrap_err();
        assert!(matches!(err, ResolveError::UnknownVariable { .. }));
    }

    #[test]
    fn parameter_parse_error() {
        let mut params = HashMap::new();
        params.insert("bad".to_string(), param_derived("1 + *"));
        assert!(matches!(
            resolve_parameters(&params).unwrap_err(),
            ResolveError::ParameterParse { .. }
        ));
    }

    #[test]
    fn referenced_names_collects_all() {
        let mut b = Bindings::new();
        b.bind(BindingKey::new(1, "x"), Expr::formula("a + b"));
        b.bind(BindingKey::new(2, "y"), Expr::formula("b * 2"));
        b.bind(BindingKey::new(3, "z"), Expr::num(5.0));
        let names = referenced_parameter_names(&b);
        assert!(names.contains("a"));
        assert!(names.contains("b"));
        assert_eq!(names.len(), 2);
    }
}
