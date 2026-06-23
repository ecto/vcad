import SwiftUI
import RealityKit
import AppKit
import simd
import CVcadFFI

// M1: interactive native viewport. Two sources of geometry, both driven by the
// real vcad Rust kernel over the C ABI:
//   • Fillet demo — a cube re-filleted + re-tessellated live as you scrub.
//   • Real .vcad documents — parsed + evaluated by vcad_eval::evaluate_document
//     and auto-fit into view.

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

@MainActor
@Observable
final class EditorModel {
    // Orbit camera (radians / scene meters).
    var azimuth: Float = .pi / 5
    var elevation: Float = .pi / 7
    var distance: Float = 1.5

    // Geometry source + the hero parameter.
    var source: GeometrySource = .filletDemo { didSet { geometryDirty = true } }
    var filletRadius: Double = 3.0 {
        didSet {
            // In the demo, stream into the existing GPU buffers (the hot loop);
            // otherwise fall back to a full rebuild.
            if source.isDemo { parameterDirty = true } else { geometryDirty = true }
            // Trackpad haptic detent on each whole-millimetre crossing.
            if Int(filletRadius.rounded()) != Int(oldValue.rounded()) {
                NSHapticFeedbackManager.defaultPerformer.perform(.alignment, performanceTime: .default)
            }
        }
    }

    // Live readouts.
    var triangleCount: Int = 0
    var partCount: Int = 1
    var solveMillis: Double = 0

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

    var cameraPosition: SIMD3<Float> {
        let r = distance
        return SIMD3<Float>(
            r * cos(elevation) * sin(azimuth),
            r * sin(elevation),
            r * cos(elevation) * cos(azimuth)
        )
    }

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
        if filletRadius > 0.05, let filleted = vcad_solid_fillet(base, filletRadius) {
            target = filleted; owned = filleted
        }
        defer { if let o = owned { vcad_solid_free(o) } }

        guard let mesh = vcad_solid_to_mesh(target, 48) else { return nil }
        defer { vcad_mesh_free(mesh) }

        let km = KernelMesh.fromView(vcad_mesh_view(mesh))
        solveMillis = Date().timeIntervalSince(start) * 1000
        triangleCount = km.triangleCount
        partCount = 1
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

struct EditorView: View {
    @State private var model = EditorModel()

    var body: some View {
        // Register observation dependencies so the RealityView `update:`
        // closure re-runs when any of these change.
        _ = (model.azimuth, model.elevation, model.distance, model.filletRadius, model.source)

        return ZStack {
            Color.black.ignoresSafeArea()

            RealityView { content in
                addLightsAndCamera(content)
                rebuildGeometry(content)
                model.geometryDirty = false
            } update: { content in
                if let camera = content.entities.first(where: { $0.name == "camera" }) {
                    camera.position = model.cameraPosition
                    camera.look(at: .zero, from: model.cameraPosition, relativeTo: nil)
                }
                if model.geometryDirty {
                    rebuildGeometry(content)
                    model.geometryDirty = false
                    model.parameterDirty = false
                } else if model.parameterDirty {
                    let recreated = model.streamFillet()
                    if recreated, let res = model.streaming.resource,
                       let root = content.entities.first(where: { $0.name == "geomRoot" }),
                       let entity = root.findEntity(named: "part0") as? ModelEntity {
                        entity.model?.mesh = res
                    }
                    model.parameterDirty = false
                }
            }
            .gesture(orbitGesture)
            .simultaneousGesture(zoomGesture)

            sourcePicker
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
                .padding(.top, 14)

            statsPanel
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(16)

            if model.source.isDemo {
                filletPanel
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottom)
                    .padding(.bottom, 28)
            }
        }
    }

    // MARK: scene

    private func addLightsAndCamera(_ content: RealityViewCameraContent) {
        let camera = Entity()
        camera.name = "camera"
        camera.components.set(PerspectiveCameraComponent())
        camera.position = model.cameraPosition
        camera.look(at: .zero, from: model.cameraPosition, relativeTo: nil)
        content.add(camera)

        let key = DirectionalLight()
        key.light.intensity = 7000
        key.look(at: .zero, from: [1.0, 1.4, 1.0], relativeTo: nil)
        content.add(key)

        let fill = DirectionalLight()
        fill.light.intensity = 2200
        fill.look(at: .zero, from: [-1.0, 0.4, -0.6], relativeTo: nil)
        content.add(fill)
    }

    private func rebuildGeometry(_ content: RealityViewCameraContent) {
        content.entities.filter { $0.name == "geomRoot" }.forEach { $0.removeFromParent() }

        let scene = model.buildScene()
        let sceneScale = 0.6 / max(scene.size, 0.0001)

        let centering = Entity()
        centering.position = -scene.center  // kernel units, before rotation/scale
        for (i, item) in scene.meshes.enumerated() {
            var mat = PhysicallyBasedMaterial()
            mat.baseColor = .init(tint: item.color)
            mat.roughness = 0.30
            mat.metallic = 0.06
            let entity = ModelEntity(mesh: item.mesh, materials: [mat])
            entity.name = "part\(i)"
            centering.addChild(entity)
        }

        let zUp = Entity()
        zUp.addChild(centering)
        zUp.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])  // kernel Z-up -> Y-up

        let geomRoot = Entity()
        geomRoot.name = "geomRoot"
        geomRoot.addChild(zUp)
        geomRoot.scale = SIMD3<Float>(repeating: sceneScale)
        content.add(geomRoot)
    }

    // MARK: gestures

    private var orbitGesture: some Gesture {
        DragGesture()
            .onChanged { value in
                let dx = Float(value.translation.width - model.lastDrag.width)
                let dy = Float(value.translation.height - model.lastDrag.height)
                model.azimuth -= dx * 0.01
                model.elevation = max(-1.45, min(1.45, model.elevation + dy * 0.01))
                model.lastDrag = value.translation
            }
            .onEnded { _ in model.lastDrag = .zero }
    }

    private var zoomGesture: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                model.distance = max(0.45, min(5.0, model.pinchBaseline / Float(value.magnification)))
            }
            .onEnded { _ in model.pinchBaseline = model.distance }
    }

    // MARK: chrome

    private var sourcePicker: some View {
        Picker("", selection: Binding(get: { model.source }, set: { model.source = $0 })) {
            ForEach(model.samples) { s in Text(s.label).tag(s) }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(maxWidth: 460)
        .padding(6)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var filletPanel: some View {
        HStack(spacing: 14) {
            Image(systemName: "circle.dashed").foregroundStyle(.cyan)
            Text("Fillet").font(.callout.weight(.semibold))
            Slider(value: Binding(get: { model.filletRadius }, set: { model.filletRadius = $0 }), in: 0...12)
            Text(String(format: "%.1f mm", model.filletRadius))
                .font(.callout.monospacedDigit())
                .frame(width: 66, alignment: .trailing)
        }
        .padding(.horizontal, 18)
        .padding(.vertical, 14)
        .frame(maxWidth: 480)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 18, style: .continuous).strokeBorder(.white.opacity(0.08), lineWidth: 1))
    }

    private var statsPanel: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text("vcad kernel").font(.caption.weight(.bold)).foregroundStyle(.cyan)
            Text(model.partCount == 1 ? "1 part" : "\(model.partCount) parts")
                .font(.caption2.monospacedDigit())
            Text("\(model.triangleCount) triangles").font(.caption2.monospacedDigit())
            Text(String(format: "solve %.1f ms", model.solveMillis)).font(.caption2.monospacedDigit())
        }
        .foregroundStyle(.white.opacity(0.9))
        .padding(.horizontal, 12)
        .padding(.vertical, 9)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}
