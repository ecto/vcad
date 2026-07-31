# Multiplayer Physics

**Score: 70/100** | **Priority: #20**

## Status

| Field | Value |
|-------|-------|
| State | `proposed` |

> Verified against repo 2026-07-30. No CRDT sync (no Yjs/Automerge dependency), no real-time co-editing, no physics-state broadcast. Adjacent shipped features: read-only session sharing via the MCP `share_session` tool and Supabase document sync (`packages/auth/src/sync.ts`) — neither is real-time multi-writer collaboration.

## Overview

Share a URL. Two people manipulate the same robot in real-time. The mechanical designer adjusts geometry while the controls engineer tunes PID gains. Both see the same simulation. Changes merge like git.

This is Figma for robotics—real-time collaborative CAD with live physics simulation.

## Why It Matters

Robot design is inherently collaborative:
- **Mechanical engineers** design the structure
- **Electrical engineers** place sensors and actuators
- **Software engineers** tune control parameters

Current workflow is painful:
- Screen sharing with one driver
- Taking turns on the same file
- Manual merge of parallel changes
- "Send me the latest version" conversations

Figma proved that real-time collaboration transforms creative workflows. Remote and hybrid teams need this capability—waiting for async handoffs kills momentum.

## Technical Implementation

### CRDT-Based Document Sync

Document state uses Conflict-free Replicated Data Types for automatic merge:

```typescript
interface CollaborativeDocument {
  // CRDT types for each component
  geometry: Y.Map<Part>;           // Yjs for geometry tree
  physics: Y.Map<PhysicsParams>;   // Joint limits, PID gains
  simulation: SharedState;         // Separate broadcast channel
}
```

Key properties:
- **Eventual consistency**: All clients converge to same state
- **No coordination required**: Edits apply immediately locally
- **Automatic conflict resolution**: Last-writer-wins with causal ordering

### Physics State Broadcast

Physics simulation runs on one authoritative client (or server):

```
Physics Authority ──(30Hz)──▶ All Clients
     │
     ├── Joint positions/velocities
     ├── Contact points
     └── Sensor readings
```

Reduced broadcast rate (10-30Hz) balances:
- Network bandwidth
- Visual smoothness (interpolate between frames)
- Latency tolerance

### Optimistic Local Updates

```
User Input ──▶ Apply Locally ──▶ Send to Server
                    │
                    ▼
              Render Immediately
                    │
                    ▼
              Reconcile on Server Response
```

Edits appear instantly (<16ms). Server reconciliation handles conflicts.

### Cursor Presence

See where collaborators are working:
- 3D cursor position in viewport
- Selected entities highlighted with user color
- Active tool indicator

## Architecture

```
Client A                    Server                    Client B
   │                          │                          │
   │──edit geometry──────────▶│◀──────tune PID gains────│
   │                          │                          │
   │◀──sync─────────CRDT──────┼──────sync───────────────▶│
   │                          │                          │
   │◀──physics state (30Hz)───┼───physics state─────────▶│
   │                          │                          │
   │◀──presence (cursors)─────┼───presence──────────────▶│
```

### Component Breakdown

| Component | Technology | Role |
|-----------|------------|------|
| Document sync | Yjs + WebSocket | CRDT replication |
| Physics broadcast | WebRTC DataChannel | Low-latency state |
| Presence | WebSocket | Cursors, selections |
| Media | WebRTC | Voice/video (optional) |

### Server Requirements

- **Stateless relay**: Forward CRDT updates, no business logic
- **Physics authority**: Optional server-side simulation for consistency
- **Room management**: Create/join sessions, handle disconnects

## User Experience

### Sharing Flow

1. Click **Share** button
2. Copy link: `vcad.io/doc/abc123?collab=true`
3. Collaborator opens link
4. Instant connection—no account required for viewing

### Collaboration UI

- **User avatars**: Top bar shows who's in the session
- **Colored cursors**: Each user has a distinct color
- **Selection halos**: See what others have selected
- **Activity feed**: "Alice moved Joint 1" notifications

### Follow Mode

Click a collaborator's avatar to:
- Lock your viewport to theirs
- See their cursor and selections
- Hear their voice (if enabled)

### Communication

- **Built-in chat**: Text sidebar
- **Voice**: One-click audio (WebRTC)
- **Video**: Optional picture-in-picture

## Challenges

### Physics Determinism

Floating-point math varies across platforms. Solutions:
- **Single authority**: One client runs physics, broadcasts state
- **Deterministic engine**: Fixed-point math (expensive)
- **Tolerance-based sync**: Accept small divergence, snap on large

### Latency Compensation

At 100ms round-trip:
- Local edits feel instant (optimistic)
- Remote edits appear with delay
- Physics interpolation smooths jitter

### Conflict Resolution UX

When two users edit the same property:
- **Last-writer-wins**: Simple, can lose work
- **Operational transform**: Merge intentions
- **User notification**: "Bob also edited this—review?"

Recommended: LWW with undo history. Users can revert if needed.

### Scaling to 5+ Users

| Users | Bandwidth | Latency | Strategy |
|-------|-----------|---------|----------|
| 2-3 | Low | <100ms | P2P mesh |
| 4-8 | Medium | <150ms | Server relay |
| 9+ | High | <200ms | Interest management |

Interest management: Only sync what's in each user's viewport.

## Success Metrics

| Metric | Target |
|--------|--------|
| Edit-to-visible latency | <100ms |
| Physics sync tolerance | <1mm position error |
| Reconnection time | <2s |
| Data loss on conflict | 0% |
| Concurrent users | 8+ per session |

## Competitive Advantage

**No CAD tool offers real-time collaborative physics simulation.**

- Onshape: Real-time CAD, no physics
- Gazebo: Physics sim, no collaboration
- Figma: Real-time collaboration, no 3D/physics

vcad can own this intersection. For robotics teams, this is the missing workflow.

## Implementation Phases

### Phase 1: Document Sync
- Yjs integration for geometry tree
- Basic presence (cursors)
- Share link generation

### Phase 2: Physics Broadcast
- Single-authority physics
- State interpolation
- Latency indicators

### Phase 3: Polish
- Voice/video integration
- Follow mode
- Conflict notifications
- Mobile viewer support

## Dependencies

- Yjs or Automerge (CRDT library)
- WebRTC for low-latency channels
- Signaling server (can use existing WebSocket infra)
- TURN server for NAT traversal

## Open Questions

1. **Server-side physics?** More consistent but adds infrastructure cost
2. **Recording?** Save collaboration sessions for playback
3. **Permissions?** View-only vs. edit access per user
4. **Offline?** Queue edits and sync on reconnect
