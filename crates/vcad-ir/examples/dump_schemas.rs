// Dumps CsgOp::tool_schemas() as the TypeScript module
// packages/core/src/commands/static-schemas.ts.
//
//   cargo run --quiet --example dump_schemas -p vcad-ir > \
//       packages/core/src/commands/static-schemas.ts
use vcad_ir::CsgOp;

fn main() {
    let schemas = CsgOp::tool_schemas();
    let json = serde_json::to_string_pretty(&schemas).unwrap();
    // Indent one extra level to align with the export const declaration below.
    let indented: String = json
        .lines()
        .enumerate()
        .map(|(i, line)| {
            if i == 0 {
                line.to_string()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    println!("import type {{ ToolSchemaEntry }} from \"./types.js\";");
    println!();
    println!("/** Static tool schemas generated from CsgOp. Regenerate with: cargo run --quiet --example dump_schemas -p vcad-ir > packages/core/src/commands/static-schemas.ts */");
    println!("export const STATIC_TOOL_SCHEMAS: ToolSchemaEntry[] = {indented};");
}
