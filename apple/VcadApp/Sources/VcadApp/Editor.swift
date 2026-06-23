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

    var id: String {
        switch self {
        case .sandbox: return "sandbox"
        case .document(let path, _): return path
        }
    }
    var label: String {
        switch self {
        case .sandbox: return "Sandbox"
        case .document(_, let label): return label
        }
    }
    var isSandbox: Bool { if case .sandbox = self { return true }; return false }
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

@MainActor
@Observable
final class EditorModel {
    // Orbit camera (radians / scene meters).
    var azimuth: Float = .pi / 5
    var elevation: Float = .pi / 7
    var distance: Float = 1.5

    var source: GeometrySource = .sandbox {
        didSet { geometryDirty = true; selectedFeatureID = source.isSandbox ? "modifier" : "part0" }
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
    var lastDrag: CGSize = .zero
    var pinchBaseline: Float = 1.5
    var draggingHandle = false
    var handleBaseline: Double = 0

    let samples: [GeometrySource] = [
        .sandbox,
        .document(path: "\(kSamplesDir)/mecheval/tasks/a6-pulley-01.vcad", label: "Pulley"),
        .document(path: "\(kSamplesDir)/mecheval/tasks/a4-counterbore-plate-01.vcad", label: "Counterbore"),
        .document(path: "\(kSamplesDir)/mecheval/tasks/a5-ribbed-plate-01.vcad", label: "Ribbed plate"),
    ]

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
                Tool(id: "mod.\(mod.rawValue)", label: mod.label, symbol: mod.symbol,
                     isActive: modifier == mod) { [weak self] in self?.modifier = mod }
            }
        }
    }

    // MARK: feature tree

    var features: [Feature] {
        switch source {
        case .sandbox:
            return [
                Feature(id: "base", name: baseShape.label, symbol: baseShape.symbol, kind: .base),
                Feature(id: "modifier", name: modifier == .none ? "No modifier" : modifier.label,
                        symbol: modifier.symbol, kind: .modifier),
            ]
        case .document(_, let label):
            let n = max(partCount, 1)
            return (0..<n).map { i in
                Feature(id: "part\(i)", name: n == 1 ? label : "Part \(i + 1)",
                        symbol: "cube.transparent", kind: .part)
            }
        }
    }
    var selectedFeature: Feature? { features.first { $0.id == selectedFeatureID } }

    /// Whether the grabbable 3D handle is shown (only the cube+fillet case has
    /// a parametric anchor today).
    var showsHandle: Bool {
        source.isSandbox && baseShape == .cube && modifier == .fillet && selectedFeatureID == "modifier"
    }

    var cameraPosition: SIMD3<Float> {
        let r = distance
        return SIMD3<Float>(
            r * cos(elevation) * sin(azimuth),
            r * sin(elevation),
            r * cos(elevation) * cos(azimuth)
        )
    }

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
        }
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

    static let heroColor = NSColor(red: 0.23, green: 0.72, blue: 0.96, alpha: 1.0)
    static let partColors: [NSColor] = [
        NSColor(red: 0.62, green: 0.66, blue: 0.70, alpha: 1.0),
        NSColor(red: 0.82, green: 0.62, blue: 0.30, alpha: 1.0),
        NSColor(red: 0.45, green: 0.62, blue: 0.82, alpha: 1.0),
        NSColor(red: 0.72, green: 0.46, blue: 0.46, alpha: 1.0),
    ]
}
