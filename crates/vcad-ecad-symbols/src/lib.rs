#![warn(missing_docs)]

//! KiCad S-expression symbol and footprint library parser.
//!
//! This crate parses KiCad `.kicad_sym` (schematic symbol) and `.kicad_mod`
//! (footprint) files using `nom` combinators over the KiCad S-expression format.
//!
//! # Example
//!
//! ```
//! use vcad_ecad_symbols::{parse_symbol_lib, parse_footprint_lib};
//!
//! let sym_input = r#"(kicad_symbol_lib (version 20211014) (generator test)
//!   (symbol "R" (in_bom yes) (on_board yes)
//!     (property "Reference" "R" (at 0 0 0))
//!     (symbol "R_0_1"
//!       (rectangle (start -1.016 -2.54) (end 1.016 2.54)
//!         (stroke (width 0) (type default)) (fill (type none))))
//!     (symbol "R_1_1"
//!       (pin passive line (at 0 3.81 270) (length 1.27)
//!         (name "~" (effects (font (size 1.27 1.27))))
//!         (number "1" (effects (font (size 1.27 1.27))))))
//!   )
//! )"#;
//! let lib = parse_symbol_lib(sym_input).unwrap();
//! assert_eq!(lib.symbols.len(), 1);
//! assert_eq!(lib.symbols[0].name, "R");
//! ```

pub mod builtin;
pub mod footprint;
pub mod kicad_mod;
pub mod kicad_pcb;
pub mod kicad_sym;
pub mod kicad_write;

mod sexpr;

use serde::{Deserialize, Serialize};

// Re-export public API
pub use kicad_mod::{parse_footprint_lib, FootprintDef, FootprintLib, GraphicDef, PadDef};
pub use kicad_pcb::parse_kicad_pcb;
pub use kicad_sym::{parse_symbol_lib, Symbol, SymbolGraphic, SymbolLib, SymbolPin};
pub use kicad_write::{write_kicad_pcb, write_kicad_sch};

/// A key-value property from a KiCad library element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Property {
    /// Property key (e.g. "Reference", "Value", "Footprint").
    pub key: String,
    /// Property value.
    pub value: String,
}

/// Errors that can occur while parsing KiCad files.
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// A nom parsing error with context.
    #[error("parse error at: {0}")]
    Nom(String),
    /// The input was not fully consumed.
    #[error("unexpected trailing input")]
    TrailingInput,
    /// An I/O error while reading a file.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl<'a> From<nom::Err<nom::error::Error<&'a str>>> for ParseError {
    fn from(e: nom::Err<nom::error::Error<&'a str>>) -> Self {
        ParseError::Nom(format!("{}", e))
    }
}
