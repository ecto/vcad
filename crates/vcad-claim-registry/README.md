# vcad-claim-registry

Maps a claim-family schema id (`vcad.cam-claims/1`, `vcad.thermal-claims/1`,
…) to the family behind it: its metadata, and the adapter that turns that
family's own serialized report into unified
[`vcad-receipt`](../vcad-receipt) claims.

vcad has one receipt schema and a dozen-odd per-domain claim families. Each
family crate already knew how to translate itself (`design_claims`); nothing
knew how to *find* them, so no Rust claim family reached a receipt. This crate
is that lookup, and nothing else — every adapter deserializes the family's own
`ClaimSet` and calls the family's own `design_claims`. A registry that
re-implemented a ladder would be a second answer to "does this claim hold".

It cannot live inside `vcad-receipt`: every family depends on `vcad-receipt`,
so a registry there would be a dependency cycle. It sits above them instead,
one cargo feature per family.

```rust
use vcad_claim_registry as registry;

for family in registry::families() {
    println!("{} → {} ({})", family.schema, family.domain, family.crate_name);
}

let claims = registry::claims_for("vcad.cam-claims/1", report_json)?;
```

Three entry points, the last two optional per family (a family that has
neither says so rather than pretending):

| Function | Answers |
|---|---|
| `claims_for` | this family's serialized report → unified claims |
| `restate` | …against these input digests, so a moved input reads `Stale` |
| `bind` | …with this measurement of the real part bound to a claim |

Reachable from the browser and from MCP through the kernel WASM as
`receiptFamilies()`, `receiptClaimsFor()`, `receiptRestate()`,
`receiptBind()`.
