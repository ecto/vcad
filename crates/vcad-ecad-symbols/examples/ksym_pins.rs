//! Dump KiCad library symbols as `create_schematic`-ready pin arrays.
//!
//! ```text
//! cargo run -p vcad-ecad-symbols --example ksym_pins -- \
//!     /path/MCU_ST_STM32H7.kicad_sym STM32H723VGTx [MORE_SYMBOLS...]
//! ```
//!
//! Prints `{ "<symbol>": [{number, name, type, x, y}, ...], ... }` on stdout.
//! Pin numbers, names and electrical types come straight from the KiCad
//! library, so a schematic authored against them inherits KiCad's pinout.

use vcad_ecad_symbols::kicad_sym::parse_symbol_lib_file;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: ksym_pins <lib.kicad_sym> <symbol>...");
        std::process::exit(2);
    }
    let lib = parse_symbol_lib_file(&args[0]).unwrap_or_else(|e| {
        eprintln!("parse {}: {e}", args[0]);
        std::process::exit(1);
    });
    let mut out = serde_json::Map::new();
    for name in &args[1..] {
        let Some(sym) = lib.get(name) else {
            eprintln!("symbol {name} not in {}", args[0]);
            std::process::exit(1);
        };
        let pins: Vec<serde_json::Value> = sym
            .schematic_pins()
            .into_iter()
            .map(|p| {
                serde_json::json!({
                    "number": p.number,
                    "name": p.name,
                    "type": p.pin_type,
                    "x": p.position.x,
                    "y": p.position.y,
                })
            })
            .collect();
        out.insert(name.clone(), serde_json::Value::Array(pins));
    }
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
