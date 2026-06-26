#!/usr/bin/env node
/**
 * F405 flight controller, co-verified inside its 3D-printable case.
 *
 * The end-to-end showcase for vcad's cross-domain verification axis: it builds
 * the board (PCB engine) AND the case (BRep kernel) in one process, then
 * cross-checks them — board fits, components clear the lid, mounting holes land
 * on the standoffs, the USB-C port lines up with the wall cutout — and ships
 * fab outputs for both domains: Gerbers for the board, STL/GLB for the case.
 *
 * Run from the repo root after building the workspace:
 *   npm run build --workspaces
 *   node examples/f405-enclosure/build.mjs
 *
 * Outputs land in examples/f405-enclosure/out/.
 */

import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { mkdirSync, writeFileSync } from "node:fs";

import { Engine } from "@vcad/engine";
import { getNodePcb, getPcbNodeIds } from "@vcad/core";
import {
  createSchematic,
  placeComponents,
  setPlacement,
  routeNets,
  runDrc,
  exportGerber,
  buildReceipt,
} from "../../packages/mcp/dist/tools/ecad.js";
import { openDocument, getSession } from "../../packages/mcp/dist/tools/session.js";
import { checkEnclosureFit } from "../../packages/mcp/dist/tools/enclosure.js";
import { exportCad } from "../../packages/mcp/dist/tools/export.js";

import {
  f405CaseDocument,
  boardHoleCenters,
  usbConnectorLocal,
  PARAMS,
} from "./geometry.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const OUT = join(HERE, "out");
mkdirSync(OUT, { recursive: true });
process.env.VCAD_MCP_EXPORT_DIR = OUT;

const parse = (r) => JSON.parse(r.content[0].text);
const log = (...a) => console.log(...a);
const rule = () => log("─".repeat(64));

function getPcb(id) {
  const doc = getSession(id);
  const nodeIds = getPcbNodeIds(doc);
  return nodeIds.length ? getNodePcb(doc, nodeIds[0]) : doc.pcb ?? null;
}

/** Inject four M3 NPTH mounting holes at the board-local 30.5mm pattern. */
function addMountingHoles(pcb) {
  boardHoleCenters().forEach((h, i) => {
    pcb.footprints.push({
      ref: `H${i + 1}`,
      value: "M3",
      footprintName: "MountingHole_3.2mm_M3",
      position: { x: h.x, y: h.y },
      pads: [
        {
          number: "1",
          padType: "NPTH",
          shape: { type: "Circle", diameter: 3.2 },
          position: { x: 0, y: 0 },
          layers: ["FCu", "BCu"],
          drill: { diameter: 3.2 },
        },
      ],
    });
  });
}

async function main() {
  const engine = await Engine.init();

  rule();
  log("  vcad cross-domain co-design — F405 FC in a 3D-printed case");
  rule();

  // 1. The enclosure (BRep CAD session).
  const caseDoc = f405CaseDocument();
  const enc = parse(openDocument({ initial: caseDoc }));
  log(`\n[1] Enclosure: ${PARAMS.outer.x}×${PARAMS.outer.y}×${PARAMS.outer.z}mm case`);
  log(`    open-top tray, ${PARAMS.wall}mm walls, 4 standoffs on the 30.5mm M3 pattern, USB-C cutout`);

  // 2. The board (PCB engine session).
  const created = parse(
    await createSchematic({
      components: [
        {
          ref: "U1",
          value: "STM32F405RGT6",
          footprint: "QFP-64",
          x: 18,
          y: 18,
          pins: [
            { number: "1", name: "VBAT", type: "PowerInput", x: 0, y: 0 },
            { number: "2", name: "PA11_DM", type: "Bidirectional", x: 0, y: 1 },
            { number: "3", name: "PA12_DP", type: "Bidirectional", x: 0, y: 2 },
          ],
        },
        {
          ref: "U2",
          value: "ICM-42688-P",
          footprint: "QFN-16",
          x: 12,
          y: 12,
          pins: [{ number: "1", name: "VDD", type: "PowerInput", x: 0, y: 0 }],
        },
        {
          ref: "J1",
          value: "USB-C",
          footprint: "USB_C_Receptacle",
          x: 30,
          y: 18,
          pins: [
            { number: "A6", name: "DP", type: "Bidirectional", x: 0, y: 0 },
            { number: "A7", name: "DM", type: "Bidirectional", x: 0, y: 1 },
          ],
        },
      ],
      nets: { USB_DP: ["U1.3", "J1.A6"], USB_DM: ["U1.2", "J1.A7"] },
    }),
  );
  const boardId = created.document_id;
  parse(await placeComponents({ document_id: boardId, board_width: 36, board_height: 36 }));
  const usb = usbConnectorLocal();
  parse(
    await setPlacement({
      document_id: boardId,
      placements: [
        { ref: "J1", x: usb.x, y: usb.y }, // USB-C on the +X edge
        { ref: "U1", x: 18, y: 18 }, // MCU centered
        { ref: "U2", x: 9, y: 27 }, // IMU in a quiet corner
      ],
    }),
  );
  addMountingHoles(getPcb(boardId));
  log(`\n[2] Board: 36×36mm, STM32F405 + ICM-42688 IMU + USB-C, 4× M3 holes`);

  // Route the USB pair and check copper.
  parse(await routeNets({ document_id: boardId }));
  const drc = parse(await runDrc({ document_id: boardId }));
  log(`    DRC: ${drc.violation_count ?? drc.violations ?? 0} violations`);

  // 3. Cross-domain verification.
  const fit = parse(
    await checkEnclosureFit(
      { document_id: boardId, enclosure_document_id: enc.document_id, derive: false },
      engine,
    ),
  );
  rule();
  log(`\n[3] ${fit.summary}`);
  log(`    cavity ${round(fit.cavity.maxX - fit.cavity.minX)}×${round(fit.cavity.maxY - fit.cavity.minY)}mm` +
    ` × ${round(fit.cavity.ceilZ - fit.cavity.floorZ)}mm deep · ${fit.standoffs_detected} standoffs · ${fit.openings_detected} wall cutout`);
  for (const c of fit.checks) {
    const mark = c.status === "pass" ? "✓" : c.status === "skip" ? "·" : "✗";
    log(`    ${mark} ${c.label}: ${c.detail}`);
  }
  rule();

  // 4. Durable receipt (DRC + enclosure fit).
  const receipt = await buildReceipt(
    { document_id: boardId, enclosure_document_id: enc.document_id },
    engine,
  );
  const rc = receipt.structuredContent;
  log(`\n[4] Receipt: board_hash ${rc.receipt.board_hash?.slice(0, 12)}… · ` +
    `enclosure ${rc.enclosure_fit.ok ? "FITS" : "DOES NOT FIT"}`);

  // 5. Fab outputs — Gerbers for the board, STL/GLB + .vcad for the case.
  const gerb = parse(await exportGerber({ document_id: boardId, output_dir: OUT }));
  const stl = parse(exportCad({ ir: caseDoc, filename: "f405-case.stl" }, engine));
  const glb = parse(exportCad({ ir: caseDoc, filename: "f405-case.glb" }, engine));
  writeFileSync(join(OUT, "f405-case.vcad"), JSON.stringify(caseDoc, null, 2));
  writeFileSync(
    join(OUT, "verification.json"),
    JSON.stringify({ summary: fit.summary, ok: fit.ok, checks: fit.checks, cavity: fit.cavity }, null, 2),
  );

  log(`\n[5] Exports → examples/f405-enclosure/out/`);
  log(`    board:  ${gerb.files.length} Gerber/drill files (fab-ready)`);
  log(`    case:   f405-case.stl (${(stl.bytes / 1024).toFixed(0)} KB, 3D-print ready), ` +
    `f405-case.glb (${(glb.bytes / 1024).toFixed(0)} KB), f405-case.vcad (editable source)`);
  log(`    proof:  verification.json`);
  rule();
  log(fit.ok ? "  ✓ Board and case co-verified." : "  ✗ Fit check failed — see above.");
  rule();

  process.exit(fit.ok ? 0 : 1);
}

const round = (n) => Math.round(n * 10) / 10;

main().catch((e) => {
  console.error("FAILED:", e);
  process.exit(1);
});
