//! String-expression parser and direct evaluator.
//!
//! A self-contained Pratt parser for arithmetic expressions used by the
//! parameter resolution pre-pass. Kept inside `vcad-ir` (rather than
//! pushed down to `tang-expr`, which is oriented toward symbolic-diff
//! graphs) so the parametric-document feature doesn't leak into the
//! constraint-solver's dependency shape.
//!
//! # Grammar
//!
//! ```text
//! expr    := term (('+' | '-') term)*
//! term    := unary (('*' | '/' | '%') unary)*
//! unary   := ('-' | '+') unary | power
//! power   := atom ('^' unary)?                   // right-assoc, tighter than unary
//! atom    := NUMBER | IDENT ('(' args? ')')? | '(' expr ')'
//! args    := expr (',' expr)*
//! ```
//!
//! # Supported functions and constants
//!
//! - `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2(y, x)`
//! - `sqrt`, `abs`, `floor`, `ceil`, `round`
//! - `ln`, `log(x)` (natural log), `log2`, `exp`, `pow(b, e)`
//! - `min(a, b)`, `max(a, b)`
//! - `deg(rad)` → degrees, `rad(deg)` → radians
//! - Constants: `pi`, `tau`, `e`
//!
//! # Quick start
//!
//! ```
//! use std::collections::HashMap;
//! use vcad_ir::expr_parser::{parse, eval};
//!
//! let ast = parse("wheelbase * 0.5 + offset").unwrap();
//! let mut env = HashMap::new();
//! env.insert("wheelbase".to_string(), 1000.0);
//! env.insert("offset".to_string(), 25.0);
//! assert_eq!(eval(&ast, &env).unwrap(), 525.0);
//! ```

use std::collections::HashMap;
use std::fmt;

// ============================================================================
// AST
// ============================================================================

/// Parsed expression AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Ast {
    /// Literal number.
    Number(f64),
    /// Identifier reference (variable or named constant).
    Ident(String),
    /// Binary operation.
    Binary {
        /// Operator.
        op: BinOp,
        /// Left operand.
        lhs: Box<Ast>,
        /// Right operand.
        rhs: Box<Ast>,
    },
    /// Unary operation.
    Unary {
        /// Operator.
        op: UnOp,
        /// Operand.
        arg: Box<Ast>,
    },
    /// Function call.
    Call {
        /// Function name.
        name: String,
        /// Arguments.
        args: Vec<Ast>,
    },
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Mod,
    /// `^`
    Pow,
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    /// `-`
    Neg,
    /// `+` (no-op, for parser symmetry)
    Pos,
}

// ============================================================================
// Errors
// ============================================================================

/// Error produced while parsing an expression string.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseError {
    /// Human-readable message.
    pub message: String,
    /// Byte offset into the source string where the error was detected.
    pub offset: usize,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "parse error at offset {}: {}", self.offset, self.message)
    }
}

impl std::error::Error for ParseError {}

/// Error produced while evaluating an AST.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// Referenced an identifier not in the environment.
    UndefinedVariable(String),
    /// Called a function not in the builtin table.
    UnknownFunction(String),
    /// Called a function with the wrong number of arguments.
    ArityMismatch {
        /// Function name.
        name: String,
        /// Expected argument count.
        expected: usize,
        /// Actual argument count.
        got: usize,
    },
    /// Division by zero, log of zero/negative, etc.
    MathDomain(&'static str),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UndefinedVariable(name) => write!(f, "undefined variable: {}", name),
            Self::UnknownFunction(name) => write!(f, "unknown function: {}", name),
            Self::ArityMismatch {
                name,
                expected,
                got,
            } => write!(
                f,
                "function '{}' expected {} argument(s), got {}",
                name, expected, got
            ),
            Self::MathDomain(msg) => write!(f, "math domain error: {}", msg),
        }
    }
}

impl std::error::Error for EvalError {}

// ============================================================================
// Tokens
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Number(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    LParen,
    RParen,
    Comma,
    End,
}

struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src: src.as_bytes(),
            pos: 0,
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek_byte() {
            if c.is_ascii_whitespace() {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Result<(Tok, usize), ParseError> {
        self.skip_ws();
        let start = self.pos;
        let Some(c) = self.peek_byte() else {
            return Ok((Tok::End, start));
        };
        match c {
            b'+' => {
                self.pos += 1;
                Ok((Tok::Plus, start))
            }
            b'-' => {
                self.pos += 1;
                Ok((Tok::Minus, start))
            }
            b'*' => {
                self.pos += 1;
                Ok((Tok::Star, start))
            }
            b'/' => {
                self.pos += 1;
                Ok((Tok::Slash, start))
            }
            b'%' => {
                self.pos += 1;
                Ok((Tok::Percent, start))
            }
            b'^' => {
                self.pos += 1;
                Ok((Tok::Caret, start))
            }
            b'(' => {
                self.pos += 1;
                Ok((Tok::LParen, start))
            }
            b')' => {
                self.pos += 1;
                Ok((Tok::RParen, start))
            }
            b',' => {
                self.pos += 1;
                Ok((Tok::Comma, start))
            }
            b'0'..=b'9' | b'.' => self.lex_number(start),
            c if c == b'_' || c.is_ascii_alphabetic() => self.lex_ident(start),
            other => Err(ParseError {
                message: format!("unexpected character '{}'", other as char),
                offset: start,
            }),
        }
    }

    fn lex_number(&mut self, start: usize) -> Result<(Tok, usize), ParseError> {
        let mut has_digit = false;
        let mut has_dot = false;
        let mut has_exp = false;
        while let Some(c) = self.peek_byte() {
            match c {
                b'0'..=b'9' => {
                    has_digit = true;
                    self.pos += 1;
                }
                b'.' if !has_dot && !has_exp => {
                    has_dot = true;
                    self.pos += 1;
                }
                b'e' | b'E' if !has_exp => {
                    has_exp = true;
                    self.pos += 1;
                    if matches!(self.peek_byte(), Some(b'+') | Some(b'-')) {
                        self.pos += 1;
                    }
                }
                _ => break,
            }
        }
        if !has_digit {
            return Err(ParseError {
                message: "expected digit in number literal".to_string(),
                offset: start,
            });
        }
        let slice = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        let value: f64 = slice.parse().map_err(|_| ParseError {
            message: format!("invalid number literal '{}'", slice),
            offset: start,
        })?;
        Ok((Tok::Number(value), start))
    }

    fn lex_ident(&mut self, start: usize) -> Result<(Tok, usize), ParseError> {
        while let Some(c) = self.peek_byte() {
            if c == b'_' || c.is_ascii_alphanumeric() {
                self.pos += 1;
            } else {
                break;
            }
        }
        let slice = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        Ok((Tok::Ident(slice.to_string()), start))
    }
}

// ============================================================================
// Parser (Pratt-style precedence climbing)
// ============================================================================

struct Parser<'a> {
    lex: Lexer<'a>,
    cur: Tok,
    cur_offset: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Result<Self, ParseError> {
        let mut lex = Lexer::new(src);
        let (tok, offset) = lex.next_token()?;
        Ok(Self {
            lex,
            cur: tok,
            cur_offset: offset,
        })
    }

    fn advance(&mut self) -> Result<(), ParseError> {
        let (tok, offset) = self.lex.next_token()?;
        self.cur = tok;
        self.cur_offset = offset;
        Ok(())
    }

    fn expect(&mut self, expected: &Tok, what: &str) -> Result<(), ParseError> {
        if std::mem::discriminant(&self.cur) == std::mem::discriminant(expected) {
            self.advance()?;
            Ok(())
        } else {
            Err(ParseError {
                message: format!("expected {}", what),
                offset: self.cur_offset,
            })
        }
    }

    fn parse_expr(&mut self) -> Result<Ast, ParseError> {
        let mut lhs = self.parse_term()?;
        loop {
            let op = match &self.cur {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_term()?;
            lhs = Ast::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_term(&mut self) -> Result<Ast, ParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match &self.cur {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                Tok::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance()?;
            let rhs = self.parse_unary()?;
            lhs = Ast::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<Ast, ParseError> {
        match &self.cur {
            Tok::Minus => {
                self.advance()?;
                let arg = self.parse_unary()?;
                Ok(Ast::Unary {
                    op: UnOp::Neg,
                    arg: Box::new(arg),
                })
            }
            Tok::Plus => {
                self.advance()?;
                let arg = self.parse_unary()?;
                Ok(Ast::Unary {
                    op: UnOp::Pos,
                    arg: Box::new(arg),
                })
            }
            _ => self.parse_power(),
        }
    }

    fn parse_power(&mut self) -> Result<Ast, ParseError> {
        let lhs = self.parse_atom()?;
        if matches!(&self.cur, Tok::Caret) {
            self.advance()?;
            // RHS is unary so `2 ^ -3` parses and `2 ^ 3 ^ 2` stays right-assoc.
            let rhs = self.parse_unary()?;
            Ok(Ast::Binary {
                op: BinOp::Pow,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            })
        } else {
            Ok(lhs)
        }
    }

    fn parse_atom(&mut self) -> Result<Ast, ParseError> {
        match std::mem::replace(&mut self.cur, Tok::End) {
            Tok::Number(n) => {
                self.advance()?;
                Ok(Ast::Number(n))
            }
            Tok::LParen => {
                self.advance()?;
                let e = self.parse_expr()?;
                self.expect(&Tok::RParen, "')'")?;
                Ok(e)
            }
            Tok::Ident(name) => {
                self.advance()?;
                if matches!(&self.cur, Tok::LParen) {
                    self.advance()?;
                    let mut args = Vec::new();
                    if !matches!(&self.cur, Tok::RParen) {
                        args.push(self.parse_expr()?);
                        while matches!(&self.cur, Tok::Comma) {
                            self.advance()?;
                            args.push(self.parse_expr()?);
                        }
                    }
                    self.expect(&Tok::RParen, "')'")?;
                    Ok(Ast::Call { name, args })
                } else {
                    Ok(Ast::Ident(name))
                }
            }
            tok => {
                // Restore cur so offset reporting is sane.
                self.cur = tok;
                Err(ParseError {
                    message: "expected number, identifier, or '('".to_string(),
                    offset: self.cur_offset,
                })
            }
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Parse a string expression into an [`Ast`].
pub fn parse(input: &str) -> Result<Ast, ParseError> {
    let mut parser = Parser::new(input)?;
    let ast = parser.parse_expr()?;
    if !matches!(parser.cur, Tok::End) {
        return Err(ParseError {
            message: "unexpected trailing input".to_string(),
            offset: parser.cur_offset,
        });
    }
    Ok(ast)
}

/// Evaluate an [`Ast`] against a named-variable environment.
pub fn eval(ast: &Ast, env: &HashMap<String, f64>) -> Result<f64, EvalError> {
    match ast {
        Ast::Number(n) => Ok(*n),
        Ast::Ident(name) => match name.as_str() {
            "pi" => Ok(std::f64::consts::PI),
            "tau" => Ok(std::f64::consts::TAU),
            "e" => Ok(std::f64::consts::E),
            _ => env
                .get(name)
                .copied()
                .ok_or_else(|| EvalError::UndefinedVariable(name.clone())),
        },
        Ast::Binary { op, lhs, rhs } => {
            let l = eval(lhs, env)?;
            let r = eval(rhs, env)?;
            Ok(match op {
                BinOp::Add => l + r,
                BinOp::Sub => l - r,
                BinOp::Mul => l * r,
                BinOp::Div => {
                    if r == 0.0 {
                        return Err(EvalError::MathDomain("division by zero"));
                    }
                    l / r
                }
                BinOp::Mod => {
                    if r == 0.0 {
                        return Err(EvalError::MathDomain("modulo by zero"));
                    }
                    l.rem_euclid(r)
                }
                BinOp::Pow => l.powf(r),
            })
        }
        Ast::Unary { op, arg } => {
            let v = eval(arg, env)?;
            Ok(match op {
                UnOp::Neg => -v,
                UnOp::Pos => v,
            })
        }
        Ast::Call { name, args } => call_builtin(name, args, env),
    }
}

fn arity(name: &str, args: &[Ast], expected: usize) -> Result<(), EvalError> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(EvalError::ArityMismatch {
            name: name.to_string(),
            expected,
            got: args.len(),
        })
    }
}

fn call_builtin(name: &str, args: &[Ast], env: &HashMap<String, f64>) -> Result<f64, EvalError> {
    let evaluated: Result<Vec<f64>, _> = args.iter().map(|a| eval(a, env)).collect();
    let v = evaluated?;
    match name {
        "sin" => {
            arity(name, args, 1)?;
            Ok(v[0].sin())
        }
        "cos" => {
            arity(name, args, 1)?;
            Ok(v[0].cos())
        }
        "tan" => {
            arity(name, args, 1)?;
            Ok(v[0].tan())
        }
        "asin" => {
            arity(name, args, 1)?;
            if !(-1.0..=1.0).contains(&v[0]) {
                return Err(EvalError::MathDomain("asin argument out of [-1, 1]"));
            }
            Ok(v[0].asin())
        }
        "acos" => {
            arity(name, args, 1)?;
            if !(-1.0..=1.0).contains(&v[0]) {
                return Err(EvalError::MathDomain("acos argument out of [-1, 1]"));
            }
            Ok(v[0].acos())
        }
        "atan" => {
            arity(name, args, 1)?;
            Ok(v[0].atan())
        }
        "atan2" => {
            arity(name, args, 2)?;
            Ok(v[0].atan2(v[1]))
        }
        "sqrt" => {
            arity(name, args, 1)?;
            if v[0] < 0.0 {
                return Err(EvalError::MathDomain("sqrt of negative"));
            }
            Ok(v[0].sqrt())
        }
        "abs" => {
            arity(name, args, 1)?;
            Ok(v[0].abs())
        }
        "floor" => {
            arity(name, args, 1)?;
            Ok(v[0].floor())
        }
        "ceil" => {
            arity(name, args, 1)?;
            Ok(v[0].ceil())
        }
        "round" => {
            arity(name, args, 1)?;
            Ok(v[0].round())
        }
        "ln" | "log" => {
            arity(name, args, 1)?;
            if v[0] <= 0.0 {
                return Err(EvalError::MathDomain("log of non-positive"));
            }
            Ok(v[0].ln())
        }
        "log2" => {
            arity(name, args, 1)?;
            if v[0] <= 0.0 {
                return Err(EvalError::MathDomain("log2 of non-positive"));
            }
            Ok(v[0].log2())
        }
        "exp" => {
            arity(name, args, 1)?;
            Ok(v[0].exp())
        }
        "pow" => {
            arity(name, args, 2)?;
            Ok(v[0].powf(v[1]))
        }
        "min" => {
            arity(name, args, 2)?;
            Ok(v[0].min(v[1]))
        }
        "max" => {
            arity(name, args, 2)?;
            Ok(v[0].max(v[1]))
        }
        "deg" => {
            arity(name, args, 1)?;
            Ok(v[0].to_degrees())
        }
        "rad" => {
            arity(name, args, 1)?;
            Ok(v[0].to_radians())
        }
        _ => Err(EvalError::UnknownFunction(name.to_string())),
    }
}

/// Collect every free identifier referenced by an AST (variables that
/// would need to be in the environment at evaluation time). Named
/// constants `pi`, `tau`, `e` are excluded.
pub fn free_vars(ast: &Ast) -> Vec<String> {
    fn walk(ast: &Ast, out: &mut Vec<String>) {
        match ast {
            Ast::Number(_) => {}
            Ast::Ident(name) => {
                if !matches!(name.as_str(), "pi" | "tau" | "e") && !out.contains(name) {
                    out.push(name.clone());
                }
            }
            Ast::Binary { lhs, rhs, .. } => {
                walk(lhs, out);
                walk(rhs, out);
            }
            Ast::Unary { arg, .. } => walk(arg, out),
            Ast::Call { args, .. } => {
                for a in args {
                    walk(a, out);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(ast, &mut out);
    out
}

/// Convenience: parse and evaluate in one shot.
pub fn parse_and_eval(input: &str, env: &HashMap<String, f64>) -> Result<f64, ExprError> {
    let ast = parse(input).map_err(ExprError::Parse)?;
    eval(&ast, env).map_err(ExprError::Eval)
}

/// Combined parse-or-eval error for convenience APIs.
#[derive(Debug, Clone, PartialEq)]
pub enum ExprError {
    /// Parser error.
    Parse(ParseError),
    /// Evaluator error.
    Eval(EvalError),
}

impl fmt::Display for ExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(e) => write!(f, "{}", e),
            Self::Eval(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ExprError {}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> HashMap<String, f64> {
        let mut e = HashMap::new();
        e.insert("x".to_string(), 3.0);
        e.insert("y".to_string(), 4.0);
        e.insert("wheelbase".to_string(), 1000.0);
        e
    }

    fn pe(s: &str) -> f64 {
        parse_and_eval(s, &env()).unwrap()
    }

    #[test]
    fn literals_and_ops() {
        assert_eq!(pe("1+2"), 3.0);
        assert_eq!(pe("1 - 2"), -1.0);
        assert_eq!(pe("2 * 3"), 6.0);
        assert_eq!(pe("10 / 4"), 2.5);
        assert_eq!(pe("7 % 3"), 1.0);
    }

    #[test]
    fn precedence() {
        assert_eq!(pe("1 + 2 * 3"), 7.0);
        assert_eq!(pe("(1 + 2) * 3"), 9.0);
        assert_eq!(pe("2 ^ 3 ^ 2"), 512.0); // right-assoc
        assert_eq!(pe("-2 ^ 2"), -4.0); // unary minus binds looser than ^
    }

    #[test]
    fn scientific() {
        assert_eq!(pe("1e3"), 1000.0);
        assert_eq!(pe("2.5e-2"), 0.025);
    }

    #[test]
    fn variables() {
        assert_eq!(pe("x + y"), 7.0);
        assert_eq!(pe("wheelbase * 0.5"), 500.0);
    }

    #[test]
    fn functions() {
        assert!((pe("sin(0)") - 0.0).abs() < 1e-12);
        assert!((pe("cos(0)") - 1.0).abs() < 1e-12);
        assert_eq!(pe("sqrt(9)"), 3.0);
        assert_eq!(pe("abs(-5)"), 5.0);
        assert_eq!(pe("min(3, 7)"), 3.0);
        assert_eq!(pe("max(3, 7)"), 7.0);
        assert_eq!(pe("pow(2, 10)"), 1024.0);
        assert_eq!(pe("round(2.7)"), 3.0);
    }

    #[test]
    fn constants() {
        assert!((pe("pi") - std::f64::consts::PI).abs() < 1e-12);
        assert!((pe("tau") - std::f64::consts::TAU).abs() < 1e-12);
        assert!((pe("e") - std::f64::consts::E).abs() < 1e-12);
    }

    #[test]
    fn deg_rad() {
        assert!((pe("deg(pi)") - 180.0).abs() < 1e-12);
        assert!((pe("rad(180)") - std::f64::consts::PI).abs() < 1e-12);
    }

    #[test]
    fn undefined_variable() {
        let ast = parse("a + 1").unwrap();
        let err = eval(&ast, &HashMap::new()).unwrap_err();
        assert!(matches!(err, EvalError::UndefinedVariable(ref n) if n == "a"));
    }

    #[test]
    fn unknown_function() {
        let err = parse_and_eval("bogus(1)", &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            ExprError::Eval(EvalError::UnknownFunction(_))
        ));
    }

    #[test]
    fn arity_mismatch() {
        let err = parse_and_eval("min(1)", &HashMap::new()).unwrap_err();
        assert!(matches!(
            err,
            ExprError::Eval(EvalError::ArityMismatch { .. })
        ));
    }

    #[test]
    fn parse_error_offset() {
        let err = parse("1 + * 2").unwrap_err();
        assert!(err.offset >= 4);
    }

    #[test]
    fn trailing_garbage_is_error() {
        assert!(parse("1 + 2 3").is_err());
    }

    #[test]
    fn free_vars_basic() {
        let ast = parse("a + b * 2 + a").unwrap();
        let vars = free_vars(&ast);
        assert_eq!(vars, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn free_vars_skips_constants() {
        let ast = parse("pi * r ^ 2").unwrap();
        let vars = free_vars(&ast);
        assert_eq!(vars, vec!["r".to_string()]);
    }

    #[test]
    fn unary_chain() {
        assert_eq!(pe("--3"), 3.0);
        assert_eq!(pe("+-3"), -3.0);
    }

    #[test]
    fn division_by_zero() {
        let err = parse_and_eval("1/0", &HashMap::new()).unwrap_err();
        assert!(matches!(err, ExprError::Eval(EvalError::MathDomain(_))));
    }

    #[test]
    fn nested_calls() {
        assert_eq!(pe("min(max(1, 2), 3)"), 2.0);
        assert!((pe("sqrt(pow(3, 2) + pow(4, 2))") - 5.0).abs() < 1e-12);
    }
}
