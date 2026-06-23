import SwiftUI
import RealityKit
import AppKit
import simd

// The three-pane shell: feature tree (sidebar) │ RealityView viewport │
// inspector. One obsessive window; Liquid Glass chrome over live geometry;
// selection binds the tree to the inspector.

struct EditorView: View {
    @State private var model = EditorModel()

    var body: some View {
        NavigationSplitView {
            FeatureTreeView(model: model)
                .navigationSplitViewColumnWidth(min: 200, ideal: 240, max: 320)
        } detail: {
            ViewportView(model: model)
                .ignoresSafeArea()
                .navigationTitle("vcad")
                .navigationSubtitle(model.source.label)
                .toolbar {
                    ToolbarItem(placement: .principal) { SourcePicker(model: model) }
                }
                .inspector(isPresented: .constant(true)) {
                    InspectorView(model: model)
                        .inspectorColumnWidth(min: 260, ideal: 300, max: 380)
                }
        }
    }
}

struct SourcePicker: View {
    @Bindable var model: EditorModel
    var body: some View {
        Picker("Source", selection: $model.source) {
            ForEach(model.samples) { Text($0.label).tag($0) }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .frame(minWidth: 360)
    }
}

struct FeatureTreeView: View {
    @Bindable var model: EditorModel
    var body: some View {
        List(selection: $model.selectedFeatureID) {
            Section("History") {
                ForEach(model.features) { f in
                    Label(f.name, systemImage: f.symbol)
                        .tag(f.id)
                }
            }
        }
        .listStyle(.sidebar)
    }
}

struct InspectorView: View {
    @Bindable var model: EditorModel
    var body: some View {
        Form {
            if let f = model.selectedFeature {
                Section(f.name) {
                    switch f.kind {
                    case .fillet:
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                Text("Radius")
                                Spacer()
                                Text(String(format: "%.1f mm", model.filletRadius))
                                    .monospacedDigit()
                                    .foregroundStyle(.secondary)
                            }
                            Slider(value: $model.filletRadius, in: 0...12)
                        }
                        .padding(.vertical, 2)
                    case .box:
                        LabeledContent("Size",
                            value: String(format: "%.0f × %.0f × %.0f mm",
                                          model.cubeSize, model.cubeSize, model.cubeSize))
                    case .part:
                        LabeledContent("Type", value: "Solid")
                    }
                }
            }
            Section("Measurements") {
                LabeledContent("Triangles", value: model.triangleCount.formatted())
                LabeledContent("Bounds", value: boundsText)
                LabeledContent("Solve", value: String(format: "%.1f ms", model.solveMillis))
            }
        }
        .formStyle(.grouped)
    }

    private var boundsText: String {
        let s = model.sizeMM
        return String(format: "%.1f × %.1f × %.1f mm", s.x, s.y, s.z)
    }
}

struct ViewportView: View {
    let model: EditorModel

    var body: some View {
        // Register observation dependencies so `update:` re-runs on change.
        _ = (model.azimuth, model.elevation, model.distance,
             model.filletRadius, model.source, model.selectedFeatureID)

        return RealityView { content in
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
                if let root = content.entities.first(where: { $0.name == "geomRoot" }) {
                    if recreated, let res = model.streaming.resource,
                       let entity = root.findEntity(named: "part0") as? ModelEntity {
                        entity.model?.mesh = res
                    }
                    root.findEntity(named: "filletHandle")?.position =
                        model.handlePosition(radius: model.filletRadius)
                }
                model.parameterDirty = false
            }
        }
        .background(.black)
        .highPriorityGesture(handleDrag)
        .gesture(orbitGesture)
        .simultaneousGesture(zoomGesture)
        .overlay(alignment: .bottomTrailing) { statsBadge.padding(12) }
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
        centering.position = -scene.center
        for (i, item) in scene.meshes.enumerated() {
            let entity = ModelEntity(mesh: item.mesh, materials: [material(item.color)])
            entity.name = "part\(i)"
            centering.addChild(entity)
        }

        if model.source.isDemo, model.selectedFeatureID == "fillet" {
            centering.addChild(makeHandle(radius: model.filletRadius))
        }

        let zUp = Entity()
        zUp.addChild(centering)
        zUp.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])

        let geomRoot = Entity()
        geomRoot.name = "geomRoot"
        geomRoot.addChild(zUp)
        geomRoot.scale = SIMD3<Float>(repeating: sceneScale)
        content.add(geomRoot)
    }

    private func material(_ color: NSColor) -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: color)
        m.roughness = 0.30
        m.metallic = 0.06
        return m
    }

    /// A glowing, grabbable handle that floats on the filleted edge.
    private func makeHandle(radius: Double) -> ModelEntity {
        let handle = ModelEntity(
            mesh: .generateSphere(radius: 1.8),
            materials: [UnlitMaterial(color: NSColor(red: 0.55, green: 0.95, blue: 1.0, alpha: 1.0))]
        )
        handle.name = "filletHandle"
        handle.position = model.handlePosition(radius: radius)
        handle.components.set(CollisionComponent(shapes: [ShapeResource.generateSphere(radius: 3.0)]))
        handle.components.set(InputTargetComponent())
        return handle
    }

    // MARK: gestures

    /// Grab the glowing handle and drag to scrub the fillet radius in-scene —
    /// the Mac rehearsal for the Vision Pro pinch-scrub. Targeted to the handle
    /// entity, so it only fires when the drag starts on the handle.
    private var handleDrag: some Gesture {
        DragGesture()
            .targetedToAnyEntity()
            .onChanged { value in
                guard value.entity.name == "filletHandle" else { return }
                if !model.draggingHandle {
                    model.draggingHandle = true
                    model.handleBaseline = model.filletRadius
                }
                let delta = Double(-value.translation.height) * 0.03
                model.filletRadius = max(0, min(12, model.handleBaseline + delta))
            }
            .onEnded { _ in model.draggingHandle = false }
    }

    private var orbitGesture: some Gesture {
        DragGesture()
            .onChanged { value in
                guard !model.draggingHandle else { return }
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

    private var statsBadge: some View {
        Text("\(model.triangleCount.formatted()) tris · \(String(format: "%.0f", model.solveMillis)) ms")
            .font(.caption2.monospacedDigit())
            .foregroundStyle(.secondary)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule())
    }
}
