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
- **Simulation** — Physics with phyz, gym-style RL interface
- **Import/Export** — STEP import, STL/GLB/STEP/DXF export
- **Rendering** — Direct BRep ray tracing + tessellated mode
- **Cloud** — Supabase sync with Google/GitHub auth

## Use vcad

### Web App

Visit [vcad.io](https://vcad.io) — no install required.

### Desktop App

Download the installer for your platform from the [latest release](https://github.com/ecto/vcad/releases/latest):

- **macOS** — universal `.dmg` (Intel + Apple Silicon)
- **Windows** — `.msi` installer
- **Linux** — `.AppImage`, `.deb`, or `.rpm`

#### First launch on macOS

Releases are not yet notarized with Apple, so on first launch macOS shows
*"Apple could not verify 'vcad.app' is free of malware"*. To open it:

- **macOS 15+** — open **System Settings → Privacy & Security**, scroll to the
  "vcad was blocked" notice, click **Open Anyway**, then re-launch vcad.
- **Earlier macOS** — right-click `vcad.app` in `/Applications` and choose
  **Open**, then **Open** in the confirmation dialog.
- **From Terminal** — `xattr -d com.apple.quarantine /Applications/vcad.app`
  removes the quarantine flag so the app launches normally.

You only need to do this once per install. We'll remove this step once
Apple Developer signing is set up.

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

## Export controls

The hosted services (vcad.io and mcp.vcad.io) are not available in
jurisdictions subject to U.S. sanctions and export controls — currently
Russia, Belarus, Iran, Cuba, North Korea, Syria, and the Crimea, Sevastopol,
Donetsk, and Luhansk regions of Ukraine. Users must comply with all
applicable export laws. The open-source code in this repository is not
affected and remains available to everyone. Details in
[docs/export-controls.md](docs/export-controls.md).

## License

Copyright © 2026 Municipal Robotics Corporation

Licensed under the [Apache License 2.0](LICENSE). See [NOTICE](NOTICE) for
attributions. Contributions are accepted under the same license per the
[Apache 2.0 Section 5](LICENSE) inbound-equals-outbound rule.
