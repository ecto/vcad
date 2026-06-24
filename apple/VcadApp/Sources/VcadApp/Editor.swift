import SwiftUI
import RealityKit
import AppKit
import simd
import CVcadFFI

// Model layer. The SwiftUI shell is in Shell.swift; kernel/streaming in Kernel.swift.

private let kSamplesDir = "/Users/cam/Developer/vcad/.claude/worktrees/elated-mclaren-925de7"

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
    case create, modify
    var id: String { rawValue }
    var label: String { rawValue.capitalized }
    var symbol: String { self == .create ? "plus.square.on.square" : "wand.and.rays" }
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
            selectedFeatureID = source.isSandbox ? "modifier" : "part0"
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
        didSet { if source.isSandbox { geometryDirty = true } }
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
    @ObservationIgnored private var connectorTarget: Double = 40
    @ObservationIgnored private var lastConnectorTick = 40
    @ObservationIgnored private var lastConnectorOK = true

    deinit {
        if let d = gripperDoc { vcad_doc_free(d) }
        if let m = scrollMonitor { NSEvent.removeMonitor(m) }
    }

    /// Clearance from the connector cutout to the nearest enclosure side wall.
    var connectorMinWall: Double { min(connectorX - 6, 74 - connectorX) }
    var connectorOK: Bool { connectorMinWall >= 6 }
    var showsConnectorHandle: Bool { source.isGripper }
    func connectorHandlePosition() -> SIMD3<Float> { SIMD3(Float(connectorX), 9, 13) }

    func openGripper() {
        jumpConnector(40)
        source = .gripper
    }

    /// Snap the connector with no easing — first load / dev hook.
    func jumpConnector(_ x: Double) {
        let c = min(max(x, connectorRange.lowerBound), connectorRange.upperBound)
        connectorX = c
        connectorTarget = c
        lastConnectorTick = Int(c.rounded())
        lastConnectorOK = connectorOK
    }

    func beginConnectorDrag() { connectorBaseline = connectorTarget }
    var connectorDragBaseline: Double { connectorBaseline }

    /// Set the drag TARGET; the geometry eases toward it (advanceConnector) so
    /// the solved drag tweens to a smooth, weighted settle instead of snapping
    /// to each raw delta.
    func setConnectorX(_ x: Double) {
        connectorTarget = min(max(x, connectorRange.lowerBound), connectorRange.upperBound)
        parameterDirty = true
    }

    /// Ease connectorX one frame toward the target, firing the per-mm detent and
    /// the verdict-flip haptic/chime on the eased value (so they match the
    /// motion). Returns true while still animating — the viewport keeps
    /// re-solving until it settles.
    func advanceConnector() -> Bool {
        let diff = connectorTarget - connectorX
        let animating = abs(diff) >= 0.05
        connectorX = animating ? connectorX + diff * 0.45 : connectorTarget
        let tick = Int(connectorX.rounded())
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
        return animating
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
        switch tab {
        case .create:
            return BaseShape.allCases.map { shape in
                Tool(id: "shape.\(shape.rawValue)", label: shape.label, symbol: shape.symbol,
                     isActive: baseShape == shape) { [weak self] in self?.baseShape = shape }
            }
        case .modify:
            return Modifier.allCases.map { mod in
                // Fillet/chamfer are no-ops on a sphere (no edges) — surface that.
                let ok = mod == .none || baseShape != .sphere
                return Tool(id: "mod.\(mod.rawValue)", label: mod.label, symbol: mod.symbol,
                            isActive: modifier == mod, enabled: ok,
                            hint: ok ? "" : "No edges on a sphere") { [weak self] in self?.modifier = mod }
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
        guard let data = try? Data(contentsOf: URL(fileURLWithPath: path)), !data.isEmpty else {
            return .empty
        }
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
        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
        var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
        var tris = 0
        for i in 0..<count {
            let km = KernelMesh.fromView(vcad_scene_part_mesh(scene, i))
            if km.isEmpty { continue }
            lo = simd_min(lo, km.minBound); hi = simd_max(hi, km.maxBound)
            tris += km.triangleCount
            meshes.append((km.resource(name: "part\(i)"), Self.partColors[i % Self.partColors.count]))
        }
        guard !meshes.isEmpty else { return .empty }

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
