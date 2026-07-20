import AppKit
import RealityKit
import SwiftUI

// Release-to-desktop: RealityView's drawable on macOS always clears opaque, so
// the released experience lives in its OWN borderless transparent window
// hosting an ARView (which supports `environment.background = .color(.clear)`).
// The parts composite straight over the desktop; the main window hides while
// released and comes back on return.
//
// Around the floating parts we open Borland C++ Builder-style tool windows —
// a Component Palette and an Object Inspector — as separate floating panels,
// the way the classic multi-window IDEs scattered themselves over the desktop.

/// Borderless windows refuse key status by default, which starves SwiftUI
/// gestures inside them — opt back in.
final class KeyableWindow: NSWindow {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { true }
}

@MainActor
final class ReleaseWindowController {
    static let shared = ReleaseWindowController()
    private var window: NSWindow?
    private var mainWindow: NSWindow?
    private var mouseMonitors: [Any] = []
    /// Set by ReleasedARView so the pass-through hit test can raycast the scene.
    weak var arView: ARView?
    /// Chrome regions (tool windows, pill) in overlay view coords (top-left
    /// origin), reported by SwiftUI — the overlay owns the mouse over these.
    var chromeRects: [String: CGRect] = [:]

    func show(model: EditorModel) {
        guard window == nil else { return }
        mainWindow = NSApp.keyWindow ?? NSApp.mainWindow

        let frame = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let w = KeyableWindow(contentRect: frame, styleMask: [.borderless],
                              backing: .buffered, defer: false)
        w.isOpaque = false
        w.backgroundColor = .clear
        w.hasShadow = false
        w.level = .normal                       // stacks like any window, not always-on-top
        w.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        w.contentView = NSHostingView(rootView: ReleasedOverlayView(model: model))
        w.makeKeyAndOrderFront(nil)
        window = w
        mainWindow?.orderOut(nil)

        let update: () -> Void = { [weak self] in self?.updatePassThrough() }
        if let m = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved]) { _ in
            Task { @MainActor in update() }
        } { mouseMonitors.append(m) }
        mouseMonitors.append(NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved]) { e in
            Task { @MainActor in update() }
            return e
        } as Any)
        updatePassThrough()
    }

    /// Own the mouse over geometry or chrome; pass everything else through.
    /// Never flips mid-drag (the orbit would be dropped half-way).
    private func updatePassThrough() {
        guard let w = window else { return }
        guard NSEvent.pressedMouseButtons == 0 else { return }
        let winPoint = w.convertPoint(fromScreen: NSEvent.mouseLocation)
        let topLeft = CGPoint(x: winPoint.x, y: w.frame.height - winPoint.y)
        if chromeRects.values.contains(where: { $0.insetBy(dx: -8, dy: -8).contains(topLeft) }) {
            w.ignoresMouseEvents = false
            return
        }
        guard let ar = arView else { return }
        var viewPoint = ar.convert(winPoint, from: nil)
        if !ar.isFlipped { viewPoint.y = ar.bounds.height - viewPoint.y }
        w.ignoresMouseEvents = ar.hitTest(viewPoint).isEmpty
    }

    func hide() {
        mouseMonitors.forEach { NSEvent.removeMonitor($0) }
        mouseMonitors = []
        arView = nil
        chromeRects = [:]
        window?.orderOut(nil)
        window = nil
        mainWindow?.makeKeyAndOrderFront(nil)
        mainWindow = nil
    }
}

/// Reports a chrome element's frame (overlay view coords) to the controller
/// so the pass-through hit test knows the mouse belongs to us there.
struct ChromeRegion: ViewModifier {
    let key: String
    func body(content: Content) -> some View {
        content.onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: {
            ReleaseWindowController.shared.chromeRects[key] = $0
        }
    }
}

/// Full-screen overlay content: transparent ARView + orbit/zoom gestures +
/// the return pill. Reading the model's orbit state here keeps the ARView's
/// camera live (flick momentum included — endOrbit's coast mutates azimuth,
/// which re-renders this view).
struct ReleasedOverlayView: View {
    @Bindable var model: EditorModel

    var body: some View {
        ZStack(alignment: .topLeading) {
            ReleasedARView(model: model,
                           cameraPosition: model.cameraPosition,
                           lookAt: model.panOffset,
                           geometryKey: "\(model.baseShape)|\(model.modifier)|\(model.modifierValue)|\(model.triangleCount)|\(model.zebraMode)|\(model.highlightedParts.sorted())|\(model.hiddenParts.sorted())|\(String(describing: model.isolatedPart))")
                .ignoresSafeArea()
            // ARView consumes mouse events — capture orbit/zoom on a clear
            // layer above it instead.
            Color.clear
                .contentShape(Rectangle())
                .ignoresSafeArea()
                .gesture(orbitGesture)
                .simultaneousGesture(zoomGesture)
                .gesture(SpatialTapGesture(coordinateSpace: .local).modifiers(.command)
                    .onEnded { value in tapSelect(at: value.location, additive: true) })
                .gesture(SpatialTapGesture(coordinateSpace: .local)
                    .onEnded { value in tapSelect(at: value.location, additive: false) })
            // BCB-style tool windows, hosted in the overlay itself (separate
            // NSPanels break SwiftUI hit testing after auto-resize).
            VStack(alignment: .leading, spacing: 12) {
                ComponentPaletteWindow(model: model)
                    .modifier(ChromeRegion(key: "palette"))
                ObjectInspectorWindow(model: model)
                    .modifier(ChromeRegion(key: "inspector"))
                ScrollView {
                    FeatureTreeView(model: model)
                }
                .frame(width: 230)
                .frame(maxHeight: 380)
                .fixedSize(horizontal: false, vertical: true)
                .modifier(ChromeRegion(key: "tree"))
            }
            .padding(16)
        }
        .overlay(alignment: .topTrailing) {
            ReleaseReturnPill(model: model)
                .padding(16)
                .modifier(ChromeRegion(key: "pill"))
        }
    }

    /// Click a part to select it (⌘-click for the boolean multi-selection,
    /// empty space to deselect) — the released twin of the studio's pick.
    private func tapSelect(at p: CGPoint, additive: Bool) {
        guard model.usesDocumentTree,
              let ar = ReleaseWindowController.shared.arView else { return }
        if let e = ar.hitTest(p).first?.entity, e.name.hasPrefix("part"),
           let pi = Int(e.name.dropFirst(4)),
           let fid = model.featureNodes.first(where: { $0.partIndex == pi })?.id {
            // Modifiers come from the gesture (.modifiers(.command)), not
            // NSEvent.modifierFlags — that reads hardware key state and misses
            // synthetic events (and some tap orderings).
            if additive {
                model.toggleMultiSelect(part: pi, featureID: fid)
            } else {
                model.selectFeature(fid)
            }
        } else if !additive {
            model.deselectAll()
        }
        model.selectionDirty = false   // released view rebuilds via geometryKey
    }

    private var orbitGesture: some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { value in
                if model.lastDrag == .zero { model.beginOrbit() }
                let dx = Float(value.translation.width - model.lastDrag.width)
                let dy = Float(value.translation.height - model.lastDrag.height)
                if NSEvent.modifierFlags.contains(.shift) {
                    model.panBy(dx: dx, dy: dy)
                } else {
                    model.orbitDrag(dx: dx, dy: dy)
                }
                model.lastDrag = value.translation
            }
            .onEnded { _ in
                model.lastDrag = .zero
                model.endOrbit()
            }
    }

    private var zoomGesture: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                model.stopSpin()
                model.distance = max(0.45, min(8.0, model.pinchBaseline / Float(value.magnification)))
            }
            .onEnded { _ in model.pinchBaseline = model.distance }
    }
}

/// An ARView with a clear background showing the document's parts, camera and
/// geometry kept in sync with the editor model.
struct ReleasedARView: NSViewRepresentable {
    let model: EditorModel
    let cameraPosition: SIMD3<Float>
    let lookAt: SIMD3<Float>
    let geometryKey: String

    final class Coordinator {
        var camera: PerspectiveCamera?
        var anchor: AnchorEntity?
        var builtKey = ""
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> ARView {
        let ar = ARView(frame: .zero)
        ar.environment.background = .color(.clear)
        ReleaseWindowController.shared.arView = ar
        let anchor = AnchorEntity(world: .zero)
        let camera = PerspectiveCamera()
        anchor.addChild(camera)
        ar.scene.addAnchor(anchor)
        context.coordinator.anchor = anchor
        context.coordinator.camera = camera
        rebuild(in: anchor, coordinator: context.coordinator)
        syncCamera(context.coordinator)
        return ar
    }

    func updateNSView(_ ar: ARView, context: Context) {
        if context.coordinator.builtKey != geometryKey {
            rebuild(in: context.coordinator.anchor, coordinator: context.coordinator)
            // Zebra in released mode: striped IBL on chrome parts, desktop
            // still visible behind (lighting only — never a skybox).
            ar.environment.lighting.resource =
                model.zebraMode ? ViewportView.zebraEnvironment : nil
        }
        syncCamera(context.coordinator)
    }

    private func syncCamera(_ c: Coordinator) {
        guard let camera = c.camera else { return }
        camera.position = cameraPosition
        camera.look(at: lookAt, from: cameraPosition, relativeTo: nil)
    }

    private func rebuild(in anchor: AnchorEntity?, coordinator: Coordinator) {
        guard let anchor else { return }
        coordinator.builtKey = geometryKey
        anchor.children.filter { $0.name == "geomRoot" }.forEach { $0.removeFromParent() }

        let scene = model.buildScene()
        let sceneScale = 0.6 / max(scene.size, 0.0001)

        let centering = Entity()
        centering.position = -scene.center
        for (i, item) in scene.meshes.enumerated() {
            var m = PhysicallyBasedMaterial()
            if model.zebraMode {
                m = ViewportView.zebraChrome
            } else if model.usesDocumentTree {
                let r = model.resolvedMaterial(forPart: i)
                m.baseColor = .init(tint: r.color)
                m.roughness = .init(floatLiteral: r.roughness)
                m.metallic = .init(floatLiteral: r.metallic)
            } else {
                m.baseColor = .init(tint: item.color)
                m.roughness = 0.34
                m.metallic = 0.55
            }
            if model.usesDocumentTree, !model.zebraMode, model.highlightedParts.contains(i) {
                // Brand orange = action: a subtle emissive lift marks selection
                // without repainting the material (mirrors the studio rule).
                m.emissiveColor = .init(color: NSColor(red: 1.0, green: 0.62, blue: 0.12, alpha: 1.0))
                m.emissiveIntensity = 0.35
            }
            let e = ModelEntity(mesh: item.mesh, materials: [m])
            e.name = "part\(i)"
            e.isEnabled = model.isPartVisible(i)
            e.generateCollisionShapes(recursive: false)   // pass-through hit test
            centering.addChild(e)
        }

        let zUp = Entity()
        zUp.addChild(centering)
        zUp.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])

        let geomRoot = Entity()
        geomRoot.name = "geomRoot"
        geomRoot.addChild(zUp)
        geomRoot.scale = SIMD3<Float>(repeating: sceneScale)
        anchor.addChild(geomRoot)
    }
}

// MARK: - Floating tool windows (BCB-style UX, Apple HIG styling)
//
// The C++ Builder idea — the IDE as a constellation of small floating tool
// windows around your work — implemented as native macOS panels: material
// backgrounds, SF Symbols, standard controls.

/// A floating tool window: compact title row (drags the panel) over a
/// material body, rounded and stroked like a native HUD panel.
struct ToolWindow<Content: View>: View {
    let title: String
    let onClose: () -> Void
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                Text(title)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 24)
            }
            .overlay(alignment: .trailing) {
                Button(action: onClose) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 12))
                        .foregroundStyle(.tertiary)
                        .padding(4)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Close")
            }
            content
        }
        .padding(12)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.separator, lineWidth: 1))
        .fixedSize()
    }
}

/// The Component Palette: create + modify sections and a document-action
/// cluster (undo/redo, zebra analysis, export) — the BCB palette, HIG-ified.
struct ComponentPaletteWindow: View {
    @Bindable var model: EditorModel

    var body: some View {
        ToolWindow(title: "Components", onClose: { model.releaseMode = false }) {
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 10) {
                    // Same tabs, same tools, same enable/hint logic as the
                    // in-studio palette: one source of truth (model.tools(for:)).
                    Picker("", selection: $model.toolTab) {
                        ForEach(availableTabs) { t in Text(t.label).tag(t) }
                    }
                    .pickerStyle(.segmented)
                    .labelsHidden()
                    .controlSize(.small)
                    .fixedSize()
                    Divider().frame(height: 22)
                    HStack(spacing: 2) {
                        iconButton("Zebra", "line.3.horizontal", enabled: true,
                                   active: model.zebraMode) { model.zebraMode.toggle() }
                        iconButton("Undo", "arrow.uturn.backward", enabled: model.canUndo) { model.undo() }
                        iconButton("Redo", "arrow.uturn.forward", enabled: model.canRedo) { model.redo() }
                        iconButton("STL", "square.and.arrow.up", enabled: model.canExport) {
                            exportPanel(ext: "stl") { model.exportSTL(to: $0) }
                        }
                        iconButton("USDZ", "arkit", enabled: model.canExport) {
                            exportPanel(ext: "usdz") { model.exportUSDZ(to: $0) }
                        }
                    }
                }
                HStack(spacing: 2) {
                    ForEach(model.tools(for: activeTab)) { tool in
                        toolButton(tool)
                    }
                }
            }
        }
    }

    /// Combine only exists for documents (needs two selected parts).
    private var availableTabs: [ToolTab] {
        model.usesDocumentTree ? ToolTab.allCases : [.create, .modify]
    }
    private var activeTab: ToolTab {
        availableTabs.contains(model.toolTab) ? model.toolTab : .create
    }

    private func toolButton(_ tool: Tool) -> some View {
        Button(action: tool.action) {
            VStack(spacing: 3) {
                Image(systemName: tool.symbol).font(.system(size: 14))
                Text(tool.label).font(.system(size: 9)).lineLimit(1)
            }
            .frame(minWidth: 52, minHeight: 40)
            .padding(.horizontal, 4)
            .foregroundStyle(tool.isActive ? Color.accentColor
                             : tool.enabled ? Color.primary : Color.secondary.opacity(0.4))
            .background(tool.isActive ? AnyShapeStyle(.selection) : AnyShapeStyle(.clear),
                        in: RoundedRectangle(cornerRadius: 7))
            // .plain buttons only hit-test opaque pixels; without this, clicks
            // in the transparent padding between glyph and label do nothing.
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!tool.enabled)
        .help(tool.hint.isEmpty ? tool.label : tool.hint)
    }

    private func exportPanel(ext: String, _ export: @escaping (URL) -> Bool) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(model.documentName).\(ext)"
        panel.begin { resp in
            guard resp == .OK, let url = panel.url else { return }
            _ = export(url)
        }
    }

    private func iconButton(_ label: String, _ symbol: String, enabled: Bool,
                            active: Bool = false,
                            action: @escaping () -> Void) -> some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: symbol).font(.system(size: 14))
                Text(label).font(.system(size: 9))
            }
            .frame(width: 52, height: 40)
            .foregroundStyle(active ? Color.accentColor
                             : enabled ? Color.primary : Color.secondary.opacity(0.4))
            .background(active ? AnyShapeStyle(.selection) : AnyShapeStyle(.clear),
                        in: RoundedRectangle(cornerRadius: 7))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }
}

/// The Object Inspector: parameters, camera, material, and measurements.
struct ObjectInspectorWindow: View {
    @Bindable var model: EditorModel

    var body: some View {
        ToolWindow(title: "Object Inspector", onClose: { model.releaseMode = false }) {
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                row("Shape", model.baseShape.label)
                row("Modifier", model.modifier.label)
                GridRow {
                    Text(model.modifier.paramLabel)
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                    HStack(spacing: 6) {
                        Slider(value: $model.modifierValue, in: 0...12)
                            .controlSize(.mini)
                            .frame(width: 108)
                        Text(String(format: "%.1f mm", model.modifierValue))
                            .font(.system(size: 11).monospacedDigit())
                    }
                }
                if model.usesDocumentTree {
                    GridRow {
                        Text("Material").font(.system(size: 11)).foregroundStyle(.secondary)
                        Picker("", selection: materialBinding) {
                            ForEach(MaterialPreset.grouped, id: \.category) { group in
                                Section(group.category.capitalized) {
                                    ForEach(group.items) { m in Text(m.name).tag(m.key) }
                                }
                            }
                        }
                        .labelsHidden()
                        .controlSize(.small)
                        .frame(width: 150)
                    }
                }
                Divider().gridCellUnsizedAxes(.horizontal)
                GridRow {
                    Text("Camera").font(.system(size: 11)).foregroundStyle(.secondary)
                    HStack(spacing: 2) {
                        camButton("Iso", az: .pi / 5, el: .pi / 7)
                        camButton("Front", az: 0, el: 0)
                        camButton("Right", az: .pi / 2, el: 0)
                        camButton("Top", az: 0, el: 1.45)
                    }
                }
                GridRow {
                    Text("Zoom").font(.system(size: 11)).foregroundStyle(.secondary)
                    Slider(value: zoomBinding, in: 0...1)
                        .controlSize(.mini)
                        .frame(width: 150)
                }
                Divider().gridCellUnsizedAxes(.horizontal)
                row("Triangles", "\(model.triangleCount)")
                row("Bounds", String(format: "%.0f × %.0f × %.0f mm",
                                     model.sizeMM.x, model.sizeMM.y, model.sizeMM.z))
            }
        }
    }

    /// Zoom slider ∈ [0,1] mapped onto the orbit distance (inverted: right = closer).
    private var zoomBinding: Binding<Double> {
        Binding(
            get: { Double(1 - (model.distance - 0.45) / (8.0 - 0.45)) },
            set: {
                model.stopSpin()
                model.distance = 0.45 + Float(1 - $0) * (8.0 - 0.45)
                model.pinchBaseline = model.distance
            })
    }

    private var materialBinding: Binding<String> {
        let pi = model.selectedPartIndex ?? 0
        return Binding(
            get: { model.materialName(forPart: pi) ?? "aluminum" },
            set: { model.setPartMaterial(pi, $0) })
    }

    private func camButton(_ label: String, az: Float, el: Float) -> some View {
        Button(label) {
            model.stopSpin()
            withAnimation(.smooth(duration: 0.25)) {
                model.azimuth = az
                model.elevation = el
            }
        }
        .buttonStyle(.bordered)
        .controlSize(.mini)
    }

    private func row(_ k: String, _ v: String) -> some View {
        GridRow {
            Text(k).font(.system(size: 11)).foregroundStyle(.secondary)
            Text(v).font(.system(size: 11).monospacedDigit())
        }
    }
}
