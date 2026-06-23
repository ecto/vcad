import SwiftUI
import RealityKit
import AppKit
import simd
import CoreGraphics

// The shell: tool palette + feature tree │ viewport │ inspector, over a dense
// native status bar. Liquid Glass throughout; the tool palette is the native
// reinterpretation of the web app's Borland tabbed tool picker (same model,
// native skin). World-class 3D-tool layout: a full-bleed studio canvas with
// floating frosted-glass panels over it, not opaque columns.

/// A floating frosted-glass panel over the studio viewport.
private struct GlassCard: ViewModifier {
    var cornerRadius: CGFloat = 16
    func body(content: Content) -> some View {
        content
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: cornerRadius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: cornerRadius, style: .continuous)
                    .strokeBorder(.white.opacity(0.10), lineWidth: 0.5)
            )
            .shadow(color: .black.opacity(0.38), radius: 16, y: 7)
    }
}
extension View {
    func glassCard(_ cornerRadius: CGFloat = 16) -> some View { modifier(GlassCard(cornerRadius: cornerRadius)) }
}

struct EditorView: View {
    @State private var model = EditorModel()

    var body: some View {
        ViewportView(model: model)
            .ignoresSafeArea()
            .overlay(alignment: .topLeading) {
                FeatureTreeView(model: model)
                    .frame(width: 206)
                    .padding(14)
            }
            .overlay(alignment: .top) {
                if model.source.isSandbox {
                    ToolPaletteView(model: model).padding(.top, 14)
                }
            }
            .overlay(alignment: .topTrailing) {
                InspectorView(model: model)
                    .frame(width: 280)
                    .padding(14)
            }
            .overlay(alignment: .bottom) {
                StatusBarView(model: model).padding(14)
            }
            .toolbar {
                ToolbarItem(placement: .principal) { SourcePicker(model: model) }
            }
            .navigationTitle("vcad")
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
        }
        .padding(.horizontal, 10).padding(.vertical, 6)
        .glassCard(13)
    }
}

struct FeatureTreeView: View {
    @Bindable var model: EditorModel
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("HISTORY")
                .font(.system(size: 10, weight: .semibold))
                .tracking(0.6)
                .foregroundStyle(.tertiary)
                .padding(.horizontal, 8).padding(.top, 6).padding(.bottom, 4)
            ForEach(model.features) { f in
                let selected = model.selectedFeatureID == f.id
                Button { model.selectedFeatureID = f.id } label: {
                    HStack(spacing: 8) {
                        Image(systemName: f.symbol).font(.system(size: 13)).frame(width: 16)
                        Text(f.name).font(.system(size: 13))
                        Spacer(minLength: 0)
                    }
                    .padding(.horizontal, 8).padding(.vertical, 6)
                    .background(selected ? Color.accentColor.opacity(0.22) : .clear,
                                in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                    .foregroundStyle(selected ? Color.primary : Color.secondary)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
        .padding(6)
        .glassCard()
    }
}

struct InspectorView: View {
    @Bindable var model: EditorModel
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if let f = model.selectedFeature {
                section(f.name) {
                    switch f.kind {
                    case .base:
                        row("Shape", model.baseShape.label)
                    case .modifier:
                        if model.modifier == .none {
                            Text("No modifier").font(.system(size: 12)).foregroundStyle(.secondary)
                        } else {
                            VStack(alignment: .leading, spacing: 8) {
                                HStack {
                                    Text(model.modifier.paramLabel).font(.system(size: 12))
                                    Spacer()
                                    Text(String(format: "%.1f mm", model.modifierValue))
                                        .font(.system(size: 12).monospacedDigit())
                                        .foregroundStyle(.secondary)
                                }
                                Slider(value: $model.modifierValue, in: 0...12)
                            }
                        }
                    case .part:
                        row("Type", "Solid")
                    }
                }
            }
            section("Measurements") {
                row("Triangles", model.triangleCount.formatted())
                row("Bounds", boundsText)
                row("Solve", String(format: "%.1f ms", model.solveMillis))
            }
            if let info = model.pickInfo {
                section("Picked") {
                    Text(info).font(.system(size: 12).monospacedDigit()).foregroundStyle(.secondary)
                }
            }
        }
        .padding(14)
        .glassCard()
    }

    @ViewBuilder private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(title.uppercased())
                .font(.system(size: 10, weight: .semibold))
                .tracking(0.6)
                .foregroundStyle(.tertiary)
            content()
        }
    }

    private func row(_ key: String, _ value: String) -> some View {
        HStack {
            Text(key).font(.system(size: 12))
            Spacer()
            Text(value).font(.system(size: 12).monospacedDigit()).foregroundStyle(.secondary)
        }
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
        HStack(spacing: 11) {
            HStack(spacing: 5) { Image(systemName: "cube.transparent"); Text(model.source.label) }
            bar
            Text("\(model.triangleCount.formatted()) tris")
            bar
            Text(String(format: "%.0f × %.0f × %.0f mm", abs(model.sizeMM.x), abs(model.sizeMM.y), abs(model.sizeMM.z)))
            bar
            Text(String(format: "%.1f ms", model.solveMillis))
            if let info = model.pickInfo {
                bar
                HStack(spacing: 5) { Image(systemName: "scope"); Text(info.replacingOccurrences(of: "\n", with: " · ")) }
            }
            bar
            HStack(spacing: 5) { Circle().fill(.green).frame(width: 5, height: 5); Text("kernel") }
        }
        .font(.system(size: 11, design: .monospaced))
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .padding(.horizontal, 14).padding(.vertical, 7)
        .glassCard(11)
    }
    private var bar: some View { Rectangle().fill(.secondary.opacity(0.25)).frame(width: 1, height: 11) }
}

struct ViewportView: View {
    let model: EditorModel

    var body: some View {
        _ = (model.azimuth, model.elevation, model.distance, model.modifierValue,
             model.source, model.selectedFeatureID, model.baseShape, model.modifier, model.pickDirty)

        return GeometryReader { geo in
          RealityView { content in
            if let env = makeStudioEnvironment() { content.environment = .skybox(env) }
            setupScene(content)
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
          .background(
              RadialGradient(colors: [Color(white: 0.10), Color(white: 0.015)],
                             center: .center, startRadius: 40, endRadius: 800)
          )
          .highPriorityGesture(handleDrag)
          .gesture(orbitGesture)
          .simultaneousGesture(zoomGesture)
          .gesture(SpatialTapGesture(coordinateSpace: .local).onEnded { value in
              pick(at: value.location, viewSize: geo.size)
          })
        }
    }

    // MARK: scene

    private func setupScene(_ content: RealityViewCameraContent) {
        let camera = Entity()
        camera.name = "camera"
        camera.components.set(PerspectiveCameraComponent())
        camera.position = model.cameraPosition
        camera.look(at: .zero, from: model.cameraPosition, relativeTo: nil)
        content.add(camera)

        // Key light with a soft grounding shadow.
        let key = DirectionalLight()
        key.light.intensity = 5500
        key.shadow = DirectionalLightComponent.Shadow(maximumDistance: 4, depthBias: 2)
        key.look(at: .zero, from: [0.7, 1.1, 0.85], relativeTo: nil)
        content.add(key)

        // Cool rim for silhouette separation.
        let rim = DirectionalLight()
        rim.light.intensity = 2600
        rim.look(at: .zero, from: [-0.9, 0.35, -1.0], relativeTo: nil)
        content.add(rim)
    }

    /// A dark studio environment drawn procedurally (no bundled HDR), used for
    /// both the skybox backdrop and image-based reflections on the geometry.
    private func makeStudioEnvironment() -> EnvironmentResource? {
        let w = 1024, h = 512
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        // Vertical gradient: a faintly warm zenith down to a near-black floor.
        let base = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0.12, green: 0.13, blue: 0.16, alpha: 1),
            CGColor(red: 0.06, green: 0.07, blue: 0.09, alpha: 1),
            CGColor(red: 0.015, green: 0.015, blue: 0.02, alpha: 1),
        ] as CFArray, locations: [0, 0.55, 1])!
        ctx.drawLinearGradient(base, start: CGPoint(x: 0, y: CGFloat(h)),
                               end: CGPoint(x: 0, y: 0), options: [])
        // Soft broad key glow → a gentle specular sweep across metal.
        ctx.setBlendMode(.plusLighter)
        let glow = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0.42, green: 0.48, blue: 0.58, alpha: 1),
            CGColor(red: 0.42, green: 0.48, blue: 0.58, alpha: 0),
        ] as CFArray, locations: [0, 1])!
        ctx.drawRadialGradient(glow,
            startCenter: CGPoint(x: CGFloat(w) * 0.34, y: CGFloat(h) * 0.74), startRadius: 0,
            endCenter: CGPoint(x: CGFloat(w) * 0.34, y: CGFloat(h) * 0.74), endRadius: CGFloat(h) * 0.55,
            options: [])
        guard let img = ctx.makeImage() else { return nil }
        return try? EnvironmentResource(equirectangular: img)
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

        // Grounding floor that catches the soft contact shadow.
        content.entities.filter { $0.name == "floor" }.forEach { $0.removeFromParent() }
        let floorY = -(model.sizeMM.z * 0.5 * sceneScale) - 0.004
        var floorMat = PhysicallyBasedMaterial()
        floorMat.baseColor = .init(tint: NSColor(white: 0.03, alpha: 1.0))
        floorMat.roughness = 0.55
        floorMat.metallic = 0.0
        let floor = ModelEntity(mesh: .generatePlane(width: 8, depth: 8), materials: [floorMat])
        floor.name = "floor"
        floor.position = [0, floorY, 0]
        content.add(floor)
    }

    private func material(_ color: NSColor) -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: color)
        m.roughness = 0.34
        m.metallic = 0.55
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
