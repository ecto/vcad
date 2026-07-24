//! Parametric-expression WASM bindings.
//!
//! Exposes the `vcad-ir` expression parser/evaluator and the `vcad-eval`
//! parameter/binding resolution pre-pass to TypeScript, so the engine's
//! `expressions.ts` can be a thin wrapper instead of a hand-maintained
//! mirror of the grammar. Rust is the single source of truth for
//! expression semantics.
//!
//! The AST crosses the boundary in the wire shape the TS side has always
//! used (`{ kind: "num" | "ident" | "binary" | "unary" | "call", ... }`)
//! so existing consumers that walk the AST (e.g. parameter rename) keep
//! working unchanged.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use vcad_ir::expr_parser::{self, Ast, BinOp, UnOp};
use wasm_bindgen::prelude::*;

// ============================================================================
// Wire AST (matches the historical TS discriminated union)
// ============================================================================

/// Expression AST in the TS wire shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WireAst {
    /// Literal number.
    Num {
        /// The value.
        value: f64,
    },
    /// Identifier reference.
    Ident {
        /// Identifier name.
        name: String,
    },
    /// Binary operation.
    Binary {
        /// Operator symbol: `+ - * / % ^`.
        op: String,
        /// Left operand.
        lhs: Box<WireAst>,
        /// Right operand.
        rhs: Box<WireAst>,
    },
    /// Unary operation.
    Unary {
        /// Operator symbol: `+` or `-`.
        op: String,
        /// Operand.
        arg: Box<WireAst>,
    },
    /// Function call.
    Call {
        /// Function name.
        name: String,
        /// Arguments.
        args: Vec<WireAst>,
    },
}

fn bin_op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Mod => "%",
        BinOp::Pow => "^",
    }
}

fn bin_op_from_str(s: &str) -> Option<BinOp> {
    Some(match s {
        "+" => BinOp::Add,
        "-" => BinOp::Sub,
        "*" => BinOp::Mul,
        "/" => BinOp::Div,
        "%" => BinOp::Mod,
        "^" => BinOp::Pow,
        _ => return None,
    })
}

impl From<&Ast> for WireAst {
    fn from(ast: &Ast) -> Self {
        match ast {
            Ast::Number(v) => WireAst::Num { value: *v },
            Ast::Ident(name) => WireAst::Ident { name: name.clone() },
            Ast::Binary { op, lhs, rhs } => WireAst::Binary {
                op: bin_op_str(*op).to_string(),
                lhs: Box::new(WireAst::from(lhs.as_ref())),
                rhs: Box::new(WireAst::from(rhs.as_ref())),
            },
            Ast::Unary { op, arg } => WireAst::Unary {
                op: match op {
                    UnOp::Neg => "-".to_string(),
                    UnOp::Pos => "+".to_string(),
                },
                arg: Box::new(WireAst::from(arg.as_ref())),
            },
            Ast::Call { name, args } => WireAst::Call {
                name: name.clone(),
                args: args.iter().map(WireAst::from).collect(),
            },
        }
    }
}

impl TryFrom<&WireAst> for Ast {
    type Error = String;

    fn try_from(w: &WireAst) -> Result<Self, Self::Error> {
        Ok(match w {
            WireAst::Num { value } => Ast::Number(*value),
            WireAst::Ident { name } => Ast::Ident(name.clone()),
            WireAst::Binary { op, lhs, rhs } => Ast::Binary {
                op: bin_op_from_str(op).ok_or_else(|| format!("unknown binary op '{op}'"))?,
                lhs: Box::new(Ast::try_from(lhs.as_ref())?),
                rhs: Box::new(Ast::try_from(rhs.as_ref())?),
            },
            WireAst::Unary { op, arg } => Ast::Unary {
                op: match op.as_str() {
                    "-" => UnOp::Neg,
                    "+" => UnOp::Pos,
                    other => return Err(format!("unknown unary op '{other}'")),
                },
                arg: Box::new(Ast::try_from(arg.as_ref())?),
            },
            WireAst::Call { name, args } => Ast::Call {
                name: name.clone(),
                args: args
                    .iter()
                    .map(Ast::try_from)
                    .collect::<Result<Vec<_>, _>>()?,
            },
        })
    }
}

fn json_serializer() -> serde_wasm_bindgen::Serializer {
    // Plain JS objects (not Maps) so TS consumers can index the result.
    serde_wasm_bindgen::Serializer::json_compatible()
}

// ============================================================================
// Bindings
// ============================================================================

/// Parse an expression string into its wire AST.
/// Errors carry the message `parse error at offset N: ...`.
#[wasm_bindgen(js_name = exprParse)]
pub fn expr_parse(src: &str) -> Result<JsValue, JsError> {
    let ast = expr_parser::parse(src).map_err(|e| JsError::new(&e.to_string()))?;
    WireAst::from(&ast)
        .serialize(&json_serializer())
        .map_err(|e| JsError::new(&e.to_string()))
}

/// Evaluate a previously parsed wire AST against `env` (a plain
/// `{ name: number }` object).
#[wasm_bindgen(js_name = exprEvalAst)]
pub fn expr_eval_ast(ast: JsValue, env: JsValue) -> Result<f64, JsError> {
    let wire: WireAst =
        serde_wasm_bindgen::from_value(ast).map_err(|e| JsError::new(&e.to_string()))?;
    let ast = Ast::try_from(&wire).map_err(|e| JsError::new(&e))?;
    let env: HashMap<String, f64> =
        serde_wasm_bindgen::from_value(env).map_err(|e| JsError::new(&e.to_string()))?;
    expr_parser::eval(&ast, &env).map_err(|e| JsError::new(&e.to_string()))
}

/// Parse and evaluate an expression string in one shot.
#[wasm_bindgen(js_name = exprEvaluate)]
pub fn expr_evaluate(src: &str, env: JsValue) -> Result<f64, JsError> {
    let env: HashMap<String, f64> =
        serde_wasm_bindgen::from_value(env).map_err(|e| JsError::new(&e.to_string()))?;
    expr_parser::parse_and_eval(src, &env).map_err(|e| JsError::new(&e.to_string()))
}

/// Resolve a `{ name: Parameter }` map (JSON string) into a concrete
/// environment, returned as a JSON string `{ name: number }`.
#[wasm_bindgen(js_name = resolveParametersJson)]
pub fn resolve_parameters_json(params_json: &str) -> Result<String, JsError> {
    let params: HashMap<String, vcad_ir::Parameter> =
        serde_json::from_str(params_json).map_err(|e| JsError::new(&e.to_string()))?;
    let env = vcad_ir::resolve_parameters(&params).map_err(|e| JsError::new(&e.to_string()))?;
    serde_json::to_string(&env).map_err(|e| JsError::new(&e.to_string()))
}

/// Resolve a whole document: evaluate parameters, apply bindings onto
/// concrete node fields. Takes the document as a JSON string and returns
/// `{"doc": <resolved document>, "env": {name: number}}` as a JSON string.
#[wasm_bindgen(js_name = resolveDocumentJson)]
pub fn resolve_document_json(doc_json: &str) -> Result<String, JsError> {
    let doc: vcad_ir::Document =
        serde_json::from_str(doc_json).map_err(|e| JsError::new(&e.to_string()))?;
    let (resolved, env) =
        vcad_eval::resolve_document_cloned(&doc).map_err(|e| JsError::new(&e.to_string()))?;
    #[derive(Serialize)]
    struct Out {
        doc: vcad_ir::Document,
        env: HashMap<String, f64>,
    }
    serde_json::to_string(&Out { doc: resolved, env }).map_err(|e| JsError::new(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_ast_round_trips() {
        let ast = expr_parser::parse("min(-a, 2) * (b + 1) ^ 2 % 3").unwrap();
        let wire = WireAst::from(&ast);
        let back = Ast::try_from(&wire).unwrap();
        assert_eq!(ast, back);
    }

    #[test]
    fn wire_ast_serde_shape_matches_ts() {
        let ast = expr_parser::parse("x + 1").unwrap();
        let json = serde_json::to_value(WireAst::from(&ast)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": "binary",
                "op": "+",
                "lhs": { "kind": "ident", "name": "x" },
                "rhs": { "kind": "num", "value": 1.0 },
            })
        );
    }
}
