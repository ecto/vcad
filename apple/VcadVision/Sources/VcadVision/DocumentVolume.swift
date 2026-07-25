import SwiftUI
import RealityKit
import UniformTypeIdentifiers
import CVcadFFI

/// The volumetric viewport: the full document scene (parts, assembly
/// instances, pattern instancing, materials, feature edges, selection
/// highlight, transform gizmo) floating in the volume, orbitable by
/// pinch-drag and selectable by look-and-tap. The visionOS twin of the mac
/// app's ViewportView/ReleasedARView — same EditorModel, same shared builders,
/// no camera (the person is the camera).
struct DocumentVolume: View {
    @Bindable var model: EditorModel
    @Bindable var intent: IntentEngine
    @State private var spinBase: Float = 0
    /// Yaw applied by pinch-drag, radians. Lives here (not in EditorModel):
    /// azimuth/elevation drive a camera that doesn't exist in a volume.
    @State private var spin: Float = 0
    @State private var importing = false

    var body: some View {
        RealityView { content in
            let root = Entity()
            root.name = "volumeRoot"
            content.add(root)
            rebuild(root)
        } update: { content in
            guard let root = content.entities.first(where: { $0.name == "volumeRoot" }) else { return }
            if root.components[GeometryKey.self]?.key != geometryKey {
                rebuild(root)
            } else if model.docParamDirty {
                // Parameter scrub: re-evaluate and swap meshes in place so the
                // part doesn't pop through a full rebuild (the volumetric twin
                // of the studio's docParamDirty path).
                refreshGeometry()
            }
            if model.sketchDirty { refreshSketchInk() }
            root.orientation = simd_quatf(angle: spin, axis: [0, 1, 0])
        }
        .gesture(orbitGesture)
        .gesture(
            SpatialTapGesture()
                .targetedToAnyEntity()
                .onEnded { value in tapSelect(value.entity, additive: false) }
        )
        .gesture(
            // No modifier keys in a volume: a long press is the ⌘-click —
            // it toggles the part in the boolean multi-selection.
            LongPressGesture(minimumDuration: 0.45)
                .targetedToAnyEntity()
                .onEnded { value in tapSelect(value.entity, additive: true) }
        )
        .ornament(attachmentAnchor: .scene(.leading), contentAlignment: .trailing) {
            VStack(alignment: .leading, spacing: 10) {
                OpenDocumentButton(importing: $importing)
                FeatureTreeView(model: model)
                    .frame(width: 230)
            }
            .padding(12)
            .glassBackgroundEffect()
        }
        .ornament(attachmentAnchor: .scene(.trailing), contentAlignment: .leading) {
            Group {
                if model.source.isGripper {
                    ReceiptLedger(model: model)
                } else {
                    InspectorView(model: model)
                }
            }
            .frame(width: 300)
            .padding(12)
            .glassBackgroundEffect()
        }
        .ornament(attachmentAnchor: .scene(.top), contentAlignment: .bottom) {
            ToolPaletteView(model: model)
                .padding(10)
                .glassBackgroundEffect()
        }
        .ornament(attachmentAnchor: .scene(.bottom), contentAlignment: .top) {
            VStack(spacing: 10) {
                if model.sketching {
                    SketchHintBar(model: model)
                    SketchPaletteView(model: model)
                } else {
                    if model.timeline != nil {
                        PlaybackBar(model: model)
                    }
                    if model.source.isSandbox && intent.draft.isEmpty && !intent.isThinking {
                        ExampleChips(intent: intent)
                    }
                    ComposerBar(engine: intent, model: model)
                }
            }
            .padding(12)
            .glassBackgroundEffect()
            .animation(Motion.smooth, value: intent.draft.isEmpty)
            .animation(Motion.smooth, value: model.timeline == nil)
        }
        .fileImporter(isPresented: $importing,
                      allowedContentTypes: [UTType(filenameExtension: "vcad") ?? .json]) { result in
            // visionOS has no NSOpenPanel; the document picker is the port.
            guard case let .success(url) = result else { return }
            let scoped = url.startAccessingSecurityScopedResource()
            defer { if scoped { url.stopAccessingSecurityScopedResource() } }
            model.openDocument(url)
        }
    }

    /// Everything that requires rebuilding the entity tree. Parameter scrubs
    /// deliberately stay OUT of this (they ride the in-place swap above).
    private var geometryKey: String {
        "\(model.baseShape)|\(model.modifier)|\(model.modifierValue)|\(model.triangleCount)|\(model.highlightedParts.sorted())|\(model.multiSelectedParts.sorted())|\(model.hiddenParts.sorted())|\(String(describing: model.isolatedPart))|\(model.sketching)|\(model.playbackTime)|\(model.showsGizmo)"
    }

    private var orbitGesture: some Gesture {
        DragGesture()
            .targetedToAnyEntity()
            .onChanged { value in
                spin = spinBase + Float(value.translation.width) * 0.008
            }
            .onEnded { _ in spinBase = spin }
    }

    /// Look-and-tap a part to select it; long-press toggles it in the boolean
    /// multi-selection — the volumetric twin of the studio's pick + ⌘-click.
    private func tapSelect(_ entity: Entity, additive: Bool) {
        guard model.usesDocumentTree else { return }
        var e: Entity? = entity
        while let cur = e, !(cur.name.hasPrefix("part") || cur.name.hasPrefix("inst")) {
            e = cur.parent
        }
        // "part3" and "part3_inst7" both resolve to part index 3.
        guard let hit = e, hit.name.hasPrefix("part") else {
            if !additive { model.deselectAll() }
            return
        }
        let digits = hit.name.dropFirst(4).prefix(while: \.isNumber)
        guard let pi = Int(digits),
              let fid = model.featureNodes.first(where: { $0.partIndex == pi })?.id else { return }
        if additive {
            model.toggleMultiSelect(part: pi, featureID: fid)
        } else {
            model.selectFeature(fid)
        }
        model.selectionDirty = false       // consumed via geometryKey rebuild
    }

    /// In-place re-eval → mesh swap (plain parts and pattern instance children),
    /// so a scrub updates the floating part without a rebuild pop.
    private func refreshGeometry() {
        guard let centering = model.centeringEntity,
              let meshes = model.reevalDocumentMeshes() else { return }
        for (i, m) in meshes.enumerated() {
            guard let pe = centering.findEntity(named: "part\(i)") as? ModelEntity else { continue }
            if pe.model != nil {
                pe.model?.mesh = m
                pe.generateCollisionShapes(recursive: false)
            } else {
                for child in pe.children.compactMap({ $0 as? ModelEntity })
                where child.name.hasPrefix("part\(i)_inst") {
                    child.model?.mesh = m
                    child.generateCollisionShapes(recursive: false)
                }
            }
        }
        // The gizmo rides the part it's anchored to.
        if let c = model.gizmoCenterKernel() {
            centering.findEntity(named: "gizmoRoot")?.position = c
        }
        model.docParamDirty = false
    }

    /// Rebuild the sketch ink imperatively (not through geometryKey) so moving
    /// the cursor never re-tessellates the parts.
    private func refreshSketchInk() {
        guard let centering = model.centeringEntity else { return }
        centering.findEntity(named: "sketchRoot")?.removeFromParent()
        if model.sketching { centering.addChild(buildSketchRoot(model: model)) }
        model.sketchDirty = false
    }

    /// Kernel mm (Z-up) → volume meters (Y-up), auto-fit. Assembly instances
    /// win over plain parts when present (mirrors the web + mac viewports).
    private func rebuild(_ root: Entity) {
        root.children.removeAll()
        root.components.set(GeometryKey(key: geometryKey))
        model.docParamDirty = false

        let scene = model.buildScene()
        let fit = 0.42 / max(scene.size, 0.0001)

        let centering = Entity()
        centering.name = "centering"
        centering.position = -scene.center
        model.centeringEntity = centering        // handle for in-place swaps

        if !scene.instances.isEmpty {
            for inst in scene.instances {
                let e = ModelEntity(mesh: inst.mesh, materials: [pbr(inst.material)])
                e.name = "inst\(inst.index)"
                e.transform = Transform(matrix: inst.transform)
                e.components.set(InputTargetComponent())
                e.generateCollisionShapes(recursive: false)
                centering.addChild(e)
            }
        } else {
            for (i, item) in scene.meshes.enumerated() {
                var m: PhysicallyBasedMaterial = model.usesDocumentTree
                    ? pbr(model.resolvedMaterial(forPart: i))
                    : plain(item.color)
                if model.usesDocumentTree, model.highlightedParts.contains(i) {
                    // Brand orange = action (same emissive lift as mac).
                    m.emissiveColor = .init(color: NSColor(red: 1.0, green: 0.62, blue: 0.12, alpha: 1.0))
                    m.emissiveIntensity = 0.35
                }
                let e = ModelEntity()
                e.name = "part\(i)"
                if let pat = i < scene.instancing.count ? scene.instancing[i] : nil {
                    // Pattern root: one shared MeshResource, N instance
                    // entities with per-copy rigid transforms (same naming as
                    // the studio viewport, so shared helpers keep working).
                    for (j, t) in pat.transforms.enumerated() {
                        let child = ModelEntity(mesh: item.mesh, materials: [m])
                        child.name = "part\(i)_inst\(j)"
                        child.transform = Transform(matrix: t)
                        child.components.set(InputTargetComponent())
                        child.generateCollisionShapes(recursive: false)
                        e.addChild(child)
                    }
                } else {
                    e.model = ModelComponent(mesh: item.mesh, materials: [m])
                    e.components.set(InputTargetComponent())
                    e.generateCollisionShapes(recursive: false)
                }
                e.isEnabled = model.isPartVisible(i)
                centering.addChild(e)

                // CAD edge overlay: crisp feature edges over the shading, width
                // proportional to the scene so it reads the same at any size.
                if i < scene.edges.count,
                   let ribbon = EdgeOverlay.ribbonResource(
                       segments: scene.edges[i],
                       width: max(scene.size * 0.0016, 0.02),
                       name: "edges\(i)") {
                    let edges = ModelEntity(mesh: ribbon, materials: [Self.edgeMaterial])
                    edges.name = "edges\(i)"
                    edges.isEnabled = model.isPartVisible(i)
                    centering.addChild(edges)
                }
            }
        }

        if model.sketching { centering.addChild(buildSketchRoot(model: model)) }
        if model.showsGizmo { centering.addChild(buildGizmo(model: model)) }

        let zUp = Entity()
        zUp.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])
        zUp.addChild(centering)

        let fitE = Entity()
        fitE.scale = SIMD3<Float>(repeating: fit)
        fitE.addChild(zUp)
        root.addChild(fitE)
    }

    private func pbr(_ r: ResolvedMaterial) -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: r.color)
        m.roughness = .init(floatLiteral: r.roughness)
        m.metallic = .init(floatLiteral: r.metallic)
        return m
    }

    private func plain(_ c: NSColor) -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: c)
        m.roughness = 0.34
        m.metallic = 0.55
        return m
    }

    private static let edgeMaterial = UnlitMaterial(color: NSColor(white: 0.09, alpha: 1.0))
}

/// Tags the entity tree with the model state it was built from.
struct GeometryKey: Component {
    var key: String
}

/// visionOS document open — the port of the mac app's NSOpenPanel path.
struct OpenDocumentButton: View {
    @Binding var importing: Bool

    var body: some View {
        Button {
            importing = true
        } label: {
            Label("Open…", systemImage: "folder")
                .font(.system(size: 13))
                .frame(maxWidth: .infinity)
        }
        .buttonStyle(.bordered)
        .frame(width: 230)
    }
}
