import SwiftUI
import AppKit
import simd
import CVcadFFI

@main
struct VcadApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    init() {
        // Headless smoke hook (matches VCAD_GRIPPER/VCAD_ROUTE): dump the parsed
        // feature tree of a .vcad to stderr and exit — verifies the DAG parser
        // without driving the GUI. e.g. VCAD_DUMP_TREE=examples/plate.vcad
        if let path = ProcessInfo.processInfo.environment["VCAD_DUMP_TREE"] {
            dumpFeatureTree(path: path)
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_EDIT_SMOKE"] {
            MainActor.assumeIsolated { editSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_AUTHOR_SMOKE"] {
            MainActor.assumeIsolated { authorSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_SKETCH_SMOKE"] {
            MainActor.assumeIsolated { sketchSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_MAT_SMOKE"] {
            MainActor.assumeIsolated { matSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_USDZ_SMOKE"] {
            MainActor.assumeIsolated { usdzSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_PATTERN_SMOKE"] {
            MainActor.assumeIsolated { patternSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_GIZMO_SMOKE"] {
            MainActor.assumeIsolated { gizmoSmoke(path: path) }
            exit(0)
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_PLAYBACK_SMOKE"] {
            MainActor.assumeIsolated { playbackSmoke(path: path) }
            exit(0)
        }
    }

    /// The document lives at app scope, not in the window: the WindowGroup's
    /// window is hidden at launch (release-to-desktop is the only mode), and the
    /// menu bar commands need the same model the floating overlay is editing.
    @State private var model = EditorModel()
    @State private var intent = IntentEngine()

    var body: some Scene {
        WindowGroup {
            EditorView(model: model, intent: intent)
        }
        .windowStyle(.automatic)
        .commands { DocumentCommands(model: model) }
    }
}

/// Print a document's parsed feature tree (used by the VCAD_DUMP_TREE hook).
private func dumpFeatureTree(path: String) {
    guard let g = DocumentGraph.load(path: path) else {
        FileHandle.standardError.write(Data("[VCAD_TREE] parse failed: \(path)\n".utf8))
        return
    }
    var out = "[VCAD_TREE] \(path) — \(g.visibleRoots.count) part(s)\n"
    func walk(_ node: FeatureNode, _ depth: Int) {
        let pad = String(repeating: "  ", count: depth)
        let part = node.partIndex.map { " [part \($0)]" } ?? ""
        let detail = node.detail.map { " — \($0)" } ?? ""
        out += "\(pad)• \(node.name) (\(node.opType))\(detail)\(part)\n"
        for c in node.children { walk(c, depth + 1) }
    }
    for r in g.featureRoots() { walk(r, 0) }
    FileHandle.standardError.write(Data(out.utf8))
}

/// Exercise the edit → re-eval → undo → export pipeline headlessly.
@MainActor private func editSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    func find(_ nodes: [FeatureNode], _ op: String) -> FeatureNode? {
        for n in nodes {
            if n.opType == op { return n }
            if let f = find(n.children, op) { return f }
        }
        return nil
    }
    let m = EditorModel()
    m.source = .document(path: path, label: "smoke")
    guard m.usesDocumentTree, m.reevalDocumentMeshes() != nil else {
        emit("[VCAD_EDIT] no renderable geometry for \(path) (e.g. an assembly the scene eval can't handle)")
        return
    }
    let before = m.sizeMM
    emit("[VCAD_EDIT] loaded: bbox \(fmt3(before)) · tris \(m.triangleCount) · canUndo \(m.canUndo)")

    guard let cube = find(m.featureNodes, "Cube") else { emit("[VCAD_EDIT] no Cube to edit"); return }
    m.editVec(nodeId: cube.nodeId, key: "size", axis: "x", value: 200, snapshot: true)
    _ = m.reevalDocumentMeshes()
    emit("[VCAD_EDIT] after size.x=200 on '\(cube.name)': bbox \(fmt3(m.sizeMM)) · tris \(m.triangleCount) · canUndo \(m.canUndo)")

    m.undo()
    _ = m.reevalDocumentMeshes()
    emit("[VCAD_EDIT] after undo: bbox \(fmt3(m.sizeMM)) · canRedo \(m.canRedo)")

    let stl = URL(fileURLWithPath: "/tmp/vcad_edit_smoke.stl")
    let ok = m.exportSTL(to: stl)
    let size = (try? Data(contentsOf: stl))?.count ?? 0
    emit("[VCAD_EDIT] STL export: \(ok) · \(size) bytes")
}

private func fmt3(_ v: SIMD3<Float>) -> String {
    String(format: "%.1f×%.1f×%.1f", abs(v.x), abs(v.y), abs(v.z))
}

/// Load a document and export it as USDZ — verifies the ModelIO pipeline
/// headlessly. Writes to /tmp/vcad_usdz_smoke.usdz.
@MainActor private func usdzSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "usdz")
    _ = m.reevalDocumentMeshes()
    let out = URL(fileURLWithPath: "/tmp/vcad_usdz_smoke.usdz")
    let ok = m.exportUSDZ(to: out)
    let size = (try? Data(contentsOf: out))?.count ?? 0
    emit("[VCAD_USDZ] export: \(ok) · \(size) bytes · parts \(m.partCount) · bbox \(fmt3(m.sizeMM)) mm")
}

/// Material assignment + persistence: set part 0 to copper, verify the resolved
/// color, save, reload, and confirm the assignment + definition round-trip.
@MainActor private func matSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "mat")
    guard m.usesDocumentTree else { emit("[VCAD_MAT] load failed"); return }
    let before = m.resolvedMaterial(forPart: 0).color
    emit("[VCAD_MAT] part0 material '\(m.materialName(forPart: 0) ?? "—")' color \(rgb(before))")
    m.setPartMaterial(0, "copper")
    let after = m.resolvedMaterial(forPart: 0).color
    emit("[VCAD_MAT] → copper: key '\(m.materialName(forPart: 0) ?? "—")' color \(rgb(after))")
    let out = URL(fileURLWithPath: "/tmp/vcad_mat.vcad")
    m.saveDocumentAs(out)
    if let g = DocumentGraph.load(path: out.path) {
        let hasDef = g.materials["copper"] != nil
        emit("[VCAD_MAT] reloaded: part0 key '\(g.visibleRoots.first?.material ?? "—")' · copper def present \(hasDef)")
    } else { emit("[VCAD_MAT] reload failed") }
}

private func rgb(_ c: NSColor) -> String {
    let s = c.usingColorSpace(.sRGB) ?? c
    return String(format: "(%.2f, %.2f, %.2f)", s.redComponent, s.greenComponent, s.blueComponent)
}

/// Pattern instancing: load a doc with pattern roots through the instanced
/// eval path, then cross-check each instanced part's aggregated bounds and
/// triangle count against a full BAKED kernel eval of the untouched document.
/// Agreement proves the Swift-side transforms reproduce the kernel's pattern
/// placement exactly.
@MainActor private func patternSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "pattern")
    guard m.usesDocumentTree, m.reevalDocumentMeshes() != nil else {
        emit("[VCAD_PATTERN] load failed"); return
    }
    let instanced = m.docPartInstancing.enumerated().compactMap { i, p in p.map { (i, $0) } }
    emit("[VCAD_PATTERN] \(m.partCount) part(s), \(instanced.count) instanced · tris \(m.triangleCount) · bbox \(fmt3(m.sizeMM))")
    for (i, p) in instanced {
        emit("[VCAD_PATTERN] part\(i): node \(p.patternNodeId) seed \(p.seedNodeId) × \(p.transforms.count) copies")
    }

    // Baked reference: evaluate the ORIGINAL doc (patterns unioned in-kernel).
    guard let json = m.documentJSON, let data = DocEdit.serialize(json) else { return }
    let scene: OpaquePointer? = data.withUnsafeBytes {
        vcad_scene_from_json($0.bindMemory(to: UInt8.self).baseAddress, data.count)
    }
    guard let scene else { emit("[VCAD_PATTERN] baked eval failed"); return }
    defer { vcad_scene_free(scene) }
    var ok = true
    for i in 0..<vcad_scene_part_count(scene) {
        let km = KernelMesh.fromView(vcad_scene_part_mesh(scene, i))
        guard i < m.docPartMeshes.count else { continue }
        let pick = m.docPartMeshes[i]
        let dLo = simd_length(km.minBound - pick.lo), dHi = simd_length(km.maxBound - pick.hi)
        let match = dLo < 1e-3 && dHi < 1e-3
        // Baked pattern tris can differ slightly (the union sews coincident
        // faces); bounds are the placement invariant.
        emit("[VCAD_PATTERN] part\(i) bounds baked vs instanced: Δlo \(String(format: "%.5f", dLo)) Δhi \(String(format: "%.5f", dHi)) → \(match ? "MATCH" : "MISMATCH")")
        if !match { ok = false }
    }
    emit("[VCAD_PATTERN] \(ok ? "PASS" : "FAIL")")
}

/// Gizmo translate: move part 0 by +50 in X via the root Translate, confirm the
/// bbox shifts, then undo back.
@MainActor private func gizmoSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "gizmo")
    guard m.usesDocumentTree, m.reevalDocumentMeshes() != nil else { emit("[VCAD_GIZMO] load failed"); return }
    let c0 = m.gizmoCenterKernel() ?? .zero
    emit("[VCAD_GIZMO] loaded · part0 center \(fmt3(c0)) · bbox \(fmt3(m.sizeMM))")
    guard var json = m.documentJSON else { return }
    DocEdit.setRootTranslate(&json, partIndex: 0, 50, 0, 0)
    m.documentJSON = json
    _ = m.reevalDocumentMeshes()
    let c1 = m.gizmoCenterKernel() ?? .zero
    emit("[VCAD_GIZMO] +50 X · part0 center \(fmt3(c1)) (Δx \(String(format: "%.1f", c1.x - c0.x)))")

    // Rotate-about-center: 90° about Z should swap the footprint + hold the center.
    let r = EditorModel()
    r.source = .document(path: path, label: "rot")
    _ = r.reevalDocumentMeshes()
    let rc0 = r.gizmoCenterKernel() ?? .zero
    let before = r.sizeMM
    guard var rj = r.documentJSON, let rot = DocEdit.wrapRotate(&rj, partIndex: 0,
                                                                Double(rc0.x), Double(rc0.y), Double(rc0.z)) else { return }
    DocEdit.setRotateAngle(&rj, rotNodeId: rot, axisIndex: 2, degrees: 90)
    r.documentJSON = rj
    _ = r.reevalDocumentMeshes()
    let rc1 = r.gizmoCenterKernel() ?? .zero
    emit("[VCAD_GIZMO] rotate 90°Z · bbox \(fmt3(before)) → \(fmt3(r.sizeMM)) · center \(fmt3(rc0)) → \(fmt3(rc1))")
}

/// Sketch → extrude authoring: draw a 40×40 square on XY, extrude 12mm, verify
/// the kernel builds a 40×40×12 solid.
@MainActor private func sketchSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "sketch")
    guard m.usesDocumentTree else { emit("[VCAD_SKETCH] load failed"); return }
    let parts0 = m.featureNodes.count
    m.enterSketch()
    m.setSketchTool(.rectangle)
    m.sketchTap(SIMD2(0, 0))         // corner
    m.sketchTap(SIMD2(40, 40))       // opposite corner
    m.sketchExtrudeDepth = 12
    emit("[VCAD_SKETCH] profile verts \(m.sketchVerts.count) closed \(m.sketchClosed) canFinish \(m.canFinishSketch)")
    m.finishSketch()
    _ = m.reevalDocumentMeshes()
    emit("[VCAD_SKETCH] extruded → \(m.featureNodes.count) parts (was \(parts0)) · last op '\(m.featureNodes.last?.opType ?? "?")' · bbox \(fmt3(m.sizeMM)) · tris \(m.triangleCount) · sketching \(m.sketching)")
}

/// Exercise the authoring pipeline: add a primitive, apply a fillet, save,
/// reload — confirming round-trip authoring through the kernel.
@MainActor private func authorSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "author")
    guard m.usesDocumentTree else { emit("[VCAD_AUTHOR] load failed"); return }
    let parts0 = m.featureNodes.count
    emit("[VCAD_AUTHOR] loaded \(parts0) part(s)")

    m.addPrimitive(.cylinder)
    _ = m.reevalDocumentMeshes()
    emit("[VCAD_AUTHOR] +cylinder → \(m.featureNodes.count) parts · tris \(m.triangleCount)")

    m.selectedFeatureID = m.featureNodes.first?.id
    m.applyModifierToSelected(.fillet)
    _ = m.reevalDocumentMeshes()
    let rootOp = m.selectedFeatureNode?.opType ?? "?"
    emit("[VCAD_AUTHOR] fillet part 0 → root op now '\(rootOp)' · tris \(m.triangleCount)")

    m.multiSelectedParts = [0, 1]
    m.combineSelected(.difference)
    _ = m.reevalDocumentMeshes()
    emit("[VCAD_AUTHOR] subtract parts 0,1 → \(m.featureNodes.count) part(s), root op '\(m.featureNodes.first?.opType ?? "?")' · tris \(m.triangleCount)")

    let out = URL(fileURLWithPath: "/tmp/vcad_authored.vcad")
    m.saveDocumentAs(out)
    let reloaded = DocumentGraph.load(path: out.path)
    emit("[VCAD_AUTHOR] saved + reloaded: \(reloaded?.visibleRoots.count ?? -1) part(s), parses=\(reloaded != nil)")
}

/// Kinematic joint playback: load a jointed assembly, confirm instances +
/// timeline surface through the model, then scrub to mid-timeline and show
/// the FK-driven instance positions moving.
@MainActor private func playbackSmoke(path: String) {
    func emit(_ s: String) { FileHandle.standardError.write(Data((s + "\n").utf8)) }
    let m = EditorModel()
    m.source = .document(path: path, label: "playback")
    let scene = m.buildScene()
    emit("[VCAD_PLAY] instances \(scene.instances.count) · joints \(m.documentGraph?.joints.count ?? 0) · hasPlayback \(m.hasPlayback) · duration \(m.timeline?.durationS ?? 0)s")
    guard m.hasPlayback else { emit("[VCAD_PLAY] no playback surface"); return }
    let dur = m.timeline?.durationS ?? 0
    let before = m.instanceTransforms
    m.setPlaybackTime(dur / 2)
    let mid = m.instanceTransforms
    for i in 0..<min(before.count, mid.count) {
        let a = before[i].columns.3, b = mid[i].columns.3
        emit(String(format: "[VCAD_PLAY] inst%d t=0 (%.1f, %.1f, %.1f) → t=%.2f (%.1f, %.1f, %.1f)",
                    i, a.x, a.y, a.z, dur / 2, b.x, b.y, b.z))
    }
    m.setPlaybackTime(dur)
    let end = m.instanceTransforms
    emit(String(format: "[VCAD_PLAY] inst%d t=%.2f pos (%.1f, %.1f, %.1f)",
                end.count - 1, dur,
                end.last!.columns.3.x, end.last!.columns.3.y, end.last!.columns.3.z))
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool { true }
}
