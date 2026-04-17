//! Tool schema sourcing.
//!
//! `vcad-ir` already owns the authoritative schema list: `CsgOp` carries
//! `#[derive(ToolSchema)]` which generates `CsgOp::tool_schemas() ->
//! Vec<ToolSchemaEntry>`. We re-export thin helpers so the rest of this crate
//! and downstream frontends never have to touch `CsgOp` directly.
//!
//! The TypeScript `STATIC_TOOL_SCHEMAS` in `packages/core/src/commands/static-schemas.ts`
//! is generated from this same source via
//! `cargo run --quiet --example dump_schemas -p vcad-ir > packages/core/src/commands/static-schemas.ts`,
//! so the Rust and TS views are guaranteed to match.

use vcad_ir::{CsgOp, ToolSchemaEntry};

/// All tool schemas that can appear as `create` tool parameters. Order matches
/// the `CsgOp` variant order.
pub fn all_schemas() -> Vec<ToolSchemaEntry> {
    CsgOp::tool_schemas()
}

/// The list of valid `type` enum values for the Anthropic `create` tool —
/// just the variant names from [`all_schemas`], in the same order.
pub fn type_enum() -> Vec<String> {
    all_schemas().into_iter().map(|s| s.name).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_nonempty() {
        let schemas = all_schemas();
        assert!(!schemas.is_empty(), "CsgOp should have tool schemas");
    }

    #[test]
    fn type_enum_matches_schemas() {
        let schemas = all_schemas();
        let types = type_enum();
        assert_eq!(schemas.len(), types.len());
        for (s, t) in schemas.iter().zip(types.iter()) {
            assert_eq!(&s.name, t);
        }
    }

    #[test]
    fn has_core_primitives() {
        let types = type_enum();
        for expected in ["cube", "cylinder", "sphere"] {
            assert!(
                types.iter().any(|t| t == expected),
                "expected `{expected}` in schema list, got: {types:?}"
            );
        }
    }
}
