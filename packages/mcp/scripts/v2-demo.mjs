#!/usr/bin/env node
/**
 * v2 demo — runs an agent-style session against the v2 surface and
 * writes the render PNG, drawing SVG, and exported STL to /tmp so
 * you can eyeball the output.
 *
 * Run from the monorepo root:
 *   node packages/mcp/scripts/v2-demo.mjs
 */

import { Engine } from "@vcad/engine";
import { buildTool } from "../dist/tools/build.js";
import { inspectV2 } from "../dist/tools/inspect-v2.js";
import { queryTool } from "../dist/tools/query.js";
import { render } from "../dist/tools/render.js";
import { drawing } from "../dist/tools/drawing.js";
import { exportV2 } from "../dist/tools/export-v2.js";
import { shareV2 } from "../dist/tools/share-v2.js";
import { writeFileSync } from "node:fs";

const engine = await Engine.init();

const env = (r) => JSON.parse(r.content.find((c) => c.type === "text").text);

console.log("\n=== v2 demo: plate with hole + fillet ===\n");

// 1. Build
const r1 = env(
  buildTool(
    {
      ops: [
        {
          op: "primitive",
          kind: "cube",
          size: { x: 50, y: 30, z: 5 },
          name: "plate",
          material: "aluminum",
        },
      ],
      materials: { aluminum: { kind: "named", name: "aluminum" } },
    },
    engine,
  ),
);
console.log(`build      → ${r1.doc}  vol=${r1.stats.volume_mm3} mm³  parts=${r1.stats.parts}`);

// 2. Hole
const r2 = env(
  buildTool(
    {
      doc: r1.doc,
      ops: [
        {
          op: "hole",
          target: "plate",
          at: { x: 25, y: 15, z: 5 },
          kind: "through",
          diameter: 6,
          depth: "through",
          name: "drilled",
        },
      ],
    },
    engine,
  ),
);
console.log(`hole       → ${r2.doc}  vol=${r2.stats.volume_mm3} mm³`);

// 3. Fillet
const r3 = env(
  buildTool(
    {
      doc: r2.doc,
      ops: [
        {
          op: "fillet",
          edges: [{ node: "drilled", edges_role: "all_top" }],
          radius: 0.5,
          name: "filleted",
        },
      ],
    },
    engine,
  ),
);
console.log(`fillet     → ${r3.doc}  vol=${r3.stats.volume_mm3} mm³`);

// 4. Inspect
const r4 = env(inspectV2({ doc: r3.doc }, engine));
console.log(
  `inspect    → bbox ${JSON.stringify(r4.result.aggregate.bbox.size)}  COM ${JSON.stringify(r4.result.aggregate.center_of_mass)}  manifold=${r4.result.validity.manifold}`,
);

// 5. Query tree
const r5 = env(queryTool({ doc: r3.doc, q: { kind: "tree" } }, engine));
const walkTree = (n, depth = 0) =>
  `${"  ".repeat(depth)}${n.op}${n.name ? ` "${n.name}"` : ""}\n${n.children.map((c) => walkTree(c, depth + 1)).join("")}`;
console.log("query.tree:");
for (const n of r5.result) process.stdout.write(walkTree(n, 1));

// 6. Render
const r6 = env(render({ doc: r3.doc, quality: "preview" }, engine));
const pngPath = "/tmp/vcad-v2-demo.png";
writeFileSync(pngPath, Buffer.from(r6.result.image.data_base64, "base64"));
console.log(`render     → ${r6.result.width}×${r6.result.height} PNG saved to ${pngPath}`);

// 7. Drawing
const r7 = env(drawing({ doc: r3.doc }, engine));
const svgPath = "/tmp/vcad-v2-demo.svg";
writeFileSync(svgPath, r7.result.svg);
console.log(`drawing    → ${r7.result.views.length}-view SVG saved to ${svgPath}`);

// 8. Export STL
const r8 = env(exportV2({ doc: r3.doc, format: "stl" }, engine));
const stlPath = "/tmp/vcad-v2-demo.stl";
writeFileSync(stlPath, Buffer.from(r8.result.resource.data_base64, "base64"));
console.log(`export STL → ${r8.result.bytes} bytes saved to ${stlPath}`);

// 9. Share
const r9 = env(shareV2({ doc: r3.doc, name: "demo plate" }, engine));
console.log(`share      → ${r9.result.url.slice(0, 100)}…`);

console.log("\n✓ v2 surface end-to-end works\n");
