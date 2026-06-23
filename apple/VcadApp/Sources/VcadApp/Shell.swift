import SwiftUI
import RealityKit
import AppKit
import simd

// The shell: tool palette + feature tree │ viewport │ inspector, over a dense
// native status bar. Liquid Glass throughout; the tool palette is the native
// reinterpretation of the web app's Borland tabbed tool picker (same model,
// native skin).

struct EditorView: View {
    @State private var model = EditorModel()

    var body: some View {
        VStack(spacing: 0) {
            NavigationSplitView {
                FeatureTreeView(model: model)
                    .navigationSplitViewColumnWidth(min: 200, ideal: 240, max: 320)
            } detail: {
                VStack(spacing: 0) {
                    if model.source.isSandbox {
                        ToolPaletteView(model: model)
                    }
                    ViewportView(model: model)
                }
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
            StatusBarView(model: model)
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

// MARK: tool palette (native Borland-model)

struct ToolPaletteView: View {
    @Bindable var model: EditorModel

    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 3) {
                ForEach(Array(ToolTab.allCases.enumerated()), id: \.element.id) { idx, tab in
                    let active = model.toolTab == tab
                    Button { model.toolTab = tab } label: {
                        HStack(spacing: 5) {
                            Image(systemName: tab.symbol).font(.system(size: 12))
                            Text(tab.label).font(.system(size: 12, weight: .medium))
                            Text("\(idx + 1)").font(.system(size: 9, design: .monospaced)).opacity(0.5)
                        }
                        .padding(.horizontal, 10).padding(.vertical, 5)
                        .background(active ? Color.accentColor.opacity(0.18) : .clear,
                                    in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                        .foregroundStyle(active ? Color.accentColor : Color.secondary)
                    }
                    .buttonStyle(.plain)
                    .keyboardShortcut(KeyEquivalent(Character(String(idx + 1))), modifiers: [])
                }
            }

            Divider().frame(height: 18)

            HStack(spacing: 6) {
                ForEach(model.tools(for: model.toolTab)) { tool in
                    Button { tool.action() } label: {
                        HStack(spacing: 5) {
                            Image(systemName: tool.symbol).font(.system(size: 12))
                            Text(tool.label).font(.system(size: 12))
                        }
                        .padding(.horizontal, 9).padding(.vertical, 5)
                        .background(tool.isActive ? Color.white.opacity(0.10) : .clear,
                                    in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: 6, style: .continuous)
                                .strokeBorder(tool.isActive ? Color.white.opacity(0.18) : .clear)
                        )
                        .foregroundStyle(tool.isActive ? Color.primary : Color.secondary)
                    }
                    .buttonStyle(.plain)
                }
            }

            Spacer()
        }
        .padding(.horizontal, 12).padding(.vertical, 7)
        .background(.ultraThinMaterial)
        .overlay(alignment: .bottom) { Divider() }
    }
}

struct FeatureTreeView: View {
    @Bindable var model: EditorModel
    var body: some View {
        List(selection: $model.selectedFeatureID) {
            Section("History") {
                ForEach(model.features) { f in
                    Label(f.name, systemImage: f.symbol).tag(f.id)
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
                    case .base:
                        LabeledContent("Shape", value: model.baseShape.label)
                    case .modifier:
                        if model.modifier == .none {
                            Text("No modifier").foregroundStyle(.secondary)
                        } else {
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    Text(model.modifier.paramLabel)
                                    Spacer()
                                    Text(String(format: "%.1f mm", model.modifierValue))
                                        .monospacedDigit().foregroundStyle(.secondary)
                                }
                                Slider(value: $model.modifierValue, in: 0...12)
                            }
                            .padding(.vertical, 2)
                        }
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
            if let info = model.pickInfo {
                Section("Picked") {
                    Text(info).font(.callout.monospacedDigit())
                }
            }
        }
        .formStyle(.grouped)
    }

    private var boundsText: String {
        let s = model.sizeMM
        return String(format: "%.1f × %.1f × %.1f mm", abs(s.x), abs(s.y), abs(s.z))
    }
}

// MARK: status bar (dense, native)

struct StatusBarView: View {
    let model: EditorModel
    var body: some View {
        HStack(spacing: 14) {
            Label(model.source.label, systemImage: "cube.transparent")
            Text(model.partCount == 1 ? "1 part" : "\(model.partCount) parts")
            Text("\(model.triangleCount.formatted()) tris")
            Text(String(format: "%.0f × %.0f × %.0f mm", abs(model.sizeMM.x), abs(model.sizeMM.y), abs(model.sizeMM.z)))
            Text(String(format: "solve %.1f ms", model.solveMillis))
            if let info = model.pickInfo {
                Label(info.replacingOccurrences(of: "\n", with: " · "), systemImage: "scope")
            }
            Spacer()
            HStack(spacing: 5) {
                Circle().fill(.green).frame(width: 6, height: 6)
                Text("kernel")
            }
        }
        .font(.system(size: 11, design: .monospaced))
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .padding(.horizontal, 12)
        .frame(height: 24)
        .background(.ultraThinMaterial)
        .overlay(alignment: .top) { Divider() }
    }
}

struct ViewportView: View {
    let model: EditorModel

    var body: some View {
        _ = (model.azimuth, model.elevation, model.distance, model.modifierValue,
             model.source, model.selectedFeatureID, model.baseShape, model.modifier, model.pickDirty)

        return GeometryReader { geo in
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
                let recreated = model.streamSandbox()
                if let root = content.entities.first(where: { $0.name == "geomRoot" }) {
                    if recreated, let res = model.streaming.resource,
                       let entity = root.findEntity(named: "part0") as? ModelEntity {
                        entity.model?.mesh = res
                    }
                    if model.showsHandle {
                        root.findEntity(named: "filletHandle")?.position =
                            model.handlePosition(radius: model.modifierValue)
                    }
                }
                model.parameterDirty = false
            }
            if model.pickDirty {
                if let root = content.entities.first(where: { $0.name == "geomRoot" }),
                   let centering = root.findEntity(named: "centering") {
                    centering.findEntity(named: "pickMarker")?.removeFromParent()
                    if let p = model.pickPoint {
                        let marker = ModelEntity(
                            mesh: .generateSphere(radius: 1.3),
                            materials: [UnlitMaterial(color: NSColor(red: 1.0, green: 0.62, blue: 0.12, alpha: 1.0))]
                        )
                        marker.name = "pickMarker"
                        marker.position = p
                        centering.addChild(marker)
                    }
                }
                model.pickDirty = false
            }
          }
          .background(.black)
          .highPriorityGesture(handleDrag)
          .gesture(orbitGesture)
          .simultaneousGesture(zoomGesture)
          .gesture(SpatialTapGesture(coordinateSpace: .local).onEnded { value in
              pick(at: value.location, viewSize: geo.size)
          })
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
        centering.name = "centering"
        centering.position = -scene.center
        for (i, item) in scene.meshes.enumerated() {
            let entity = ModelEntity(mesh: item.mesh, materials: [material(item.color)])
            entity.name = "part\(i)"
            centering.addChild(entity)
        }
        if model.showsHandle {
            centering.addChild(makeHandle(radius: model.modifierValue))
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

    private var handleDrag: some Gesture {
        DragGesture()
            .targetedToAnyEntity()
            .onChanged { value in
                guard value.entity.name == "filletHandle" else { return }
                if !model.draggingHandle {
                    model.draggingHandle = true
                    model.handleBaseline = model.modifierValue
                }
                let delta = Double(-value.translation.height) * 0.03
                model.modifierValue = max(0, min(12, model.handleBaseline + delta))
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

    // MARK: picking (#7)

    private func pick(at p: CGPoint, viewSize: CGSize) {
        guard viewSize.width > 1, viewSize.height > 1 else { return }
        let cam = model.cameraPosition
        let forward = normalize(-cam)
        let right = normalize(cross(forward, SIMD3<Float>(0, 1, 0)))
        let up = cross(right, forward)
        let tanHalf = Float(tan(Double.pi / 6.0))   // 60° vertical FOV / 2
        let aspect = Float(viewSize.width / viewSize.height)
        let ndcX = Float(2 * p.x / viewSize.width - 1)
        let ndcY = Float(1 - 2 * p.y / viewSize.height)
        let dirWorld = normalize(forward + ndcX * tanHalf * aspect * right + ndcY * tanHalf * up)

        let s = model.displayScale
        let camKernel = rxPlus90(cam / s) + model.displayCenter
        let dirKernel = normalize(rxPlus90(dirWorld))

        if let hit = model.raycastSandbox(originKernel: camKernel, dirKernel: dirKernel) {
            model.pickPoint = hit.point
            model.pickInfo = describe(hit)
        } else {
            model.pickPoint = nil
            model.pickInfo = nil
        }
        model.pickDirty = true
    }

    private func rxPlus90(_ v: SIMD3<Float>) -> SIMD3<Float> { SIMD3(v.x, -v.z, v.y) }

    private func describe(_ hit: EditorModel.PickHit) -> String {
        let n = hit.normal
        let maxAxis = max(abs(n.x), max(abs(n.y), abs(n.z)))
        let kind = maxAxis > 0.97 ? "Planar face" : "Curved face"
        return String(format: "%@\n(%.1f, %.1f, %.1f) mm", kind, hit.point.x, hit.point.y, hit.point.z)
    }
}
