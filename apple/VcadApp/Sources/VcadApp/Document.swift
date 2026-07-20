import Foundation
import simd

// Parses a `.vcad` parametric document (JSON: a node DAG + scene roots) into a
// navigable feature tree — the native counterpart of the web app's FeatureTree.
// The kernel evaluates the same file into renderable meshes; this layer recovers
// the *operations* behind those meshes (types, parameters, parent/child
// hierarchy) so the sidebar shows real modeling history instead of an anonymous
// "Part 1, Part 2" list.
//
// Index contract: `evaluate_document` (crates/vcad-eval) builds one part per
// scene root with `visible != false`, in array order. So the i-th VISIBLE root
// maps to kernel scene part `i`. `visibleRoots` preserves exactly that mapping,
// which is what lets a tree row drive a viewport entity (visibility, selection).

/// One operation node in the DAG.
struct DocNode {
    let id: Int
    let name: String?
    let opType: String
    /// Full op dict, kept for on-demand parameter readout.
    let op: [String: Any]
}

/// A scene root. One visible root == one kernel scene part (in order).
struct DocRoot {
    let nodeId: Int
    let material: String?
    let visible: Bool
}

/// A named document-level parameter (`parameters` in the .vcad JSON). Literal
/// parameters scrub live in the inspector; formula parameters (derived from
/// other parameters) display read-only. Bindings fan a parameter out to node
/// fields inside the kernel's `evaluate_document`, so writing one value here
/// re-solves every bound field together.
struct DocParameter: Identifiable {
    let name: String
    /// Literal value, when `value` is a bare number. nil for formulas.
    let value: Double?
    /// Formula source, when `value` is an expression string. nil for literals.
    let formula: String?
    let unit: String?
    let min: Double?
    let max: Double?
    let description: String?

    var id: String { name }
    var isLiteral: Bool { value != nil }
}

/// A material definition embedded in the document (color + PBR scalars).
struct DocMaterial {
    let color: (Double, Double, Double)
    let metallic: Float
    let roughness: Float
    let transmission: Float
}

/// An assembly joint — just what playback and the transport UI need. The
/// kernel FK solver owns the full kinematic semantics; this is display data.
struct DocJoint: Identifiable {
    let id: String
    let name: String?
    /// Joint kind tag ("Revolute", "Slider", …).
    let kind: String
    /// Authored driven state (degrees for revolute, mm for slider).
    let state: Double
}

/// One keyframe of an animation track (port of `vcad_ir::animation::AnimKey`).
struct DocAnimKey {
    let t: Double
    let value: Double
    /// "linear" | "step" | "ease-in-out" (kebab-case, as serialized).
    let ease: String
}

/// A joint-target animation track.
struct DocJointTrack {
    let jointId: String
    let keys: [DocAnimKey]
}

/// The document's animation timeline, reduced to the joint tracks native
/// playback drives. Sampling mirrors `Timeline::sample_track` in vcad-ir
/// exactly (clamp at the ends, ease from the previous key), so the native
/// pose at time t matches the web evaluator and the kernel sequencer.
struct DocTimeline {
    let durationS: Double
    let fps: Double
    let jointTracks: [DocJointTrack]

    /// Joint id → value at time `t` (seconds).
    func jointValues(at t: Double) -> [String: Double] {
        var out: [String: Double] = [:]
        for track in jointTracks {
            if let v = Self.sample(track.keys, at: t) { out[track.jointId] = v }
        }
        return out
    }

    static func sample(_ keys: [DocAnimKey], at t: Double) -> Double? {
        guard let first = keys.first, let last = keys.last else { return nil }
        if t <= first.t { return first.value }
        if t >= last.t { return last.value }
        guard let idx = keys.firstIndex(where: { $0.t > t }), idx > 0 else { return last.value }
        let a = keys[idx - 1], b = keys[idx]
        let span = b.t - a.t
        var u = span <= 0 ? 1.0 : (t - a.t) / span
        switch b.ease {
        case "step": u = u >= 1.0 ? 1.0 : 0.0
        case "ease-in-out": u = u * u * (3.0 - 2.0 * u)
        default: break                                   // linear
        }
        return a.value + (b.value - a.value) * u
    }
}

/// A parsed parametric document — just enough structure to render the tree and
/// the inspector; geometry still comes from the kernel.
struct DocumentGraph {
    let nodes: [Int: DocNode]
    let roots: [DocRoot]
    let materials: [String: DocMaterial]
    /// Document-level named parameters, sorted by name for a stable inspector.
    let parameters: [DocParameter]
    /// Assembly joints (empty for part-only documents).
    let joints: [DocJoint]
    /// Animation timeline, when the document carries one with joint tracks.
    let timeline: DocTimeline?

    /// Visible roots in order — index-aligned with the kernel scene's parts.
    var visibleRoots: [DocRoot] { roots.filter { $0.visible } }

    static func load(path: String) -> DocumentGraph? {
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)) else { return nil }
        return parse(data)
    }

    static func parse(_ data: Data) -> DocumentGraph? {
        guard let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
        return parse(obj)
    }

    /// Build a graph from an already-parsed JSON object — the editable path,
    /// where the model keeps a mutable doc dict and re-parses after each edit.
    static func parse(_ obj: [String: Any]) -> DocumentGraph? {
        guard let nodesRaw = obj["nodes"] as? [String: Any] else { return nil }

        var nodes: [Int: DocNode] = [:]
        for (_, v) in nodesRaw {
            guard let n = v as? [String: Any],
                  let id = (n["id"] as? NSNumber)?.intValue,
                  let op = n["op"] as? [String: Any],
                  let opType = op["type"] as? String else { continue }
            nodes[id] = DocNode(id: id, name: n["name"] as? String, opType: opType, op: op)
        }

        var roots: [DocRoot] = []
        if let rootsRaw = obj["roots"] as? [[String: Any]] {
            for r in rootsRaw {
                guard let nodeId = (r["root"] as? NSNumber)?.intValue else { continue }
                roots.append(DocRoot(nodeId: nodeId,
                                     material: r["material"] as? String,
                                     visible: (r["visible"] as? Bool) ?? true))
            }
        }

        var materials: [String: DocMaterial] = [:]
        if let raw = obj["materials"] as? [String: Any] {
            for (k, v) in raw {
                guard let m = v as? [String: Any], let c = m["color"] as? [Any], c.count >= 3,
                      let r = (c[0] as? NSNumber)?.doubleValue,
                      let g = (c[1] as? NSNumber)?.doubleValue,
                      let b = (c[2] as? NSNumber)?.doubleValue else { continue }
                materials[k] = DocMaterial(
                    color: (r, g, b),
                    metallic: Float((m["metallic"] as? NSNumber)?.doubleValue ?? 0.4),
                    roughness: Float((m["roughness"] as? NSNumber)?.doubleValue ?? 0.5),
                    transmission: Float((m["transmission"] as? NSNumber)?.doubleValue ?? 0))
            }
        }

        var parameters: [DocParameter] = []
        if let raw = obj["parameters"] as? [String: Any] {
            for (name, v) in raw {
                guard let p = v as? [String: Any] else { continue }
                // `value` is serde-untagged: a bare number (literal) or a
                // string (formula). Anything else is a malformed entry — skip.
                let literal = (p["value"] as? NSNumber)?.doubleValue
                let formula = p["value"] as? String
                guard literal != nil || formula != nil else { continue }
                parameters.append(DocParameter(
                    name: name,
                    value: literal,
                    formula: formula,
                    unit: p["unit"] as? String,
                    min: (p["min"] as? NSNumber)?.doubleValue,
                    max: (p["max"] as? NSNumber)?.doubleValue,
                    description: p["description"] as? String))
            }
            parameters.sort { $0.name < $1.name }
        }

        // Assembly joints: accept both the canonical camelCase keys and the
        // snake_case of older sample documents (only id/name/kind/state are
        // needed here — the kernel re-parses the full doc for FK).
        var joints: [DocJoint] = []
        if let raw = obj["joints"] as? [[String: Any]] {
            for j in raw {
                guard let id = j["id"] as? String else { continue }
                let kind = (j["kind"] as? [String: Any])?["type"] as? String ?? "Revolute"
                joints.append(DocJoint(id: id,
                                       name: j["name"] as? String,
                                       kind: kind,
                                       state: (j["state"] as? NSNumber)?.doubleValue ?? 0))
            }
        }

        var timeline: DocTimeline?
        if let tl = obj["timeline"] as? [String: Any],
           let duration = (tl["durationS"] as? NSNumber)?.doubleValue, duration > 0 {
            var tracks: [DocJointTrack] = []
            for t in (tl["tracks"] as? [[String: Any]]) ?? [] {
                guard let target = t["target"] as? [String: Any],
                      (target["type"] as? String) == "Joint",
                      let jointId = target["jointId"] as? String else { continue }
                var keys: [DocAnimKey] = []
                for k in (t["keys"] as? [[String: Any]]) ?? [] {
                    guard let kt = (k["t"] as? NSNumber)?.doubleValue,
                          let v = (k["value"] as? NSNumber)?.doubleValue else { continue }
                    keys.append(DocAnimKey(t: kt, value: v,
                                           ease: k["ease"] as? String ?? "linear"))
                }
                if !keys.isEmpty { tracks.append(DocJointTrack(jointId: jointId, keys: keys)) }
            }
            if !tracks.isEmpty {
                timeline = DocTimeline(durationS: duration,
                                       fps: (tl["fps"] as? NSNumber)?.doubleValue ?? 24,
                                       jointTracks: tracks)
            }
        }

        // Assembly documents may have no scene roots — instances carry the
        // geometry there, so an empty `roots` is only fatal without them.
        let hasInstances = ((obj["instances"] as? [[String: Any]])?.isEmpty == false)
        guard !nodes.isEmpty, !roots.isEmpty || hasInstances else { return nil }
        return DocumentGraph(nodes: nodes, roots: roots, materials: materials,
                             parameters: parameters, joints: joints, timeline: timeline)
    }

    // MARK: feature tree

    /// Top-level rows: one per VISIBLE root (so `partIndex` aligns with the
    /// kernel scene), each expandable into its operation sub-tree.
    func featureRoots() -> [FeatureNode] {
        visibleRoots.enumerated().map { partIndex, root in
            buildNode(root.nodeId, partIndex: partIndex, depth: 0, visited: [])
        }
    }

    private func buildNode(_ id: Int, partIndex: Int?, depth: Int, visited: Set<Int>) -> FeatureNode {
        guard let node = nodes[id] else {
            return FeatureNode(id: "missing\(id)", nodeId: id, name: "Missing #\(id)",
                               opType: "Empty", symbol: "questionmark.circle", detail: nil,
                               partIndex: partIndex, children: [])
        }
        var seen = visited
        seen.insert(id)
        var kids: [FeatureNode] = []
        if depth < 32 {   // cycle + runaway-depth guard
            for ref in childRefs(of: node.op) where !seen.contains(ref) {
                kids.append(buildNode(ref, partIndex: nil, depth: depth + 1, visited: seen))
            }
        }
        return FeatureNode(
            id: "n\(id)",
            nodeId: id,
            name: node.name?.isEmpty == false ? node.name! : Self.label(node.opType),
            opType: node.opType,
            symbol: Self.symbol(node.opType),
            detail: Self.paramSummary(node),
            partIndex: partIndex,
            children: kids)
    }

    /// Child node ids an op references, in reading order (operands, then the
    /// profile/parent it builds on). Only true node-ref keys are pulled, so
    /// scalar fields like `count`/`radius` are never mistaken for children.
    private func childRefs(of op: [String: Any]) -> [Int] {
        var out: [Int] = []
        for key in ["left", "right", "child", "children", "base",
                    "sketch", "sketches", "profile", "profiles", "parent"] {
            if let n = (op[key] as? NSNumber)?.intValue {
                out.append(n)
            } else if let arr = op[key] as? [Any] {
                for e in arr { if let n = (e as? NSNumber)?.intValue { out.append(n) } }
            }
        }
        return out
    }

    // MARK: op presentation (matches the web FeatureTree vocabulary)

    static func label(_ t: String) -> String {
        switch t {
        case "Cube": return "Box"
        case "Sketch2D": return "Sketch"
        case "LinearPattern": return "Linear Pattern"
        case "CircularPattern": return "Circular Pattern"
        case "PcbBoard": return "PCB Board"
        case "StepImport", "MeshImport", "ImportedMesh": return "Imported"
        case "EmbroideryPattern": return "Embroidery"
        case "PartInstance": return "Instance"
        case let s where s.hasPrefix("SheetMetal"):
            return "Sheet Metal " + s.dropFirst("SheetMetal".count)
        default: return t
        }
    }

    static func symbol(_ t: String) -> String {
        switch t {
        case "Cube": return "cube"
        case "Cylinder": return "cylinder"
        case "Sphere": return "circle.circle"
        case "Cone": return "cone"
        case "Empty": return "circle.dashed"
        case "Union": return "plus.square.on.square"
        case "Difference": return "minus.square"
        case "Intersection": return "square.on.square.dashed"
        case "Translate": return "move.3d"
        case "Rotate": return "rotate.3d"
        case "Scale": return "scale.3d"
        case "Sketch", "Sketch2D": return "scribble.variable"
        case "Extrude": return "arrow.up.to.line"
        case "Revolve": return "arrow.trianglehead.2.clockwise.rotate.90"
        case "LinearPattern": return "rectangle.split.3x1"
        case "CircularPattern": return "circle.grid.cross"
        case "Shell": return "cube.transparent"
        case "Fillet": return "square.on.circle"
        case "Chamfer": return "triangle"
        case "Text": return "textformat"
        case "Sweep": return "scribble"
        case "Loft": return "square.stack.3d.up"
        case "PcbBoard": return "cpu"
        case "EmbroideryPattern": return "scribble.variable"
        case "PartInstance": return "shippingbox"
        case "StepImport", "MeshImport", "ImportedMesh": return "square.and.arrow.down"
        case let s where s.hasPrefix("SheetMetal"): return "angle"
        default: return "cube.transparent"
        }
    }

    /// A short, correct parameter readout for the common ops. nil when the op
    /// has no concise scalar summary worth showing inline.
    static func paramSummary(_ node: DocNode) -> String? {
        let op = node.op
        func num(_ k: String) -> Double? { (op[k] as? NSNumber)?.doubleValue }
        func intv(_ k: String) -> Int? { (op[k] as? NSNumber)?.intValue }
        func vec(_ k: String) -> (Double, Double, Double)? {
            guard let v = op[k] as? [String: Any],
                  let x = (v["x"] as? NSNumber)?.doubleValue,
                  let y = (v["y"] as? NSNumber)?.doubleValue,
                  let z = (v["z"] as? NSNumber)?.doubleValue else { return nil }
            return (x, y, z)
        }
        func g(_ d: Double) -> String {
            d == d.rounded() ? String(Int(d)) : String(format: "%.1f", d)
        }
        switch node.opType {
        case "Cube":
            if let s = vec("size") { return "\(g(s.0)) × \(g(s.1)) × \(g(s.2)) mm" }
        case "Cylinder":
            if let r = num("radius"), let h = num("height") { return "⌀\(g(r * 2)) × \(g(h)) mm" }
        case "Sphere":
            if let r = num("radius") { return "⌀\(g(r * 2)) mm" }
        case "Cone":
            let r = num("radius1") ?? num("radius") ?? 0
            return "⌀\(g(r * 2)) × \(g(num("height") ?? 0)) mm"
        case "Translate":
            if let o = vec("offset") { return "(\(g(o.0)), \(g(o.1)), \(g(o.2)))" }
        case "Rotate":
            if let a = vec("angles") { return "\(g(a.0))°, \(g(a.1))°, \(g(a.2))°" }
        case "Scale":
            if let f = vec("factor") { return "×(\(g(f.0)), \(g(f.1)), \(g(f.2)))" }
        case "Fillet":
            if let r = num("radius") { return "r\(g(r)) mm" }
        case "Chamfer":
            if let d = num("distance") { return "\(g(d)) mm" }
        case "Shell":
            if let t = num("thickness") { return "t\(g(t)) mm" }
        case "Extrude":
            if let d = vec("direction") {
                return "h\(g((d.0 * d.0 + d.1 * d.1 + d.2 * d.2).squareRoot())) mm"
            }
        case "Revolve":
            if let a = num("angle_deg") { return "\(g(a))°" }
        case "LinearPattern":
            if let c = intv("count") { return "×\(c)" }
        case "CircularPattern":
            if let c = intv("count") {
                if let a = num("angle_deg") { return "×\(c) over \(g(a))°" }
                return "×\(c)"
            }
        default:
            break
        }
        return nil
    }
}

/// Viewport instancing for a pattern root: the seed subtree is tessellated
/// once and rendered as N rigid copies (one shared MeshResource, N entities)
/// instead of N independently tessellated + unioned meshes.
struct PatternInstancing {
    /// The LinearPattern/CircularPattern node at the root.
    let patternNodeId: Int
    /// The pattern's child — the seed the kernel tessellates once.
    let seedNodeId: Int
    /// Kernel-space (Z-up, mm) rigid transforms, one per copy; first = identity.
    let transforms: [simd_float4x4]
}

// MARK: in-place JSON edits
// The kernel is a pure `JSON → meshes` evaluator, so editing a document is just
// mutating its JSON dict and re-evaluating — no editable-doc FFI needed. These
// operate on the model's live `documentJSON` and are deliberately total (a bad
// path is a silent no-op, never a crash) since the UI only offers valid edits.
// Node-map keys equal the node id stringified, which the .vcad format guarantees.

enum DocEdit {
    static func serialize(_ json: [String: Any]) -> Data? {
        try? JSONSerialization.data(withJSONObject: json, options: [.sortedKeys])
    }
    /// Human-readable variant for writing back to disk.
    static func serializePretty(_ json: [String: Any]) -> Data? {
        try? JSONSerialization.data(withJSONObject: json, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
    }

    private static func nextNodeId(_ json: [String: Any]) -> Int {
        let nodes = json["nodes"] as? [String: Any] ?? [:]
        let maxId = nodes.values
            .compactMap { ($0 as? [String: Any])?["id"] as? NSNumber }
            .map(\.intValue).max() ?? 0
        return maxId + 1
    }

    private static func anyMaterial(_ json: [String: Any]) -> String {
        if let m = (json["materials"] as? [String: Any])?.keys.sorted().first { return m }
        if let r = (json["roots"] as? [[String: Any]])?.first?["material"] as? String { return r }
        return "default"
    }

    /// Append a fresh primitive as a new scene root (the Create tools, on docs).
    static func addPrimitiveRoot(_ json: inout [String: Any], shape: BaseShape) {
        let id = nextNodeId(json)
        var nodes = json["nodes"] as? [String: Any] ?? [:]
        let op: [String: Any]
        let name: String
        switch shape {
        case .cube: op = ["type": "Cube", "size": ["x": 30.0, "y": 30.0, "z": 30.0]]; name = "Box"
        case .cylinder: op = ["type": "Cylinder", "radius": 15.0, "height": 30.0, "segments": 64]; name = "Cylinder"
        case .sphere: op = ["type": "Sphere", "radius": 15.0, "segments": 48]; name = "Sphere"
        }
        nodes[String(id)] = ["id": id, "name": name, "op": op]
        json["nodes"] = nodes
        var roots = json["roots"] as? [[String: Any]] ?? []
        roots.append(["root": id, "material": anyMaterial(json)])
        json["roots"] = roots
    }

    /// Author a Sketch2D (closed Line loop) + Extrude as a new part — the
    /// sketch→solid workflow. `verts` are 2D plane coords (mm); the basis vectors
    /// place the plane in 3D and `direction` is the extrude vector (length=depth).
    static func addExtrudedProfile(_ json: inout [String: Any],
                                   verts: [(Double, Double)],
                                   origin: (Double, Double, Double),
                                   xDir: (Double, Double, Double),
                                   yDir: (Double, Double, Double),
                                   direction: (Double, Double, Double)) {
        guard verts.count >= 3 else { return }
        func v3(_ t: (Double, Double, Double)) -> [String: Any] { ["x": t.0, "y": t.1, "z": t.2] }
        let sid = nextNodeId(json)
        let eid = sid + 1
        var segs: [[String: Any]] = []
        for i in 0..<verts.count {
            let a = verts[i], b = verts[(i + 1) % verts.count]
            segs.append(["type": "Line",
                         "start": ["x": a.0, "y": a.1],
                         "end": ["x": b.0, "y": b.1]])
        }
        var nodes = json["nodes"] as? [String: Any] ?? [:]
        nodes[String(sid)] = ["id": sid, "name": "Sketch",
                              "op": ["type": "Sketch2D", "origin": v3(origin),
                                     "x_dir": v3(xDir), "y_dir": v3(yDir), "segments": segs]]
        nodes[String(eid)] = ["id": eid, "name": "Extrude",
                              "op": ["type": "Extrude", "sketch": sid, "direction": v3(direction)]]
        json["nodes"] = nodes
        var roots = json["roots"] as? [[String: Any]] ?? []
        roots.append(["root": eid, "material": anyMaterial(json)])
        json["roots"] = roots
    }

    /// Combine two visible parts (by part index, `a` first = base/left) into one
    /// boolean root. The two source roots are removed and a single new root
    /// referencing both their nodes is appended. `op` is "Union"/"Difference"/
    /// "Intersection". Order matters for Difference (a − b).
    static func combineRoots(_ json: inout [String: Any], _ a: Int, _ b: Int, op: String) {
        guard a != b, var roots = json["roots"] as? [[String: Any]] else { return }
        var visToRaw: [Int] = []
        for (i, r) in roots.enumerated() where (r["visible"] as? Bool) ?? true { visToRaw.append(i) }
        guard a < visToRaw.count, b < visToRaw.count else { return }
        let ra = visToRaw[a], rb = visToRaw[b]
        guard let nodeA = (roots[ra]["root"] as? NSNumber)?.intValue,
              let nodeB = (roots[rb]["root"] as? NSNumber)?.intValue else { return }
        let material = (roots[ra]["material"] as? String) ?? anyMaterial(json)
        let id = nextNodeId(json)
        var nodes = json["nodes"] as? [String: Any] ?? [:]
        nodes[String(id)] = ["id": id, "name": op, "op": ["type": op, "left": nodeA, "right": nodeB]]
        json["nodes"] = nodes
        // Remove both source roots (higher raw index first), append the combined one.
        roots.remove(at: max(ra, rb))
        roots.remove(at: min(ra, rb))
        roots.append(["root": id, "material": material])
        json["roots"] = roots
    }

    /// Wrap the `partIndex`-th visible root's node in a Fillet/Chamfer (the
    /// Modify tools, on docs). Re-points the root at the new modifier node.
    static func wrapRootWithModifier(_ json: inout [String: Any], partIndex: Int, fillet: Bool) {
        guard var roots = json["roots"] as? [[String: Any]] else { return }
        var vis = 0
        for (i, r) in roots.enumerated() where (r["visible"] as? Bool) ?? true {
            if vis == partIndex {
                guard let childId = (r["root"] as? NSNumber)?.intValue else { return }
                let id = nextNodeId(json)
                var nodes = json["nodes"] as? [String: Any] ?? [:]
                let op: [String: Any] = fillet
                    ? ["type": "Fillet", "child": childId, "radius": 2.0]
                    : ["type": "Chamfer", "child": childId, "distance": 2.0]
                nodes[String(id)] = ["id": id, "name": fillet ? "Fillet" : "Chamfer", "op": op]
                json["nodes"] = nodes
                roots[i]["root"] = id
                json["roots"] = roots
                return
            }
            vis += 1
        }
    }

    /// Instancing plan for a visible root whose node is a Linear/CircularPattern.
    /// Because the pattern IS the root, nothing downstream consumes its result,
    /// so the copies are pure rigid transforms of the seed — safe to instance.
    /// Returns nil (→ fall back to the baked kernel mesh) when the op isn't a
    /// pattern, the parameters are degenerate, or a document binding targets
    /// this node (bindings rewrite fields inside the kernel at eval time, so
    /// the raw JSON values here could be stale).
    static func patternInstancing(_ json: [String: Any], rootNodeId: Int) -> PatternInstancing? {
        guard let nodes = json["nodes"] as? [String: Any],
              let node = nodes[String(rootNodeId)] as? [String: Any],
              let op = node["op"] as? [String: Any],
              let type = op["type"] as? String,
              type == "LinearPattern" || type == "CircularPattern",
              let seed = (op["child"] as? NSNumber)?.intValue,
              nodes[String(seed)] != nil,
              let count = (op["count"] as? NSNumber)?.intValue, count >= 2
        else { return nil }
        if let bindings = json["bindings"] as? [String: Any],
           bindings.keys.contains(where: { $0.hasPrefix("\(rootNodeId):") }) { return nil }
        func vec(_ k: String) -> SIMD3<Double>? {
            guard let v = op[k] as? [String: Any],
                  let x = (v["x"] as? NSNumber)?.doubleValue,
                  let y = (v["y"] as? NSNumber)?.doubleValue,
                  let z = (v["z"] as? NSNumber)?.doubleValue else { return nil }
            return SIMD3(x, y, z)
        }
        var transforms: [simd_float4x4] = []
        transforms.reserveCapacity(count)
        if type == "LinearPattern" {
            // Kernel: copies at i·spacing along the normalized direction.
            guard let d = vec("direction"),
                  let spacing = (op["spacing"] as? NSNumber)?.doubleValue else { return nil }
            let n = simd_length(d)
            guard n > 1e-12 else { return nil }
            let dir = d / n
            for i in 0..<count {
                var m = matrix_identity_float4x4
                let o = dir * (spacing * Double(i))
                m.columns.3 = SIMD4(Float(o.x), Float(o.y), Float(o.z), 1)
                transforms.append(m)
            }
        } else {
            // Kernel: step = angle_deg/count about the axis through axis_origin.
            guard let o = vec("axis_origin"), let a = vec("axis_dir"),
                  let angle = (op["angle_deg"] as? NSNumber)?.doubleValue else { return nil }
            let n = simd_length(a)
            guard n > 1e-12 else { return nil }
            let axis = SIMD3<Float>(a / n)
            let origin = SIMD3<Float>(o)
            let step = angle * .pi / 180 / Double(count)
            for i in 0..<count {
                let q = simd_quatf(angle: Float(step * Double(i)), axis: axis)
                var m = simd_float4x4(q)
                m.columns.3 = SIMD4(origin - q.act(origin), 1)
                transforms.append(m)
            }
        }
        return PatternInstancing(patternNodeId: rootNodeId, seedNodeId: seed,
                                 transforms: transforms)
    }

    /// Current op dict for a node (for populating the inspector's editors).
    static func op(_ json: [String: Any], nodeId: Int) -> [String: Any]? {
        (json["nodes"] as? [String: Any])?[String(nodeId)].flatMap { $0 as? [String: Any] }?["op"] as? [String: Any]
    }

    /// The `partIndex`-th visible root's translate offset, if its root node is a
    /// Translate (else nil — the part sits at its authored position).
    static func rootTranslateOffset(_ json: [String: Any], partIndex: Int) -> (Double, Double, Double)? {
        guard let childId = rootNodeId(json, partIndex: partIndex),
              let node = (json["nodes"] as? [String: Any])?[String(childId)] as? [String: Any],
              let op = node["op"] as? [String: Any], (op["type"] as? String) == "Translate",
              let o = op["offset"] as? [String: Any] else { return nil }
        return ((o["x"] as? NSNumber)?.doubleValue ?? 0,
                (o["y"] as? NSNumber)?.doubleValue ?? 0,
                (o["z"] as? NSNumber)?.doubleValue ?? 0)
    }

    /// Set the part's world translation — updating its root Translate in place, or
    /// wrapping the root in a fresh "Move" Translate the first time (so the gizmo
    /// never nests Translates).
    static func setRootTranslate(_ json: inout [String: Any], partIndex: Int,
                                 _ x: Double, _ y: Double, _ z: Double) {
        guard let childId = rootNodeId(json, partIndex: partIndex),
              var nodes = json["nodes"] as? [String: Any] else { return }
        let off: [String: Any] = ["x": x, "y": y, "z": z]
        if var node = nodes[String(childId)] as? [String: Any],
           var op = node["op"] as? [String: Any], (op["type"] as? String) == "Translate" {
            op["offset"] = off
            node["op"] = op
            nodes[String(childId)] = node
            json["nodes"] = nodes
        } else {
            let id = nextNodeId(json)
            nodes[String(id)] = ["id": id, "name": "Move",
                                 "op": ["type": "Translate", "child": childId, "offset": off]]
            json["nodes"] = nodes
            setRootNode(&json, partIndex: partIndex, nodeId: id)
        }
    }

    /// Wrap the part's root in a rotate-about-center sandwich
    /// (Translate(C) · Rotate(0) · Translate(−C)) so a ring drag spins it in
    /// place, not around the world origin. Returns the Rotate node id to drive.
    static func wrapRotate(_ json: inout [String: Any], partIndex: Int,
                           _ cx: Double, _ cy: Double, _ cz: Double) -> Int? {
        guard let childId = rootNodeId(json, partIndex: partIndex),
              var nodes = json["nodes"] as? [String: Any] else { return nil }
        let base = nodes.values.compactMap { ($0 as? [String: Any])?["id"] as? NSNumber }
            .map(\.intValue).max() ?? 0
        let inId = base + 1, rotId = base + 2, outId = base + 3
        nodes[String(inId)] = ["id": inId,
                               "op": ["type": "Translate", "child": childId,
                                      "offset": ["x": -cx, "y": -cy, "z": -cz]]]
        nodes[String(rotId)] = ["id": rotId, "name": "Rotate",
                                "op": ["type": "Rotate", "child": inId,
                                       "angles": ["x": 0.0, "y": 0.0, "z": 0.0]]]
        nodes[String(outId)] = ["id": outId,
                                "op": ["type": "Translate", "child": rotId,
                                       "offset": ["x": cx, "y": cy, "z": cz]]]
        json["nodes"] = nodes
        setRootNode(&json, partIndex: partIndex, nodeId: outId)
        return rotId
    }

    /// Set one component (0=x,1=y,2=z, degrees) of a Rotate node's angles.
    static func setRotateAngle(_ json: inout [String: Any], rotNodeId: Int, axisIndex: Int, degrees: Double) {
        guard var nodes = json["nodes"] as? [String: Any],
              var node = nodes[String(rotNodeId)] as? [String: Any],
              var op = node["op"] as? [String: Any],
              var ang = op["angles"] as? [String: Any] else { return }
        ang[["x", "y", "z"][axisIndex]] = degrees
        op["angles"] = ang
        node["op"] = op
        nodes[String(rotNodeId)] = node
        json["nodes"] = nodes
    }

    private static func rootNodeId(_ json: [String: Any], partIndex: Int) -> Int? {
        guard let roots = json["roots"] as? [[String: Any]] else { return nil }
        var vis = 0
        for r in roots where (r["visible"] as? Bool) ?? true {
            if vis == partIndex { return (r["root"] as? NSNumber)?.intValue }
            vis += 1
        }
        return nil
    }
    private static func setRootNode(_ json: inout [String: Any], partIndex: Int, nodeId: Int) {
        guard var roots = json["roots"] as? [[String: Any]] else { return }
        var vis = 0
        for (i, r) in roots.enumerated() where (r["visible"] as? Bool) ?? true {
            if vis == partIndex { roots[i]["root"] = nodeId; json["roots"] = roots; return }
            vis += 1
        }
    }

    /// Assign a material to the `partIndex`-th visible root, and ensure the
    /// document carries a definition for the key (from the preset table) so it
    /// renders + survives save/reload.
    static func setRootMaterial(_ json: inout [String: Any], partIndex: Int, key: String) {
        guard var roots = json["roots"] as? [[String: Any]] else { return }
        var vis = 0
        for (i, r) in roots.enumerated() where (r["visible"] as? Bool) ?? true {
            if vis == partIndex { roots[i]["material"] = key; json["roots"] = roots; break }
            vis += 1
        }
        var mats = json["materials"] as? [String: Any] ?? [:]
        if mats[key] == nil, let p = MaterialPreset.byKey(key) {
            mats[key] = ["name": key,
                         "color": [p.color.0, p.color.1, p.color.2],
                         "metallic": Double(p.metallic),
                         "roughness": Double(p.roughness),
                         "transmission": Double(p.transmission),
                         "density": 1000.0, "friction": 0.5]
            json["materials"] = mats
        }
    }

    static func setName(_ json: inout [String: Any], nodeId: Int, name: String) {
        guard var nodes = json["nodes"] as? [String: Any],
              var node = nodes[String(nodeId)] as? [String: Any] else { return }
        node["name"] = name
        nodes[String(nodeId)] = node
        json["nodes"] = nodes
    }

    static func setScalar(_ json: inout [String: Any], nodeId: Int, key: String, value: Double) {
        mutateOp(&json, nodeId: nodeId) { $0[key] = value }
    }

    /// Overwrite a document-level parameter's literal value. Formula parameters
    /// and unknown names are left untouched (the inspector only scrubs
    /// literals). Sidecar fields (unit/min/max/description) are preserved.
    static func setParameter(_ json: inout [String: Any], name: String, value: Double) {
        guard var params = json["parameters"] as? [String: Any],
              var p = params[name] as? [String: Any],
              p["value"] is NSNumber else { return }
        p["value"] = value
        params[name] = p
        json["parameters"] = params
    }

    static func setInt(_ json: inout [String: Any], nodeId: Int, key: String, value: Int) {
        mutateOp(&json, nodeId: nodeId) { $0[key] = value }
    }

    static func setVecComponent(_ json: inout [String: Any], nodeId: Int,
                                key: String, axis: String, value: Double) {
        mutateOp(&json, nodeId: nodeId) { op in
            var v = (op[key] as? [String: Any]) ?? [:]
            v[axis] = value
            op[key] = v
        }
    }

    /// Remove the `partIndex`-th VISIBLE root (mapping back into the raw roots
    /// array, which may contain hidden entries). The orphaned subtree is left in
    /// `nodes` — harmless, the evaluator only walks reachable roots.
    static func removeRoot(_ json: inout [String: Any], partIndex: Int) {
        guard var roots = json["roots"] as? [[String: Any]] else { return }
        var vis = 0
        for (i, r) in roots.enumerated() where (r["visible"] as? Bool) ?? true {
            if vis == partIndex { roots.remove(at: i); json["roots"] = roots; return }
            vis += 1
        }
    }

    private static func mutateOp(_ json: inout [String: Any], nodeId: Int,
                                 _ body: (inout [String: Any]) -> Void) {
        guard var nodes = json["nodes"] as? [String: Any],
              var node = nodes[String(nodeId)] as? [String: Any],
              var op = node["op"] as? [String: Any] else { return }
        body(&op)
        node["op"] = op
        nodes[String(nodeId)] = node
        json["nodes"] = nodes
    }
}

/// One row in the native feature tree — a node in the DAG, possibly with operand
/// children. Root rows carry a `partIndex` that links them to a viewport entity
/// (`part<i>`), driving visibility and selection sync.
struct FeatureNode: Identifiable {
    let id: String
    let nodeId: Int
    let name: String
    let opType: String
    let symbol: String
    let detail: String?
    let partIndex: Int?
    let children: [FeatureNode]
    var hasChildren: Bool { !children.isEmpty }
}
