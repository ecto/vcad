import SwiftUI
import AppKit
import simd

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
    }

    var body: some Scene {
        WindowGroup {
            EditorView()
                .frame(minWidth: 560, minHeight: 600)
        }
        .windowStyle(.automatic)
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

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool { true }
}
