# vcad Claude Code Plugin

Parametric CAD for AI agents. Create 3D parts, assemblies, and physics simulations directly from Claude Code.

## Installation

```bash
claude plugin add ./plugins/claude-code
```

Or from a local vcad checkout:

```bash
claude plugin add /path/to/vcad/plugins/claude-code
```

## What's Included

### MCP Server

The plugin bundles the `@vcad/mcp` server with 11 tools:

| Tool | Description |
|------|-------------|
| `create_cad_document` | Build 3D geometry from primitives, sketches, and operations |
| `export_cad` | Export to STL (3D printing) or GLB (visualization) |
| `inspect_cad` | Volume, surface area, bounding box, center of mass, mass |
| `import_step` | Import STEP files from other CAD software |
| `open_in_browser` | Generate shareable vcad.io URL |
| `create_robot_env` | Create physics simulation from assembly |
| `gym_step` | Step simulation with torque/position/velocity actions |
| `gym_reset` | Reset simulation to initial state |
| `gym_observe` | Get observation without stepping |
| `gym_close` | Clean up simulation |
| `get_changelog` | Query recent changes and features |

### Skills

- **cad-modeling** — Core CAD: primitives, sketch operations (extrude, revolve, sweep, loft), booleans, patterns, fillets, shell
- **assembly-physics** — Multi-part assemblies, joints, physics simulation, RL training
- **step-import** — STEP file import workflows

### Slash Commands

- `/vcad:new-part` — Guided part creation wizard
- `/vcad:export` — Quick export to STL or GLB
- `/vcad:examples` — Browse the pattern library

## Quick Start

Just ask Claude to build something:

> "Create a mounting plate with 4 corner holes, 100x60x5mm"

> "Build a 2-DOF robot arm and simulate it"

> "Import bracket.step and export it as STL"

Or use a slash command:

> `/vcad:new-part`

> `/vcad:examples`

## Links

- **Live app:** https://vcad.io
- **Documentation:** https://docs.vcad.io
- **Source:** https://github.com/vcad-io/vcad
