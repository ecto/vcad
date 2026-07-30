# Headless Mode & API

Programmatic access to vcad without the UI for automation, CI/CD, and AI integration.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |
| Priority | `p1` |
| Effort | n/a (complete) |

> Verified against repo 2026-07-30. Two corrections: (1) the Ink/React JS CLI (`packages/cli`, `npx @vcad/cli`) no longer exists — the TUI now lives in the Rust CLI (`vcad tui` / `vcad repl` in `crates/vcad-cli`); (2) the surface described here is a small subset of what shipped — the Rust CLI also has `import-step`, `import-urdf`, `render`, `fab-prep`, and the MCP server exposes 100+ tools (see CLAUDE.md), not just the three listed.

## Problem

CAD tools are traditionally GUI-only, forcing manual workflows for repetitive tasks:

1. **CI/CD integration** — No way to auto-export models on commit or validate geometry in pipelines
2. **AI agent integration** — LLMs can describe designs but can't create them without human clicking
3. **Batch processing** — Converting 100 STEP files to STL requires clicking through each one
4. **Scripting** — Power users want to automate parametric changes programmatically

SolidWorks/Fusion 360 offer APIs but require expensive licenses and Windows. Open-source alternatives lack first-class headless support.

## Solution

Three complementary headless interfaces covering different use cases:

### Rust CLI (`vcad`)

Native command-line tool for file operations:

```bash
# Export to mesh formats
vcad export model.vcad output.stl
vcad export model.vcad output.glb
vcad export model.vcad output.step

# Import STEP files
vcad import part.step output.vcad --name "Imported Part"

# Inspect document
vcad info model.vcad
```

Output from `vcad info`:
```
vcad document: model.vcad
  Version: 1.0.0
  Nodes: 5
  Materials: 2
  Scene entries: 1

Scene:
  1: Bracket (material: aluminum)

Mesh stats:
  Total triangles: 1284
  Total vertices: 642
```

### JS CLI (TUI)

Interactive terminal UI built with Ink + React for visual editing without a browser:

```bash
npx @vcad/cli
npx @vcad/cli model.vcad
```

Features:
- ASCII wireframe 3D viewport with software rendering
- Feature tree navigation (j/k to move, Enter to edit)
- Keyboard shortcuts for primitives (1: box, 2: cylinder, 3: sphere)
- Real-time mesh evaluation via WASM engine

### MCP Server (for AI Agents)

Model Context Protocol server enabling LLMs to create CAD directly:

```bash
# Add to Claude Desktop config
{
  "mcpServers": {
    "vcad": {
      "command": "npx",
      "args": ["@vcad/mcp"]
    }
  }
}
```

**Tools exposed:**

| Tool | Description |
|------|-------------|
| `create_cad_document` | Build geometry from primitives + operations |
| `export_cad` | Export to STL or GLB file |
| `inspect_cad` | Get volume, area, bbox, center of mass |

**Example: AI creating a bracket**

```json
{
  "parts": [{
    "name": "Bracket",
    "primitive": {
      "type": "cube",
      "size": {"x": 50, "y": 30, "z": 5}
    },
    "operations": [
      {
        "type": "hole",
        "diameter": 5,
        "at": {"x": 10, "y": 15}
      },
      {
        "type": "hole",
        "diameter": 5,
        "at": {"x": 40, "y": 15}
      }
    ],
    "material": "aluminum"
  }]
}
```

**Position specifications:**
- Absolute: `{x: 25, y: 15, z: 0}`
- Named: `"center"`, `"top-center"`, `"bottom-center"`
- Percentage: `{x: "50%", y: "50%"}`

**Not included:** REST API, GraphQL, WebSocket streaming.

## UX Details

### Rust CLI

| Command | Behavior |
|---------|----------|
| `vcad export` | Writes file, prints path on success |
| `vcad import` | Parses STEP, creates .vcad document, prints solid count |
| `vcad info` | Reads .vcad, evaluates mesh, prints stats |
| Invalid format | Exits with error message and code 1 |

### MCP Server

| State | Behavior |
|-------|----------|
| Tool success | Returns JSON result |
| Invalid input | Returns error with descriptive message |
| WASM init fail | Exits process with error |
| Large model | Evaluates synchronously (blocking) |

### Error Handling

- STEP import failures include line numbers and entity IDs
- Export failures report missing geometry or unsupported operations
- MCP tool errors are returned to the LLM for self-correction

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-cli/src/main.rs` | Rust CLI entry point and command dispatch |
| `crates/vcad-cli/src/app.rs` | Document evaluation for export/info |
| `packages/cli/src/App.tsx` | JS TUI main component |
| `packages/cli/src/components/Viewport3D.tsx` | ASCII wireframe renderer |
| `packages/cli/src/renderer/software-renderer.ts` | CPU-based 3D rasterization |
| `packages/mcp/src/index.ts` | MCP server entry point |
| `packages/mcp/src/server.ts` | Server setup and tool registration |
| `packages/mcp/src/tools/create.ts` | `create_cad_document` implementation |
| `packages/mcp/src/tools/export.ts` | `export_cad` implementation |
| `packages/mcp/src/tools/inspect.ts` | `inspect_cad` with mesh analysis |
| `packages/mcp/src/export/stl.ts` | Binary STL generation |
| `packages/mcp/src/export/glb.ts` | GLB/glTF generation |

### Architecture

**Rust CLI**
```
CLI Args → Clap Parser → Command Handler → vcad-kernel → File Output
```

Uses `vcad-ir` for document parsing, `vcad-kernel` for BRep operations, and `vcad-kernel-step` for STEP I/O.

**JS CLI**
```
Terminal → Ink/React → @vcad/core stores → @vcad/engine (WASM) → ASCII Render
```

Shares stores with web app. WASM engine evaluates IR to meshes.

**MCP Server**
```
LLM → MCP Protocol → Tool Handler → @vcad/engine → JSON Response
```

Stateless tool calls. Engine initialized once at startup.

### Dependencies

| Package | Purpose |
|---------|---------|
| `clap` | Rust CLI argument parsing |
| `ink` | React for CLI UIs |
| `@modelcontextprotocol/sdk` | MCP server implementation |
| `@vcad/engine` | WASM-based geometry evaluation |
| `@vcad/ir` | Document type definitions |

## Tasks

### Rust CLI

- [x] Export command with STL output
- [x] Export command with STEP output (primitives)
- [x] Import command for STEP files
- [x] Info command with mesh stats
- [x] Error handling with descriptive messages

### JS CLI (TUI)

- [x] Ink-based terminal UI
- [x] ASCII wireframe 3D viewport
- [x] Software renderer for CPU rasterization
- [x] Feature tree component
- [x] Keyboard navigation
- [x] WASM engine integration

### MCP Server

- [x] Server scaffolding with MCP SDK
- [x] `create_cad_document` tool
- [x] `export_cad` tool (STL, GLB)
- [x] `inspect_cad` tool with volume/area/bbox
- [x] Position resolution (absolute, named, percentage)
- [x] Hole operation for through-holes

## Acceptance Criteria

- [x] `vcad export input.vcad output.stl` produces valid binary STL
- [x] `vcad export input.vcad output.step` exports primitive B-rep
- [x] `vcad import input.step output.vcad` creates valid document
- [x] `vcad info` shows node count, materials, mesh stats
- [x] JS CLI renders 3D viewport in terminal
- [x] MCP server connects to Claude Desktop
- [x] `create_cad_document` produces valid IR from primitives + ops
- [x] `export_cad` writes STL/GLB files to disk
- [x] `inspect_cad` returns volume, surface area, bounding box, center of mass
- [x] AI can create CAD models end-to-end via MCP

## Future Enhancements

- [ ] REST API for web service integration
- [ ] GitHub Actions reusable workflow (`vcad-action`)
- [ ] Batch mode for processing multiple files
- [ ] Webhooks for async export notifications
- [ ] Watch mode for auto-rebuild on file change
- [ ] gRPC API for high-performance integration
- [ ] Python bindings via PyO3
- [ ] Assembly support in MCP tools
- [ ] Parametric updates via MCP (modify existing document)
