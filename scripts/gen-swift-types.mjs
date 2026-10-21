#!/usr/bin/env node
/**
 * Generate typed Swift `Codable` types for the native Mac app from the Rust
 * engine types (DocCommand, FeatureInput, EngineEnvelope, …).
 *
 * Rust stays the single source of truth. Rather than add a second
 * reflection layer (schemars + JSON Schema), this reads the ts-rs output the
 * TypeScript side already gets (`<crate>/bindings/bindings/*.ts`, produced by
 * the `export_bindings` tests — see `npm run ir:gen`). ts-rs emits a small,
 * regular subset of TypeScript (literal unions, object literals, tagged
 * unions, `Array<T>`, `Record<K, V>`, tuples), which maps 1:1 onto Swift
 * enums and structs. Anything outside that subset is a hard error, so a new
 * Rust shape fails loudly here instead of generating something wrong.
 *
 * Output: apple/VcadApp/Sources/VcadApp/Generated/EngineTypes.swift.
 * Deterministic (types sorted by name), so `swift:check` can assert the
 * committed file is byte-identical to a fresh generation.
 *
 * Usage:
 *   node scripts/gen-swift-types.mjs            # write
 *   node scripts/gen-swift-types.mjs --check    # fail on drift
 */
import { readFileSync, writeFileSync, existsSync, readdirSync, mkdirSync } from "node:fs";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const outPath = join(root, "apple", "VcadApp", "Sources", "VcadApp", "Generated", "EngineTypes.swift");
const check = process.argv.includes("--check");

// Which ts-rs bindings to translate: every vcad-app type, plus the vcad-ir
// types the engine vocabulary references by name.
const SOURCES = [
  { crate: "vcad-app", types: null },
  { crate: "vcad-ir", types: ["MaterialDef"] },
];
// Referenced but deliberately untyped in Swift: the IR document is huge and
// the editor reads it as a JSON tree.
const OPAQUE = new Set(["Document"]);
// ts-rs renders every Rust integer as `number`; these fields are integers.
const isIntField = (name) => ["segments", "count", "index"].includes(name) || /NodeIds?$/.test(name);

// ---------------------------------------------------------------- parse

function tokenize(src) {
  const toks = [];
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    if (/\s/.test(c)) { i++; continue; }
    if (src.startsWith("//", i)) { i = src.indexOf("\n", i); if (i < 0) break; continue; }
    if (src.startsWith("/**", i)) {
      const end = src.indexOf("*/", i);
      const body = src.slice(i + 3, end).split("\n")
        .map((l) => l.replace(/^\s*\*\s?/, "").trimEnd())
        .join("\n").trim();
      toks.push({ t: "doc", v: body });
      i = end + 2; continue;
    }
    if (c === '"') {
      const end = src.indexOf('"', i + 1);
      toks.push({ t: "str", v: src.slice(i + 1, end) });
      i = end + 1; continue;
    }
    const id = /^[A-Za-z_$][\w$]*/.exec(src.slice(i));
    if (id) { toks.push({ t: "id", v: id[0] }); i += id[0].length; continue; }
    toks.push({ t: "p", v: c }); i++;
  }
  return toks;
}

function parseFile(src, file) {
  const toks = tokenize(src);
  let p = 0;
  const peek = () => toks[p];
  const next = () => toks[p++];
  const fail = (msg) => { throw new Error(`[swift:gen] ${file}: ${msg} at token ${JSON.stringify(peek())}`); };
  const eat = (v) => { const t = next(); if (!t || t.v !== v) { p--; fail(`expected '${v}'`); } return t; };
  const isP = (v) => peek() && peek().t === "p" && peek().v === v;

  function typeExpr() {
    const items = [inter()];
    while (isP("|")) { next(); items.push(inter()); }
    return items.length === 1 ? items[0] : { k: "union", items };
  }
  function inter() {
    const items = [primary()];
    while (isP("&")) { next(); items.push(primary()); }
    return items.length === 1 ? items[0] : { k: "inter", items };
  }
  function primary() {
    let node;
    const t = peek();
    if (t.t === "str") { next(); node = { k: "lit", v: t.v }; }
    else if (isP("(")) { next(); node = typeExpr(); eat(")"); }
    else if (isP("[")) {
      next(); const items = [];
      while (!isP("]")) { items.push(typeExpr()); if (isP(",")) next(); }
      eat("]"); node = { k: "tuple", items };
    } else if (isP("{")) {
      next(); const fields = [];
      while (!isP("}")) {
        let doc = null;
        if (peek().t === "doc") doc = next().v;
        const name = next();
        if (name.t !== "id" && name.t !== "str") fail("expected field name");
        let opt = false;
        if (isP("?")) { next(); opt = true; }
        eat(":");
        fields.push({ name: name.v, opt, type: typeExpr(), doc });
        if (isP(",")) next();
      }
      eat("}"); node = { k: "obj", fields };
    } else if (t.t === "id") {
      next(); const args = [];
      if (isP("<")) {
        next();
        while (!isP(">")) { args.push(typeExpr()); if (isP(",")) next(); }
        eat(">");
      }
      node = { k: "ref", name: t.v, args };
    } else fail("unexpected token");
    while (isP("[") && toks[p + 1]?.v === "]") { p += 2; node = { k: "ref", name: "Array", args: [node] }; }
    return node;
  }

  const decls = [];
  while (p < toks.length) {
    let doc = null;
    if (peek().t === "doc") doc = next().v;
    if (peek()?.v === "import") { while (next().v !== ";"); continue; }
    if (p >= toks.length) break;
    eat("export"); eat("type");
    const name = next().v;
    eat("=");
    decls.push({ name, doc, type: typeExpr() });
    if (isP(";")) next();
  }
  return decls;
}

// ---------------------------------------------------------------- emit

const KEYWORDS = new Set(["default", "case", "class", "enum", "extension", "func", "import", "in", "init",
  "internal", "let", "operator", "private", "protocol", "public", "return", "self", "static", "struct",
  "subscript", "super", "switch", "true", "false", "nil", "var", "where", "while", "do", "if", "else",
  "for", "repeat", "throw", "throws", "try", "is", "as", "Type", "Self", "any", "some"]);
const camel = (s) => {
  const parts = s.split(/[_\-\s]+/).filter(Boolean);
  const out = parts.map((w, i) => (i === 0 ? w[0].toLowerCase() + w.slice(1) : w[0].toUpperCase() + w.slice(1))).join("");
  // `F64` → `f64`, `FeatureRefList` → `featureRefList`.
  return out;
};
const ident = (s) => (KEYWORDS.has(s) ? `\`${s}\`` : s);
const plain = (s) => s.replace(/`/g, "");

let decls;
let names;

function swiftType(t, field) {
  switch (t.k) {
    case "ref":
      switch (t.name) {
        case "string": return "String";
        case "number": return field && isIntField(field) ? "Int" : "Double";
        case "boolean": return "Bool";
        case "unknown": return "EngineJSON";
        case "Array": return `[${swiftType(t.args[0], field)}]`;
        case "Record": return `[String: ${swiftType(t.args[1])}]`;
        default:
          if (OPAQUE.has(t.name)) return "EngineJSON";
          if (!names.has(t.name)) throw new Error(`[swift:gen] unknown type '${t.name}' — add its crate/type to SOURCES or OPAQUE`);
          return t.name;
      }
    case "tuple": {
      const inner = new Set(t.items.map((i) => swiftType(i, field)));
      if (inner.size !== 1) throw new Error("[swift:gen] heterogeneous tuples are not supported");
      return `[${[...inner][0]}]`;
    }
    case "union": {
      const nonNull = t.items.filter((i) => !(i.k === "ref" && i.name === "null"));
      if (nonNull.length === 1) return `${swiftType(nonNull[0], field)}?`;
      break;
    }
  }
  throw new Error(`[swift:gen] unsupported inline type ${JSON.stringify(t)}`);
}

const docLines = (doc, indent = "") =>
  doc ? doc.split("\n").map((l) => `${indent}///${l ? " " + l : ""}`).join("\n") + "\n" : "";

/** The discriminator key shared by every member of a union, if any. */
function tagKey(union) {
  if (!union.items.every((i) => i.k === "obj")) return null;
  const first = union.items[0].fields.filter((f) => f.type.k === "lit").map((f) => f.name);
  return first.find((key) => union.items.every((o) => o.fields.some((f) => f.name === key && f.type.k === "lit"))) ?? null;
}

function emitStringEnum(d) {
  let s = docLines(d.doc);
  s += `public enum ${d.name}: String, Codable, Equatable, Sendable {\n`;
  for (const i of d.type.items) s += `    case ${ident(camel(i.v))} = "${i.v}"\n`;
  return s + "}\n";
}

function fieldsDoc(fields, indent) {
  const documented = fields.filter((f) => f.doc);
  if (!documented.length) return "";
  return documented.map((f) => `${indent}/// - ${camel(f.name)}: ${f.doc.split("\n").join(" ")}`).join("\n") + "\n";
}

function emitTaggedEnum(d, key) {
  const cases = d.type.items.map((o) => {
    const tag = o.fields.find((f) => f.name === key).type.v;
    const fields = o.fields.filter((f) => f.name !== key);
    return { tag, name: ident(camel(tag)), fields };
  });
  const recursive = JSON.stringify(d.type).includes(`"name":"${d.name}","args":[]`);
  let s = docLines(d.doc);
  s += `public ${recursive ? "indirect " : ""}enum ${d.name}: Codable, Equatable, Sendable {\n`;
  for (const c of cases) {
    s += fieldsDoc(c.fields, "    ");
    const assoc = c.fields.map((f) => `${ident(camel(f.name))}: ${swiftType(f.type, f.name)}${f.opt ? "? = nil" : ""}`);
    s += `    case ${c.name}${assoc.length ? `(${assoc.join(", ")})` : ""}\n`;
  }
  // The discriminator, and every field all cases share (same name and type).
  s += `\n    /// The \`${key}\` discriminator.\n`;
  s += `    public var ${ident(camel(key))}: String {\n        switch self {\n`;
  for (const c of cases) s += `        case .${plain(c.name)}: return "${c.tag}"\n`;
  s += `        }\n    }\n`;
  const sig = (f) => `${f.name}:${swiftType(f.type, f.name)}:${f.opt}`;
  const shared = cases[0].fields.filter((f) => !f.opt && camel(f.name) !== camel(key) &&
    cases.every((c) => c.fields.some((g) => sig(g) === sig(f))));
  for (const f of shared) {
    s += `\n    public var ${ident(camel(f.name))}: ${swiftType(f.type, f.name)} {\n        switch self {\n`;
    for (const c of cases) {
      const pats = c.fields.map((g) => (g.name === f.name ? "let v" : "_"));
      s += `        case .${plain(c.name)}(${pats.join(", ")}): return v\n`;
    }
    s += `        }\n    }\n`;
  }
  s += `\n    public init(from decoder: Decoder) throws {\n`;
  s += `        let c = try decoder.container(keyedBy: GenKey.self)\n`;
  s += `        switch try c.decode(String.self, forKey: "${key}") {\n`;
  for (const c of cases) {
    s += `        case "${c.tag}":\n`;
    if (!c.fields.length) { s += `            self = .${plain(c.name)}\n`; continue; }
    const args = c.fields.map((f) => {
      const ty = swiftType(f.type, f.name);
      return `${ident(camel(f.name))}: try c.${f.opt ? "decodeIfPresent" : "decode"}(${ty}.self, forKey: "${f.name}")`;
    });
    s += `            self = .${plain(c.name)}(\n                ${args.join(",\n                ")}\n            )\n`;
  }
  s += `        case let other:\n`;
  s += `            throw DecodingError.dataCorruptedError(forKey: "${key}", in: c, debugDescription: "unknown ${d.name} \\(other)")\n`;
  s += `        }\n    }\n`;
  s += `\n    public func encode(to encoder: Encoder) throws {\n`;
  s += `        var c = encoder.container(keyedBy: GenKey.self)\n`;
  s += `        switch self {\n`;
  for (const c of cases) {
    const binds = c.fields.map((f) => `let ${ident(camel(f.name))}`);
    s += `        case .${plain(c.name)}${binds.length ? `(${binds.join(", ")})` : ""}:\n`;
    s += `            try c.encode("${c.tag}", forKey: "${key}")\n`;
    for (const f of c.fields) {
      s += `            try c.${f.opt ? "encodeIfPresent" : "encode"}(${ident(camel(f.name))}, forKey: "${f.name}")\n`;
    }
  }
  s += `        }\n    }\n}\n`;
  return s;
}

function emitExternalEnum(d) {
  let s = docLines(d.doc);
  s += `public enum ${d.name}: Codable, Equatable, Sendable {\n`;
  const cases = d.type.items.map((o) => ({ key: o.fields[0].name, type: swiftType(o.fields[0].type, o.fields[0].name) }));
  for (const c of cases) s += `    case ${ident(camel(c.key))}(${c.type})\n`;
  s += `\n    public init(from decoder: Decoder) throws {\n`;
  s += `        let c = try decoder.container(keyedBy: GenKey.self)\n`;
  s += `        guard let key = c.allKeys.first, c.allKeys.count == 1 else {\n`;
  s += `            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "${d.name} wants exactly one key"))\n`;
  s += `        }\n        switch key.stringValue {\n`;
  for (const c of cases) s += `        case "${c.key}": self = .${plain(ident(camel(c.key)))}(try c.decode(${c.type}.self, forKey: key))\n`;
  s += `        default:\n`;
  s += `            throw DecodingError.dataCorruptedError(forKey: key, in: c, debugDescription: "unknown ${d.name} variant")\n`;
  s += `        }\n    }\n`;
  s += `\n    public func encode(to encoder: Encoder) throws {\n`;
  s += `        var c = encoder.container(keyedBy: GenKey.self)\n`;
  s += `        switch self {\n`;
  for (const c of cases) s += `        case .${plain(ident(camel(c.key)))}(let v): try c.encode(v, forKey: "${c.key}")\n`;
  s += `        }\n    }\n}\n`;
  return s;
}

function emitStruct(d, obj, flattened = null) {
  let s = docLines(d.doc);
  s += `public struct ${d.name}: Codable, Equatable, Sendable {\n`;
  for (const f of obj.fields) {
    s += docLines(f.doc, "    ");
    s += `    public var ${ident(camel(f.name))}: ${swiftType(f.type, f.name)}${f.opt ? "?" : ""}\n`;
  }
  if (flattened) {
    s += `    /// Flattened into the same JSON object (\`#[serde(flatten)]\`).\n`;
    s += `    public var ${flattened.prop}: ${flattened.type}\n`;
  }
  // Memberwise init (public types need an explicit one).
  const params = obj.fields.map((f) => `${ident(camel(f.name))}: ${swiftType(f.type, f.name)}${f.opt ? "? = nil" : ""}`);
  if (flattened) params.push(`${flattened.prop}: ${flattened.type}`);
  s += `\n    public init(${params.join(", ")}) {\n`;
  for (const f of obj.fields) s += `        self.${plain(camel(f.name))} = ${ident(camel(f.name))}\n`;
  if (flattened) s += `        self.${flattened.prop} = ${flattened.prop}\n`;
  s += `    }\n`;
  s += `\n    public init(from decoder: Decoder) throws {\n`;
  s += `        let c = try decoder.container(keyedBy: GenKey.self)\n`;
  for (const f of obj.fields) {
    s += `        ${plain(camel(f.name))} = try c.${f.opt ? "decodeIfPresent" : "decode"}(${swiftType(f.type, f.name)}.self, forKey: "${f.name}")\n`;
  }
  if (flattened) s += `        ${flattened.prop} = try ${flattened.type}(from: decoder)\n`;
  s += `    }\n`;
  s += `\n    public func encode(to encoder: Encoder) throws {\n`;
  if (flattened) s += `        try ${flattened.prop}.encode(to: encoder)\n`;
  s += `        var c = encoder.container(keyedBy: GenKey.self)\n`;
  for (const f of obj.fields) {
    s += `        try c.${f.opt ? "encodeIfPresent" : "encode"}(${plain(camel(f.name))}, forKey: "${f.name}")\n`;
  }
  s += `    }\n}\n`;
  return s;
}

function emit(d) {
  const t = d.type;
  if (t.k === "union" && t.items.every((i) => i.k === "lit")) return emitStringEnum(d);
  if (t.k === "union") {
    const key = tagKey(t);
    if (key) return emitTaggedEnum(d, key);
    if (t.items.every((i) => i.k === "obj" && i.fields.length === 1)) return emitExternalEnum(d);
  }
  if (t.k === "obj") return emitStruct(d, t);
  if (t.k === "inter" && t.items.length === 2 && t.items[0].k === "obj") {
    // `{ own fields } & (inlined named union)` — serde flatten. Point back at
    // the named type it inlined so Swift reuses it.
    const inlined = JSON.stringify(t.items[1]);
    const match = decls.find((o) => o !== d && JSON.stringify(o.type) === inlined);
    if (match) return emitStruct(d, t.items[0], { prop: camel(match.name), type: match.name });
  }
  throw new Error(`[swift:gen] ${d.name}: unsupported top-level shape (${t.k})`);
}

// ---------------------------------------------------------------- main

decls = [];
for (const { crate, types } of SOURCES) {
  const dir = join(root, "crates", crate, "bindings", "bindings");
  if (!existsSync(dir)) {
    console.error(`[swift:gen] bindings dir not found: ${dir}\nRun: npm run swift:gen (it exports the ts-rs bindings first)`);
    process.exit(1);
  }
  const files = types ? types.map((t) => `${t}.ts`) : readdirSync(dir).filter((f) => f.endsWith(".ts"));
  for (const f of files) {
    const path = join(dir, f);
    if (!existsSync(path)) { console.error(`[swift:gen] missing ${path}`); process.exit(1); }
    decls.push(...parseFile(readFileSync(path, "utf8"), `${crate}/${f}`));
  }
}
decls.sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0));
names = new Set(decls.map((d) => d.name));

let out = `// Generated by scripts/gen-swift-types.mjs from the Rust engine types (via
// their ts-rs bindings). Do not edit — change the Rust type and run
// \`npm run swift:gen\`. CI fails when this file drifts (\`npm run swift:check\`).

import Foundation

/// String-keyed \`CodingKey\` the generated coders share.
struct GenKey: CodingKey, ExpressibleByStringLiteral {
    let stringValue: String
    var intValue: Int? { nil }
    init(stringValue: String) { self.stringValue = stringValue }
    init?(intValue: Int) { nil }
    init(stringLiteral value: String) { stringValue = value }
}

/// Untyped JSON, for values Rust keeps as \`serde_json::Value\` (the IR
/// document, tool arguments).
public enum EngineJSON: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([EngineJSON])
    case object([String: EngineJSON])

    public init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([EngineJSON].self) { self = .array(v) }
        else { self = .object(try c.decode([String: EngineJSON].self)) }
    }

    public func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        }
    }
}

`;
out += decls.map((d) => `// MARK: - ${d.name}\n\n${emit(d)}`).join("\n");

if (check) {
  const current = existsSync(outPath) ? readFileSync(outPath, "utf8") : "";
  if (current !== out) {
    console.error(`[swift:check] ${outPath} is stale — run \`npm run swift:gen\` and commit the result.`);
    process.exit(1);
  }
  console.log(`[swift:check] up to date (${decls.length} types)`);
} else {
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, out);
  console.log(`[swift:gen] wrote ${outPath} (${decls.length} types)`);
}
