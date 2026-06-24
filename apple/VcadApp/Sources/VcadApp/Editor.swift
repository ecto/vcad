import SwiftUI
import RealityKit
import AppKit
import simd
import CVcadFFI

// Model layer. The SwiftUI shell is in Shell.swift; kernel/streaming in Kernel.swift.

private let kSamplesDir = "/Users/cam/Developer/vcad/.claude/worktrees/great-bohr-4c355d"

enum GeometrySource: Hashable, Identifiable {
    case sandbox
    case document(path: String, label: String)
    /// Geometry authored by the AI intent bar: a loon program the kernel
    /// compiles + evaluates exactly like a loaded `.vcad` file.
    case generated(loon: String, label: String)
    /// The cross-domain slice: a resident parametric doc where one `connector_x`
    /// couples the enclosure cutout (mechanical) + the board connector (electrical).
    case gripper

    var id: String {
        switch self {
        case .sandbox: return "sandbox"
        case .document(let path, _): return path
        case .generated: return "generated"
        case .gripper: return "gripper"
        }
    }
    var label: String {
        switch self {
        case .sandbox: return "Sandbox"
        case .document(_, let label): return label
        case .generated(_, let label): return label
        case .gripper: return "Gripper"
        }
    }
    var isSandbox: Bool { if case .sandbox = self { return true }; return false }
    var isGenerated: Bool { if case .generated = self { return true }; return false }
    var isGripper: Bool { if case .gripper = self { return true }; return false }
}

/// The sandbox primitive — chosen from the Create tab of the tool palette.
enum BaseShape: String, CaseIterable {
    case cube, cylinder, sphere
    var label: String { rawValue.capitalized }
    var symbol: String {
        switch self {
        case .cube: return "cube"
        case .cylinder: return "cylinder"
        case .sphere: return "circle.circle"
        }
    }
}

/// The sandbox modifier — chosen from the Modify tab.
enum Modifier: String, CaseIterable {
    case none, fillet, chamfer
    var label: String { self == .none ? "None" : rawValue.capitalized }
    var symbol: String {
        switch self {
        case .none: return "minus.circle"
        case .fillet: return "square.on.circle.fill"
        case .chamfer: return "triangle"
        }
    }
    var paramLabel: String { self == .chamfer ? "Distance" : "Radius" }
}

/// A tab in the tool palette (the native reinterpretation of the web app's
/// Borland tool picker — same model, native skin).
enum ToolTab: String, CaseIterable, Identifiable {
    case create, modify, combine
    var id: String { rawValue }
    var label: String { rawValue.capitalized }
    var symbol: String {
        switch self {
        case .create: return "plus.square.on.square"
        case .modify: return "wand.and.rays"
        case .combine: return "square.on.square.dashed"
        }
    }
}

/// A boolean combine operation (the Combine tab — acts on two selected parts).
enum BooleanOp: String, CaseIterable, Identifiable {
    case union, difference, intersection
    var id: String { rawValue }
    var label: String {
        switch self {
        case .union: return "Union"
        case .difference: return "Subtract"
        case .intersection: return "Intersect"
        }
    }
    /// The IR op tag emitted into the document.
    var opType: String {
        switch self {
        case .union: return "Union"
        case .difference: return "Difference"
        case .intersection: return "Intersection"
        }
    }
    var symbol: String {
        switch self {
        case .union: return "plus.circle"
        case .difference: return "minus.circle"
        case .intersection: return "circle.circle"
        }
    }
}

/// One tool button within a tab.
struct Tool: Identifiable {
    let id: String
    let label: String
    let symbol: String
    let isActive: Bool
    var enabled: Bool = true
    var hint: String = ""
    let action: () -> Void
}

/// A node in the feature tree (the parametric DAG, shown in the sidebar).
struct Feature: Identifiable, Hashable {
    enum Kind { case base, modifier, part }
    let id: String
    let name: String
    let symbol: String
    let kind: Kind
}

/// Stats captured when a generated program is validated — drives the intent
/// bar's "Built · N parts · W×H×D mm" confirmation.
struct GenStats {
    var parts: Int
    var size: SIMD3<Float>
    var triangles: Int
}

@MainActor
@Observable
final class EditorModel {
    // Orbit camera (radians / scene meters).
    var azimuth: Float = .pi / 5
    var elevation: Float = .pi / 7
    var distance: Float = 1.5

    var source: GeometrySource = .sandbox {
        didSet {
            geometryDirty = true
            loadDocumentTree()                    // parse the DAG for a .vcad
            if !availableTabs.contains(toolTab) { toolTab = .create }
            selectedFeatureID = defaultSelection()
            resetCamera()        // frame the new part cleanly
        }
    }

    // Sandbox model: primitive + modifier, driven by the tool palette.
    var baseShape: BaseShape = .cube { didSet { if source.isSandbox { geometryDirty = true } } }
    var modifier: Modifier = .fillet { didSet { if source.isSandbox { geometryDirty = true } } }
    var modifierValue: Double = 3.0 {
        didSet {
            if source.isSandbox { parameterDirty = true }
            if Int(modifierValue.rounded()) != Int(oldValue.rounded()) {
                NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .default)
            }
        }
    }

    var toolTab: ToolTab = .create

    // Selection binds tree -> inspector AND rolls history (selecting "base"
    // shows the modifier's input).
    var selectedFeatureID: String? = "modifier" {
        didSet {
            if source.isSandbox { geometryDirty = true }
            else if selectedFeatureID != oldValue { selectionDirty = true }
        }
    }

    // Live readouts.
    var triangleCount: Int = 0
    var partCount: Int = 1
    var solveMillis: Double = 0
    var sizeMM: SIMD3<Float> = .zero

    // Picking.
    var pickPoint: SIMD3<Float>?
    var pickInfo: String?
    var pickDirty = false
    var displayScale: Float = 0.02
    var displayCenter: SIMD3<Float> = .zero

    var geometryDirty = true
    var parameterDirty = false
    let streaming = StreamingMesh()
    let chime = Chime()
    var lastDrag: CGSize = .zero
    var pinchBaseline: Float = 1.5
    var draggingHandle = false
    var handleBaseline: Double = 0

    // MARK: document feature tree
    // The parsed DAG behind a loaded .vcad — drives the hierarchical sidebar,
    // per-part visibility, and selection ↔ viewport highlight. Geometry still
    // comes from the kernel; this is the *operations* view of the same file.

    var documentGraph: DocumentGraph?
    var featureNodes: [FeatureNode] = []
    var expandedFeatureIDs: Set<String> = []
    /// Part indices hidden via the eye toggle (empty = all shown).
    var hiddenParts: Set<Int> = []
    /// Isolate: when set, only this part shows (supersedes `hiddenParts`).
    var isolatedPart: Int?
    /// Viewport needs to re-apply part visibility / re-highlight the selection.
    var visibilityDirty = false
    var selectionDirty = false
    /// Parts ⌘-clicked for a multi-part action (booleans). Ordered: first = base.
    var multiSelectedParts: [Int] = []
    /// Per-part kernel-space AABBs, index-aligned with the rendered parts —
    /// the cheap basis for picking a part in the viewport (tap → select row).
    @ObservationIgnored var docPartBounds: [(min: SIMD3<Float>, max: SIMD3<Float>)] = []

    var usesDocumentTree: Bool { documentGraph != nil && !featureNodes.isEmpty }

    /// Parse the DAG when a `.vcad` document loads; clear it for every other
    /// source. Resets visibility + expansion so each document opens clean.
    private func loadDocumentTree() {
        hiddenParts.removeAll()
        isolatedPart = nil
        multiSelectedParts = []
        expandedFeatureIDs.removeAll()
        docPartBounds = []
        documentJSON = nil
        undoStack.removeAll(); redoStack.removeAll()
        renamingFeatureID = nil
        documentDirty = false
        if case let .document(path, _) = source,
           let data = try? Data(contentsOf: URL(fileURLWithPath: path)),
           let dict = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
           let g = DocumentGraph.parse(dict) {
            documentJSON = dict
            documentGraph = g
            featureNodes = g.featureRoots()
            // Auto-expand a single-root doc so its history reads at a glance.
            if featureNodes.count == 1, let only = featureNodes.first {
                expandedFeatureIDs.insert(only.id)
            }
        } else {
            documentGraph = nil
            featureNodes = []
        }
    }

    private func defaultSelection() -> String? {
        switch source {
        case .sandbox: return "modifier"
        case .document: return featureNodes.first?.id
        default: return "part0"
        }
    }

    func toggleExpanded(_ id: String) {
        if expandedFeatureIDs.contains(id) { expandedFeatureIDs.remove(id) }
        else { expandedFeatureIDs.insert(id) }
    }

    // MARK: per-part visibility

    func isPartVisible(_ i: Int) -> Bool {
        if let iso = isolatedPart { return i == iso }
        return !hiddenParts.contains(i)
    }
    var hasHiddenParts: Bool { isolatedPart != nil || !hiddenParts.isEmpty }

    func toggleVisibility(part i: Int) {
        isolatedPart = nil                       // an explicit toggle exits isolate
        if hiddenParts.contains(i) { hiddenParts.remove(i) } else { hiddenParts.insert(i) }
        visibilityDirty = true
    }
    func isolate(part i: Int) {
        isolatedPart = (isolatedPart == i) ? nil : i
        hiddenParts.removeAll()
        visibilityDirty = true
    }
    func showAllParts() {
        hiddenParts.removeAll()
        isolatedPart = nil
        visibilityDirty = true
    }

    // MARK: selection ↔ part mapping

    /// The viewport part index implied by the current selection — the owning
    /// root of the selected feature row, or nil (selecting deep operands still
    /// highlights the part they build).
    var selectedPartIndex: Int? {
        guard let sel = selectedFeatureID else { return nil }
        for node in featureNodes {
            if let pi = node.partIndex, Self.subtree(node, contains: sel) { return pi }
        }
        return nil
    }
    private static func subtree(_ node: FeatureNode, contains id: String) -> Bool {
        if node.id == id { return true }
        return node.children.contains { subtree($0, contains: id) }
    }

    /// Single-select a row (clears any multi-selection).
    func selectFeature(_ id: String) {
        if !multiSelectedParts.isEmpty { multiSelectedParts = [] }
        selectedFeatureID = id
    }
    /// ⌘-click: toggle a part in the multi-selection; the last click stays primary.
    func toggleMultiSelect(part pi: Int, featureID id: String) {
        if let idx = multiSelectedParts.firstIndex(of: pi) { multiSelectedParts.remove(at: idx) }
        else { multiSelectedParts.append(pi) }
        selectedFeatureID = id
        selectionDirty = true
    }
    /// The parts to highlight: the multi-selection if any, else the primary part.
    var highlightedParts: Set<Int> {
        multiSelectedParts.isEmpty ? Set([selectedPartIndex].compactMap { $0 }) : Set(multiSelectedParts)
    }

    var selectedFeatureNode: FeatureNode? { Self.find(featureNodes, id: selectedFeatureID) }
    private static func find(_ nodes: [FeatureNode], id: String?) -> FeatureNode? {
        guard let id else { return nil }
        for n in nodes {
            if n.id == id { return n }
            if let f = find(n.children, id: id) { return f }
        }
        return nil
    }

    /// Material key assigned to a rendered part (root order), if any.
    func materialName(forPart i: Int) -> String? {
        guard let g = documentGraph, i < g.visibleRoots.count else { return nil }
        return g.visibleRoots[i].material
    }

    func documentBaseColor(_ i: Int) -> NSColor { Self.partColors[i % Self.partColors.count] }
    static let brandPink = NSColor(srgbRed: 0.976, green: 0.149, blue: 0.447, alpha: 1.0)

    /// Ray-pick a document part by its AABB (kernel coords) — nearest entry
    /// wins. Coarse (box, not triangle) but robust and ample for selection.
    func pickDocumentPart(originKernel o: SIMD3<Float>, dirKernel d: SIMD3<Float>) -> Int? {
        var best: (i: Int, t: Float)?
        for (i, b) in docPartBounds.enumerated() where isPartVisible(i) {
            guard let t = Self.rayAABB(o: o, d: d, lo: b.min, hi: b.max) else { continue }
            if best == nil || t < best!.t { best = (i, t) }
        }
        return best?.i
    }

    /// Slab test → entry distance along the ray, or nil if it misses.
    private static func rayAABB(o: SIMD3<Float>, d: SIMD3<Float>,
                                lo: SIMD3<Float>, hi: SIMD3<Float>) -> Float? {
        var tmin: Float = -.greatestFiniteMagnitude
        var tmax: Float = .greatestFiniteMagnitude
        for a in 0..<3 {
            let di = d[a]
            if abs(di) < 1e-9 {
                if o[a] < lo[a] || o[a] > hi[a] { return nil }
            } else {
                let inv = 1 / di
                var t1 = (lo[a] - o[a]) * inv
                var t2 = (hi[a] - o[a]) * inv
                if t1 > t2 { swap(&t1, &t2) }
                tmin = max(tmin, t1)
                tmax = min(tmax, t2)
                if tmin > tmax { return nil }
            }
        }
        return tmax < 0 ? nil : max(tmin, 0)
    }

    // MARK: document editing
    // The kernel is a pure JSON→meshes evaluator, so editing = mutate the live
    // doc dict + re-evaluate. `documentJSON` is the source of truth; `featureNodes`
    // / geometry are re-derived after each edit. Undo/redo are JSON snapshots.

    @ObservationIgnored var documentJSON: [String: Any]?
    var undoStack: [Data] = []
    var redoStack: [Data] = []
    var documentDirty = false
    /// One scrub gesture = one undo entry; skip the materialize-pop on edits.
    @ObservationIgnored var suppressMaterializePop = false
    /// In-place re-eval of a parameter edit (mesh swap, no full rebuild / pop).
    var docParamDirty = false
    /// The feature row currently being renamed inline, if any.
    var renamingFeatureID: String?

    var canUndo: Bool { !undoStack.isEmpty }
    var canRedo: Bool { !redoStack.isEmpty }
    /// Export is available for anything with exportable geometry (not the gripper).
    var canExport: Bool { if case .gripper = source { return false }; return true }

    // MARK: authoring (Create/Modify tools on a loaded document)

    /// Add a fresh primitive as a new part in the loaded document.
    func addPrimitive(_ shape: BaseShape) {
        guard documentJSON != nil else { return }
        applyEdit(snapshot: true, reeval: .rebuild) { DocEdit.addPrimitiveRoot(&$0, shape: shape) }
        selectedFeatureID = featureNodes.last?.id        // focus the new part
    }

    /// Combine the two ⌘-selected parts with a boolean op (first = base).
    func combineSelected(_ op: BooleanOp) {
        guard documentJSON != nil, multiSelectedParts.count == 2 else { return }
        let a = multiSelectedParts[0], b = multiSelectedParts[1]
        applyEdit(snapshot: true, reeval: .rebuild) { DocEdit.combineRoots(&$0, a, b, op: op.opType) }
        multiSelectedParts = []
        hiddenParts.removeAll(); isolatedPart = nil       // indices collapsed
        selectedFeatureID = featureNodes.last?.id          // the new combined part
    }

    /// Tabs available in the tool palette — Combine only applies to documents.
    var availableTabs: [ToolTab] { usesDocumentTree ? [.create, .modify, .combine] : [.create, .modify] }

    /// Wrap the selected part with a fillet/chamfer over all its edges.
    func applyModifierToSelected(_ mod: Modifier) {
        guard documentJSON != nil, mod != .none, let pi = selectedPartIndex else { return }
        let selBefore = selectedPartIndex
        applyEdit(snapshot: true, reeval: .rebuild) {
            DocEdit.wrapRootWithModifier(&$0, partIndex: pi, fillet: mod == .fillet)
        }
        // Keep the same part selected (its root node id changed → select its row).
        if let pi = selBefore, pi < featureNodes.count { selectedFeatureID = featureNodes[pi].id }
    }

    // MARK: save

    func saveDocument() {
        guard case let .document(path, _) = source,
              let json = documentJSON, let data = DocEdit.serializePretty(json) else { return }
        if (try? data.write(to: URL(fileURLWithPath: path))) != nil { documentDirty = false }
    }

    func saveDocumentAs(_ url: URL) {
        guard let json = documentJSON, let data = DocEdit.serializePretty(json) else { return }
        guard (try? data.write(to: url)) != nil else { return }
        openDocument(url)        // re-open from the saved file (resets dirty/undo)
    }

    /// Discard edits and reload the document from disk.
    func revertDocument() {
        guard case .document = source else { return }
        loadDocumentTree()
        geometryDirty = true; selectionDirty = true; visibilityDirty = true
        selectedFeatureID = featureNodes.first?.id
    }

    private enum Reeval { case inPlace, rebuild, none }

    private func pushUndo() {
        guard let json = documentJSON, let data = DocEdit.serialize(json) else { return }
        undoStack.append(data)
        if undoStack.count > 64 { undoStack.removeFirst() }
        redoStack.removeAll()
    }

    private func applyEdit(snapshot: Bool, reeval: Reeval, _ mutate: (inout [String: Any]) -> Void) {
        guard var json = documentJSON else { return }
        if snapshot { pushUndo() }
        mutate(&json)
        documentJSON = json
        documentDirty = true
        if let g = DocumentGraph.parse(json) {
            documentGraph = g
            featureNodes = g.featureRoots()
        }
        switch reeval {
        case .inPlace:
            docParamDirty = true
        case .rebuild:
            suppressMaterializePop = true
            geometryDirty = true; selectionDirty = true; visibilityDirty = true
        case .none:
            break
        }
    }

    /// Current op dict for a node — feeds the inspector's parameter editors.
    func opDict(nodeId: Int) -> [String: Any]? {
        documentJSON.flatMap { DocEdit.op($0, nodeId: nodeId) }
    }

    func editScalar(nodeId: Int, key: String, value: Double, snapshot: Bool) {
        applyEdit(snapshot: snapshot, reeval: .inPlace) {
            DocEdit.setScalar(&$0, nodeId: nodeId, key: key, value: value)
        }
    }
    func editVec(nodeId: Int, key: String, axis: String, value: Double, snapshot: Bool) {
        applyEdit(snapshot: snapshot, reeval: .inPlace) {
            DocEdit.setVecComponent(&$0, nodeId: nodeId, key: key, axis: axis, value: value)
        }
    }
    func editInt(nodeId: Int, key: String, value: Int, snapshot: Bool) {
        applyEdit(snapshot: snapshot, reeval: .inPlace) {
            DocEdit.setInt(&$0, nodeId: nodeId, key: key, value: value)
        }
    }
    func renameFeature(_ nodeId: Int, to name: String) {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        applyEdit(snapshot: true, reeval: .none) {
            DocEdit.setName(&$0, nodeId: nodeId, name: trimmed)
        }
    }
    func deletePart(_ partIndex: Int) {
        applyEdit(snapshot: true, reeval: .rebuild) {
            DocEdit.removeRoot(&$0, partIndex: partIndex)
        }
        // Indices shifted; clear visibility/multi-select and re-anchor selection.
        hiddenParts.removeAll(); isolatedPart = nil; multiSelectedParts = []
        if selectedFeatureNode == nil { selectedFeatureID = featureNodes.first?.id }
    }

    func undo() {
        guard let data = undoStack.popLast() else { return }
        if let cur = documentJSON, let curData = DocEdit.serialize(cur) { redoStack.append(curData) }
        restore(data)
    }
    func redo() {
        guard let data = redoStack.popLast() else { return }
        if let cur = documentJSON, let curData = DocEdit.serialize(cur) { undoStack.append(curData) }
        restore(data)
    }
    private func restore(_ data: Data) {
        guard let dict = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return }
        documentJSON = dict
        if let g = DocumentGraph.parse(dict) { documentGraph = g; featureNodes = g.featureRoots() }
        hiddenParts.removeAll(); isolatedPart = nil
        suppressMaterializePop = true
        geometryDirty = true; selectionDirty = true; visibilityDirty = true
        if selectedFeatureNode == nil { selectedFeatureID = featureNodes.first?.id }
    }

    /// Re-evaluate the edited doc and return fresh part meshes for an in-place
    /// swap (no entity rebuild → smooth scrubbing). Keeps display scale/center
    /// fixed so the part doesn't jump mid-edit; updates the live readouts +
    /// pick bounds. Returns nil if the edit produced no geometry.
    func reevalDocumentMeshes() -> [MeshResource]? {
        guard let json = documentJSON, let data = DocEdit.serialize(json) else { return nil }
        let start = Date()
        let scene: OpaquePointer? = data.withUnsafeBytes {
            vcad_scene_from_json($0.bindMemory(to: UInt8.self).baseAddress, data.count)
        }
        guard let scene else { return nil }
        defer { vcad_scene_free(scene) }
        let count = vcad_scene_part_count(scene)
        var meshes: [MeshResource] = []
        var bounds: [(min: SIMD3<Float>, max: SIMD3<Float>)] = []
        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
        var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
        var tris = 0
        for i in 0..<count {
            let km = KernelMesh.fromView(vcad_scene_part_mesh(scene, i))
            if km.isEmpty { continue }
            lo = simd_min(lo, km.minBound); hi = simd_max(hi, km.maxBound)
            tris += km.triangleCount
            meshes.append(km.resource(name: "part\(i)"))
            bounds.append((km.minBound, km.maxBound))
        }
        guard !meshes.isEmpty else { return nil }
        docPartBounds = bounds
        solveMillis = Date().timeIntervalSince(start) * 1000
        triangleCount = tris
        partCount = meshes.count
        sizeMM = hi - lo
        return meshes
    }

    // MARK: export
    // No export FFI today, so STL is written here from the current part meshes
    // (binary STL = just triangles). Works for the sandbox + any loaded/edited doc.

    /// Kernel meshes for whatever is on screen right now (sandbox primitive or
    /// the evaluated document), for export.
    private func currentKernelMeshes() -> [KernelMesh] {
        switch source {
        case .sandbox:
            return sandboxKernelMesh().map { [$0] } ?? []
        case .document, .generated:
            let data: Data?
            if let json = documentJSON { data = DocEdit.serialize(json) }
            else if case let .document(path, _) = source { data = try? Data(contentsOf: URL(fileURLWithPath: path)) }
            else if case let .generated(loon, _) = source { data = Data(loon.utf8) }
            else { data = nil }
            guard let data else { return [] }
            let isLoon = { if case .generated = source { return true } else { return false } }()
            let scene: OpaquePointer? = data.withUnsafeBytes { raw in
                let base = raw.bindMemory(to: UInt8.self).baseAddress
                return isLoon ? vcad_scene_from_loon(base, data.count) : vcad_scene_from_json(base, data.count)
            }
            guard let scene else { return [] }
            defer { vcad_scene_free(scene) }
            var out: [KernelMesh] = []
            for i in 0..<vcad_scene_part_count(scene) {
                let km = KernelMesh.fromView(vcad_scene_part_mesh(scene, i))
                if !km.isEmpty { out.append(km) }
            }
            return out
        case .gripper:
            return []
        }
    }

    @discardableResult
    func exportSTL(to url: URL) -> Bool {
        let meshes = currentKernelMeshes()
        guard !meshes.isEmpty else { return false }
        var triCount = 0
        for m in meshes { triCount += m.triangleCount }

        var data = Data()
        data.append(Data(count: 80))                    // header
        var n = UInt32(triCount).littleEndian
        withUnsafeBytes(of: &n) { data.append(contentsOf: $0) }

        func put(_ f: Float) { var v = f.bitPattern.littleEndian; withUnsafeBytes(of: &v) { data.append(contentsOf: $0) } }
        for m in meshes {
            var i = 0
            while i + 2 < m.indices.count {
                let a = m.positions[Int(m.indices[i])]
                let b = m.positions[Int(m.indices[i + 1])]
                let c = m.positions[Int(m.indices[i + 2])]
                let nrm = normalize(cross(b - a, c - a))
                put(nrm.x.isFinite ? nrm.x : 0); put(nrm.y.isFinite ? nrm.y : 0); put(nrm.z.isFinite ? nrm.z : 1)
                put(a.x); put(a.y); put(a.z)
                put(b.x); put(b.y); put(b.z)
                put(c.x); put(c.y); put(c.z)
                data.append(contentsOf: [0, 0])         // attribute byte count
                i += 3
            }
        }
        return (try? data.write(to: url)) != nil
    }

    // Hover affordance for the draggable handles (cursor + a subtle scale pop).
    // The handles' live world positions are projected to screen to hit-test the
    // pointer; `hoveredHandle` drives the highlight + cursor.
    var hoveredHandle: String?
    @ObservationIgnored var connectorHandleWorld: SIMD3<Float> = .zero
    @ObservationIgnored var filletHandleWorld: SIMD3<Float> = .zero

    /// Frame the geometry at a clean 3/4 view. Parts auto-fit to a constant
    /// display size, so fixed camera params frame any part well.
    func resetCamera() {
        azimuth = .pi / 5
        elevation = .pi / 7
        distance = 1.5
        pinchBaseline = 1.5
        panOffset = .zero
    }

    /// Pan the look-at target in the camera's screen plane (⇧-drag).
    func panBy(dx: Float, dy: Float) {
        let forward = normalize(-orbitVector)
        let right = normalize(cross(forward, SIMD3<Float>(0, 1, 0)))
        let up = cross(right, forward)
        panOffset += (-dx * right + dy * up) * (distance * 0.0016)
    }

    // Two-finger / wheel scroll → zoom. Installed once when the viewport appears.
    nonisolated(unsafe) private var scrollMonitor: Any?
    func installScrollZoom() {
        guard scrollMonitor == nil else { return }
        scrollMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self] event in
            MainActor.assumeIsolated {
                guard let self, !self.draggingHandle else { return }
                let dy = Float(event.scrollingDeltaY)
                let k = Float(event.hasPreciseScrollingDeltas ? 0.004 : 0.04)
                self.distance = max(0.45, min(8.0, self.distance * (1 - dy * k)))
                self.pinchBaseline = self.distance
            }
            return event
        }
    }

    // MARK: cross-domain slice — the gripper
    // One `connector_x` couples the enclosure cutout (mechanical) + the board
    // connector (electrical); see docs/plans/2026-06-23-gripper-vertical-slice.md.

    nonisolated(unsafe) private var gripperDoc: OpaquePointer?
    var connectorX: Double = 40
    let connectorRange: ClosedRange<Double> = 4...76
    @ObservationIgnored private var connectorBaseline: Double = 40
    @ObservationIgnored private var lastConnectorTick = 40
    @ObservationIgnored private var lastConnectorOK = true

    deinit {
        if let d = gripperDoc { vcad_doc_free(d) }
        if let m = scrollMonitor { NSEvent.removeMonitor(m) }
    }

    /// Clearance from the connector cutout to the nearest enclosure side wall —
    /// the REAL geometric min-wall, measured by the kernel (`vcad_doc_min_wall`)
    /// from the resolved `box − cutout`, not arithmetic on the box's literals.
    /// Refreshed live on every cheap per-frame re-solve and on settle; seeded so
    /// the centered open-state reads correctly before the first drag.
    var connectorMinWall: Double = 34
    var connectorOK: Bool { connectorMinWall >= 6 }
    /// Pull the real min-wall from the kernel at the current `connector_x`.
    /// `vcad_doc_min_wall` resolves the resident doc (whose `connector_x` was
    /// just written by the cheap/full re-solve) and measures the box−cutout
    /// geometry — no fold, no routing, safe every drag frame.
    func refreshMinWall() {
        guard let doc = gripperDoc else { return }
        let w = vcad_doc_min_wall(doc)
        if w.isFinite { connectorMinWall = w }
    }
    var showsConnectorHandle: Bool { source.isGripper }
    func connectorHandlePosition() -> SIMD3<Float> { SIMD3(Float(connectorX), 9, 13) }

    func openGripper() {
        connectorX = 40
        lastConnectorTick = 40
        lastConnectorOK = true
        copperStale = false
        copperDirty = false
        copperUnrouted = 0
        // A consistent 3/4 view tilted toward the board top, so all four domains
        // (cutout, board + copper, bracket) read at once instead of edge-on.
        azimuth = .pi / 5
        elevation = 0.62
        distance = 1.45
        panOffset = .zero
        source = .gripper
        // Run the full solve once after the first rebuild so the Receipt's
        // settle-fields (quote, lead time, bracket DFM) start populated instead
        // of showing $0.00 until the first drag.
        copperDirty = true
    }

    func beginConnectorDrag() { connectorBaseline = connectorX }
    var connectorDragBaseline: Double { connectorBaseline }

    /// Drag handler: clamp, flag a re-solve, and fire the felt detents — a tick
    /// per millimetre, a firm "wall" the instant min-wall is violated.
    func setConnectorX(_ x: Double) {
        let clamped = min(max(x, connectorRange.lowerBound), connectorRange.upperBound)
        connectorX = clamped
        parameterDirty = true
        // The shown copper + the expensive Receipt rows now lag the connector —
        // mark them stale ("recomputing"); they re-solve on settle.
        copperStale = true
        receiptStale = true
        let tick = Int(clamped.rounded())
        if tick != lastConnectorTick {
            lastConnectorTick = tick
            NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .default)
        }
        let ok = connectorOK
        if ok != lastConnectorOK {
            lastConnectorOK = ok
            if !ok {
                NSHapticFeedbackManager.defaultPerformer.perform(.levelChange, performanceTime: .default)
                chime.play(.warning)
            } else {
                chime.play(.solved)
            }
        }
    }

    private func ensureGripperDoc() -> OpaquePointer? {
        if gripperDoc == nil { gripperDoc = vcad_doc_gripper_slice1() }
        return gripperDoc
    }

    /// Re-solve the coupled doc at the current `connector_x` and gather both
    /// parts. Re-evaluating sets `connector_x`; bindings move the cutout AND the
    /// connector together — one finger, two domains.
    func gripperScene() -> RenderScene {
        guard let doc = ensureGripperDoc() else { return .empty }
        let start = Date()
        let scene: OpaquePointer? = "connector_x".withCString {
            vcad_doc_set_param(doc, $0, connectorX)
        }
        guard let scene else { return .empty }
        defer { vcad_scene_free(scene) }
        return sceneFromHandle(scene, start: start)
    }

    /// Per-FRAME re-solve: cheap roots only. The mechanical cutout + board
    /// connector follow the finger live (~15 ms); the expensive sheet-metal fold
    /// is skipped here and re-folded once on settle (`gripperScene`). Same
    /// `connector_x` writes through to the resident doc, so the settle solve sees
    /// it — one DAG, staged by cost.
    func gripperSceneCheap() -> RenderScene {
        guard let doc = ensureGripperDoc() else { return .empty }
        let start = Date()
        let scene: OpaquePointer? = "connector_x".withCString {
            vcad_doc_set_param_cheap(doc, $0, connectorX)
        }
        guard let scene else { return .empty }
        defer { vcad_scene_free(scene) }
        // The cheap path wrote connector_x to the resident doc; pull the real
        // min-wall back so the live Receipt row tracks geometry, not arithmetic.
        refreshMinWall()
        return sceneFromHandle(scene, start: start)
    }

    /// The unified cross-domain solve — ONE FFI call returns every domain at the
    /// current `connector_x`: the meshes (enclosure + board + sheet-metal fold),
    /// the routed copper, and the receipt scalars, all descending from the same
    /// resolved parameter. The SETTLE path: the kernel fans connector_x out, not
    /// this view layer.
    struct GripperSolve {
        var meshes: [MeshResource]
        var copper: [CopperSeg]
        var minWall: Double
        var unrouted: Int
    }

    func gripperSolve() -> GripperSolve? {
        guard source.isGripper, let doc = ensureGripperDoc() else { return nil }
        let start = Date()
        let s: OpaquePointer? = "connector_x".withCString { vcad_doc_solve(doc, $0, connectorX) }
        guard let s else { return nil }
        defer { vcad_solve_free(s) }

        var meshes: [MeshResource] = []
        let pc = Int(vcad_solve_part_count(s))
        for i in 0..<pc {
            let km = KernelMesh.fromView(vcad_solve_part_mesh(s, i))
            meshes.append(km.isEmpty ? .generateBox(size: 0.001) : km.resource(name: "part\(i)"))
        }

        var copper: [CopperSeg] = []
        let ox: Float = 5, oy: Float = 5, z: Float = 7.1
        let tc = Int(vcad_solve_trace_count(s))
        for i in 0..<tc {
            let t = vcad_solve_trace(s, i)
            copper.append(CopperSeg(
                a: SIMD3<Float>(Float(t.start.0) + ox, Float(t.start.1) + oy, z),
                b: SIMD3<Float>(Float(t.end.0) + ox, Float(t.end.1) + oy, z),
                width: Float(t.width), net: t.net_id))
        }
        copperUnrouted = Int(vcad_solve_unrouted(s))
        // Settle: refresh the Receipt's expensive verdicts (these came from the
        // same one solve as the meshes + copper).
        bracketOK = vcad_solve_bracket_ok(s) == 1
        bracketSeverity = Int(vcad_solve_bracket_severity(s))
        quoteCents = vcad_solve_quote_cost_cents(s)
        quoteEnclosureCents = vcad_solve_quote_enclosure_cents(s)
        quoteBoardCents = vcad_solve_quote_board_cents(s)
        quoteBracketCents = vcad_solve_quote_bracket_cents(s)
        quoteHasEstimate = vcad_solve_quote_has_estimate(s) == 1
        leadDays = Int(vcad_solve_lead_days(s))
        // Authoritative min-wall from the full solve (same value the cheap path
        // approximates per-frame). Keeps the settled Receipt row exact.
        let solvedWall = vcad_solve_min_wall(s)
        if solvedWall.isFinite { connectorMinWall = solvedWall }
        let held = vcad_solve_all_held(s) == 1
        receiptStale = false
        // Chime on the gate transition (the felt "verified" / "violated" moment).
        if held != allHeld { chime.play(held ? .solved : .failed) }
        allHeld = held
        solveMillis = Date().timeIntervalSince(start) * 1000
        return GripperSolve(meshes: meshes, copper: copper,
                            minWall: vcad_solve_min_wall(s), unrouted: copperUnrouted)
    }

    // MARK: copper — slice 2, the electrical domain of the Connector Drag

    /// One routed copper segment, mapped into the board plate's frame.
    struct CopperSeg { var a: SIMD3<Float>; var b: SIMD3<Float>; var width: Float; var net: UInt32 }

    /// Copper is showing a pre-drag route — drawn dim until it re-routes on settle.
    var copperStale = false
    /// Set on drag-settle to request exactly ONE re-route (never per drag frame).
    var copperDirty = false
    /// Nets that failed to route at the current connector_x (0 = fully routed).
    /// Observable (NOT @ObservationIgnored) so the Receipt's copper row refreshes.
    var copperUnrouted = 0

    // Receipt verdicts — the expensive ones settle on drag-release; min-wall is
    // recomputed live from connectorX. `receiptStale` marks the expensive rows
    // "recomputing" mid-drag (mirrors copperStale).
    var receiptStale = false
    var bracketOK = true
    var bracketSeverity = 0
    /// Total quote (cents) = enclosure + board + bracket. Per-domain breakdown
    /// below so the Receipt can show three labeled line items with an "est."
    /// tag on the board (the one labeled estimate; the other two are kernel-real).
    var quoteCents: UInt64 = 0
    var quoteEnclosureCents: UInt64 = 0
    var quoteBoardCents: UInt64 = 0
    var quoteBracketCents: UInt64 = 0
    var quoteHasEstimate = false
    var leadDays = 0
    var allHeld = true

    /// Route the slice-2 board at the current `connector_x` and map the copper
    /// into the board plate's frame. ONE `route_all` per call — heeding the tween
    /// lesson, this runs on settle, never per drag frame.
    func routeGripperCopper() -> [CopperSeg] {
        guard source.isGripper, let r = vcad_route_traces(connectorX, 0.25) else {
            copperUnrouted = 0
            return []
        }
        defer { vcad_route_result_free(r) }
        copperUnrouted = Int(vcad_route_result_unrouted_count(r))
        let n = Int(vcad_route_result_trace_count(r))
        // Board-local mm → kernel frame: the board plate is translated (5,5,5) and
        // is 2 mm thick, so its top face is z=7; copper sits a hair above it.
        let ox: Float = 5, oy: Float = 5, z: Float = 7.1
        var segs: [CopperSeg] = []
        segs.reserveCapacity(n)
        for i in 0..<n {
            let t = vcad_route_result_trace(r, i)
            let a = SIMD3<Float>(Float(t.start.0) + ox, Float(t.start.1) + oy, z)
            let b = SIMD3<Float>(Float(t.end.0) + ox, Float(t.end.1) + oy, z)
            segs.append(CopperSeg(a: a, b: b, width: Float(t.width), net: t.net_id))
        }
        return segs
    }

    let examples: [(name: String, path: String)] = [
        ("Pulley", "\(kSamplesDir)/mecheval/tasks/a6-pulley-01.vcad"),
        ("Counterbore", "\(kSamplesDir)/mecheval/tasks/a4-counterbore-plate-01.vcad"),
        ("Ribbed plate", "\(kSamplesDir)/mecheval/tasks/a5-ribbed-plate-01.vcad"),
        ("Robot arm", "\(kSamplesDir)/examples/robot-arm-2dof.vcad"),
        ("Sensor mast", "\(kSamplesDir)/examples/sensor-mast.vcad"),
    ]

    var recents: [URL] = (UserDefaults.standard.array(forKey: "vcad.recents") as? [String] ?? [])
        .map { URL(fileURLWithPath: $0) }

    var documentName: String { source.label }

    func newDocument() { source = .sandbox }

    func openDocument(_ url: URL) {
        source = .document(path: url.path, label: url.deletingPathExtension().lastPathComponent)
        recents.removeAll { $0 == url }
        recents.insert(url, at: 0)
        if recents.count > 8 { recents = Array(recents.prefix(8)) }
        UserDefaults.standard.set(recents.map { $0.path }, forKey: "vcad.recents")
    }

    // MARK: tool palette

    func tools(for tab: ToolTab) -> [Tool] {
        let docMode = usesDocumentTree
        switch tab {
        case .create:
            return BaseShape.allCases.map { shape in
                // Sandbox: pick the primitive. Document: add a new part.
                Tool(id: "shape.\(shape.rawValue)",
                     label: docMode ? "Add \(shape.label)" : shape.label, symbol: shape.symbol,
                     isActive: !docMode && baseShape == shape) { [weak self] in
                    if docMode { self?.addPrimitive(shape) } else { self?.baseShape = shape }
                }
            }
        case .modify:
            return Modifier.allCases.compactMap { mod in
                if docMode {
                    guard mod != .none else { return nil }     // no "None" tool when authoring
                    let ok = selectedPartIndex != nil
                    return Tool(id: "mod.\(mod.rawValue)", label: mod.label, symbol: mod.symbol,
                                isActive: false, enabled: ok,
                                hint: ok ? "" : "Select a part first") { [weak self] in
                        self?.applyModifierToSelected(mod)
                    }
                }
                // Fillet/chamfer are no-ops on a sphere (no edges) — surface that.
                let ok = mod == .none || baseShape != .sphere
                return Tool(id: "mod.\(mod.rawValue)", label: mod.label, symbol: mod.symbol,
                            isActive: modifier == mod, enabled: ok,
                            hint: ok ? "" : "No edges on a sphere") { [weak self] in self?.modifier = mod }
            }
        case .combine:
            guard docMode else { return [] }
            let ready = multiSelectedParts.count == 2
            return BooleanOp.allCases.map { b in
                Tool(id: "bool.\(b.rawValue)", label: b.label, symbol: b.symbol,
                     isActive: false, enabled: ready,
                     hint: ready ? "" : "⌘-click 2 parts") { [weak self] in self?.combineSelected(b) }
            }
        }
    }

    /// Whether the active modifier actually changes the geometry (a fillet on a
    /// sphere has no edges to round).
    var modifierEffective: Bool { !(baseShape == .sphere && modifier != .none) }

    // MARK: feature tree

    var features: [Feature] {
        switch source {
        case .sandbox:
            return [
                Feature(id: "base", name: baseShape.label, symbol: baseShape.symbol, kind: .base),
                Feature(id: "modifier", name: modifier == .none ? "No modifier" : modifier.label,
                        symbol: modifier.symbol, kind: .modifier),
            ]
        case .document(_, let label), .generated(_, let label):
            let n = max(partCount, 1)
            return (0..<n).map { i in
                Feature(id: "part\(i)", name: n == 1 ? label : "Part \(i + 1)",
                        symbol: "cube.transparent", kind: .part)
            }
        case .gripper:
            return [
                Feature(id: "part0", name: "Enclosure", symbol: "cube", kind: .part),
                Feature(id: "part1", name: "Board", symbol: "cpu", kind: .part),
            ]
        }
    }
    var selectedFeature: Feature? { features.first { $0.id == selectedFeatureID } }

    /// Whether the grabbable 3D handle is shown (only the cube+fillet case has
    /// a parametric anchor today).
    var showsHandle: Bool {
        source.isSandbox && baseShape == .cube && modifier == .fillet && selectedFeatureID == "modifier"
    }

    var panOffset: SIMD3<Float> = .zero
    var orbitVector: SIMD3<Float> {
        let r = distance
        return SIMD3<Float>(
            r * cos(elevation) * sin(azimuth),
            r * sin(elevation),
            r * cos(elevation) * cos(azimuth)
        )
    }
    var cameraPosition: SIMD3<Float> { panOffset + orbitVector }

    func handlePosition(radius: Double) -> SIMD3<Float> {
        let mid = SIMD3<Float>(15, 0, 30)   // top-front edge midpoint of the 30mm cube
        let outward = normalize(SIMD3<Float>(0, -1, 1))
        return mid + outward * (2.5 + Float(radius))
    }

    // MARK: scene building

    func buildScene() -> RenderScene {
        switch source {
        case .sandbox: return sandboxScene()
        case .document(let path, _): return documentScene(path: path)
        case .generated(let loon, _): return generatedScene(loon: loon)
        case .gripper: return gripperScene()
        }
    }

    /// Validate a generated loon program; if it evaluates to geometry, switch the
    /// active source to it (which re-renders via `generatedScene`). Returns the
    /// evaluated part count, or nil if the program failed — leaving the current
    /// scene untouched so a bad generation never blanks the studio.
    @discardableResult
    func applyGenerated(loon: String, label: String) -> GenStats? {
        let data = Data(loon.utf8)
        guard !data.isEmpty else { return nil }
        let stats: GenStats? = data.withUnsafeBytes { raw -> GenStats? in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress,
                  let scene = vcad_scene_from_loon(base, data.count) else { return nil }
            defer { vcad_scene_free(scene) }
            let n = vcad_scene_part_count(scene)
            guard n > 0 else { return nil }
            var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
            var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
            var tris = 0, real = 0
            for i in 0..<n {
                let km = KernelMesh.fromView(vcad_scene_part_mesh(scene, i))
                if km.isEmpty { continue }
                lo = simd_min(lo, km.minBound); hi = simd_max(hi, km.maxBound)
                tris += km.triangleCount; real += 1
            }
            guard real > 0 else { return nil }
            return GenStats(parts: real, size: hi - lo, triangles: tris)
        }
        guard let stats else { return nil }
        source = .generated(loon: loon, label: label)
        return stats
    }

    private func makeBase() -> OpaquePointer? {
        switch baseShape {
        case .cube: return vcad_solid_cube(30, 30, 30)
        case .cylinder: return vcad_solid_cylinder(15, 30, 64)
        case .sphere: return vcad_solid_sphere(15, 48)
        }
    }

    private var modifierApplies: Bool { selectedFeatureID != "base" && modifier != .none && modifierValue > 0.05 }

    /// Build the sandbox solid (primitive + modifier) and tessellate it.
    private func sandboxKernelMesh() -> KernelMesh? {
        guard let base = makeBase() else { return nil }
        defer { vcad_solid_free(base) }
        let start = Date()
        var target = base
        var owned: OpaquePointer?
        if modifierApplies {
            switch modifier {
            case .fillet: if let f = vcad_solid_fillet(base, modifierValue) { target = f; owned = f }
            case .chamfer: if let c = vcad_solid_chamfer(base, modifierValue) { target = c; owned = c }
            case .none: break
            }
        }
        defer { if let o = owned { vcad_solid_free(o) } }
        guard let mesh = vcad_solid_to_mesh(target, 64) else { return nil }
        defer { vcad_mesh_free(mesh) }
        let km = KernelMesh.fromView(vcad_mesh_view(mesh))
        solveMillis = Date().timeIntervalSince(start) * 1000
        triangleCount = km.triangleCount
        partCount = 1
        sizeMM = km.maxBound - km.minBound
        return km
    }

    private func sandboxScene() -> RenderScene {
        guard let km = sandboxKernelMesh() else { return .empty }
        streaming.update(from: km)
        guard let res = streaming.resource else { return .empty }
        let center = (km.minBound + km.maxBound) / 2
        let size = Self.extent(km.minBound, km.maxBound)
        displayCenter = center
        displayScale = 0.6 / max(size, 0.0001)
        return RenderScene(meshes: [(res, Self.heroColor)], center: center, size: size,
                           triangleCount: km.triangleCount, partCount: 1)
    }

    /// Hot path: re-solve the sandbox and stream into the GPU buffers in place.
    func streamSandbox() -> Bool {
        guard let km = sandboxKernelMesh() else { return false }
        return streaming.update(from: km)
    }

    struct PickHit { var point: SIMD3<Float>; var normal: SIMD3<Float> }

    /// Cast a ray (kernel coords) against the current sandbox solid.
    func raycastSandbox(originKernel: SIMD3<Float>, dirKernel: SIMD3<Float>) -> PickHit? {
        guard source.isSandbox, let base = makeBase() else { return nil }
        defer { vcad_solid_free(base) }
        var target = base
        var owned: OpaquePointer?
        if modifierApplies {
            switch modifier {
            case .fillet: if let f = vcad_solid_fillet(base, modifierValue) { target = f; owned = f }
            case .chamfer: if let c = vcad_solid_chamfer(base, modifierValue) { target = c; owned = c }
            case .none: break
            }
        }
        defer { if let o = owned { vcad_solid_free(o) } }
        let o: [Double] = [Double(originKernel.x), Double(originKernel.y), Double(originKernel.z)]
        let d: [Double] = [Double(dirKernel.x), Double(dirKernel.y), Double(dirKernel.z)]
        let hit = vcad_solid_raycast(target, o, d)
        guard hit.hit != 0 else { return nil }
        return PickHit(
            point: SIMD3(Float(hit.point.0), Float(hit.point.1), Float(hit.point.2)),
            normal: SIMD3(Float(hit.normal.0), Float(hit.normal.1), Float(hit.normal.2))
        )
    }

    private func documentScene(path: String) -> RenderScene {
        let start = Date()
        // Prefer the live (possibly edited) doc; fall back to the file on disk
        // for formats the JSON parser couldn't load (legacy terse `.vcad`).
        let data: Data?
        if let json = documentJSON { data = DocEdit.serialize(json) }
        else { data = try? Data(contentsOf: URL(fileURLWithPath: path)) }
        guard let data, !data.isEmpty else { return .empty }
        let scene: OpaquePointer? = data.withUnsafeBytes { raw in
            vcad_scene_from_json(raw.bindMemory(to: UInt8.self).baseAddress, data.count)
        }
        guard let scene else { return .empty }
        defer { vcad_scene_free(scene) }
        return sceneFromHandle(scene, start: start)
    }

    /// Render the AI-generated loon program. Same evaluator + part-gather as a
    /// loaded document, so the studio, feature tree, and materialize-pop are
    /// shared for free.
    private func generatedScene(loon: String) -> RenderScene {
        let start = Date()
        let data = Data(loon.utf8)
        guard !data.isEmpty else { return .empty }
        let scene: OpaquePointer? = data.withUnsafeBytes { raw in
            vcad_scene_from_loon(raw.bindMemory(to: UInt8.self).baseAddress, data.count)
        }
        guard let scene else { return .empty }
        defer { vcad_scene_free(scene) }
        return sceneFromHandle(scene, start: start)
    }

    /// Gather every part mesh out of an evaluated scene handle into a centered,
    /// auto-fit `RenderScene`, updating the live readouts. Shared by the
    /// document and AI-generated paths.
    private func sceneFromHandle(_ scene: OpaquePointer, start: Date) -> RenderScene {
        let count = vcad_scene_part_count(scene)
        var meshes: [(MeshResource, NSColor)] = []
        var bounds: [(min: SIMD3<Float>, max: SIMD3<Float>)] = []
        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
        var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
        var tris = 0
        for i in 0..<count {
            let km = KernelMesh.fromView(vcad_scene_part_mesh(scene, i))
            if km.isEmpty { continue }
            lo = simd_min(lo, km.minBound); hi = simd_max(hi, km.maxBound)
            tris += km.triangleCount
            meshes.append((km.resource(name: "part\(i)"), Self.partColors[i % Self.partColors.count]))
            // Index-aligned with `meshes` (and thus the rendered part entities),
            // so a viewport tap maps back to the right feature-tree row.
            bounds.append((km.minBound, km.maxBound))
        }
        guard !meshes.isEmpty else { return .empty }
        docPartBounds = bounds

        solveMillis = Date().timeIntervalSince(start) * 1000
        triangleCount = tris
        partCount = meshes.count
        sizeMM = hi - lo
        let center = (lo + hi) / 2
        let size = Self.extent(lo, hi)
        displayCenter = center
        displayScale = 0.6 / max(size, 0.0001)
        return RenderScene(meshes: meshes, center: center, size: size,
                           triangleCount: tris, partCount: meshes.count)
    }

    static func extent(_ lo: SIMD3<Float>, _ hi: SIMD3<Float>) -> Float {
        let d = hi - lo
        return max(d.x, max(d.y, d.z))
    }

    static let heroColor = NSColor(red: 0.40, green: 0.60, blue: 0.80, alpha: 1.0)
    static let partColors: [NSColor] = [
        NSColor(red: 0.62, green: 0.66, blue: 0.70, alpha: 1.0),
        NSColor(red: 0.82, green: 0.62, blue: 0.30, alpha: 1.0),
        NSColor(red: 0.45, green: 0.62, blue: 0.82, alpha: 1.0),
        NSColor(red: 0.72, green: 0.46, blue: 0.46, alpha: 1.0),
    ]
}
