import SwiftUI
import SceneKit

/// Geometric preview only; no manufacturing or collision claims.
struct ElectronicsBoardPreview: NSViewRepresentable {
    var board: ECObject
    func makeCoordinator() -> Coordinator { Coordinator() }
    final class Coordinator { var signature: Data? }
    func makeNSView(context: Context) -> SCNView {
        let view = SCNView()
        view.allowsCameraControl = true; view.autoenablesDefaultLighting = true
        view.backgroundColor = .windowBackgroundColor
        view.antialiasingMode = .multisampling4X
        return view
    }
    func updateNSView(_ view: SCNView, context: Context) {
        let signature = try? JSONSerialization.data(withJSONObject: board, options: .sortedKeys)
        guard signature != context.coordinator.signature else { return }
        context.coordinator.signature = signature
        let scene = SCNScene(), outline = board["outline"] as? ECObject ?? [:]
        let vertices = ecRows(outline["vertices"]).map { ecPoint($0) }
        let path = NSBezierPath()
        if let first = vertices.first { path.move(to: first); vertices.dropFirst().forEach { path.line(to: $0) }; path.close() }
        for cutout in outline["cutouts"] as? [[ECObject]] ?? [] {
            let points = cutout.map { ecPoint($0) }
            if let first = points.first { path.move(to: first); points.dropFirst().forEach { path.line(to: $0) }; path.close() }
        }
        path.windingRule = .evenOdd
        let thickness = max(0.1, ecNumber(outline["thickness"], 1.6))
        let shape = SCNShape(path: path, extrusionDepth: thickness)
        shape.firstMaterial?.diffuse.contents = NSColor.systemGreen.withAlphaComponent(0.8)
        scene.rootNode.addChildNode(SCNNode(geometry: shape))
        for footprint in ecRows(board["footprints"]) {
            let p = ecPoint(footprint["position"]), front = footprint["front"] as? Bool ?? true
            let body = SCNBox(width: 5, height: 2.5, length: 1.2, chamferRadius: 0.15)
            body.firstMaterial?.diffuse.contents = NSColor.darkGray
            let node = SCNNode(geometry: body)
            node.position = SCNVector3(p.x, p.y, front ? thickness + 0.6 : -0.6)
            node.eulerAngles.z = CGFloat(ecNumber(footprint["rotation"]) * .pi / 180)
            scene.rootNode.addChildNode(node)
            for pad in ecRows(footprint["pads"]) {
                let q = ElectronicsCanvas.world(pad, in: footprint)
                let geometry = SCNBox(width: 1.5, height: 2, length: 0.08, chamferRadius: 0.1)
                geometry.firstMaterial?.diffuse.contents = NSColor.systemOrange
                let n = SCNNode(geometry: geometry); n.position = SCNVector3(q.x, q.y, front ? thickness + 0.05 : -0.05); scene.rootNode.addChildNode(n)
            }
        }
        for trace in ecRows(board["traces"]) {
            let a = ecPoint(trace["start"]), b = ecPoint(trace["end"]), length = hypot(b.x - a.x, b.y - a.y)
            guard length > 0 else { continue }
            let geometry = SCNBox(width: length, height: max(0.05, ecNumber(trace["width"], 0.25)), length: 0.04, chamferRadius: 0)
            geometry.firstMaterial?.diffuse.contents = trace["layer"] as? String == "BCu" ? NSColor.systemCyan : NSColor.systemOrange
            let n = SCNNode(geometry: geometry); n.position = SCNVector3((a.x + b.x) / 2, (a.y + b.y) / 2, trace["layer"] as? String == "BCu" ? -0.04 : thickness + 0.04)
            n.eulerAngles.z = atan2(b.y - a.y, b.x - a.x); scene.rootNode.addChildNode(n)
        }
        let camera = SCNNode(); camera.camera = SCNCamera(); camera.camera?.zFar = 100_000
        let center = CGPoint(x: ((vertices.map(\.x).min() ?? 0) + (vertices.map(\.x).max() ?? 80)) / 2,
                             y: ((vertices.map(\.y).min() ?? 0) + (vertices.map(\.y).max() ?? 50)) / 2)
        let span = max(50, (vertices.map(\.x).max() ?? 80) - (vertices.map(\.x).min() ?? 0), (vertices.map(\.y).max() ?? 50) - (vertices.map(\.y).min() ?? 0))
        camera.position = SCNVector3(center.x + span, center.y - span, span)
        camera.look(at: SCNVector3(center.x, center.y, 0)); scene.rootNode.addChildNode(camera)
        view.scene = scene; view.pointOfView = camera
    }
}
