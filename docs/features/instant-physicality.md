# Instant Physicality

**Score: 61/100** | **Priority: #29**

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |

> Verified against repo 2026-07-30. A substantial slice shipped via MCP tools (`packages/mcp`): BOM generation (`bom_create`/`bom_add_line`/`bom_export`), manufacturing quotes (`quote_manufacturing`), COTS sourcing (`search_mechanical_parts`, `search_electronic_parts`, `find_alternatives`), and ordering (`place_order`, `authorize_spend`, `list_orders`, `get_order_status`, `get_order_feed` — see `packages/mcp/src/fabricate/broker.ts`). Not shipped: in-app "Order Parts" button, motor/fastener auto-selection from joint loads, generated assembly instructions.

## Overview

Click "Order Parts" and get a quote for real parts. 3D printed links, CNC brackets, motors, fasteners — all spec'd and priced. The bridge from simulation to reality.

## Why It Matters

Simulation without building is just a game.

**Traditional path:**
1. Export STL
2. Find a 3D printer or service
3. Source motors separately
4. Buy fasteners (wrong ones, twice)
5. Figure out tolerances after parts don't fit
6. Assemble with frustration

**vcad path:**
1. Validated design
2. One click
3. Parts arrive
4. Assemble

Closes the loop. **Design → Simulate → BUILD.**

## Technical Implementation

### BOM Generation

Extract bill of materials directly from vcad document:
- Parse part tree for custom geometry
- Identify joints requiring actuators
- Calculate fastener requirements from joint definitions
- Sum quantities and generate line items

### Part Classification

| Category | Criteria | Routing |
|----------|----------|---------|
| 3D Printable | No tight tolerances, reasonable geometry | FDM/SLA services |
| CNC Required | Tight fits, metal, thin features | CNC machining |
| COTS | Standard components (bearings, motors) | Distributor APIs |

### Tolerance Analysis

- Analyze mating faces for fit type (clearance, transition, interference)
- Flag fits tighter than manufacturing process allows
- Suggest tolerance relaxation or process upgrade
- Account for material shrinkage in 3D printing

### Motor Selection

From joint torque requirements in simulation:
```
joint.max_torque → motor database lookup
  - Safety factor: 1.5x
  - Speed matching
  - Form factor constraints
  - Protocol compatibility (if multiple joints)
```

### Fastener Selection

From joint types and loads:
- Revolute joints → shoulder bolts or pins
- Fixed joints → screws with appropriate thread engagement
- Load analysis → bolt grade selection

### Manufacturing API Integration

**3D Printing:**
- Xometry — instant quoting API
- Craftcloud — multi-vendor comparison
- Shapeways — consumer-friendly

**Motors:**
- Actuonix — linear actuators
- Dynamixel — smart servos
- ODrive — brushless controllers

**Fasteners:**
- McMaster-Carr — comprehensive catalog
- Misumi — configurable components

## User Flow

```
1. User clicks "Order Parts"

2. System analyzes design:
   - 5 custom parts → 3D printable (PLA suggested)
   - 3 joints → need motors (recommends Dynamixel XM430)
   - 12 fasteners needed (M3x10, M4x16 socket head)
   - 2 bearings → 608ZZ standard

3. Shows quote breakdown:
   ┌─────────────────────────────────────────┐
   │ 3D Printing (Xometry, 5-day)    $45.00  │
   │ Motors (3x Dynamixel XM430)    $180.00  │
   │ Hardware (McMaster)             $12.00  │
   │ Bearings (2x 608ZZ)              $3.00  │
   ├─────────────────────────────────────────┤
   │ Total                          $240.00  │
   └─────────────────────────────────────────┘

4. User clicks "Order" → parts ship from respective vendors

5. Assembly instructions generated with tracking links
```

## Smart Recommendations

| Design Choice | System Response |
|---------------|-----------------|
| Joint torque 2Nm | "Recommend Dynamixel XM430-W350 (2.8Nm stall)" |
| Wall thickness 0.8mm | "Too thin for FDM. Suggest SLA (+$15) or increase to 1.5mm" |
| Overhang 60° | "Requires supports. Redesign with 45° chamfer to reduce cost" |
| Bore tolerance ±0.05mm | "Requires CNC machining (+$30) or ream after print" |
| Large flat surface | "Suggest adding ribs to prevent warping" |
| Dissimilar materials mating | "Thermal expansion mismatch — add clearance or use same material" |

## Assembly Support

### Generated Documentation

1. **Step-by-step instructions**
   - Ordered by dependency (install bearings before motor)
   - Photos/renders of each step
   - Required tools listed

2. **Exploded view animation**
   - Interactive 3D exploded view
   - Click part to highlight in instructions
   - Assembly sequence playback

3. **Fastener torque specs**
   - Calculated from bolt grade and thread size
   - Adjusted for plastic threads vs metal inserts

4. **Wiring diagram**
   - Motor connections
   - Power distribution
   - Communication bus (if Dynamixel chain)

### Example Assembly Step

```
Step 4: Install Motor in Bracket

Parts needed:
- Motor_Bracket_Left (3D printed)
- Dynamixel XM430-W350
- M2.5x8 socket head (4x)

Tools:
- 2mm hex driver

Instructions:
1. Orient motor with cable facing down
2. Align mounting holes
3. Install 4x M2.5 screws
4. Torque to 0.3Nm (finger tight + 1/4 turn)

[Render showing assembled state]
```

## Success Metrics

| Metric | Target | Measurement |
|--------|--------|-------------|
| BOM accuracy | >95% | Parts list matches what's needed |
| Motor recommendations | >90% appropriate | Torque/speed sufficient, not over-spec |
| First-try fit | >85% | Parts assemble without rework |
| Quote accuracy | ±10% | Final invoice vs initial quote |
| Order completion | >80% | Users who start checkout finish |

## Competitive Advantage

No CAD tool goes from simulation to ordering in one click.

- **Fusion 360**: Export STL, upload to separate service, figure out motors yourself
- **Onshape**: Same manual process
- **SolidWorks**: Expensive PDM/PLM add-ons for enterprise only

vcad makes the physical world accessible to hobbyists and startups who don't have a supply chain team.

## Business Model

**Revenue streams:**

1. **Referral fees** — Manufacturing partners pay 5-15% for qualified leads
2. **Convenience markup** — Optional "vcad handles shipping consolidation" (+10%)
3. **Premium recommendations** — Pro users get multi-vendor price comparison

**Partner incentives:**
- Guaranteed correct file formats
- Pre-validated geometry (no failed prints)
- Streamlined communication (specs embedded in order)

## Implementation Phases

### Phase 1: Manual BOM
- Generate parts list from document
- User exports and orders manually
- Validate classification accuracy

### Phase 2: 3D Print Integration
- Xometry API integration
- Automatic STL generation with correct orientation
- In-app quoting

### Phase 3: Full Stack
- Motor database and selection
- Fastener catalog integration
- Assembly instruction generation

### Phase 4: One-Click Order
- Unified checkout
- Multi-vendor coordination
- Shipment tracking in-app

## Open Questions

1. **Liability** — If motor recommendation is wrong, who's responsible?
2. **International** — Different fastener standards (metric vs imperial)
3. **Returns** — Handle returns for wrong parts through vcad or direct?
4. **Custom motors** — What if no COTS motor fits the requirements?

## Related Features

- **Simulation Accuracy** — Better sim = better motor selection
- **Tolerance Visualization** — Show which fits are tight
- **Material Database** — Mechanical properties for stress analysis
- **Cost Optimization Mode** — Redesign suggestions to reduce manufacturing cost
