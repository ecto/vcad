//! Part 21 parser: builds a raw entity graph from tokens.
//!
//! The parser constructs a graph of STEP entities without interpreting their
//! semantics. Each entity has an ID, a type name, and a list of arguments.
//! Arguments can be nested (lists within lists).

use crate::error::StepError;
use crate::lexer::{Lexer, SpannedToken, Token};
use std::collections::HashMap;

/// A single argument value in a STEP entity.
#[derive(Debug, Clone, PartialEq)]
pub enum StepValue {
    /// Entity reference (e.g., `#123`).
    EntityRef(u64),
    /// String literal.
    String(String),
    /// Real number.
    Real(f64),
    /// Integer number.
    Integer(i64),
    /// Enumeration (e.g., `.TRUE.`).
    Enum(String),
    /// List of values (nested in parentheses).
    List(Vec<StepValue>),
    /// Derived/computed value (`*`).
    Derived,
    /// Null/unset value (`$`).
    Null,
    /// Typed value: `TYPE_NAME(args)` inline complex entity.
    Typed {
        /// The type name.
        type_name: String,
        /// Arguments.
        args: Vec<StepValue>,
    },
}

impl StepValue {
    /// Try to get as an entity reference.
    pub fn as_entity_ref(&self) -> Option<u64> {
        match self {
            StepValue::EntityRef(id) => Some(*id),
            _ => None,
        }
    }

    /// Try to get as a real number (also accepts integer).
    pub fn as_real(&self) -> Option<f64> {
        match self {
            StepValue::Real(v) => Some(*v),
            StepValue::Integer(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// Try to get as an integer.
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            StepValue::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Try to get as a string.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            StepValue::String(s) => Some(s),
            _ => None,
        }
    }

    /// Try to get as an enum.
    pub fn as_enum(&self) -> Option<&str> {
        match self {
            StepValue::Enum(s) => Some(s),
            _ => None,
        }
    }

    /// Try to get as a list.
    pub fn as_list(&self) -> Option<&[StepValue]> {
        match self {
            StepValue::List(v) => Some(v),
            _ => None,
        }
    }

    /// Check if this is a null value.
    pub fn is_null(&self) -> bool {
        matches!(self, StepValue::Null)
    }

    /// Check if this is a derived value.
    pub fn is_derived(&self) -> bool {
        matches!(self, StepValue::Derived)
    }
}

/// A parsed STEP entity.
#[derive(Debug, Clone)]
pub struct StepEntity {
    /// Entity ID (from `#123`).
    pub id: u64,
    /// Entity type name (e.g., `CARTESIAN_POINT`).
    pub type_name: String,
    /// Arguments to the entity constructor.
    pub args: Vec<StepValue>,
}

/// The complete parsed content of a STEP file.
#[derive(Debug, Clone)]
pub struct StepFile {
    /// Header section contents.
    pub header: Vec<StepEntity>,
    /// Data section entities, indexed by ID.
    pub entities: HashMap<u64, StepEntity>,
}

impl StepFile {
    /// Get an entity by ID.
    pub fn get(&self, id: u64) -> Option<&StepEntity> {
        self.entities.get(&id)
    }

    /// Get all entities of a given type.
    pub fn entities_of_type(&self, type_name: &str) -> Vec<&StepEntity> {
        self.entities
            .values()
            .filter(|e| e.type_name == type_name)
            .collect()
    }
}

/// Parser for Part 21 STEP files.
pub struct Parser {
    tokens: Vec<SpannedToken>,
    pos: usize,
}

impl Parser {
    /// Parse a STEP file from bytes.
    pub fn parse(input: &[u8]) -> Result<StepFile, StepError> {
        let mut lexer = Lexer::new(input);
        let tokens = lexer.tokenize()?;
        let mut parser = Parser { tokens, pos: 0 };
        parser.parse_file()
    }

    fn parse_file(&mut self) -> Result<StepFile, StepError> {
        let mut header = Vec::new();
        let mut entities = HashMap::new();

        // Parse ISO-10303-21; at the start
        self.expect_keyword("ISO-10303-21")?;
        self.expect_token(&Token::Semicolon)?;

        // Parse sections
        while !self.is_at_end() {
            if self.check_keyword("HEADER") {
                self.advance();
                self.expect_token(&Token::Semicolon)?;
                header = self.parse_section_entities()?;
                self.expect_keyword("ENDSEC")?;
                self.expect_token(&Token::Semicolon)?;
            } else if self.check_keyword("DATA") {
                self.advance();
                self.expect_token(&Token::Semicolon)?;
                let data_entities = self.parse_data_section()?;
                for entity in data_entities {
                    entities.insert(entity.id, entity);
                }
                self.expect_keyword("ENDSEC")?;
                self.expect_token(&Token::Semicolon)?;
            } else if self.check_keyword("END-ISO-10303-21") {
                self.advance();
                self.expect_token(&Token::Semicolon)?;
                break;
            } else {
                let tok = self.peek().cloned();
                return Err(StepError::parser(
                    None,
                    format!("unexpected token: {tok:?}"),
                ));
            }
        }

        Ok(StepFile { header, entities })
    }

    fn parse_section_entities(&mut self) -> Result<Vec<StepEntity>, StepError> {
        let mut entities = Vec::new();
        while !self.check_keyword("ENDSEC") && !self.is_at_end() {
            // Header entities don't have IDs, just type and args
            if let Some(Token::Keyword(type_name)) = self.peek().map(|t| t.token.clone()) {
                self.advance();
                let args = self.parse_args()?;
                self.expect_token(&Token::Semicolon)?;
                entities.push(StepEntity {
                    id: 0,
                    type_name,
                    args,
                });
            } else {
                break;
            }
        }
        Ok(entities)
    }

    fn parse_data_section(&mut self) -> Result<Vec<StepEntity>, StepError> {
        let mut entities = Vec::new();
        while !self.check_keyword("ENDSEC") && !self.is_at_end() {
            if let Some(Token::EntityRef(id)) = self.peek().map(|t| t.token.clone()) {
                self.advance();
                self.expect_token(&Token::Equals)?;

                // Check for complex entity: #id = (TYPE1(args) TYPE2(args) ...);
                if self.peek().map(|t| &t.token) == Some(&Token::LParen) {
                    // Complex entity - parse all typed components
                    self.advance(); // consume '('
                    let mut components = Vec::new();

                    while self.peek().map(|t| &t.token) != Some(&Token::RParen) {
                        // Each component is TYPE_NAME(args)
                        let comp_type = match self.peek().map(|t| t.token.clone()) {
                            Some(Token::Keyword(name)) => {
                                self.advance();
                                name
                            }
                            other => {
                                return Err(StepError::parser(
                                    Some(id),
                                    format!("expected type name in complex entity, got {other:?}"),
                                ));
                            }
                        };
                        let comp_args = self.parse_args()?;
                        components.push(StepValue::Typed {
                            type_name: comp_type,
                            args: comp_args,
                        });
                    }

                    self.expect_token(&Token::RParen)?;
                    self.expect_token(&Token::Semicolon)?;

                    // Use first type as the entity type, store others in args
                    let (type_name, args) = if let Some(StepValue::Typed {
                        type_name: first_type,
                        args: first_args,
                    }) = components.first().cloned()
                    {
                        let mut args = first_args;
                        // Append remaining types as Typed values
                        args.extend(components.into_iter().skip(1));
                        (first_type, args)
                    } else {
                        ("__COMPLEX__".to_string(), components)
                    };

                    entities.push(StepEntity {
                        id,
                        type_name,
                        args,
                    });
                } else {
                    // Simple entity: #id = TYPE_NAME(args);
                    let type_name = match self.peek().map(|t| t.token.clone()) {
                        Some(Token::Keyword(name)) => {
                            self.advance();
                            name
                        }
                        other => {
                            return Err(StepError::parser(
                                Some(id),
                                format!("expected type name, got {other:?}"),
                            ));
                        }
                    };

                    let args = self.parse_args()?;
                    self.expect_token(&Token::Semicolon)?;

                    entities.push(StepEntity {
                        id,
                        type_name,
                        args,
                    });
                }
            } else {
                break;
            }
        }
        Ok(entities)
    }

    fn parse_args(&mut self) -> Result<Vec<StepValue>, StepError> {
        self.expect_token(&Token::LParen)?;
        let mut args = Vec::new();
        if !self.check_token(&Token::RParen) {
            args.push(self.parse_value()?);
            while self.check_token(&Token::Comma) {
                self.advance();
                args.push(self.parse_value()?);
            }
        }
        self.expect_token(&Token::RParen)?;
        Ok(args)
    }

    fn parse_value(&mut self) -> Result<StepValue, StepError> {
        let tok = self.peek().cloned();
        match tok.map(|t| t.token) {
            Some(Token::EntityRef(id)) => {
                self.advance();
                Ok(StepValue::EntityRef(id))
            }
            Some(Token::String(s)) => {
                self.advance();
                Ok(StepValue::String(s))
            }
            Some(Token::Real(v)) => {
                self.advance();
                Ok(StepValue::Real(v))
            }
            Some(Token::Integer(v)) => {
                self.advance();
                Ok(StepValue::Integer(v))
            }
            Some(Token::Enum(s)) => {
                self.advance();
                Ok(StepValue::Enum(s))
            }
            Some(Token::Asterisk) => {
                self.advance();
                Ok(StepValue::Derived)
            }
            Some(Token::Dollar) => {
                self.advance();
                Ok(StepValue::Null)
            }
            Some(Token::LParen) => {
                self.advance();
                let mut list = Vec::new();
                if !self.check_token(&Token::RParen) {
                    list.push(self.parse_value()?);
                    while self.check_token(&Token::Comma) {
                        self.advance();
                        list.push(self.parse_value()?);
                    }
                }
                self.expect_token(&Token::RParen)?;
                Ok(StepValue::List(list))
            }
            Some(Token::Keyword(type_name)) => {
                // Typed/complex value: TYPE_NAME(args)
                self.advance();
                let args = self.parse_args()?;
                Ok(StepValue::Typed { type_name, args })
            }
            other => Err(StepError::parser(
                None,
                format!("unexpected value: {other:?}"),
            )),
        }
    }

    fn peek(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&SpannedToken> {
        let tok = self.tokens.get(self.pos);
        self.pos += 1;
        tok
    }

    fn is_at_end(&self) -> bool {
        self.pos >= self.tokens.len()
    }

    fn check_token(&self, expected: &Token) -> bool {
        self.peek().map(|t| &t.token == expected).unwrap_or(false)
    }

    fn check_keyword(&self, name: &str) -> bool {
        match self.peek() {
            Some(SpannedToken {
                token: Token::Keyword(k),
                ..
            }) => k == name,
            _ => false,
        }
    }

    fn expect_token(&mut self, expected: &Token) -> Result<(), StepError> {
        if self.check_token(expected) {
            self.advance();
            Ok(())
        } else {
            let actual = self.peek().cloned();
            Err(StepError::parser(
                None,
                format!("expected {expected:?}, got {actual:?}"),
            ))
        }
    }

    fn expect_keyword(&mut self, name: &str) -> Result<(), StepError> {
        if self.check_keyword(name) {
            self.advance();
            Ok(())
        } else {
            let actual = self.peek().cloned();
            Err(StepError::parser(
                None,
                format!("expected keyword '{name}', got {actual:?}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple() {
        let input = r#"
ISO-10303-21;
HEADER;
FILE_DESCRIPTION((''), '2;1');
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('origin', (0.0, 0.0, 0.0));
#2 = DIRECTION('x', (1.0, 0.0, 0.0));
ENDSEC;
END-ISO-10303-21;
"#;
        let file = Parser::parse(input.as_bytes()).unwrap();
        assert_eq!(file.header.len(), 1);
        assert_eq!(file.entities.len(), 2);

        let p1 = file.get(1).unwrap();
        assert_eq!(p1.type_name, "CARTESIAN_POINT");
        assert_eq!(p1.args.len(), 2);
        assert_eq!(p1.args[0].as_string(), Some("origin"));

        let coords = p1.args[1].as_list().unwrap();
        assert_eq!(coords.len(), 3);
        assert_eq!(coords[0].as_real(), Some(0.0));
    }

    #[test]
    fn test_parse_nested_list() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = B_SPLINE_CURVE_WITH_KNOTS('', 3, (#2, #3, #4), .UNSPECIFIED., .F., .F., (4, 4), (0.0, 1.0), .UNSPECIFIED.);
ENDSEC;
END-ISO-10303-21;
"#;
        let file = Parser::parse(input.as_bytes()).unwrap();
        let e = file.get(1).unwrap();
        assert_eq!(e.type_name, "B_SPLINE_CURVE_WITH_KNOTS");

        // Args: name, degree, control_points, curve_form, closed, self_intersect, knot_multiplicities, knots, knot_spec
        assert_eq!(e.args.len(), 9);

        // Check degree is integer 3
        assert_eq!(e.args[1].as_integer(), Some(3));

        // Check control_points is a list of entity refs
        let cp = e.args[2].as_list().unwrap();
        assert_eq!(cp.len(), 3);
        assert_eq!(cp[0].as_entity_ref(), Some(2));

        // Check enums
        assert_eq!(e.args[3].as_enum(), Some("UNSPECIFIED"));
        assert_eq!(e.args[4].as_enum(), Some("F"));
    }

    #[test]
    fn test_parse_null_and_derived() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = SOME_ENTITY($, *, 'value');
ENDSEC;
END-ISO-10303-21;
"#;
        let file = Parser::parse(input.as_bytes()).unwrap();
        let e = file.get(1).unwrap();
        assert!(e.args[0].is_null());
        assert!(e.args[1].is_derived());
        assert_eq!(e.args[2].as_string(), Some("value"));
    }

    #[test]
    fn test_entities_of_type() {
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
#2 = DIRECTION('', (1.0, 0.0, 0.0));
#3 = CARTESIAN_POINT('', (1.0, 0.0, 0.0));
ENDSEC;
END-ISO-10303-21;
"#;
        let file = Parser::parse(input.as_bytes()).unwrap();
        let points = file.entities_of_type("CARTESIAN_POINT");
        assert_eq!(points.len(), 2);
    }

    #[test]
    fn test_complex_entity() {
        // Complex entities combine multiple types in one definition
        // Common for units: (NAMED_UNIT(*) LENGTH_UNIT() SI_UNIT($,.METRE.))
        let input = r#"
ISO-10303-21;
HEADER;
ENDSEC;
DATA;
#1 = (NAMED_UNIT(*) LENGTH_UNIT() SI_UNIT($,.METRE.));
#2 = CARTESIAN_POINT('', (0.0, 0.0, 0.0));
ENDSEC;
END-ISO-10303-21;
"#;
        let file = Parser::parse(input.as_bytes()).unwrap();
        assert_eq!(file.entities.len(), 2);

        // Complex entity uses first type as type_name
        let unit = file.get(1).unwrap();
        assert_eq!(unit.type_name, "NAMED_UNIT");

        // Regular entity still works
        let point = file.get(2).unwrap();
        assert_eq!(point.type_name, "CARTESIAN_POINT");
    }
}
