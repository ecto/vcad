//! `when` context: a small bitflags-shaped struct plus a boolean expression
//! parser that commands use to gate their bindings.
//!
//! Hosts build a [`WhenContext`] each time they dispatch a key event, reading
//! from their own state (selection size, whether an input is focused, etc.),
//! and hand it to [`crate::registry::KeybindingRegistry::resolve`]. The
//! registry evaluates each candidate command's [`WhenExpr`] against it.
//!
//! The expression language is intentionally minimal — just flag identifiers,
//! `!`, `&&`, `||`, and parentheses. Enough to express things like
//! `!input_focused && two_selected` without pulling in a full parser.

use std::collections::HashSet;

/// Packed flag set for command-gating. Stored as `u32` so it can cross the
/// WASM boundary as a single integer — simpler than serializing a struct.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct WhenContext(u32);

impl WhenContext {
    pub const NONE: Self = Self(0);

    // ── Focus / input ────────────────────────────────────────────────
    pub const INPUT_FOCUSED: Self = Self(1 << 0);
    pub const MENU_OPEN: Self = Self(1 << 1);
    pub const COMMAND_MODE: Self = Self(1 << 2);

    // ── Selection ────────────────────────────────────────────────────
    pub const HAS_SELECTION: Self = Self(1 << 3);
    pub const TWO_SELECTED: Self = Self(1 << 4);
    pub const ONE_PART: Self = Self(1 << 5);

    // ── Document ─────────────────────────────────────────────────────
    pub const HAS_PARTS: Self = Self(1 << 6);
    pub const CAN_UNDO: Self = Self(1 << 7);
    pub const CAN_REDO: Self = Self(1 << 8);

    // ── Mode-specific flags ──────────────────────────────────────────
    pub const SKETCH_HAS_POINTS: Self = Self(1 << 9);
    pub const PHYSICS_RUNNING: Self = Self(1 << 10);
    pub const ELECTRONICS_ACTIVE: Self = Self(1 << 11);

    /// Construct from a raw bit-pattern (used when crossing the wasm
    /// boundary).
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    pub fn set(&mut self, flag: Self, on: bool) {
        if on {
            self.insert(flag);
        } else {
            self.remove(flag);
        }
    }

    /// Look up a flag identifier by name for the when-expression parser.
    pub fn parse_flag(name: &str) -> Option<Self> {
        Some(match name {
            "input_focused" => Self::INPUT_FOCUSED,
            "menu_open" => Self::MENU_OPEN,
            "command_mode" => Self::COMMAND_MODE,
            "has_selection" => Self::HAS_SELECTION,
            "two_selected" => Self::TWO_SELECTED,
            "one_part" => Self::ONE_PART,
            "has_parts" => Self::HAS_PARTS,
            "can_undo" => Self::CAN_UNDO,
            "can_redo" => Self::CAN_REDO,
            "sketch_has_points" => Self::SKETCH_HAS_POINTS,
            "physics_running" => Self::PHYSICS_RUNNING,
            "electronics_active" => Self::ELECTRONICS_ACTIVE,
            _ => return None,
        })
    }

    /// List all recognized flag names — used by validation and UI.
    pub fn all_flag_names() -> &'static [&'static str] {
        &[
            "input_focused",
            "menu_open",
            "command_mode",
            "has_selection",
            "two_selected",
            "one_part",
            "has_parts",
            "can_undo",
            "can_redo",
            "sketch_has_points",
            "physics_running",
            "electronics_active",
        ]
    }
}

impl std::ops::BitOr for WhenContext {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for WhenContext {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Parsed boolean expression over `WhenContext` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhenExpr {
    /// Always true (represents an absent `when` clause).
    True,
    Flag(WhenContext),
    Not(Box<WhenExpr>),
    And(Box<WhenExpr>, Box<WhenExpr>),
    Or(Box<WhenExpr>, Box<WhenExpr>),
}

impl WhenExpr {
    /// Parse a when-expression. Grammar:
    /// ```text
    /// expr    = or_expr
    /// or_expr = and_expr ("||" and_expr)*
    /// and_expr = unary ("&&" unary)*
    /// unary   = "!" unary | primary
    /// primary = "(" expr ")" | ident | "true"
    /// ```
    pub fn parse(src: &str) -> Result<Self, WhenParseError> {
        let mut p = Parser {
            src,
            bytes: src.as_bytes(),
            pos: 0,
        };
        let expr = p.parse_or()?;
        p.skip_ws();
        if p.pos != p.bytes.len() {
            return Err(WhenParseError::Expected {
                expected: "end of expression",
                pos: p.pos,
                src: src.to_string(),
            });
        }
        Ok(expr)
    }

    /// Evaluate against a context.
    pub fn eval(&self, ctx: WhenContext) -> bool {
        match self {
            WhenExpr::True => true,
            WhenExpr::Flag(f) => ctx.contains(*f),
            WhenExpr::Not(e) => !e.eval(ctx),
            WhenExpr::And(a, b) => a.eval(ctx) && b.eval(ctx),
            WhenExpr::Or(a, b) => a.eval(ctx) || b.eval(ctx),
        }
    }

    /// Collect the flag names referenced by this expression. Used for UI
    /// and validation.
    pub fn referenced_flags(&self) -> HashSet<WhenContext> {
        let mut out = HashSet::new();
        self.walk(&mut out);
        out
    }

    fn walk(&self, out: &mut HashSet<WhenContext>) {
        match self {
            WhenExpr::True => {}
            WhenExpr::Flag(f) => {
                out.insert(*f);
            }
            WhenExpr::Not(e) => e.walk(out),
            WhenExpr::And(a, b) | WhenExpr::Or(a, b) => {
                a.walk(out);
                b.walk(out);
            }
        }
    }
}

struct Parser<'a> {
    src: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        self.skip_ws();
        if self.src[self.pos..].starts_with(lit) {
            self.pos += lit.len();
            true
        } else {
            false
        }
    }

    fn parse_or(&mut self) -> Result<WhenExpr, WhenParseError> {
        let mut lhs = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.eat("||") {
                let rhs = self.parse_and()?;
                lhs = WhenExpr::Or(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> Result<WhenExpr, WhenParseError> {
        let mut lhs = self.parse_unary()?;
        loop {
            self.skip_ws();
            if self.eat("&&") {
                let rhs = self.parse_unary()?;
                lhs = WhenExpr::And(Box::new(lhs), Box::new(rhs));
            } else {
                break;
            }
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> Result<WhenExpr, WhenParseError> {
        self.skip_ws();
        if self.eat("!") {
            let inner = self.parse_unary()?;
            return Ok(WhenExpr::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<WhenExpr, WhenParseError> {
        self.skip_ws();
        if self.eat("(") {
            let inner = self.parse_or()?;
            self.skip_ws();
            if !self.eat(")") {
                return Err(WhenParseError::Expected {
                    expected: ")",
                    pos: self.pos,
                    src: self.src.to_string(),
                });
            }
            return Ok(inner);
        }
        let start = self.pos;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            if c.is_ascii_alphanumeric() || c == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if start == self.pos {
            return Err(WhenParseError::UnexpectedEnd(self.src.to_string()));
        }
        let ident = &self.src[start..self.pos];
        if ident == "true" {
            return Ok(WhenExpr::True);
        }
        let flag = WhenContext::parse_flag(ident)
            .ok_or_else(|| WhenParseError::UnknownFlag(ident.to_string()))?;
        Ok(WhenExpr::Flag(flag))
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WhenParseError {
    #[error("unexpected end of when-expression: {0:?}")]
    UnexpectedEnd(String),
    #[error("expected {expected} at position {pos} in {src:?}")]
    Expected {
        expected: &'static str,
        pos: usize,
        src: String,
    },
    #[error("unknown flag: {0:?}")]
    UnknownFlag(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(bits: &[WhenContext]) -> WhenContext {
        let mut c = WhenContext::NONE;
        for b in bits {
            c.insert(*b);
        }
        c
    }

    #[test]
    fn simple_flag() {
        let e = WhenExpr::parse("has_selection").unwrap();
        assert!(e.eval(make(&[WhenContext::HAS_SELECTION])));
        assert!(!e.eval(WhenContext::NONE));
    }

    #[test]
    fn not_and_or() {
        let e = WhenExpr::parse("!input_focused && two_selected").unwrap();
        assert!(e.eval(make(&[WhenContext::TWO_SELECTED])));
        assert!(!e.eval(make(&[
            WhenContext::TWO_SELECTED,
            WhenContext::INPUT_FOCUSED,
        ])));
        assert!(!e.eval(make(&[WhenContext::INPUT_FOCUSED])));
    }

    #[test]
    fn or_is_lower_precedence_than_and() {
        // `a || b && c` parses as `a || (b && c)`.
        let e = WhenExpr::parse("has_selection || two_selected && !input_focused").unwrap();
        assert!(e.eval(make(&[WhenContext::HAS_SELECTION]))); // lhs matches
        assert!(e.eval(make(&[WhenContext::TWO_SELECTED]))); // rhs matches
        assert!(!e.eval(make(&[
            WhenContext::TWO_SELECTED,
            WhenContext::INPUT_FOCUSED,
        ]))); // rhs blocked by input_focused, lhs absent
    }

    #[test]
    fn parens() {
        let e = WhenExpr::parse("(has_selection || two_selected) && !input_focused").unwrap();
        assert!(e.eval(make(&[WhenContext::HAS_SELECTION])));
        assert!(!e.eval(make(&[
            WhenContext::HAS_SELECTION,
            WhenContext::INPUT_FOCUSED,
        ])));
    }

    #[test]
    fn true_literal() {
        let e = WhenExpr::parse("true").unwrap();
        assert!(e.eval(WhenContext::NONE));
    }

    #[test]
    fn whitespace_tolerated() {
        let e = WhenExpr::parse("  has_selection   &&   !input_focused  ").unwrap();
        assert!(e.eval(make(&[WhenContext::HAS_SELECTION])));
    }

    #[test]
    fn unknown_flag_errors() {
        assert!(matches!(
            WhenExpr::parse("not_a_flag"),
            Err(WhenParseError::UnknownFlag(_))
        ));
    }

    #[test]
    fn bit_ops() {
        let a = WhenContext::HAS_SELECTION | WhenContext::TWO_SELECTED;
        assert!(a.contains(WhenContext::HAS_SELECTION));
        assert!(a.contains(WhenContext::TWO_SELECTED));
        assert!(!a.contains(WhenContext::INPUT_FOCUSED));

        let mut b = WhenContext::NONE;
        b.set(WhenContext::INPUT_FOCUSED, true);
        assert!(b.contains(WhenContext::INPUT_FOCUSED));
        b.set(WhenContext::INPUT_FOCUSED, false);
        assert!(!b.contains(WhenContext::INPUT_FOCUSED));
    }

    #[test]
    fn from_bits_round_trip() {
        let a = WhenContext::HAS_PARTS | WhenContext::CAN_UNDO;
        let bits = a.bits();
        let b = WhenContext::from_bits(bits);
        assert_eq!(a, b);
    }
}
