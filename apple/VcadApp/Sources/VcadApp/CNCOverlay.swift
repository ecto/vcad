import SwiftUI
import RealityKit
import CVcadFFI

// The CNC overlay in the shared RealityKit scene: the blank with the margin it
// really needs, the clearance plane, work zero and how far it sits from the
// blank's corner, the cutter's swept rectangle, the toolpath, the tabs as they
// were audited, and the live tool position.
//
// Everything hangs off `cncRoot`, which is placed at `cnc.origin` — where the
// stock frame's zero sits in the model. Friction-log item 47: the path used to
// be drawn at the model origin while the part sat where it was modelled, so it
// floated beside the solid. Putting the outline's own translation on the root
// is what lays one on the other.

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
        root.children.removeAll()
        let group = Entity(); group.name = key
        let width = cnc.stockWidth, height = cnc.stockHeight
        let margin = cnc.effectiveMargin
        let sane = width.isFinite && height.isFinite && width > 0 && height > 0
            && max(width, height) <= 1000 && margin.isFinite && margin >= 0 && margin < 500

        for rapid in [false, true] where cnc.mode != .setup {
            for (index, moves) in cnc.displayMoves.enumerated() {
                if let mesh = cncLineMesh(moves, rapid: rapid) {
                    let path = ModelEntity(mesh: mesh, materials: [UnlitMaterial(color: rapid ? .cyan : .orange)])
                    path.name = "cncPath-\(index)-\(rapid)"
                    group.addChild(path)
                }
            }
        }

        // Everything below is drawn in the *work* frame — the frame the
        // toolpath is already in — so a job that is shifted or turned on the
        // blank shows where it really sits, with zero at the origin.
        //
        // The group itself carries the placement backwards, so work-frame
        // geometry lands on the part where it was modelled (item 47): the part
        // stays upright on screen and the blank, the sweep and zero sit
        // crooked around it, which is the picture on the table.
        let placement = cnc.effectivePlacement
        let turn = simd_quatf(angle: Float(placement.rotationDeg * .pi / 180), axis: [0, 0, 1])
        if !cnc.usesImportedProgram, !placement.isIdentity {
            let back = simd_quatf(angle: Float(-placement.rotationDeg * .pi / 180), axis: [0, 0, 1])
            group.orientation = back
            group.position = back.act(SIMD3<Float>(Float(-placement.dx), Float(-placement.dy), 0))
        }
        func placed(_ p: [Double], _ z: Double) -> SIMD3<Float> {
            let q = placement.apply(p)
            return [Float(q[0]), Float(q[1]), Float(z)]
        }

        // The blank, drawn with the margin it needs rather than at the part's
        // own extents (item 41): the cutter runs a radius outside the profile,
        // and nothing showed how much material that asks for.
        if !cnc.usesImportedProgram && cnc.showStock && cnc.stockThickness.isFinite
            && cnc.stockThickness > 0 && cnc.stockThickness < 1000 && sane {
            var material = SimpleMaterial(color: .gray.withAlphaComponent(0.12), isMetallic: false)
            material.faceCulling = .none
            let blank = ModelEntity(
                mesh: .generateBox(size: [Float(width + 2 * margin), Float(height + 2 * margin), Float(cnc.stockThickness)]),
                materials: [material])
            blank.position = placed([width / 2, height / 2], -cnc.stockThickness / 2)
            blank.orientation = turn
            blank.name = "cncStock"
            group.addChild(blank)
        }

        // The part itself, where the job puts it on that blank: a placed job
        // is a different part on the metal, and this is what says so.
        if !cnc.usesImportedProgram, cnc.showPart, sane, let outline = cnc.outline {
            let loops = [outline.outer] + outline.holes
            for (index, loop) in loops.enumerated() where loop.points.count >= 3 {
                let moves = (loop.points + [loop.points[0]]).map { point -> CNCMove in
                    let p = placement.apply([Double(point.x), Double(point.y)])
                    return CNCMove(to: [p[0], p[1], 0.02], rapid: false)
                }
                guard let mesh = cncLineMesh(moves, rapid: false, radius: 0.12) else { continue }
                let entity = ModelEntity(mesh: mesh, materials: [UnlitMaterial(color: .white.withAlphaComponent(0.7))])
                entity.name = "cncPartOutline-\(index)"
                group.addChild(entity)
            }
        }

        // What the cutter sweeps: the part outline grown by one radius, which
        // is the rectangle that has to be clear of clamps.
        if !cnc.usesImportedProgram && cnc.showEnvelope && sane && cnc.toolDiameter.isFinite && cnc.toolDiameter > 0 {
            let r = cnc.toolDiameter / 2
            let sweep = ModelEntity(
                mesh: .generateBox(size: [Float(width + 2 * r), Float(height + 2 * r), 0.05]),
                materials: [UnlitMaterial(color: .systemYellow.withAlphaComponent(0.35))])
            sweep.position = placed([width / 2, height / 2], 0.03)
            sweep.orientation = turn
            sweep.name = "cncSweep"
            group.addChild(sweep)
        }

        // Clamps, and whether the cutter would sweep through them. The request
        // has no clamp field: this is the app's own check, and the colour says
        // which answer it got.
        if !cnc.usesImportedProgram {
            let caught = Set(cnc.clampsInTheWay.map(\.id))
            for (index, clamp) in cnc.clamps.enumerated() {
                guard [clamp.x, clamp.y, clamp.width, clamp.height].allSatisfy({ $0.isFinite && abs($0) < 100_000 }),
                      clamp.width > 0, clamp.height > 0 else { continue }
                let hit = caught.contains(clamp.id)
                let entity = ModelEntity(
                    mesh: .generateBox(size: [Float(clamp.width), Float(clamp.height), 6]),
                    materials: [SimpleMaterial(color: (hit ? NSColor.systemRed : NSColor.systemGray)
                                                .withAlphaComponent(hit ? 0.45 : 0.28), isMetallic: false)])
                entity.position = [Float(clamp.x + clamp.width / 2), Float(clamp.y + clamp.height / 2), 3]
                entity.name = "cncClamp-\(index)"
                group.addChild(entity)
            }
        }

        if !cnc.usesImportedProgram && cnc.showClearance && cnc.setup.clearance.isFinite
            && cnc.setup.clearance > 0 && cnc.setup.clearance < 1000 && sane {
            let plane = ModelEntity(mesh: .generateBox(size: [Float(width), Float(height), 0.1]),
                                    materials: [SimpleMaterial(color: .cyan.withAlphaComponent(0.12), isMetallic: false)])
            plane.position = [Float(width / 2), Float(height / 2), Float(cnc.setup.clearance)]
            plane.name = "cncClearance"; group.addChild(plane)
        }

        // Work zero — the origin of everything here — and the corner of the
        // blank it is measured from, wherever the user chose to put it.
        let origin = ModelEntity(mesh: .generateSphere(radius: 0.6), materials: [UnlitMaterial(color: .white)])
        origin.name = "cncOrigin"; group.addChild(origin)
        if !cnc.usesImportedProgram && sane {
            let corner = cnc.stockCornerFromZero
            if corner.allSatisfy({ $0.isFinite && abs($0) < 100_000 }) {
                let marker = ModelEntity(mesh: .generateSphere(radius: 0.4),
                                         materials: [UnlitMaterial(color: .systemYellow)])
                marker.position = [Float(corner[0]), Float(corner[1]), 0]
                marker.name = "cncStockCorner"; group.addChild(marker)
                // The two legs of the distance from the blank's corner to
                // zero: the numbers the inspector prints, drawn.
                if let arm = cncLineMesh([CNCMove(to: [corner[0], corner[1], 0], rapid: false),
                                          CNCMove(to: [0, corner[1], 0], rapid: false),
                                          CNCMove(to: [0, 0, 0], rapid: false)], rapid: false) {
                    let ruler = ModelEntity(mesh: arm, materials: [UnlitMaterial(color: .systemYellow)])
                    ruler.name = "cncZeroOffset"; group.addChild(ruler)
                }
            }
        }

        // Tabs, twice over and drawn differently on purpose: where they were
        // *asked for* (a handle you can drag) and where they were *cut* (what
        // the audit measured off the toolpath). Item 21: the user could not
        // see them at all before cutting, let alone move them.
        if !cnc.usesImportedProgram {
            for operation in cnc.operations where operation.setup.isContour {
                let declared = cnc.declaredTabPositions(of: operation)
                for index in declared.indices {
                    guard let xy = cnc.declaredTabPoint(of: operation, index: index),
                          xy.allSatisfy({ $0.isFinite && abs($0) < 100_000 }) else { continue }
                    let p = placement.apply(xy)
                    let handle = ModelEntity(
                        mesh: .generateSphere(radius: 0.9),
                        materials: [UnlitMaterial(color: .systemTeal.withAlphaComponent(0.85))])
                    handle.position = [Float(p[0]), Float(p[1]), 0.6]
                    // The name carries which tab it is, so a drag in the
                    // viewport can name it back to the workspace.
                    handle.name = "cncTabHandle-\(operation.id.uuidString)-\(index)"
                    handle.components.set(CollisionComponent(shapes: [.generateSphere(radius: 1.6)]))
                    handle.components.set(InputTargetComponent())
                    group.addChild(handle)
                }
            }
        }
        // …and the metal that will really be left, where the audit found it.
        if let tabs = cnc.verification?.tabs.observations, !cnc.usesImportedProgram, cnc.mode != .setup {
            for (i, tab) in tabs.enumerated() where tab.path.count >= 2 {
                let moves = tab.path.map { CNCMove(to: [$0[0], $0[1], tab.topZ], rapid: false) }
                guard let mesh = cncLineMesh(moves, rapid: false, radius: 0.22) else { continue }
                let entity = ModelEntity(mesh: mesh, materials: [UnlitMaterial(color: .systemGreen)])
                entity.name = "cncTab-\(i)"
                group.addChild(entity)
            }
        }

        // Where a selected violation is, so "which cut is that?" has an answer
        // in the viewport and not only in a list.
        if let xy = cnc.markedXY, xy.count >= 2, xy.allSatisfy({ $0.isFinite && abs($0) < 100_000 }) {
            let z = (cnc.markedZ?.isFinite == true) ? cnc.markedZ! : 0
            let mark = ModelEntity(mesh: .generateSphere(radius: 0.9),
                                   materials: [UnlitMaterial(color: .systemRed)])
            mark.position = [Float(xy[0]), Float(xy[1]), Float(z)]
            mark.name = "cncViolationMark"; group.addChild(mark)
        }

        let radius = cnc.toolDiameter.isFinite && cnc.toolDiameter > 0 && cnc.toolDiameter < 100 ? Float(cnc.toolDiameter / 2) : 1.5
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
private func cncLineMesh(_ moves: [CNCMove], rapid: Bool, radius: Float = 0.10) -> MeshResource? {
    var vertices: [SIMD3<Float>] = []
    var indices: [UInt32] = []
    var previous: SIMD3<Float>?
    for move in moves {
        let b = move.point
        defer { previous = b }
        guard let a = previous, move.rapid == rapid, simd_length(b-a) > 0.0001 else { continue }
        let d = simd_normalize(b-a)
        let u = simd_normalize(simd_cross(d, abs(d.z) < 0.9 ? SIMD3<Float>(0,0,1) : SIMD3<Float>(0,1,0))) * radius
        let v = simd_normalize(simd_cross(d, u)) * radius
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
