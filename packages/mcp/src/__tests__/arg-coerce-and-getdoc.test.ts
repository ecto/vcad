/**
 * Follow-up fixes after live testing PR #589 on the hosted server:
 *
 *  1. Boolean tool args arrived over the hosted transport as strings, so
 *     `args.allow_contact === true` silently read false and the flag had no
 *     effect. `asBool` coerces boolean-ish spellings; check_clearance now
 *     honors allow_contact regardless of how the client serialized it.
 *  2. get_document's IR was shadowed by the dispatch layer's
 *     {document_id, document_version} preview handle on clients that surface
 *     structuredContent. The IR now rides in structuredContent too.
 */
import { describe, it, expect, beforeAll } from "vitest";
import { Engine } from "@vcad/engine";
import type { Document } from "@vcad/ir";
import { asBool } from "../tools/arg-coerce.js";
import { checkClearance } from "../tools/clearance.js";
import { getDocumentTool, documents } from "../tools/session.js";

let engine: Engine;
beforeAll(async () => {
  engine = await Engine.init();
});

const ASSEMBLY_LOON = `
[let base [cube 40 40 5]]
[let post [cylinder 5 30]]
[assembly
  #[[part "base" base "aluminum"]
    [part "post" post "steel"]]
  #[[instance "base1" "base" 0 0 0]
    [instance "post1" "post" 20 20 5]]
  #[]
  "base1"]
`;

function assemblyDoc(): Document {
  const doc = engine.evalVcadSource(ASSEMBLY_LOON);
  if (!doc) throw new Error("loon eval failed");
  return doc;
}

describe("asBool", () => {
  it("accepts real booleans and boolean-ish strings/numbers", () => {
    expect(asBool(true)).toBe(true);
    expect(asBool(false)).toBe(false);
    expect(asBool("true")).toBe(true);
    expect(asBool("TRUE")).toBe(true);
    expect(asBool(" true ")).toBe(true);
    expect(asBool("false")).toBe(false);
    expect(asBool("1")).toBe(true);
    expect(asBool("0")).toBe(false);
    expect(asBool(1)).toBe(true);
    expect(asBool(0)).toBe(false);
  });
  it("falls back to the default for anything else", () => {
    expect(asBool(undefined)).toBe(false);
    expect(asBool(undefined, true)).toBe(true);
    expect(asBool("yes")).toBe(false);
    expect(asBool(null)).toBe(false);
  });
});

describe("check_clearance honors allow_contact regardless of serialization", () => {
  it("string 'true' allow_contact passes a touching pair", async () => {
    documents.clear();
    const id = "clr_bool";
    documents.set(id, assemblyDoc());
    const res = (await checkClearance(
      {
        document_id: id,
        group_a: ["base1"],
        group_b: ["post1"],
        min_mm: 0.5,
        allow_contact: "true", // string, as some clients serialize booleans
      },
      engine,
    )) as { structuredContent?: { clearance?: Record<string, unknown> } };
    const clr = res.structuredContent!.clearance!;
    expect(clr.verdict).toBe("touching");
    expect(clr.pass).toBe(true);
    expect(clr.allow_contact).toBe(true);
  });

  it("without allow_contact the same touching pair fails", async () => {
    documents.clear();
    const id = "clr_nobool";
    documents.set(id, assemblyDoc());
    const res = (await checkClearance(
      { document_id: id, group_a: ["base1"], group_b: ["post1"], min_mm: 0.5 },
      engine,
    )) as { structuredContent?: { clearance?: Record<string, unknown> } };
    const clr = res.structuredContent!.clearance!;
    expect(clr.verdict).toBe("touching");
    expect(clr.pass).toBe(false);
  });
});

describe("get_document returns the IR in structuredContent", () => {
  it("carries the document body so structuredContent-only clients see it", () => {
    documents.clear();
    const id = "getdoc_ir";
    const doc = assemblyDoc();
    documents.set(id, doc);
    const res = getDocumentTool({ document_id: id }) as {
      content: Array<{ type: string; text: string }>;
      structuredContent?: { document?: Document; document_id?: string };
    };
    // Text body still carries the full IR (unchanged contract).
    const fromText = JSON.parse(res.content[0]!.text) as Document;
    expect(fromText.instances?.length).toBe(2);
    // And structuredContent now carries the same document.
    expect(res.structuredContent?.document_id).toBe(id);
    expect(res.structuredContent?.document?.instances?.length).toBe(2);
  });
});
