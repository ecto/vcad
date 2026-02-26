//! Part 21 (STEP physical file format) lexer.
//!
//! Tokenizes STEP files according to ISO 10303-21. Handles:
//! - Keywords (e.g., `CARTESIAN_POINT`, `DATA`, `ENDSEC`)
//! - Entity references (e.g., `#123`)
//! - Strings (e.g., `'hello'`)
//! - Real numbers (e.g., `1.5E-10`, `-3.14`)
//! - Integers
//! - Enumerations (e.g., `.TRUE.`, `.UNSPECIFIED.`)
//! - Punctuation (parentheses, comma, semicolon, equals, asterisk, dollar)

use crate::error::StepError;

/// A token in a STEP file.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// Keyword or identifier (e.g., `CARTESIAN_POINT`, `DATA`).
    Keyword(String),
    /// Entity reference (e.g., `#123` becomes `EntityRef(123)`).
    EntityRef(u64),
    /// String literal (contents without quotes).
    String(String),
    /// Real number.
    Real(f64),
    /// Integer number.
    Integer(i64),
    /// Enumeration (e.g., `.TRUE.` becomes `Enum("TRUE")`).
    Enum(String),
    /// Left parenthesis `(`.
    LParen,
    /// Right parenthesis `)`.
    RParen,
    /// Comma `,`.
    Comma,
    /// Semicolon `;`.
    Semicolon,
    /// Equals `=`.
    Equals,
    /// Asterisk `*` (derived value marker).
    Asterisk,
    /// Dollar `$` (null/unset value marker).
    Dollar,
}

/// Position in the source file.
#[derive(Debug, Clone, Copy, Default)]
pub struct Position {
    /// Line number (1-indexed).
    pub line: usize,
    /// Column number (1-indexed).
    pub col: usize,
}

/// A token with its position in the source.
#[derive(Debug, Clone)]
pub struct SpannedToken {
    /// The token.
    pub token: Token,
    /// Position where the token starts.
    pub pos: Position,
}

/// Lexer for Part 21 STEP files.
pub struct Lexer<'a> {
    input: &'a [u8],
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    /// Create a new lexer for the given input.
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    /// Tokenize the entire input.
    pub fn tokenize(&mut self) -> Result<Vec<SpannedToken>, StepError> {
        let mut tokens = Vec::new();
        while let Some(tok) = self.next_token()? {
            tokens.push(tok);
        }
        Ok(tokens)
    }

    /// Get the next token, or `None` if at end of input.
    pub fn next_token(&mut self) -> Result<Option<SpannedToken>, StepError> {
        self.skip_whitespace_and_comments();

        if self.pos >= self.input.len() {
            return Ok(None);
        }

        let start_pos = Position {
            line: self.line,
            col: self.col,
        };

        let ch = self.peek_char().unwrap();

        let token = match ch {
            b'(' => {
                self.advance();
                Token::LParen
            }
            b')' => {
                self.advance();
                Token::RParen
            }
            b',' => {
                self.advance();
                Token::Comma
            }
            b';' => {
                self.advance();
                Token::Semicolon
            }
            b'=' => {
                self.advance();
                Token::Equals
            }
            b'*' => {
                self.advance();
                Token::Asterisk
            }
            b'$' => {
                self.advance();
                Token::Dollar
            }
            b'#' => self.read_entity_ref()?,
            b'\'' => self.read_string()?,
            b'.' => self.read_enum()?,
            b'-' | b'+'
                if self.pos + 1 < self.input.len() && self.input[self.pos + 1].is_ascii_digit() =>
            {
                self.read_number()?
            }
            b'0'..=b'9' => self.read_number()?,
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => self.read_keyword()?,
            _ => {
                return Err(StepError::lexer(
                    self.line,
                    self.col,
                    format!("unexpected character: '{}'", ch as char),
                ));
            }
        };

        Ok(Some(SpannedToken {
            token,
            pos: start_pos,
        }))
    }

    fn peek_char(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let ch = self.input.get(self.pos).copied()?;
        self.pos += 1;
        if ch == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(ch)
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_whitespace() {
                    self.advance();
                } else {
                    break;
                }
            }

            // Check for comment /* ... */
            if self.pos + 1 < self.input.len()
                && self.input[self.pos] == b'/'
                && self.input[self.pos + 1] == b'*'
            {
                self.advance(); // /
                self.advance(); // *
                while self.pos + 1 < self.input.len() {
                    if self.input[self.pos] == b'*' && self.input[self.pos + 1] == b'/' {
                        self.advance(); // *
                        self.advance(); // /
                        break;
                    }
                    self.advance();
                }
                continue; // Check for more whitespace/comments
            }

            break;
        }
    }

    fn read_entity_ref(&mut self) -> Result<Token, StepError> {
        let start_line = self.line;
        let start_col = self.col;
        self.advance(); // skip '#'

        let mut digits = Vec::new();
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                digits.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        if digits.is_empty() {
            return Err(StepError::lexer(
                start_line,
                start_col,
                "expected digits after '#'",
            ));
        }

        let s = String::from_utf8(digits).unwrap();
        let id: u64 = s.parse().map_err(|_| {
            StepError::lexer(start_line, start_col, format!("invalid entity ID: {s}"))
        })?;

        Ok(Token::EntityRef(id))
    }

    fn read_string(&mut self) -> Result<Token, StepError> {
        let start_line = self.line;
        let start_col = self.col;
        self.advance(); // skip opening quote

        let mut content = Vec::new();
        loop {
            match self.peek_char() {
                None => {
                    return Err(StepError::lexer(
                        start_line,
                        start_col,
                        "unterminated string",
                    ));
                }
                Some(b'\'') => {
                    self.advance();
                    // Check for escaped quote ''
                    if self.peek_char() == Some(b'\'') {
                        content.push(b'\'');
                        self.advance();
                    } else {
                        break;
                    }
                }
                Some(ch) => {
                    content.push(ch);
                    self.advance();
                }
            }
        }

        let s = String::from_utf8_lossy(&content).into_owned();
        Ok(Token::String(s))
    }

    fn read_enum(&mut self) -> Result<Token, StepError> {
        let start_line = self.line;
        let start_col = self.col;
        self.advance(); // skip opening '.'

        let mut name = Vec::new();
        while let Some(ch) = self.peek_char() {
            if ch == b'.' {
                self.advance(); // skip closing '.'
                break;
            } else if ch.is_ascii_alphanumeric() || ch == b'_' {
                name.push(ch);
                self.advance();
            } else {
                return Err(StepError::lexer(
                    start_line,
                    start_col,
                    format!("invalid character in enumeration: '{}'", ch as char),
                ));
            }
        }

        if name.is_empty() {
            return Err(StepError::lexer(start_line, start_col, "empty enumeration"));
        }

        let s = String::from_utf8(name).unwrap();
        Ok(Token::Enum(s))
    }

    fn read_number(&mut self) -> Result<Token, StepError> {
        let start_line = self.line;
        let start_col = self.col;

        let mut num_str = Vec::new();
        let mut is_real = false;

        // Sign
        if let Some(ch @ (b'-' | b'+')) = self.peek_char() {
            num_str.push(ch);
            self.advance();
        }

        // Integer part
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_digit() {
                num_str.push(ch);
                self.advance();
            } else {
                break;
            }
        }

        // Decimal part
        if self.peek_char() == Some(b'.') {
            // Check what follows the decimal point
            let next_char = self.input.get(self.pos + 1).copied();
            let is_decimal = match next_char {
                // Digit after decimal -> definitely a real number (e.g., 100.5)
                Some(ch) if ch.is_ascii_digit() => true,
                // Delimiter after decimal -> trailing decimal point (e.g., 100., common in STEP)
                Some(b',') | Some(b')') | Some(b';') | None => true,
                // Whitespace after decimal -> trailing decimal point
                Some(ch) if ch.is_ascii_whitespace() => true,
                // Exponent after decimal -> real number (e.g., 100.E5)
                Some(b'E') | Some(b'e') => true,
                // Letter after decimal -> could be enum (e.g., .T. for true), don't consume
                _ => false,
            };

            if is_decimal {
                is_real = true;
                num_str.push(b'.');
                self.advance();
                while let Some(ch) = self.peek_char() {
                    if ch.is_ascii_digit() {
                        num_str.push(ch);
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
        }

        // Exponent part
        if let Some(ch @ (b'E' | b'e')) = self.peek_char() {
            is_real = true;
            num_str.push(ch);
            self.advance();
            if let Some(ch @ (b'-' | b'+')) = self.peek_char() {
                num_str.push(ch);
                self.advance();
            }
            while let Some(ch) = self.peek_char() {
                if ch.is_ascii_digit() {
                    num_str.push(ch);
                    self.advance();
                } else {
                    break;
                }
            }
        }

        let s = String::from_utf8(num_str).unwrap();

        if is_real {
            let val: f64 = s.parse().map_err(|_| {
                StepError::lexer(start_line, start_col, format!("invalid real number: {s}"))
            })?;
            Ok(Token::Real(val))
        } else {
            let val: i64 = s.parse().map_err(|_| {
                StepError::lexer(start_line, start_col, format!("invalid integer: {s}"))
            })?;
            Ok(Token::Integer(val))
        }
    }

    fn read_keyword(&mut self) -> Result<Token, StepError> {
        let mut name = Vec::new();
        while let Some(ch) = self.peek_char() {
            // Keywords can include alphanumeric, underscore, and hyphen
            // (for identifiers like ISO-10303-21 and END-ISO-10303-21)
            if ch.is_ascii_alphanumeric() || ch == b'_' || ch == b'-' {
                name.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        let s = String::from_utf8(name).unwrap().to_uppercase();
        Ok(Token::Keyword(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(input: &str) -> Vec<Token> {
        let mut lexer = Lexer::new(input.as_bytes());
        lexer
            .tokenize()
            .unwrap()
            .into_iter()
            .map(|st| st.token)
            .collect()
    }

    #[test]
    fn test_entity_ref() {
        assert_eq!(tokenize("#123"), vec![Token::EntityRef(123)]);
        assert_eq!(tokenize("#1"), vec![Token::EntityRef(1)]);
    }

    #[test]
    fn test_string() {
        assert_eq!(tokenize("'hello'"), vec![Token::String("hello".into())]);
        assert_eq!(tokenize("'it''s'"), vec![Token::String("it's".into())]); // escaped quote
    }

    #[test]
    fn test_enum() {
        assert_eq!(tokenize(".TRUE."), vec![Token::Enum("TRUE".into())]);
        assert_eq!(
            tokenize(".UNSPECIFIED."),
            vec![Token::Enum("UNSPECIFIED".into())]
        );
    }

    #[test]
    fn test_numbers() {
        assert_eq!(tokenize("42"), vec![Token::Integer(42)]);
        assert_eq!(tokenize("-7"), vec![Token::Integer(-7)]);
        assert_eq!(tokenize("3.14"), vec![Token::Real(3.14)]);
        assert_eq!(tokenize("-1.5E-10"), vec![Token::Real(-1.5e-10)]);
        assert_eq!(tokenize("2.0E3"), vec![Token::Real(2000.0)]);
        // Trailing decimal point (common in STEP coordinate lists like "100.,200.,300.")
        assert_eq!(tokenize("100."), vec![Token::Real(100.0)]);
        assert_eq!(
            tokenize("100.,200.,300."),
            vec![
                Token::Real(100.0),
                Token::Comma,
                Token::Real(200.0),
                Token::Comma,
                Token::Real(300.0),
            ]
        );
        assert_eq!(
            tokenize("(50.,60.)"),
            vec![
                Token::LParen,
                Token::Real(50.0),
                Token::Comma,
                Token::Real(60.0),
                Token::RParen,
            ]
        );
    }

    #[test]
    fn test_keywords() {
        assert_eq!(
            tokenize("CARTESIAN_POINT"),
            vec![Token::Keyword("CARTESIAN_POINT".into())]
        );
        assert_eq!(tokenize("data"), vec![Token::Keyword("DATA".into())]); // case insensitive
    }

    #[test]
    fn test_punctuation() {
        assert_eq!(
            tokenize("()=,;*$"),
            vec![
                Token::LParen,
                Token::RParen,
                Token::Equals,
                Token::Comma,
                Token::Semicolon,
                Token::Asterisk,
                Token::Dollar,
            ]
        );
    }

    #[test]
    fn test_comments() {
        assert_eq!(tokenize("/* comment */ #1"), vec![Token::EntityRef(1)]);
        assert_eq!(
            tokenize("#1 /* inline */ #2"),
            vec![Token::EntityRef(1), Token::EntityRef(2)]
        );
    }

    #[test]
    fn test_complete_entity() {
        let input = "#1 = CARTESIAN_POINT('', (0.0, 1.5E-2, -3.0));";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                Token::EntityRef(1),
                Token::Equals,
                Token::Keyword("CARTESIAN_POINT".into()),
                Token::LParen,
                Token::String("".into()),
                Token::Comma,
                Token::LParen,
                Token::Real(0.0),
                Token::Comma,
                Token::Real(0.015),
                Token::Comma,
                Token::Real(-3.0),
                Token::RParen,
                Token::RParen,
                Token::Semicolon,
            ]
        );
    }

    #[test]
    fn test_whitespace() {
        let input = "  #1  =  POINT  (  )  ;  ";
        let tokens = tokenize(input);
        assert_eq!(
            tokens,
            vec![
                Token::EntityRef(1),
                Token::Equals,
                Token::Keyword("POINT".into()),
                Token::LParen,
                Token::RParen,
                Token::Semicolon,
            ]
        );
    }
}
