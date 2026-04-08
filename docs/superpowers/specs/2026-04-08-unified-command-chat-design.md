# Unified Command + Chat System

**Date:** 2026-04-08
**Status:** Draft
**Scope:** In-app AI chat sidebar, S-key command palette integration, unified tool API, context-aware interaction

## Problem

Fusion 360 users expect a discoverable, keyboard-driven interface with immediate access to all tools. vcad has a capable engine but its interaction model — toolbar-only input with a bolt-on property panel — doesn't match the fluency that power users expect. Meanwhile, vcad's AI capabilities (MCP server) are only accessible externally, and the MCP tool API is monolithic (single `create_cad_document` call) rather than composable.

## Goals

1. Add an AI chat sidebar that can both answer questions and execute CAD operations on the live document
2. Unify the command palette (existing Cmd+K) with AI — same input, local fast path for known commands, AI fallback for natural language
3. Design a granular, auto-generated tool API shared between in-app chat and external MCP
4. Support context pills from viewport selection so the AI always knows what the user is looking at
5. Gate AI chat behind authentication with a limited free tier for logged-out users

## Non-Goals

- Toolbar reorganization (follow-up work)
- Navigation presets / Fusion 360 mouse mode (follow-up, presets already partially exist)
- Redesigning the onboarding tutorial
- Mobile-specific layout (bottom drawer deferred)

## Design

### Interaction Model

The system has two interaction surfaces that share a single backend:

**Command Palette (S-key / Cmd+K)**
- Center-screen overlay with fuzzy search over all registered commands
- Results ranked by: exact match > selection relevance > recency
- Direct match executes locally with no network round-trip
- No match shows "Ask AI" escalation at bottom of results; pressing Enter opens the chat sidebar
- S-key added as additional trigger alongside existing Cmd+K for Fusion 360 muscle memory

**Chat Sidebar (right panel)**
- Always visible for logged-in users (collapsible)
- Replaces the property panel — inline editing moves to the feature tree, advanced parameter editing goes through the AI
- Text input at bottom, chat thread above
- Context pills auto-populate from Zustand selection state
- AI responses can include:
  - Text (markdown-rendered)
  - Tool call cards (compact, expandable to show parameters)
  - Inline action buttons ("Apply fillet", "Show me first")
- "New" button starts fresh thread; history accessible via menu
- Collapse button hides sidebar for full viewport width

**Context Pills**
- When geometry is selected (part, face, edge, vertex), a removable pill appears above the chat input
- Multiple selections produce multiple pills
- Pills are serialized as structured context sent to the AI: `{ partId, partName, geometryType, faceIndex?, dimensions? }`
- Placeholder text adapts: "Ask about Rib_1..." when a part is selected

**Logged-Out Experience**
- Chat sidebar visible with 5 free messages per session (counter in localStorage)
- After limit, input replaced with sign-in CTA
- Command palette works fully regardless of auth state (it's local)

**Chat Thread Persistence**
- Logged-in users: threads persisted to Supabase (linked to user account). Survive page reloads and device switches.
- Logged-out users: threads stored in localStorage only. Lost on clear/incognito.

### AI Backend

**Provider:** Abstract provider layer via Vercel AI SDK. Claude (Anthropic) as initial provider, designed to swap to vcad's own model. Provider configured via environment variable.

**API Route:** Single `/api/chat` endpoint using AI SDK `streamText`:
- Receives: user message, context (serialized selection + document summary), conversation history
- Returns: streamed response with interleaved text and tool calls
- Auth: Supabase JWT verification; rate-limited free tier for unauthenticated requests

**System Prompt:** The AI receives a system prompt describing vcad's capabilities, coordinate system (Z-up, millimeters), available tools, and behavioral guidelines (confirm before destructive operations, explain what it did after tool calls).

**Document Context:** The AI receives a summary of the current document state, not the full IR DAG:
- Part names, types, and key dimensions
- Current selection details
- Recent operation history (last 5-10 operations)
- Assembly structure (if applicable)

### Unified Tool API

#### Source of Truth: IR Types

The `CsgOp` type union in `packages/ir/src/index.ts` defines all CAD operations. Each variant (CubeOp, FilletOp, ExtrudeOp, etc.) has a typed interface with all parameters. The command registry is auto-generated from these types.

#### Auto-Generation Pipeline

1. IR types define the operation vocabulary (already in sync between Rust and TypeScript)
2. Runtime reflection via `zod` schemas derives JSON schemas from the TS IR types (no build step needed; codegen can be added later for performance if the registry grows large)
3. A metadata mapping table provides human-friendly names, categories, and display order:
   ```ts
   const metadata: Record<string, CommandMeta> = {
     CubeOp:        { name: "Box",        category: "Create",    shortcut: null },
     CylinderOp:    { name: "Cylinder",   category: "Create",    shortcut: null },
     FilletOp:      { name: "Fillet",      category: "Modify",    shortcut: null },
     DifferenceOp:  { name: "Difference",  category: "Boolean",   shortcut: null },
     // ... one line per CsgOp variant
   };
   ```
4. The registry combines schema + metadata into command objects consumed by all four surfaces

#### Command Structure

```ts
interface Command {
  id: string;                        // e.g. "add_fillet"
  name: string;                      // e.g. "Fillet"
  category: string;                  // e.g. "Modify"
  description: string;               // For AI tool descriptions
  shortcut?: string;                 // Keyboard shortcut if any
  schema: JSONSchema;                // Parameter schema (auto-generated from IR)
  execute: (args: unknown, store: DocumentStore) => void;
}
```

#### Four Consumers, One Registry

| Consumer | How it uses the registry |
|----------|------------------------|
| Command palette | Fuzzy search over `name` + `category`, calls `execute` directly |
| AI chat tools | `id`, `description`, `schema` become AI SDK tool definitions; tool calls invoke `execute` |
| MCP server | Same `id`, `description`, `schema` registered as MCP tools |
| Toolbar buttons | Existing UI wires into `execute` for the corresponding command |

#### Hand-Authored Commands

A small set of commands that don't map to CsgOp variants:

| Command | Category | Description |
|---------|----------|-------------|
| `inspect` | Query | Measure volume, area, dimensions of selected part |
| `measure` | Query | Distance/angle between two selections |
| `list_parts` | Query | List all parts with types and dimensions |
| `get_selection` | Query | Describe current selection |
| `select_by_name` | Query | Select geometry by part name |
| `set_parameter` | Edit | Modify an existing operation's parameters |
| `rename_part` | Edit | Rename a part in the feature tree |
| `delete_part` | Edit | Remove a part and its dependents |
| `undo` | Edit | Undo last operation |
| `redo` | Edit | Redo last undone operation |

#### File Structure

```
packages/core/src/commands/
├── registry.ts           # CommandRegistry class, registration, lookup
├── codegen.ts            # Auto-generate commands from IR CsgOp types
├── metadata.ts           # Human-friendly names, categories, shortcuts
├── manual-commands.ts    # Hand-authored query/edit commands
└── index.ts              # Public API
```

### MCP Parity

The external MCP server at `mcp.vcad.io` exposes the same command registry over MCP protocol:
- Authenticated via vcad.io login (Supabase JWT)
- Operates on a server-managed document session
- Same tool IDs, same schemas, same behavior
- External clients (Claude Desktop, etc.) get the same capabilities as in-app chat
- Existing monolithic `create_cad_document` tool retained for backward compatibility but deprecated in favor of granular tools

### Data Flow

```
Viewport Selection
  → Zustand selection store
  → Context pills render above chat input
  → Selection serialized as structured context

Chat Input (AI path)
  → Message + context → POST /api/chat
  → AI SDK streamText with tool definitions from command registry
  → Tool calls → execute against Zustand document store
  → Store updates → viewport re-renders (React Three Fiber)
  → Tool results + text stream back to chat thread

Command Palette (local path)
  → Fuzzy match against command registry
  → Match found → execute directly, no AI
  → No match → escalate to chat sidebar

External MCP
  → Claude Desktop → mcp.vcad.io (authenticated)
  → Same command registry, same schemas
  → Executes against server-side document session
```

### Error Handling

- **Tool call fails** (kernel error): Error shown inline in chat with the kernel message. AI sees the error and can suggest alternatives.
- **Rate limiting** (free tier): localStorage counter. Graceful degradation with sign-in CTA.
- **Offline**: Command palette works fully. Chat shows "Offline" with disabled input.
- **Large documents**: Only summary context sent to AI, not full IR DAG.
- **Undo integration**: AI tool calls are normal store operations — they participate in existing undo/redo stack. Users can undo what the AI did with Cmd+Z.

### UI Layout Summary

```
+-------------------+---------------------------+------------------+
| Feature Tree      |        Viewport           |    Chat Sidebar  |
| (left, ~160px)    |   (center, flexible)      |  (right, ~260px) |
|                   |                           |                  |
| > Part_1          |                           | [thread msgs]    |
|   - Box           |     [3D scene]            |                  |
|   - Fillet_1      |                           | [tool call card] |
| > Part_2          |                           |                  |
|   - Extrude       |                           | [ai response]    |
|                   |                           |                  |
|                   |                           |  [context pills] |
|                   |                           |  [input field]   |
+-------------------+---------------------------+------------------+
```

Property panel is eliminated. Feature tree items expand inline for parameter editing. Complex edits go through the chat.
