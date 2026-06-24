import Foundation

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

/// A parsed parametric document — just enough structure to render the tree and
/// the inspector; geometry still comes from the kernel.
struct DocumentGraph {
    let nodes: [Int: DocNode]
    let roots: [DocRoot]

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

        guard !nodes.isEmpty, !roots.isEmpty else { return nil }
        return DocumentGraph(nodes: nodes, roots: roots)
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

    /// Current op dict for a node (for populating the inspector's editors).
    static func op(_ json: [String: Any], nodeId: Int) -> [String: Any]? {
        (json["nodes"] as? [String: Any])?[String(nodeId)].flatMap { $0 as? [String: Any] }?["op"] as? [String: Any]
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
