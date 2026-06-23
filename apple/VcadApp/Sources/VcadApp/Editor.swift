import SwiftUI
import RealityKit
import AppKit
import simd
import CVcadFFI

// Model layer for the editor. The SwiftUI shell (three-pane window) lives in
// Shell.swift; the kernel/streaming helpers in Kernel.swift.

private let kSamplesDir = "/Users/cam/Developer/vcad/.claude/worktrees/elated-mclaren-925de7"

enum GeometrySource: Hashable, Identifiable {
    case filletDemo
    case document(path: String, label: String)

    var id: String {
        switch self {
        case .filletDemo: return "fillet-demo"
        case .document(let path, _): return path
        }
    }
    var label: String {
        switch self {
        case .filletDemo: return "Fillet demo"
        case .document(_, let label): return label
        }
    }
    var isDemo: Bool { if case .filletDemo = self { return true }; return false }
}

/// A node in the feature tree (the parametric DAG, shown in the sidebar).
struct Feature: Identifiable, Hashable {
    enum Kind { case box, fillet, part }
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

    // Geometry source + the hero parameter.
    var source: GeometrySource = .filletDemo {
        didSet {
            geometryDirty = true
            selectedFeatureID = source.isDemo ? "fillet" : "part0"
        }
    }
    var filletRadius: Double = 3.0 {
        didSet {
            if source.isDemo { parameterDirty = true } else { geometryDirty = true }
            if Int(filletRadius.rounded()) != Int(oldValue.rounded()) {
                NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .default)
            }
        }
    }

    // Selection binds tree -> inspector AND, in the demo, rolls the model
    // back/forward through history ("Box" shows the fillet's input).
    var selectedFeatureID: String? = "fillet" {
        didSet { if source.isDemo { geometryDirty = true } }
    }

    /// Radius actually applied — rolling history back to "Box" shows the bare cube.
    private var effectiveFilletRadius: Double {
        (source.isDemo && selectedFeatureID == "box") ? 0.0 : filletRadius
    }

    // Live readouts surfaced in the inspector.
    var triangleCount: Int = 0
    var partCount: Int = 1
    var solveMillis: Double = 0
    var sizeMM: SIMD3<Float> = .zero

    var geometryDirty = true
    var parameterDirty = false
    let streaming = StreamingMesh()
    var lastDrag: CGSize = .zero
    var pinchBaseline: Float = 1.5

    let cubeSize: Double = 30.0

    let samples: [GeometrySource] = [
        .filletDemo,
        .document(path: "\(kSamplesDir)/mecheval/tasks/a6-pulley-01.vcad", label: "Pulley"),
        .document(path: "\(kSamplesDir)/mecheval/tasks/a4-counterbore-plate-01.vcad", label: "Counterbore"),
        .document(path: "\(kSamplesDir)/mecheval/tasks/a5-ribbed-plate-01.vcad", label: "Ribbed plate"),
    ]

    // The fillet demo's base cube lives for the model's lifetime.
    nonisolated(unsafe) private var baseSolid: OpaquePointer?

    init() { baseSolid = vcad_solid_cube(cubeSize, cubeSize, cubeSize) }
    deinit { if let s = baseSolid { vcad_solid_free(s) } }

    // MARK: feature tree

    var features: [Feature] {
        switch source {
        case .filletDemo:
            return [
                Feature(id: "box", name: "Box", symbol: "cube", kind: .box),
                Feature(id: "fillet", name: "Fillet", symbol: "square.on.circle.fill", kind: .fillet),
            ]
        case .document(_, let label):
            let n = max(partCount, 1)
            return (0..<n).map { i in
                Feature(id: "part\(i)",
                        name: n == 1 ? label : "Part \(i + 1)",
                        symbol: "cube.transparent",
                        kind: .part)
            }
        }
    }
    var selectedFeature: Feature? { features.first { $0.id == selectedFeatureID } }

    var cameraPosition: SIMD3<Float> {
        let r = distance
        return SIMD3<Float>(
            r * cos(elevation) * sin(azimuth),
            r * sin(elevation),
            r * cos(elevation) * cos(azimuth)
        )
    }

    // MARK: scene building

    func buildScene() -> RenderScene {
        switch source {
        case .filletDemo: return filletScene()
        case .document(let path, _): return documentScene(path: path)
        }
    }

    private func filletScene() -> RenderScene {
        guard let km = filletKernelMesh() else { return .empty }
        streaming.update(from: km)
        guard let res = streaming.resource else { return .empty }
        return RenderScene(
            meshes: [(res, Self.heroColor)],
            center: (km.minBound + km.maxBound) / 2,
            size: Self.extent(km.minBound, km.maxBound),
            triangleCount: km.triangleCount, partCount: 1
        )
    }

    /// Solve the fillet demo into a KernelMesh and record stats.
    private func filletKernelMesh() -> KernelMesh? {
        guard let base = baseSolid else { return nil }
        let start = Date()
        var target = base
        var owned: OpaquePointer?
        if effectiveFilletRadius > 0.05, let filleted = vcad_solid_fillet(base, effectiveFilletRadius) {
            target = filleted; owned = filleted
        }
        defer { if let o = owned { vcad_solid_free(o) } }

        guard let mesh = vcad_solid_to_mesh(target, 48) else { return nil }
        defer { vcad_mesh_free(mesh) }

        let km = KernelMesh.fromView(vcad_mesh_view(mesh))
        solveMillis = Date().timeIntervalSince(start) * 1000
        triangleCount = km.triangleCount
        partCount = 1
        sizeMM = km.maxBound - km.minBound
        return km
    }

    /// Hot path: re-solve the fillet and stream into the GPU buffers in place.
    /// Returns true if the LowLevelMesh was recreated (caller reassigns the resource).
    func streamFillet() -> Bool {
        guard let km = filletKernelMesh() else { return false }
        return streaming.update(from: km)
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
        return RenderScene(meshes: meshes, center: (lo + hi) / 2,
                           size: Self.extent(lo, hi), triangleCount: tris, partCount: meshes.count)
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
