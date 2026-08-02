#if canImport(AppKit)
import AppKit
#endif
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
    /// The kernel-space frame entity (scale + Z-up + centering applied), set by
    /// ReleasedARView on rebuild — sketch taps convert world rays through it,
    /// and the live sketch ink parents under it.
    weak var centeringEntity: Entity?
    /// Sketch mode is modal: the overlay owns the mouse everywhere, so clicks
    /// land on the sketch plane instead of falling through to the desktop.
    var sketchModal = false
    /// Chrome regions (tool windows, pill) in overlay view coords (top-left
    /// origin), reported by SwiftUI — the overlay owns the mouse over these.
    var chromeRects: [String: CGRect] = [:]

    func show(model: EditorModel, intent: IntentEngine) {
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
        w.contentView = NSHostingView(rootView: ReleasedOverlayView(model: model, intent: intent))
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

    /// Where a part sits on screen, in overlay view coords (top-left origin).
    /// Used to float the Object Inspector next to whatever it's inspecting.
    /// Projected by hand rather than through `ARView.project`, which answers nil
    /// for a scene camera that isn't the AR session's.
    func viewPoint(forPart i: Int, from camera: SIMD3<Float>, lookAt: SIMD3<Float>) -> CGPoint? {
        guard let ar = arView, let centering = centeringEntity,
              let e = centering.findEntity(named: "part\(i)") else { return nil }
        let size = ar.bounds.size
        guard size.width > 0, size.height > 0 else { return nil }

        let fwd = normalize(lookAt - camera)
        let right = normalize(cross(fwd, SIMD3<Float>(0, 1, 0)))
        let up = cross(right, fwd)
        let v = e.visualBounds(relativeTo: nil).center - camera
        let z = dot(v, fwd)
        guard z > 0.0001 else { return nil }                 // behind the camera
        let tanHalf = tan(Float(60) / 2 * .pi / 180)         // PerspectiveCamera default FOV
        let aspect = Float(size.width / size.height)
        let ndcX = (dot(v, right) / z) / (tanHalf * aspect)
        let ndcY = (dot(v, up) / z) / tanHalf
        return CGPoint(x: CGFloat(ndcX * 0.5 + 0.5) * size.width,
                       y: CGFloat(0.5 - ndcY * 0.5) * size.height)
    }

    /// Overlay size, for clamping floating chrome on screen.
    var overlaySize: CGSize { arView?.bounds.size ?? .zero }

    /// Own the mouse over geometry or chrome; pass everything else through.
    /// Never flips mid-drag (the orbit would be dropped half-way).
    private func updatePassThrough() {
        guard let w = window else { return }
        guard NSEvent.pressedMouseButtons == 0 else { return }
        if sketchModal { w.ignoresMouseEvents = false; return }
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
        centeringEntity = nil
        sketchModal = false
        chromeRects = [:]
        window?.orderOut(nil)
        window = nil
        mainWindow?.makeKeyAndOrderFront(nil)
        mainWindow = nil
    }

}

extension ReleaseWindowController {
    /// In-place re-eval → mesh swap for the released scene (the twin of the
    /// studio's docParamDirty path): parameter scrubs and gizmo drags update
    /// the floating parts without a rebuild pop. Plain (non-instanced) part
    /// entities only — instanced docs settle on the next full rebuild.
    func refreshGeometry(model: EditorModel, collisions: Bool = true) {
        guard let centering = centeringEntity,
              let meshes = model.reevalDocumentMeshes() else { return }
        for (i, m) in meshes.enumerated() {
            if let pe = centering.findEntity(named: "part\(i)") as? ModelEntity, pe.model != nil {
                pe.model?.mesh = m
                if collisions { pe.generateCollisionShapes(recursive: false) }
            }
        }
        model.docParamDirty = false      // consumed here; the studio view is hidden
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
    @Bindable var intent: IntentEngine
    /// Bumped once after the ARView exists so the inspector can re-place itself
    /// against the built scene — the first body pass runs before it does, and a
    /// static scene never re-renders on its own.
    @State private var layoutTick = 0

    var body: some View {
        ZStack(alignment: .topLeading) {
            ReleasedScene(model: model,
                          cameraPosition: model.cameraPosition,
                          lookAt: model.panOffset,
                          geometryKey: "\(model.baseShape)|\(model.modifier)|\(model.modifierValue)|\(model.triangleCount)|\(model.zebraMode)|\(model.highlightedParts.sorted())|\(model.hiddenParts.sorted())|\(String(describing: model.isolatedPart))|\(model.sketching)")
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
                    .onEnded { value in
                        if model.sketching { sketchTap(at: value.location) }
                        else { tapSelect(at: value.location, additive: false) }
                    })
                .onContinuousHover(coordinateSpace: .local) { phase in
                    guard model.sketching else { return }
                    switch phase {
                    case .active(let p):
                        model.sketchCursor = sketchPlanePoint(at: p)
                    case .ended:
                        model.sketchCursor = nil
                    }
                    refreshSketchInk()
                }
            // BCB-style tool windows, hosted in the overlay itself (separate
            // NSPanels break SwiftUI hit testing after auto-resize).
            VStack(alignment: .leading, spacing: 12) {
                ComponentPaletteWindow(model: model)
                    .modifier(ChromeRegion(key: "palette"))
                ScrollView {
                    FeatureTreeView(model: model)
                }
                .frame(width: 230)
                .frame(maxHeight: 380)
                .fixedSize(horizontal: false, vertical: true)
                .modifier(ChromeRegion(key: "tree"))
                // Dynamics, as its own tool window. Released mode floats over
                // the desktop, so the studio's single scrolling inspector has
                // no home here — the BCB idiom is one window per concern.
                if model.canSimulate {
                    SimulationWindow(model: model)
                        .modifier(ChromeRegion(key: "sim"))
                }
            }
            .padding(16)
            // The Object Inspector floats beside whatever it inspects: it
            // tracks the selected part's projected screen position (and so
            // follows it through orbits and drags), parking under the palette
            // when nothing is selected.
            ObjectInspectorWindow(model: model)
                .modifier(ChromeRegion(key: "inspector"))
                .offset(x: inspectorOrigin.x, y: inspectorOrigin.y)
                .animation(Motion.smooth, value: model.selectedPartIndex)
        }
        .overlay(alignment: .topTrailing) {
            ReleaseReturnPill(model: model)
                .padding(16)
                .modifier(ChromeRegion(key: "pill"))
        }
        .overlay(alignment: .top) {
            // Cross-domain gripper receipt — the released twin of the studio's
            // top-center verification pill.
            if model.source.isGripper {
                GripperReceiptPill(model: model)
                    .padding(.top, 16)
                    .modifier(ChromeRegion(key: "gripper"))
            }
        }
        .overlay(alignment: .bottom) {
            // Bottom cluster: the AI command bar (the app's spine) plus the
            // kinematic transport when a timeline is loaded. Same views, same
            // model/engine bindings as the studio — just floated over the desktop.
            VStack(spacing: 10) {
                if model.sketching {
                    SketchHintBar(model: model)
                        .modifier(ChromeRegion(key: "sketchHint"))
                    SketchPaletteView(model: model)
                        .modifier(ChromeRegion(key: "sketchPalette"))
                } else {
                    if model.timeline != nil {
                        PlaybackBar(model: model)
                            .modifier(ChromeRegion(key: "playback"))
                    }
                    if model.canSimulate {
                        SimBar(model: model)
                            .modifier(ChromeRegion(key: "simBar"))
                    }
                    if model.source.isSandbox && intent.draft.isEmpty && !intent.isThinking {
                        ExampleChips(intent: intent)
                            .modifier(ChromeRegion(key: "examples"))
                    }
                    ComposerBar(engine: intent, model: model)
                        .modifier(ChromeRegion(key: "composer"))
                }
            }
            .padding(.bottom, 16)
            .animation(Motion.smooth, value: intent.draft.isEmpty)
            .animation(Motion.smooth, value: model.timeline == nil)
            .animation(Motion.panel, value: model.sketching)
        }
        .task {
            // A few settling passes: the scene builds (and can rebuild from a
            // document load) over the first moments after the overlay appears.
            for _ in 0..<12 {
                try? await Task.sleep(for: .milliseconds(150))
                layoutTick += 1
            }
        }
        .onChange(of: model.sketching) { _, on in
            // Sketch is modal over the desktop: own the mouse everywhere while
            // drawing, hand non-chrome/non-part mouse back when done.
            ReleaseWindowController.shared.sketchModal = on
            refreshSketchInk(force: true)
        }
        .onChange(of: model.sketchDirty) { _, dirty in
            if dirty { refreshSketchInk() }
        }
    }

    /// Top-left origin for the floating Object Inspector: just right of the
    /// selected part's projected center, clamped inside the overlay. With no
    /// selection it parks under the Component Palette.
    private var inspectorOrigin: CGPoint {
        _ = layoutTick
        let ctl = ReleaseWindowController.shared
        let size = ctl.chromeRects["inspector"]?.size ?? CGSize(width: 320, height: 260)
        let bounds = ctl.overlaySize
        let parked = CGPoint(x: 16, y: (ctl.chromeRects["palette"]?.maxY ?? 140) + 12)
        guard let pi = model.selectedPartIndex, bounds.width > 0,
              let p = ctl.viewPoint(forPart: pi, from: model.cameraPosition,
                                    lookAt: model.panOffset) else { return parked }
        let gap: CGFloat = 28
        // Prefer the right of the part; flip to its left when that would run
        // off the edge.
        var x = p.x + gap
        if x + size.width + 16 > bounds.width { x = p.x - gap - size.width }
        let y = p.y - size.height / 2
        return CGPoint(x: min(max(16, x), max(16, bounds.width - size.width - 16)),
                       y: min(max(16, y), max(16, bounds.height - size.height - 16)))
    }

    /// Screen point → kernel-space ray, through the ARView's native ray and
    /// the kernel-frame (centering) entity — no hand-rolled camera math.
    private func kernelRay(at p: CGPoint) -> (o: SIMD3<Float>, d: SIMD3<Float>)? {
        guard let ar = ReleaseWindowController.shared.arView,
              let centering = ReleaseWindowController.shared.centeringEntity else { return nil }
        // SwiftUI hands us top-left-origin points; ray(through:) expects the
        // ARView's native (unflipped AppKit, bottom-left) space — same flip the
        // pass-through hit test does.
        var vp = p
        if !ar.isFlipped { vp.y = ar.bounds.height - vp.y }
        guard let ray = ar.ray(through: vp) else { return nil }
        let o = centering.convert(position: ray.origin, from: nil)
        let d = normalize(centering.convert(direction: ray.direction, from: nil))
        return (o, d)
    }

    /// Screen point → 2D sketch-plane coords.
    private func sketchPlanePoint(at p: CGPoint) -> SIMD2<Float>? {
        guard let ray = kernelRay(at: p) else { return nil }
        return model.sketchPlanePoint(originKernel: ray.o, dirKernel: ray.d)
    }

    private func sketchTap(at p: CGPoint) {
        guard let pt = sketchPlanePoint(at: p) else { return }
        model.sketchTap(pt)
        refreshSketchInk()
    }

    /// Rebuild the sketch ink under the kernel frame. Imperative (not keyed
    /// through geometryKey) so a cursor move never re-tessellates the parts.
    private func refreshSketchInk(force: Bool = false) {
        guard force || model.sketchDirty || model.sketching else { return }
        guard let centering = ReleaseWindowController.shared.centeringEntity else { return }
        centering.findEntity(named: "sketchRoot")?.removeFromParent()
        if model.sketching { centering.addChild(buildSketchRoot(model: model)) }
        model.sketchDirty = false
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
                if model.lastDrag == .zero && !model.draggingHandle {
                    // A drag that starts on a gizmo handle is a part transform,
                    // not an orbit — same split the studio's targeted gesture
                    // makes, done here via hitTest (the clear layer owns events).
                    if let ar = ReleaseWindowController.shared.arView,
                       let name = ar.hitTest(value.startLocation)
                           .first(where: { model.gizmoHandle(for: $0.entity.name) != nil })?.entity.name,
                       let ray = kernelRay(at: value.startLocation) {
                        model.draggingHandle = true
                        NSCursor.closedHand.set()
                        model.beginGizmoDrag(handle: name, ray: ray)
                        model.lastDrag = value.translation
                        return
                    }
                }
                if model.draggingHandle {
                    if let ray = kernelRay(at: value.location) { model.gizmoDragTo(ray: ray) }
                    refreshDraggedGeometry(final: false)
                    model.lastDrag = value.translation
                    return
                }
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
                if model.draggingHandle {
                    NSCursor.arrow.set()
                    model.draggingHandle = false
                    model.endGizmoDrag()
                    refreshDraggedGeometry(final: true)
                } else {
                    model.endOrbit()
                }
            }
    }

    /// Live-sync the dragged part: re-evaluate the document meshes in place and
    /// swap them into the existing entities (plain parts only), then slide the
    /// gizmo with the part. On `final`, also regenerate collision shapes and
    /// rebuild the gizmo at its settled center.
    private func refreshDraggedGeometry(final: Bool) {
        guard let centering = ReleaseWindowController.shared.centeringEntity else { return }
        ReleaseWindowController.shared.refreshGeometry(model: model, collisions: final)
        if final {
            centering.findEntity(named: "gizmoRoot")?.removeFromParent()
            if model.showsGizmo { centering.addChild(buildGizmo(model: model)) }
        } else if let c = model.gizmoCenterKernel() {
            centering.findEntity(named: "gizmoRoot")?.position = c + model.gizmoLiveOffset
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
    /// Bumped by every published solve (a playback frame or a physics step).
    ///
    /// `updateNSView` only runs when one of this representable's properties
    /// changes, so without a per-frame input the released view would rebuild on
    /// geometry changes and then never re-pose — the robot would render at its
    /// rest pose and sit there while the simulation ran behind it. The studio
    /// gets this for free because a `RealityView`'s update closure observes the
    /// model directly.
    let poseTick: Int
    let geometryKey: String

    final class Coordinator {
        var camera: PerspectiveCamera?
        var anchor: AnchorEntity?
        var builtKey = ""
        /// Instance entities in scene order, captured at build time.
        ///
        /// `findEntity(named:)` walks the whole descendant tree; doing that
        /// once per instance per frame is quadratic in the model and lands
        /// squarely on the frame budget for a 24-body robot.
        var instanceEntities: [ModelEntity] = []
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> ARView {
        let ar = ARView(frame: .zero)
        ar.environment.background = .color(.clear)
        ReleaseWindowController.shared.arView = ar
        let anchor = AnchorEntity(world: .zero)
        let camera = PerspectiveCamera()
        anchor.addChild(camera)
        addLightRig(to: anchor)
        ar.scene.addAnchor(anchor)
        context.coordinator.anchor = anchor
        context.coordinator.camera = camera
        rebuild(in: anchor, coordinator: context.coordinator)
        applyLighting(ar)
        syncCamera(context.coordinator)
        return ar
    }

    /// The studio's key/rim/fill rig. The studio environment is a deliberately
    /// dark reflection probe — without these lights the floating parts read as
    /// black silhouettes over the desktop.
    private func addLightRig(to anchor: AnchorEntity) {
        let key = DirectionalLight()
        key.light.intensity = 2300
        key.look(at: .zero, from: [0.7, 1.1, 0.85], relativeTo: nil)
        anchor.addChild(key)

        let rim = DirectionalLight()
        rim.light.intensity = 1900
        rim.look(at: .zero, from: [-0.9, 0.35, -1.0], relativeTo: nil)
        anchor.addChild(rim)

        let fill = DirectionalLight()
        fill.light.intensity = 900
        fill.look(at: .zero, from: [-0.2, 0.5, 1.2], relativeTo: nil)
        anchor.addChild(fill)
    }

    /// Same IBL as the studio (zebra wins when curvature analysis is on) — the
    /// background stays clear, so only the lighting comes from the environment.
    private func applyLighting(_ ar: ARView) {
        ar.environment.lighting.resource =
            model.zebraMode ? ViewportView.zebraEnvironment : ViewportView.studioEnvironment
    }

    func updateNSView(_ ar: ARView, context: Context) {
        if context.coordinator.builtKey != geometryKey {
            rebuild(in: context.coordinator.anchor, coordinator: context.coordinator)
            applyLighting(ar)
        }
        applyInstancePoses(context.coordinator)
        syncCamera(context.coordinator)
    }

    /// Re-pose instance entities from the latest solve — the released twin of
    /// the studio's `playbackDirty` branch. Both kinematic playback and the
    /// physics engine publish through `instanceTransforms`, so this one path
    /// serves both and the released view cannot fall behind the studio.
    private func applyInstancePoses(_ c: Coordinator) {
        guard model.playbackDirty, !c.instanceEntities.isEmpty else { return }
        for (i, m) in model.instanceTransforms.enumerated() where i < c.instanceEntities.count {
            c.instanceEntities[i].transform = Transform(matrix: m)
        }
        model.playbackDirty = false
    }

    private func syncCamera(_ c: Coordinator) {
        guard let camera = c.camera else { return }
        camera.position = cameraPosition
        camera.look(at: lookAt, from: cameraPosition, relativeTo: nil)
    }

    private func rebuild(in anchor: AnchorEntity?, coordinator: Coordinator) {
        guard let anchor else { return }
        coordinator.builtKey = geometryKey
        coordinator.instanceEntities.removeAll(keepingCapacity: true)
        anchor.children.filter { $0.name == "geomRoot" }.forEach { $0.removeFromParent() }

        let scene = model.buildScene()
        let sceneScale = 0.6 / max(scene.size, 0.0001)

        let centering = Entity()
        centering.name = "centering"
        centering.position = -scene.center
        ReleaseWindowController.shared.centeringEntity = centering
        // Keep the in-progress sketch ink alive across geometry rebuilds.
        if model.sketching { centering.addChild(buildSketchRoot(model: model)) }
        // Transform gizmo on the selected part — same shared builder as the
        // studio; its handle proxies carry collision shapes, so the ARView
        // hitTest (drag start + pass-through) sees them like parts.
        if model.showsGizmo { centering.addChild(buildGizmo(model: model)) }
        // Assembly documents carry INSTANCES rather than root parts, and
        // `scene.meshes` is empty for them — so a released view that only
        // walked `meshes` drew nothing at all for any assembly, simulated or
        // not. Instances are the studio's primary path; they are the whole
        // content of a robot document.
        for (i, inst) in scene.instances.enumerated() {
            var m = PhysicallyBasedMaterial()
            if model.zebraMode {
                m = ViewportView.zebraChrome
            } else {
                m.baseColor = .init(tint: inst.material.color)
                m.roughness = .init(floatLiteral: inst.material.roughness)
                m.metallic = .init(floatLiteral: inst.material.metallic)
            }
            let e = ModelEntity(mesh: inst.mesh, materials: [m])
            e.name = "inst\(i)"
            // Live transforms win over the authored pose: kinematic playback
            // and physics both publish through `instanceTransforms`, and a
            // rebuild mid-episode must not snap the robot back to its rest
            // pose.
            e.transform = Transform(matrix: i < model.instanceTransforms.count
                                    ? model.instanceTransforms[i] : inst.transform)
            e.generateCollisionShapes(recursive: false)
            centering.addChild(e)
            coordinator.instanceEntities.append(e)
        }
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
                if !model.docParameters.isEmpty {
                    Divider().gridCellUnsizedAxes(.horizontal)
                    // Document-level named parameters — the web app's property
                    // panel scrub, floating over the desktop. Edits re-solve
                    // every bound node and mesh-swap in place (no rebuild pop).
                    ForEach(model.docParameters) { p in
                        GridRow {
                            Text(p.name).font(.system(size: 11)).foregroundStyle(.secondary)
                                .help(p.description ?? p.name)
                            if let v = p.value {
                                ScrubField(label: "", value: v, unit: p.unit ?? "mm",
                                           sensitivity: InspectorView.paramSensitivity(p),
                                           minValue: p.min ?? -.greatestFiniteMagnitude) { v, s in
                                    model.editParameter(p.name, value: v, snapshot: s)
                                    // Collisions on typed commits / scrub start
                                    // only — per-tick regen would stutter big docs.
                                    ReleaseWindowController.shared.refreshGeometry(model: model, collisions: s)
                                }
                                .frame(width: 150)
                            } else if let f = p.formula {
                                Text("= \(f)").font(.system(size: 11).monospacedDigit())
                                    .foregroundStyle(.tertiary)
                            }
                        }
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

/// The released twin of the studio's Simulation inspector section.
///
/// Wraps the same `SimInspector` the studio uses, so the two cannot drift:
/// every readout, every control, and every fail-closed message is defined once.
/// Only the chrome differs — a floating BCB tool window rather than a section
/// inside the scrolling inspector.
///
/// **Anything added here must carry a `ChromeRegion`.** Released mode passes
/// the mouse through to the desktop everywhere the hit test says the pixel is
/// not ours, so chrome that does not report its frame renders normally and
/// then silently ignores every click.
struct SimulationWindow: View {
    @Bindable var model: EditorModel

    var body: some View {
        ToolWindow(title: "Simulation", onClose: { model.sim.teardown() }) {
            SimInspector(model: model)
                .frame(width: 240)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}


/// Wraps the released ARView so that only *this* view observes `poseTick`.
///
/// Reading a 50 Hz counter directly in `ReleasedOverlayView.body` re-evaluates
/// the entire overlay on every physics step — every tool window, the feature
/// tree, the inspector and the training chart — which is how a simulation that
/// runs at 3x real time headless ends up displaying 0.3x. SwiftUI's observation
/// is per-view, so isolating the read here confines the per-frame work to the
/// one view that actually needs it.
struct ReleasedScene: View {
    @Bindable var model: EditorModel
    let cameraPosition: SIMD3<Float>
    let lookAt: SIMD3<Float>
    let geometryKey: String

    var body: some View {
        ReleasedARView(model: model,
                       cameraPosition: cameraPosition,
                       lookAt: lookAt,
                       poseTick: model.poseTick,
                       geometryKey: geometryKey)
    }
}
