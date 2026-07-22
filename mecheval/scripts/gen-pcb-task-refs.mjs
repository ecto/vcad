// Generate golden reference boards for the pcbeval P-suite tasks.
//
// Drives the local @vcad/mcp server through the same pipeline a real
// agent uses (create_schematic → place_components → route_nets → run_drc)
// and writes each DRC-clean result to mecheval/tasks/<task_id>.vcad —
// the "expected" column on the /pcb leaderboard. Fails loudly if any
// board comes back dirty, so a reference can never regress silently.
//
// Usage: node mecheval/scripts/gen-pcb-task-refs.mjs
// Requires: npm ci + `npm run build -w @vcad/mcp` (harness resolves the
// server the same way).

import { createRequire } from "node:module";
import { writeFileSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const TASKS_DIR = resolve(REPO_ROOT, "mecheval/tasks");

const pin = (number, name, type = "Passive") => ({ number, name, type });
const passive2 = [pin("1", "A"), pin("2", "B")];

/** Per-task golden recipes: schematic + board + placement strategy. */
const RECIPES = [
  {
    id: "p1-led-resistor-01",
    board: { board_width: 28, board_height: 18 },
    strategy: "grid",
    components: [
      { ref: "J1", footprint: "PinHeader_1x02_P2.54mm", value: "POWER", x: 10, y: 20,
        pins: [pin("1", "VCC"), pin("2", "GND")] },
      { ref: "R1", footprint: "Resistor_SMD:R_0805", value: "330", x: 40, y: 20, pins: passive2 },
      { ref: "D1", footprint: "LED_SMD:LED_0805", value: "LED", x: 70, y: 20,
        pins: [pin("1", "K"), pin("2", "A")] },
    ],
    nets: { VCC: ["J1.1", "R1.1"], LED_A: ["R1.2", "D1.2"], GND: ["D1.1", "J1.2"] },
  },
  {
    id: "p1-decoupled-mcu-01",
    board: { board_width: 50, board_height: 50 },
    strategy: "grid",
    components: [
      { ref: "U1", footprint: "QFP-32_7x7mm_P0.8mm", value: "MCU", x: 50, y: 30,
        pins: [pin("1", "VDD", "PowerInput"), pin("2", "GND", "PowerInput"),
               pin("3", "SWDIO", "Bidirectional"), pin("4", "SWCLK", "Input")] },
      { ref: "C1", footprint: "Capacitor_SMD:C_0402", value: "100nF", x: 30, y: 20, pins: passive2 },
      { ref: "C2", footprint: "Capacitor_SMD:C_0402", value: "100nF", x: 30, y: 40, pins: passive2 },
      { ref: "J1", footprint: "PinHeader_1x04_P2.54mm", value: "SWD", x: 80, y: 30,
        pins: [pin("1", "VCC"), pin("2", "SWDIO"), pin("3", "SWCLK"), pin("4", "GND")] },
    ],
    nets: {
      VCC: ["U1.1", "C1.1", "C2.1", "J1.1"],
      GND: ["U1.2", "C1.2", "C2.2", "J1.4"],
      SWDIO: ["U1.3", "J1.2"],
      SWCLK: ["U1.4", "J1.3"],
    },
  },
  {
    id: "p2-usb-power-01",
    board: { board_width: 40, board_height: 25 },
    strategy: "force_directed",
    // The auto-placer can leave one courtyard overlap on this dense board;
    // nudge the input cap to a known-good spot before routing.
    placements: [{ ref: "C1", x: 16, y: 6 }],
    components: [
      { ref: "J1", footprint: "USB_C_Receptacle_Simple", value: "USB", x: 10, y: 30,
        pins: [pin("1", "VBUS", "PowerOutput"), pin("2", "GND", "PowerInput")],
        pads: [
          { number: "1", padType: "SMD", shape: { type: "Rect", width: 1.2, height: 1.5 }, position: { x: -2, y: 0 } },
          { number: "2", padType: "SMD", shape: { type: "Rect", width: 1.2, height: 1.5 }, position: { x: 2, y: 0 } },
        ] },
      { ref: "U1", footprint: "SOT-23-5", value: "LDO-3V3", x: 40, y: 30,
        pins: [pin("1", "VIN", "PowerInput"), pin("2", "GND", "PowerInput"), pin("3", "VOUT", "PowerOutput")] },
      { ref: "C1", footprint: "Capacitor_SMD:C_0805", value: "1uF", x: 25, y: 15, pins: passive2 },
      { ref: "C2", footprint: "Capacitor_SMD:C_0805", value: "1uF", x: 55, y: 15, pins: passive2 },
      { ref: "R1", footprint: "Resistor_SMD:R_0805", value: "1k", x: 70, y: 30, pins: passive2 },
      { ref: "D1", footprint: "LED_SMD:LED_0805", value: "PWR", x: 85, y: 30,
        pins: [pin("1", "K"), pin("2", "A")] },
    ],
    nets: {
      VBUS: ["J1.1", "U1.1", "C1.1"],
      "3V3": ["U1.3", "C2.1", "R1.1"],
      LED_A: ["R1.2", "D1.2"],
      GND: ["J1.2", "U1.2", "C1.2", "C2.2", "D1.1"],
    },
  },
  {
    id: "p2-h-bridge-01",
    board: { board_width: 50, board_height: 40 },
    strategy: "force_directed",
    routeEffort: 4,
    components: [
      { ref: "Q1", footprint: "SOT-23", value: "PMOS-HS-A", x: 20, y: 15,
        pins: [pin("1", "G", "Input"), pin("2", "S"), pin("3", "D")] },
      { ref: "Q2", footprint: "SOT-23", value: "PMOS-HS-B", x: 60, y: 15,
        pins: [pin("1", "G", "Input"), pin("2", "S"), pin("3", "D")] },
      { ref: "Q3", footprint: "SOT-23", value: "NMOS-LS-A", x: 20, y: 45,
        pins: [pin("1", "G", "Input"), pin("2", "S"), pin("3", "D")] },
      { ref: "Q4", footprint: "SOT-23", value: "NMOS-LS-B", x: 60, y: 45,
        pins: [pin("1", "G", "Input"), pin("2", "S"), pin("3", "D")] },
      { ref: "J1", footprint: "PinHeader_1x02_P2.54mm", value: "MOTOR", x: 40, y: 30,
        pins: [pin("1", "OUT_A"), pin("2", "OUT_B")] },
      { ref: "J2", footprint: "PinHeader_1x03_P2.54mm", value: "CTRL", x: 5, y: 30,
        pins: [pin("1", "VMOT", "PowerInput"), pin("2", "GATE_A", "Input"), pin("3", "GATE_B", "Input")] },
    ],
    nets: {
      VMOT: ["J2.1", "Q1.2", "Q2.2"],
      GND: ["Q3.2", "Q4.2"],
      OUT_A: ["Q1.3", "Q3.3", "J1.1"],
      OUT_B: ["Q2.3", "Q4.3", "J1.2"],
      GATE_A: ["J2.2", "Q1.1", "Q3.1"],
      GATE_B: ["J2.3", "Q2.1", "Q4.1"],
    },
  },
];

async function connect() {
  const { Client } = await import("@modelcontextprotocol/sdk/client/index.js");
  const { StdioClientTransport } = await import("@modelcontextprotocol/sdk/client/stdio.js");
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [require.resolve("@vcad/mcp")],
  });
  const client = new Client({ name: "gen-pcb-task-refs", version: "0.0.1" }, { capabilities: {} });
  await client.connect(transport);
  return client;
}

function firstJson(result) {
  for (const c of result.content ?? []) {
    if (c.type !== "text" || c.text == null) continue;
    try {
      return JSON.parse(c.text);
    } catch {
      /* next block */
    }
  }
  throw new Error("no JSON block in tool result");
}

async function call(mcp, name, args) {
  const result = await mcp.callTool({ name, arguments: args });
  if (result.isError) {
    const text = (result.content ?? []).map((c) => c.text ?? "").join("\n");
    throw new Error(`${name} failed: ${text}`);
  }
  return firstJson(result);
}

const mcp = await connect();
let failures = 0;
try {
  for (const r of RECIPES) {
    const created = await call(mcp, "create_schematic", {
      title: `${r.id} reference`,
      components: r.components,
      nets: r.nets,
    });
    const docId = created.document_id ?? created.next_actions?.[0]?.args?.document_id;
    if (!docId) throw new Error(`${r.id}: create_schematic returned no document_id`);

    await call(mcp, "place_components", { document_id: docId, strategy: r.strategy, ...r.board });
    if (r.placements) {
      await call(mcp, "set_placement", { document_id: docId, placements: r.placements });
    }
    await call(mcp, "route_nets", { document_id: docId, effort: r.routeEffort ?? 3 });
    const drc = await call(mcp, "run_drc", { document_id: docId });
    if ((drc.violations ?? 1) !== 0) {
      failures++;
      console.error(`✗ ${r.id}: DRC dirty (${drc.violations} violations) — not written`);
      console.error(JSON.stringify(drc.sample?.slice(0, 3) ?? [], null, 1));
      continue;
    }
    const doc = await call(mcp, "get_document", { document_id: docId });
    // The server returns either {document: {...}} or the raw Document.
    const document = doc.document ?? (doc.nodes && doc.roots ? doc : null);
    if (!document) throw new Error(`${r.id}: get_document returned no Document`);
    const out = resolve(TASKS_DIR, `${r.id}.vcad`);
    writeFileSync(out, JSON.stringify(document, null, 2));
    console.log(`✓ ${r.id}: DRC clean, wrote ${out}`);
  }
} finally {
  await mcp.close();
}
if (failures > 0) process.exit(1);
