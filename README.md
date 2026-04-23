<p align="center">
  <img src="https://vcad.io/assets/mascot.png" width="200" alt="vcad mascot">
</p>

# vcad

Open-source parametric CAD for the AI era.

**[Try it now →](https://vcad.io)**

![vcad screenshot](assets/screenshot.png)

## Features

- **Modeling** — Primitives, booleans, fillets, chamfers, shell
- **Sketching** — 2D constraints, extrude, revolve, sweep, loft
- **Assembly** — Parts, instances, joints, forward kinematics
- **Simulation** — Physics with Rapier3D, gym-style RL interface
- **Import/Export** — STEP import, STL/GLB/STEP/DXF export
- **Rendering** — Direct BRep ray tracing + tessellated mode
- **Cloud** — Supabase sync with Google/GitHub auth

## Use vcad

### Web App

Visit [vcad.io](https://vcad.io) — no install required.

### CLI

```bash
cargo install vcad-cli
vcad export input.vcad output.stl
vcad import-step input.step output.vcad
```

### MCP Server (AI Agents)

The MCP server lets AI agents create and manipulate CAD models:

```bash
npm install -g @vcad/mcp
```

Tools: `create_cad_document`, `export_cad`, `inspect_cad`, `gym_step`, `gym_reset`

### Rust Library

```rust
use vcad_kernel::Solid;

// Create a box with a hole
let solid = Solid::cube(100.0, 60.0, 20.0);
let hole = Solid::cylinder(10.0, 25.0, 32);
let result = solid - hole;

// Export to mesh
let mesh = result.to_mesh(32);
```

See [crates/vcad-kernel](crates/vcad-kernel) for the full API.

## Architecture

```
vcad/
├── crates/           # Rust BRep kernel (~35K LOC)
│   ├── vcad-kernel/  # Unified API
│   ├── vcad-kernel-topo/    # Half-edge topology
│   ├── vcad-kernel-booleans/# Boolean operations
│   └── ...           # 20+ modular crates
├── packages/         # TypeScript
│   ├── app/          # React + Three.js web app
│   ├── mcp/          # MCP server for AI agents
│   └── ...
└── supabase/         # Database migrations
```

## Development

> **Heads up:** vcad depends on the `tang` math workspace at a **sibling path**
> (`../tang`). Clone it next to vcad before any `cargo` command, or the
> workspace will fail to resolve:
>
> ```bash
> git clone git@github.com:ecto/tang.git ../tang
> ```

```bash
# Rust
cargo test --workspace
cargo clippy --workspace -- -D warnings

# TypeScript
npm ci
npm run dev -w @vcad/app   # Run web app locally

# Desktop (optional — Tauri v2 shell)
# Ubuntu/Debian system deps:
#   sudo apt install libgtk-3-dev libwebkit2gtk-4.1-dev librsvg2-dev \
#                    libsoup-3.0-dev libatk1.0-dev
# macOS: Xcode command-line tools
# Windows: WebView2 (shipped with Win11)
npm run tauri:dev          # launches desktop window against the dev server
npm run tauri:build        # produces a signed/unsigned installer
```

## License

Copyright © 2025–2026 Municipal Robotics, Inc.

Licensed under the [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for
attributions. Contributions are accepted under the same license per the
[Apache 2.0 Section 5](LICENSE) inbound-equals-outbound rule.
