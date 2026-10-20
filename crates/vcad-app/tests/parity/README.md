# Engine parity fixtures

Each `*.json` file is a command log replayed by three hosts:

- native Rust — `crates/vcad-app/tests/parity.rs`
- the C ABI — `crates/vcad-ffi/src/engine.rs` (`parity_fixtures_match_golden`)
- WASM under Node — `packages/engine/src/__tests__/engine-parity.test.ts`

Every host must produce the same per-step `outcomes` and the same final
`hash` (`DocumentApi::state_hash`). A step is either
`{"command": DocCommand}` or `{"tool": "create", "args": {…}}`.
An outcome is `"ok:<createdFeatureId or ->"` or `"err:<CommandError kind>"`.

Regenerate the goldens after an intentional engine change:

    UPDATE_PARITY=1 cargo test -p vcad-app --test parity
