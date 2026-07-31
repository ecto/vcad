# Stepperoni: Standalone STEP Parser Library

Extract the self-contained STEP (ISO 10303-21) lexer and parser from vcad-kernel-step into a standalone crate called `stepperoni`, ready for crates.io publication.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p2` |
| Effort | `s` |

> Verified against repo 2026-07-30. `crates/stepperoni` exists and `vcad-kernel-step` depends on it.

## Problem

The STEP lexer and parser in `vcad-kernel-step` are already self-contained with zero vcad dependencies. This is useful code that:

1. **Others could benefit from** — Few good STEP parsers exist in the Rust ecosystem
2. **Forces clean API boundaries** — Extraction ensures the parsing layer stays decoupled from vcad internals
3. **Enables independent versioning** — Parser improvements can ship without vcad releases
4. **Reduces vcad compile times** — Smaller crate graph when parser is external

## Solution

Extract lexer, parser, and parsing-specific errors into a new `stepperoni` crate with zero dependencies beyond `thiserror`.

### What Gets Extracted

From `crates/vcad-kernel-step/src/`:
- `lexer.rs` — Part 21 tokenizer (zero vcad deps)
- `parser.rs` — Entity graph builder (zero vcad deps)
- `error.rs` — Error types (STEP-specific subset only)

### What Stays in vcad-kernel-step

- `entities/` — Depends on vcad-kernel-math
- `reader.rs` — BRep construction, heavy vcad coupling
- `writer.rs` — Same

### New Crate Structure

```
stepperoni/
├── Cargo.toml
├── LICENSE-MIT
├── LICENSE-APACHE
├── README.md
├── src/
│   ├── lib.rs        # Public API, docs, examples
│   ├── lexer.rs      # Token types + Lexer
│   ├── parser.rs     # StepValue, StepEntity, StepFile, Parser
│   └── error.rs      # StepError (parsing subset only)
└── tests/
    └── fixtures/     # Sample STEP files for integration tests
```

## API Design

```rust
// Top-level convenience
pub fn parse(input: &[u8]) -> Result<StepFile, StepError>;
pub fn tokenize(input: &[u8]) -> Result<Vec<Token>, StepError>;

// Core types
pub struct StepFile { ... }
pub struct StepEntity { ... }
pub enum StepValue { ... }
pub enum Token { ... }

// Low-level access
pub struct Lexer<'a> { ... }
pub struct Parser { ... }
```

## Implementation

### Files Created

| File | Purpose |
|------|---------|
| `stepperoni/Cargo.toml` | Crate manifest with crates.io metadata |
| `stepperoni/src/lib.rs` | Public re-exports, convenience functions, docs |
| `stepperoni/src/lexer.rs` | Token types and lexer (copied from vcad) |
| `stepperoni/src/parser.rs` | StepValue, StepEntity, StepFile, Parser |
| `stepperoni/src/error.rs` | StepError (parsing subset) |
| `stepperoni/README.md` | Documentation, examples, API overview |
| `stepperoni/LICENSE-MIT` | MIT license |
| `stepperoni/LICENSE-APACHE` | Apache 2.0 license |

### Files Modified

| File | Change |
|------|--------|
| `vcad-kernel-step/Cargo.toml` | Add stepperoni dependency |
| `vcad-kernel-step/src/lib.rs` | Remove lexer/parser modules, re-export from stepperoni |
| `vcad-kernel-step/src/error.rs` | Keep only BRep-specific errors |
| `vcad-kernel-step/src/entities/*.rs` | Update imports |
| `vcad-kernel-step/src/reader.rs` | Update imports |
| `vcad-kernel-step/src/writer.rs` | Update imports |

### Files Deleted

| File | Reason |
|------|--------|
| `vcad-kernel-step/src/lexer.rs` | Moved to stepperoni |
| `vcad-kernel-step/src/parser.rs` | Moved to stepperoni |

### Cargo.toml

```toml
[package]
name = "stepperoni"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"
description = "Fast, zero-dependency STEP (ISO 10303-21) file parser"
repository = "https://github.com/vcad-io/stepperoni"
keywords = ["step", "cad", "iso-10303", "parser", "brep"]
categories = ["parser-implementations", "science"]

[dependencies]
thiserror = "1"
```

## Tasks

### Extraction

- [x] Create `stepperoni/` directory and Cargo.toml (`xs`)
- [x] Add LICENSE-MIT and LICENSE-APACHE files (`xs`)
- [x] Extract error.rs with parsing-only variants (`xs`)
- [x] Copy lexer.rs, update imports (`xs`)
- [x] Copy parser.rs, update imports (`xs`)
- [x] Create lib.rs with public API and docs (`s`)
- [x] Write README.md with examples (`s`)
- [ ] Add integration tests with fixture files (`s`)

### Integration

- [x] Add stepperoni as vcad-kernel-step dependency (`xs`)
- [x] Delete vcad-kernel-step/src/lexer.rs (`xs`)
- [x] Delete vcad-kernel-step/src/parser.rs (`xs`)
- [x] Update vcad-kernel-step error.rs (remove parser errors) (`xs`)
- [x] Update imports in entities/*.rs (`xs`)
- [x] Update imports in reader.rs (`xs`)
- [x] Update imports in writer.rs (`xs`)
- [x] Re-export stepperoni types from vcad-kernel-step if needed (`xs`)

### Verification

- [x] `cargo test` passes in stepperoni (`xs`)
- [x] `cargo clippy -- -D warnings` clean in stepperoni (`xs`)
- [ ] `cargo doc` builds with no warnings (`xs`)
- [x] `cargo test --workspace` passes in vcad (`xs`)
- [x] `cargo clippy --workspace -- -D warnings` clean in vcad (`xs`)

### Publication (Future)

- [ ] Create github.com/vcad-io/stepperoni repo
- [ ] Publish to crates.io
- [ ] Add to README: badges, docs.rs link

## Acceptance Criteria

- [x] `stepperoni` crate compiles with only `thiserror` dependency
- [x] `stepperoni::parse(bytes)` returns `Result<StepFile, StepError>`
- [x] `stepperoni::tokenize(bytes)` returns `Result<Vec<Token>, StepError>`
- [x] vcad-kernel-step uses stepperoni for parsing
- [x] All existing vcad STEP tests pass
- [x] Example in README compiles and runs

## Future Enhancements

- [ ] Streaming parser for large files
- [ ] STEP AP203 support
- [ ] Round-trip write support
- [ ] WASM build for browser use
- [ ] Python bindings via PyO3
