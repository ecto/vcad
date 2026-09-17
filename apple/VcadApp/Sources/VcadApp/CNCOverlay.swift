import SwiftUI
import RealityKit
import CVcadFFI

// The CNC overlay in the shared RealityKit scene: stock, clearance plane, work
// origin, toolpaths and the live tool position, all in the kernel-mm frame.

/// All entities live in the existing kernel-mm frame, inheriting CAD camera,
/// centering and Z-up transforms. Path meshes rebuild only when the job changes.
@MainActor
func syncCNCOverlay(_ cnc: CNCWorkspace, in parent: Entity, model: EditorModel? = nil) {
    if let model {
        let show = !model.isWindowed || model.workspace != .manufacture || cnc.showPart
        for child in parent.children {
            if child.name.hasPrefix("part"), let index = Int(child.name.dropFirst(4)) {
                child.isEnabled = show && model.isPartVisible(index)
            } else if child.name.hasPrefix("inst") {
                child.isEnabled = show
            }
            if child.name.hasPrefix("part") || child.name.hasPrefix("inst") {
                let ghost = model.isWindowed && model.workspace == .manufacture && cnc.mode == .toolpaths
                child.components.set(OpacityComponent(opacity: ghost ? 0.28 : 1))
            }
        }
    }
    let root: Entity
    if let existing = parent.findEntity(named: "cncRoot") { root = existing }
    else { root = Entity(); root.name = "cncRoot"; parent.addChild(root) }
    root.isEnabled = cnc.overlay && (cnc.shown || cnc.program != nil || (cnc.machine.g54Active && cnc.machine.status.work != nil))
    guard root.isEnabled else { return }
    guard [cnc.origin.x, cnc.origin.y, cnc.origin.z].allSatisfy({ $0.isFinite && abs($0) < 1_000_000 }) else { root.isEnabled = false; return }
    root.position = cnc.origin.floats
    let key = "path-\(cnc.revision)-\(cnc.stockThickness)-\(cnc.displayKey)"
    if root.findEntity(named: key) == nil {
        let spec = cnc.mode == .setup ? cnc.setup : (cnc.generatedSetup ?? cnc.setup)
        root.children.removeAll()
        let group = Entity(); group.name = key
        for rapid in [false, true] where cnc.mode != .setup {
            for (index, program) in cnc.displayPrograms.enumerated() {
                if let mesh = cncLineMesh(program.moves, rapid: rapid) {
                    let path = ModelEntity(mesh: mesh, materials: [UnlitMaterial(color: rapid ? .cyan : .orange)])
                    path.name = "cncPath-\(index)-\(rapid)"
                    group.addChild(path)
                }
            }
        }
        if !cnc.usesImportedProgram && cnc.showStock && cnc.stockThickness.isFinite && cnc.stockThickness > 0 && cnc.stockThickness < 1000
            && spec.width.isFinite && spec.height.isFinite && spec.width > 0 && spec.height > 0 && max(spec.width, spec.height) <= 1000 {
            var material = SimpleMaterial(color: .gray.withAlphaComponent(0.12), isMetallic: false)
            material.faceCulling = .none
            let stock = ModelEntity(mesh: .generateBox(size: [Float(spec.width), Float(spec.height), Float(cnc.stockThickness)]), materials: [material])
            stock.position = [Float(spec.width / 2), Float(spec.height / 2), -Float(cnc.stockThickness / 2)]
            group.addChild(stock)
        }
        if !cnc.usesImportedProgram && cnc.showClearance && spec.clearance.isFinite && spec.clearance > 0 && spec.clearance < 1000 && spec.width > 0 && spec.height > 0 && max(spec.width, spec.height) <= 1000 {
            let plane = ModelEntity(mesh: .generateBox(size: [Float(spec.width), Float(spec.height), 0.1]), materials: [SimpleMaterial(color: .cyan.withAlphaComponent(0.12), isMetallic: false)])
            plane.position = [Float(spec.width / 2), Float(spec.height / 2), Float(spec.clearance)]
            plane.name = "cncClearance"; group.addChild(plane)
        }
        let origin = ModelEntity(mesh: .generateSphere(radius: 0.6), materials: [UnlitMaterial(color: .white)])
        origin.name = "cncOrigin"; group.addChild(origin)
        let radius = spec.diameter.isFinite && spec.diameter > 0 && spec.diameter < 100 ? Float(spec.diameter / 2) : 1.5
        let previewTool = ModelEntity(mesh: .generateCylinder(height: 12, radius: radius), materials: [SimpleMaterial(color: .orange.withAlphaComponent(0.65), isMetallic: false)])
        previewTool.name = "cncPreviewTool"
        previewTool.orientation = simd_quatf(angle: .pi / 2, axis: [1, 0, 0])
        group.addChild(previewTool)
        let tool = ModelEntity(mesh: .generateCylinder(height: 12, radius: radius), materials: [UnlitMaterial(color: .green)])
        tool.name = "cncTool"
        tool.orientation = simd_quatf(angle: .pi / 2, axis: [1, 0, 0])
        group.addChild(tool); root.addChild(group)
    }
    if let previewTool = root.findEntity(named: "cncPreviewTool") {
        previewTool.isEnabled = cnc.mode == .toolpaths && cnc.previewPosition != nil
        if let position = cnc.previewPosition { previewTool.position = position + [0, 0, 6] }
    }
    if let tool = root.findEntity(named: "cncTool") {
        tool.isEnabled = cnc.machine.status.isFresh && cnc.machine.g54Active && cnc.machine.status.work != nil
        if let position = cnc.machine.status.work { tool.position = position.floats + [0, 0, 6] }
    }
}

@MainActor
private func cncLineMesh(_ moves: [CNCMove], rapid: Bool) -> MeshResource? {
    var vertices: [SIMD3<Float>] = []
    var indices: [UInt32] = []
    var previous: SIMD3<Float>?
    for move in moves {
        let b = move.point
        defer { previous = b }
        guard let a = previous, move.rapid == rapid, simd_length(b-a) > 0.0001 else { continue }
        let d = simd_normalize(b-a)
        let u = simd_normalize(simd_cross(d, abs(d.z) < 0.9 ? SIMD3<Float>(0,0,1) : SIMD3<Float>(0,1,0))) * 0.10
        let v = simd_normalize(simd_cross(d, u)) * 0.10
        let n = UInt32(vertices.count)
        vertices += [a-u-v, a+u-v, a+u+v, a-u+v, b-u-v, b+u-v, b+u+v, b-u+v]
        indices += [0,1,2,0,2,3,4,6,5,4,7,6,0,4,5,0,5,1,1,5,6,1,6,2,2,6,7,2,7,3,3,7,4,3,4,0].map { n + UInt32($0) }
    }
    guard !vertices.isEmpty else { return nil }
    var descriptor = MeshDescriptor(name: rapid ? "CNC rapids" : "CNC cuts")
    descriptor.positions = MeshBuffers.Positions(vertices)
    descriptor.primitives = .triangles(indices)
    return try? MeshResource.generate(from: [descriptor])
}
