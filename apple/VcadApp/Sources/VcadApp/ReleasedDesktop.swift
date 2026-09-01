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
    /// The document being edited, for the hover pass (which runs off an AppKit
    /// event monitor, outside any SwiftUI view).
    private weak var model: EditorModel?
    /// Part under the pointer, and the gizmo handle under it. Mirrored into the
    /// model (`hoveredPartIndex`) so panels can read it; kept here too because
    /// the hover pass must know what to un-highlight.
    private(set) var hoveredPart: Int?
    private(set) var hoveredInstance: Int?
    private var paintedInstances: Set<Int> = []
    private var paintedPart: Int?
    private var hoveredHandle: String?
    private var pointerOverScene = false

    func show(model: EditorModel, intent: IntentEngine) {
        guard window == nil else { return }
        self.model = model
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
        DockIcon.shared.follow(model: model)
        mainWindow?.orderOut(nil)

        let update: () -> Void = { [weak self] in self?.updatePassThrough() }
        if let m = NSEvent.addGlobalMonitorForEvents(matching: [.mouseMoved]) { _ in
            Task { @MainActor in update() }
        } { mouseMonitors.append(m) }
        mouseMonitors.append(NSEvent.addLocalMonitorForEvents(matching: [.mouseMoved]) { e in
            Task { @MainActor in update() }
            return e
        } as Any)
        // Scroll to dolly, the way every desktop CAD app does it. Only over the
        // scene: over a tool window the event has to reach its ScrollView, and
        // over the desktop the window is already transparent to the mouse.
        mouseMonitors.append(NSEvent.addLocalMonitorForEvents(matching: [.scrollWheel]) { [weak self] e in
            guard let self else { return e }
            // Precise (trackpad / Magic Mouse) deltas are already in points and
            // arrive in a fine stream; a notched wheel sends few, large ticks.
            // Scaling them the same way makes the wheel unusable. Read the event
            // out here — NSEvent cannot cross into an isolated closure.
            let dy = e.hasPreciseScrollingDeltas
                ? Float(e.scrollingDeltaY) * 0.004
                : Float(e.scrollingDeltaY) * 0.06
            let consumed = MainActor.assumeIsolated { self.dolly(by: dy) }
            return consumed ? nil : e
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

    /// Every collision hit under a **SwiftUI** point, nearest first.
    ///
    /// Two coordinate spaces meet in this file and they disagree about which
    /// way is up:
    ///
    /// * SwiftUI gestures report top-left-origin points.
    /// * `NSEvent.mouseLocation` → `convertPoint(fromScreen:)` → `ar.convert`
    ///   gives bottom-left-origin points on an unflipped view.
    ///
    /// `ARView.hitTest` wants the latter. So SwiftUI points must be flipped —
    /// and AppKit-derived points must NOT be. Getting either one backwards
    /// mirrors picking about the horizontal midline, which is invisible on a
    /// single centred body (you hit the same thing either way) and obvious on a
    /// stack: point at the wrist, highlight the base.
    func hits(atViewPoint p: CGPoint) -> [CollisionCastHit] {
        guard let ar = arView else { return [] }
        var vp = p
        if !ar.isFlipped { vp.y = ar.bounds.height - vp.y }
        return ar.hitTest(vp)
    }

    /// Overlay size, for clamping floating chrome on screen.
    var overlaySize: CGSize { arView?.bounds.size ?? .zero }

    /// Own the mouse over geometry or chrome; pass everything else through.
    /// Never flips mid-drag (the orbit would be dropped half-way).
    private func updatePassThrough() {
        guard let w = window else { return }
        guard NSEvent.pressedMouseButtons == 0 else { return }
        if sketchModal {
            w.ignoresMouseEvents = false
            pointerOverScene = true
            return
        }
        let winPoint = w.convertPoint(fromScreen: NSEvent.mouseLocation)
        let topLeft = CGPoint(x: winPoint.x, y: w.frame.height - winPoint.y)
        if chromeRects.values.contains(where: { $0.insetBy(dx: -8, dy: -8).contains(topLeft) }) {
            w.ignoresMouseEvents = false
            pointerOverScene = false
            clearHover()                       // the pointer left the model
            return
        }
        guard let ar = arView else { return }
        // `winPoint` came from AppKit, so it is ALREADY in hitTest's space once
        // converted into the view — no flip here. The flip belongs to the
        // SwiftUI path (see `hits(atViewPoint:)`); doing it in both places is
        // what made hover point at the mirror image of the pointer.
        let viewPoint = ar.convert(winPoint, from: nil)
        // ONE raycast serves both jobs: whether we own the mouse, and what the
        // pointer is over. Hover used to cost nothing because it did not exist;
        // a second raycast per mouse-moved would have been the lazy way to add it.
        // Annotated: ARView's RealityKit hitTest overload and NSView's own
        // hitTest(_:) are both in scope, and the inferred winner is the NSView.
        let hits: [CollisionCastHit] = ar.hitTest(viewPoint)
        w.ignoresMouseEvents = hits.isEmpty
        pointerOverScene = !hits.isEmpty
        updateHover(hits)
    }

    /// Zoom by a scroll delta. Returns whether the scroll was consumed — over a
    /// tool window it must not be, or the feature tree stops scrolling.
    private func dolly(by dy: Float) -> Bool {
        guard pointerOverScene, dy != 0, let m = model else { return false }
        m.stopSpin()
        m.distance = max(0.45, min(8.0, m.distance * (1 - dy)))
        m.pinchBaseline = m.distance
        return true
    }

    // MARK: hover

    /// Highlight what the pointer is over, set the cursor that says what a click
    /// will do, and publish the surface readout. Runs on every mouse-moved, so
    /// every step is change-guarded — re-assigning an identical material or an
    /// identical cursor is a per-frame cost for no pixels.
    private func updateHover(_ hits: [CollisionCastHit]) {
        guard let model else { return }
        if model.sketching { NSCursor.crosshair.set(); return }
        // An armed palette tool retargets the whole pointer: the next click
        // places geometry, so it must not also read as "select this part".
        if model.armedShape != nil {
            setHovered(part: nil, instance: nil)
            NSCursor.crosshair.set()
            return
        }
        if model.draggingHandle { return }             // the drag owns the cursor

        // Gizmo handles sit on top of the part they move, and grabbing one is a
        // different act from selecting the part — so they win the hover.
        let handle = model.showsGizmo
            ? hits.compactMap({ model.gizmoHandle(for: $0.entity.name) != nil ? $0.entity.name : nil }).first
            : nil
        if handle != hoveredHandle {
            hoveredHandle = handle
            model.hoveredGizmoHandle = handle
            model.gizmoDirty = true
        }
        if handle != nil {
            setHovered(part: nil, instance: nil)
            NSCursor.openHand.set()
            return
        }

        // Parts and instances are the same gesture to a user — "the thing under
        // the pointer" — and differ only in which entity the renderer built.
        let hit = hits.first(where: { Self.partIndex($0.entity) != nil
                                      || Self.instanceIndex($0.entity) != nil })
        setHovered(part: hit.flatMap { Self.partIndex($0.entity) },
                   instance: hit.flatMap { Self.instanceIndex($0.entity) })
        publishPick(hit)
        ((hoveredPart ?? hoveredInstance) != nil ? NSCursor.pointingHand : NSCursor.arrow).set()
    }

    private func clearHover() {
        guard let model else { return }
        setHovered(part: nil, instance: nil)
        publishPick(nil)
        if hoveredHandle != nil {
            hoveredHandle = nil
            model.hoveredGizmoHandle = nil
            model.gizmoDirty = true
        }
        NSCursor.arrow.set()
    }

    /// Repaint every part's highlight state. Cheap: it walks the built entities
    /// and assigns materials — no tessellation, no colliders, no new entities.
    func repaintAll() {
        guard let model, let centering = centeringEntity else { return }
        let hot = model.hoveredInstances
        for i in 0..<model.partCount where centering.findEntity(named: "part\(i)") != nil {
            paint(entity: "part\(i)",
                  lift: model.hoveredPartIndex == i ? .hover : liftForPart(i))
        }
        for i in 0..<model.assemblyInstanceCount where centering.findEntity(named: "inst\(i)") != nil {
            paint(entity: "inst\(i)", lift: hot.contains(i) ? .hover : liftForInstance(i))
        }
        paintedPart = model.hoveredPartIndex
        paintedInstances = hot
    }

    /// Re-apply the hover lift after a rebuild. Selection changes rebuild the
    /// scene, which paints fresh materials — without this the part under the
    /// pointer goes dull the instant you click it, until you jiggle the mouse.
    func repaintHover() {
        paintedPart = nil
        paintedInstances = []
        syncHoverPaint()
    }

    private func setHovered(part: Int?, instance: Int?) {
        guard let model else { return }
        if part != hoveredPart || instance != hoveredInstance {
            hoveredPart = part
            hoveredInstance = instance
            model.hoverBody(part: part, instance: instance)
        }
        syncHoverPaint()
    }

    /// Paint the difference between what is lit and what should be lit. The
    /// model owns "what should be" — the tree can set it too — so this diffs
    /// rather than assuming the viewport was the last writer.
    func syncHoverPaint() {
        guard let model else { return }
        let want = model.hoveredInstances
        for i in paintedInstances.subtracting(want) {
            paint(entity: "inst\(i)", lift: liftForInstance(i))
        }
        for i in want.subtracting(paintedInstances) {
            paint(entity: "inst\(i)", lift: .hover)
        }
        paintedInstances = want
        let part = model.hoveredPartIndex
        if part != paintedPart {
            if let previous = paintedPart { paint(entity: "part\(previous)", lift: liftForPart(previous)) }
            if let part { paint(entity: "part\(part)", lift: .hover) }
            paintedPart = part
        }
    }

    /// How a part/instance should read right now, ignoring the pointer.
    private func liftForPart(_ i: Int) -> Lift {
        (model?.highlightedParts.contains(i) ?? false) ? .selected : .none
    }
    private func liftForInstance(_ i: Int) -> Lift {
        (model?.selectedInstances.contains(i) ?? false) ? .selected : .none
    }

    /// The three states a body can be in. Selection is brand orange; hover on
    /// top of it brightens rather than replaces, so a selected body never looks
    /// deselected under the pointer.
    enum Lift {
        case none, selected, hover
    }

    /// The hover lift, applied straight to the live material — NOT through a
    /// geometry rebuild. Re-tessellating a document because the pointer crossed
    /// a face is how a viewport starts dropping frames.
    private func paint(entity name: String, lift: Lift) {
        guard let e = centeringEntity?.findEntity(named: name) as? ModelEntity,
              var m = e.model?.materials.first as? PhysicallyBasedMaterial else { return }
        switch lift {
        case .hover where isSelected(name):
            m.emissiveColor = .init(color: NSColor(red: 1.0, green: 0.68, blue: 0.24, alpha: 1))
            m.emissiveIntensity = 0.6
        case .selected:
            m.emissiveColor = .init(color: NSColor(red: 1.0, green: 0.62, blue: 0.12, alpha: 1))
            m.emissiveIntensity = 0.35
        case .hover:
            m.emissiveColor = .init(color: .white)
            m.emissiveIntensity = 0.16
        case .none:
            m.emissiveIntensity = 0
        }
        e.model?.materials = [m]
    }

    private func isSelected(_ name: String) -> Bool {
        if let i = Int(name.dropFirst(4)), name.hasPrefix("part") {
            return model?.highlightedParts.contains(i) ?? false
        }
        if let i = Int(name.dropFirst(4)), name.hasPrefix("inst") {
            return model?.selectedInstances.contains(i) ?? false
        }
        return false
    }

    /// Surface readout for the inspector's "Picked" row: what kind of face, and
    /// where, in millimetres of kernel space.
    private func publishPick(_ hit: CollisionCastHit?) {
        guard let model else { return }
        guard let hit, let centering = centeringEntity else {
            if model.pickInfo != nil { model.pickInfo = nil; model.pickPoint = nil }
            return
        }
        let p = centering.convert(position: hit.position, from: nil)
        let n = normalize(centering.convert(direction: hit.normal, from: nil))
        let maxAxis = max(abs(n.x), max(abs(n.y), abs(n.z)))
        let text = String(format: "%@ (%.1f, %.1f, %.1f)",
                          maxAxis > 0.97 ? "Planar" : "Curved", p.x, p.y, p.z)
        if model.pickInfo != text {
            model.pickPoint = p
            model.pickInfo = text
        }
    }

    /// "part7" → 7; else nil.
    private static func partIndex(_ e: Entity) -> Int? {
        guard e.name.hasPrefix("part"), let i = Int(e.name.dropFirst(4)) else { return nil }
        return i
    }

    /// "inst3" → 3; else nil. Assemblies draw instances, not root parts.
    private static func instanceIndex(_ e: Entity) -> Int? {
        guard e.name.hasPrefix("inst"), let i = Int(e.name.dropFirst(4)) else { return nil }
        return i
    }

    /// Frame a single part: orbit around its centre, pulled in to fit it. The
    /// double-click gesture's half of "show me this thing".
    func frame(entityNamed name: String) {
        guard let model,
              let e = centeringEntity?.findEntity(named: name) else { return }
        let b = e.visualBounds(relativeTo: nil)
        model.stopSpin()
        withAnimation(.smooth(duration: 0.35)) {
            model.panOffset = b.center
            let extent = max(b.extents.x, max(b.extents.y, b.extents.z))
            model.distance = max(0.45, min(8.0, extent * 2.2))
        }
        model.pinchBaseline = model.distance
    }

    func hide() {
        DockIcon.shared.stop()
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

    /// The in-flight ⌘-drag rubber band, in overlay view coords.
    @State private var marquee: (start: CGPoint, current: CGPoint)?
    /// Where the last click landed and how far into that point's stack of parts
    /// it selected — clicking the same spot again walks to the part behind.
    @State private var cycleAnchor: CGPoint?
    @State private var cycleDepth = 0

    /// What the built scene is a function of. Assembled as a String rather than
    /// interpolated inline: one seven-way interpolation inside a view builder is
    /// enough to blow the type-checker's budget.
    private var geometryKey: String {
        var parts: [String] = []
        parts.append(String(describing: model.source))
        parts.append(String(describing: model.baseShape))
        parts.append(String(describing: model.modifier))
        parts.append(String(model.modifierValue))
        parts.append(String(model.triangleCount))
        parts.append(String(model.zebraMode))
        parts.append(String(model.sketching))
        return parts.joined(separator: "|")
    }

    var body: some View {
        ZStack(alignment: .topLeading) {
            ReleasedScene(model: model,
                          cameraPosition: model.cameraPosition,
                          lookAt: model.panOffset,
                          geometryKey: geometryKey)
                .ignoresSafeArea()
            // ARView consumes mouse events — capture orbit/zoom on a clear
            // layer above it instead.
            Color.clear
                .contentShape(Rectangle())
                .ignoresSafeArea()
                .gesture(marqueeOrOrbitGesture)
                .simultaneousGesture(zoomGesture)
                .gesture(SpatialTapGesture(coordinateSpace: .local).modifiers(.command)
                    .onEnded { value in tapSelect(at: value.location, additive: true) })
                .gesture(SpatialTapGesture(coordinateSpace: .local)
                    .onEnded { value in
                        if model.sketching { sketchTap(at: value.location) }
                        else { tapSelect(at: value.location, additive: false) }
                    })
                // Double-click to frame the part under the pointer — the
                // universal "show me this thing" of every 3D app. Ordered after
                // the single tap so the part is selected by the time it frames.
                .gesture(SpatialTapGesture(count: 2, coordinateSpace: .local)
                    .onEnded { value in
                        guard !model.sketching,
                              let pi = partIndex(at: value.location) else { return }
                        ReleaseWindowController.shared.frame(entityNamed: "part\(pi)")
                    })
                // Right-click acts on whatever the pointer is over. It can only
                // ever fire over geometry or chrome: everywhere else the window
                // is transparent to the mouse and the click goes to the desktop.
                .contextMenu { partContextMenu }
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
            if let m = marquee {
                let r = CGRect(x: min(m.start.x, m.current.x), y: min(m.start.y, m.current.y),
                               width: abs(m.current.x - m.start.x),
                               height: abs(m.current.y - m.start.y))
                Rectangle()
                    .fill(Color.accentColor.opacity(0.12))
                    .overlay(Rectangle().strokeBorder(Color.accentColor.opacity(0.8), lineWidth: 1))
                    .frame(width: r.width, height: r.height)
                    .offset(x: r.minX, y: r.minY)
                    .allowsHitTesting(false)
            }
            // BCB-style tool windows, hosted in the overlay itself (separate
            // NSPanels break SwiftUI hit testing after auto-resize).
            VStack(alignment: .leading, spacing: 12) {
                if model.showsPalette {
                    ComponentPaletteWindow(model: model)
                        .modifier(ChromeRegion(key: "palette"))
                }
                if model.showsTree {
                    FeatureTreeWindow(model: model)
                        .modifier(ChromeRegion(key: "tree"))
                }
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
        }
        .background {
            // Esc clears the selection. In release-to-desktop an empty-space
            // click is NOT a deselect — the window is transparent to the mouse
            // there, so that click belongs to whatever is behind us. Disabled
            // while sketching or armed so those cancels win Esc first.
            Button("") { model.deselectAll() }
                .keyboardShortcut(.cancelAction)
                .frame(width: 0, height: 0).opacity(0)
                .accessibilityHidden(true)
                .disabled(!model.hasSelection || model.sketching || model.armedShape != nil)
        }
        .overlay(alignment: .topTrailing) {
            // The right rail: identity + the inspector, docked to the screen
            // edge. (It used to chase the selected part around the viewport,
            // which reads as clever for one part and as a moving target for a
            // real assembly — CAD inspectors live in a fixed rail.)
            VStack(alignment: .trailing, spacing: 12) {
                HStack(spacing: 10) {
                    IdentityStatusBar(model: model)
                        .modifier(ChromeRegion(key: "identity"))
                    PanelsPill(model: model)
                        .modifier(ChromeRegion(key: "pill"))
                }
                if model.showsInspector {
                    if model.source.isGripper {
                        ReceiptLedgerWindow(model: model)
                            .modifier(ChromeRegion(key: "inspector"))
                    } else {
                        ObjectInspectorWindow(model: model)
                            .modifier(ChromeRegion(key: "inspector"))
                    }
                }
            }
            .padding(16)
            .animation(Motion.panel, value: model.showsInspector)
            .animation(Motion.panel, value: model.source.isGripper)
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
        .onChange(of: model.hoveredInstances) { _, _ in
            // The feature tree can set the hover too; the renderer only ever
            // heard from the pointer.
            ReleaseWindowController.shared.syncHoverPaint()
        }
        .onChange(of: model.hoveredPartIndex) { _, _ in
            ReleaseWindowController.shared.syncHoverPaint()
        }
        .onChange(of: model.armedShape) { _, shape in
            // Armed mode is modal over the desktop, like sketching: the overlay
            // owns the mouse everywhere so the placing click cannot fall through
            // to whatever window happens to be behind the part.
            ReleaseWindowController.shared.sketchModal = shape != nil || model.sketching
            if shape == nil { NSCursor.arrow.set() }
        }
        .onChange(of: model.sketching) { _, on in
            // Sketch is modal over the desktop: own the mouse everywhere while
            // drawing, hand non-chrome/non-part mouse back when done.
            ReleaseWindowController.shared.sketchModal = on || model.armedShape != nil
            refreshSketchInk(force: true)
        }
        .onChange(of: model.sketchDirty) { _, dirty in
            if dirty { refreshSketchInk() }
        }
    }

    /// Top-left origin for the floating Object Inspector: just right of the
    /// selected part's projected center, clamped inside the overlay. With no
    /// selection it parks under the Component Palette.
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

    /// The entity name of the body under a screen point — a root part or an
    /// assembly instance.
    private func bodyName(at p: CGPoint) -> String? {
        ReleaseWindowController.shared.hits(atViewPoint: p).first {
            $0.entity.name.hasPrefix("part") || $0.entity.name.hasPrefix("inst")
        }?.entity.name
    }

    /// The part under a screen point, if any.
    private func partIndex(at p: CGPoint) -> Int? {
        guard let e = ReleaseWindowController.shared.hits(atViewPoint: p).first?.entity,
              e.name.hasPrefix("part") else { return nil }
        return Int(e.name.dropFirst(4))
    }

    /// The right-click menu: what you can do to the part under the pointer,
    /// then the view-wide actions. Built from the live hover, so it always
    /// describes the thing you actually pointed at.
    @ViewBuilder private var partContextMenu: some View {
        let pi = ReleaseWindowController.shared.hoveredPart
        if let ii = ReleaseWindowController.shared.hoveredInstance {
            Text(model.instanceName(ii) ?? "Instance \(ii)")
            Divider()
            Button("Frame") { ReleaseWindowController.shared.frame(entityNamed: "inst\(ii)") }
            Button("Select") { model.selectInstance(ii) }
            Divider()
        }
        if let pi, model.usesDocumentTree {
            let name = model.featureNodes.first(where: { $0.partIndex == pi })?.name ?? "Part \(pi)"
            Text(name)
            Divider()
            Button("Frame") { ReleaseWindowController.shared.frame(entityNamed: "part\(pi)") }
            Button(model.isolatedPart == pi ? "Exit Isolate" : "Isolate") { model.isolate(part: pi) }
            Button(model.isPartVisible(pi) ? "Hide" : "Show") { model.toggleVisibility(part: pi) }
            Menu("Material") {
                ForEach(MaterialPreset.grouped, id: \.category) { group in
                    Section(group.category.capitalized) {
                        ForEach(group.items) { m in
                            Button(m.name) { model.setPartMaterial(pi, m.key) }
                        }
                    }
                }
            }
            Divider()
        }
        if model.hasHiddenParts {
            Button("Show All Parts") { model.showAllParts() }
        }
        Button("Deselect All") { model.deselectAll() }
            .disabled(model.selectedPartIndex == nil && model.multiSelectedParts.isEmpty)
        Divider()
        Button("Frame All") { model.resetCamera() }
    }

    /// Click a part to select it (⌘-click for the boolean multi-selection,
    /// empty space to deselect) — the released twin of the studio's pick.
    /// Stopwatch for the click path, behind VCAD_PERF=1. Writes one line per
    /// click to /tmp/vcad_perf.log.
    private func perf(_ label: String, _ t0: CFAbsoluteTime) {
        guard perfLogging else { return }
        let ms = (CFAbsoluteTimeGetCurrent() - t0) * 1000
        let line = String(format: "[PERF] %@ %.1f ms\n", label, ms)
        if let fh = FileHandle(forWritingAtPath: "/tmp/vcad_perf.log") {
            fh.seekToEndOfFile(); fh.write(Data(line.utf8)); try? fh.close()
        } else {
            try? Data(line.utf8).write(to: URL(fileURLWithPath: "/tmp/vcad_perf.log"))
        }
    }

    private func tapSelect(at p: CGPoint, additive: Bool) {
        let t0 = CFAbsoluteTimeGetCurrent()
        defer { perf("tapSelect", t0) }

        // Double-click frames the body — read off the CLICK COUNT of the event
        // we are already handling, not a second `SpatialTapGesture(count: 2)`.
        // A separate double-tap gesture forces SwiftUI to disambiguate, so every
        // single click sat waiting out the double-click interval before it could
        // fire: selection measured 0.2 ms of work behind ~half a second of
        // nothing. This is also how AppKit does it — the first click selects,
        // the second frames.
        if (NSApp.currentEvent?.clickCount ?? 1) >= 2, !model.sketching,
           let name = bodyName(at: p) {
            ReleaseWindowController.shared.frame(entityNamed: name)
        }
        guard model.usesDocumentTree,
              let ar = ReleaseWindowController.shared.arView else { return }

        // An armed palette tool consumes the click: this is a placement, not a
        // selection (BCB: pick the component, then click where it goes).
        if let shape = model.armedShape {
            if let point = placementPoint(at: p) {
                model.addPrimitive(shape, at: point)
            } else {
                model.addPrimitive(shape)
            }
            model.disarm()
            NSCursor.arrow.set()
            return
        }

        // Assemblies draw instances; a click on a link selects that instance and
        // walks the tree to the part def it draws.
        if model.assemblyInstanceCount > 0 {
            if let e = ReleaseWindowController.shared.hits(atViewPoint: p)
                .first(where: { $0.entity.name.hasPrefix("inst") })?.entity,
               let i = Int(e.name.dropFirst(4)) {
                model.selectInstance(i)
            } else if !additive {
                model.deselectAll()
            }
            return
        }

        // Every part under the pointer, nearest first. Clicking the same spot
        // twice walks INTO the stack instead of re-selecting the front part —
        // without it, a part inside an enclosure is unreachable without hiding
        // the shell.
        let stack = ReleaseWindowController.shared.hits(atViewPoint: p).compactMap { hit -> Int? in
            guard hit.entity.name.hasPrefix("part") else { return nil }
            return Int(hit.entity.name.dropFirst(4))
        }
        var depth = 0
        if let a = cycleAnchor, hypot(a.x - p.x, a.y - p.y) < 5, !stack.isEmpty {
            depth = (cycleDepth + 1) % stack.count
        }
        cycleAnchor = p
        cycleDepth = depth

        if depth < stack.count,
           let fid = model.featureNodes.first(where: { $0.partIndex == stack[depth] })?.id {
            // Modifiers come from the gesture (.modifiers(.command)), not
            // NSEvent.modifierFlags — that reads hardware key state and misses
            // synthetic events (and some tap orderings).
            if additive {
                model.toggleMultiSelect(part: stack[depth], featureID: fid)
            } else {
                model.selectFeature(fid)
            }
        } else if !additive {
            model.deselectAll()
        }
        model.selectionDirty = false   // released view rebuilds via geometryKey
    }

    /// Where an armed primitive should land: on the surface under the pointer if
    /// there is one, else on the ground plane (kernel z = 0). Falls back to the
    /// origin when the ray misses everything (a camera looking at the horizon).
    private func placementPoint(at p: CGPoint) -> SIMD3<Float>? {
        if let centering = ReleaseWindowController.shared.centeringEntity,
           let hit = ReleaseWindowController.shared.hits(atViewPoint: p)
               .first(where: { $0.entity.name.hasPrefix("part") }) {
            return centering.convert(position: hit.position, from: nil)
        }
        guard let ray = kernelRay(at: p), abs(ray.d.z) > 1e-5 else { return nil }
        let t = -ray.o.z / ray.d.z
        guard t > 0 else { return nil }
        return ray.o + ray.d * t
    }

    private var marqueeOrOrbitGesture: some Gesture {
        DragGesture(minimumDistance: 2)
            .onChanged { value in
                // ⌘-drag from empty space is a rubber band, matching ⌘-click's
                // "add to the selection". A plain drag stays an orbit: rebinding
                // orbit to a modifier to make room for marquee would break the
                // muscle memory of every 3D app.
                if marquee != nil || (model.lastDrag == .zero && !model.draggingHandle
                                      && model.usesDocumentTree
                                      && NSEvent.modifierFlags.contains(.command)
                                      && partIndex(at: value.startLocation) == nil) {
                    marquee = (value.startLocation, value.location)
                    return
                }
                if model.lastDrag == .zero && !model.draggingHandle {
                    // A drag that starts on a gizmo handle is a part transform,
                    // not an orbit — same split the studio's targeted gesture
                    // makes, done here via hitTest (the clear layer owns events).
                    if let name = ReleaseWindowController.shared.hits(atViewPoint: value.startLocation)
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
                if let m = marquee {
                    commitMarquee(from: m.start, to: m.current)
                    marquee = nil
                    model.lastDrag = .zero
                    return
                }
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

    /// Select every part whose projected centre falls inside the rubber band.
    /// Centres, not silhouettes: a projected bounding box would catch a long
    /// part the band merely grazes, which reads as selecting the wrong thing.
    private func commitMarquee(from a: CGPoint, to b: CGPoint) {
        let r = CGRect(x: min(a.x, b.x), y: min(a.y, b.y),
                       width: abs(b.x - a.x), height: abs(b.y - a.y))
        guard r.width > 3, r.height > 3 else { return }
        let ctl = ReleaseWindowController.shared
        let caught = (0..<model.partCount).filter { i in
            guard model.isPartVisible(i),
                  let p = ctl.viewPoint(forPart: i, from: model.cameraPosition,
                                        lookAt: model.panOffset) else { return false }
            return r.contains(p)
        }
        model.selectParts(caught)
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
    /// The model's own "the geometry changed" flag, the same one the studio
    /// rebuilt on. `geometryKey` alone cannot carry a document swap: opening a
    /// document (or the gripper) leaves every field the key is built from
    /// untouched until the new scene is built, so the key never changed, the
    /// rebuild never ran, and the old part stayed on screen — the geometry was
    /// waiting on a rebuild that was waiting on the geometry.
    let geometryDirty: Bool
    let selection: Set<Int>
    let selectedInstances: Set<Int>
    let visibility: VisibilityState
    let geometryKey: String

    final class Coordinator {
        var camera: PerspectiveCamera?
        var anchor: AnchorEntity?
        var builtKey = ""
        var builtSelection: Set<Int> = []
        var builtInstances: Set<Int> = []
        var builtVisibility = VisibilityState(hidden: [], isolated: nil)
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
        // Selecting a part used to rebuild the whole scene: `highlightedParts`
        // was part of `geometryKey`, so a click re-evaluated the document,
        // re-tessellated every part, rebuilt every entity and regenerated every
        // collider — to change one material's emissive. That was the click lag.
        if context.coordinator.builtSelection != selection
            || context.coordinator.builtInstances != selectedInstances {
            let t0 = CFAbsoluteTimeGetCurrent()
            context.coordinator.builtSelection = selection
            context.coordinator.builtInstances = selectedInstances
            ReleaseWindowController.shared.repaintAll()
            let tPaint = CFAbsoluteTimeGetCurrent()
            rebuildGizmo(context.coordinator)
            if perfLogging {
                let line = String(format: "[PERF] selectionUpdate repaint %.1f ms gizmo %.1f ms\n",
                                  (tPaint - t0) * 1000,
                                  (CFAbsoluteTimeGetCurrent() - tPaint) * 1000)
                if let fh = FileHandle(forWritingAtPath: "/tmp/vcad_perf.log") {
                    fh.seekToEndOfFile(); fh.write(Data(line.utf8)); try? fh.close()
                }
            }
        }
        if context.coordinator.builtVisibility != visibility {
            context.coordinator.builtVisibility = visibility
            applyVisibility()
        }
        if geometryDirty || context.coordinator.builtKey != geometryKey {
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
        // Constant on-screen gizmo size, the way every desktop CAD tool does it.
        // The studio rescaled the gizmo on every camera update and the released
        // view never did, so the handles were sized for whatever distance the
        // camera happened to be at when the scene was built — comically large
        // once you zoomed in, invisible once you pulled back.
        if let gizmo = ReleaseWindowController.shared.centeringEntity?
            .findEntity(named: "gizmoRoot") {
            gizmo.scale = SIMD3<Float>(repeating: gizmoScreenScale(model: model))
        }
    }

    /// The gizmo follows the selection, and the selection no longer rebuilds the
    /// scene — so it has to be re-hung on its own. It is a handful of entities;
    /// rebuilding it is nothing like rebuilding the model.
    private func rebuildGizmo(_ c: Coordinator) {
        guard let centering = ReleaseWindowController.shared.centeringEntity else { return }
        centering.findEntity(named: "gizmoRoot")?.removeFromParent()
        guard model.showsGizmo else { return }
        let gizmo = buildGizmo(model: model)
        gizmo.scale = SIMD3<Float>(repeating: gizmoScreenScale(model: model))
        centering.addChild(gizmo)
    }

    /// Show/hide parts in place — `isEnabled` on the built entities, no rebuild.
    private func applyVisibility() {
        guard let centering = ReleaseWindowController.shared.centeringEntity else { return }
        for i in 0..<model.partCount {
            centering.findEntity(named: "part\(i)")?.isEnabled = model.isPartVisible(i)
        }
    }

    private func rebuild(in anchor: AnchorEntity?, coordinator: Coordinator) {
        guard let anchor else { return }
        let tRebuild = CFAbsoluteTimeGetCurrent()
        defer {
            if perfLogging,
               let fh = FileHandle(forWritingAtPath: "/tmp/vcad_perf.log") {
                let ms = (CFAbsoluteTimeGetCurrent() - tRebuild) * 1000
                fh.seekToEndOfFile()
                fh.write(Data(String(format: "[PERF] REBUILD %.1f ms\n", ms).utf8))
                try? fh.close()
            }
        }
        coordinator.builtKey = geometryKey
        coordinator.builtSelection = model.highlightedParts
        coordinator.builtInstances = model.selectedInstances
        coordinator.builtVisibility = VisibilityState(hidden: model.hiddenParts,
                                                      isolated: model.isolatedPart)
        model.geometryDirty = false
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

        ReleaseWindowController.shared.repaintHover()

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

/// The Component Palette, after Borland C++ Builder: a speedbar of document
/// actions, a tab strip, a dense grid of icon tools, and a hint line that
/// explains whatever the pointer is over. BCB's palette was icon-only and
/// stable — same tile in the same place every time you reach for it — with the
/// words living in the hint bar rather than under every glyph. That is what is
/// modernised here (SF Symbols, a segmented tab strip, HIG focus rings), not
/// the 1997 bevels.
struct ComponentPaletteWindow: View {
    @Bindable var model: EditorModel
    /// What the pointer is over, for the hint line. BCB put hints in the status
    /// bar; a palette-local line keeps the explanation next to the thing.
    @State private var hovered: Tool.ID?
    @State private var hoveredAction: String?

    private let paletteWidth: CGFloat = 236
    private let tile: CGFloat = 34

    var body: some View {
        ToolWindow(title: "Components", onClose: { model.showsPalette = false }) {
            VStack(alignment: .leading, spacing: 8) {
                speedbar
                Divider()
                tabStrip
                toolGrid
                hintLine
            }
            .frame(width: paletteWidth, alignment: .leading)
            .background {
                Button("") { model.disarm() }
                    .keyboardShortcut(.cancelAction)
                    .frame(width: 0, height: 0).opacity(0)
                    .accessibilityHidden(true)
                    .disabled(model.armedShape == nil)
            }
        }
    }

    // MARK: speedbar — the document actions, not components

    /// BCB kept the speedbar (open/save/run) separate from the palette proper;
    /// mixing them put "export USDZ" one pixel from "add a cube". Same split
    /// here, and every action is also a menu item with a key equivalent.
    private var speedbar: some View {
        HStack(spacing: 2) {
            speedButton("Undo", "arrow.uturn.backward", enabled: model.canUndo,
                        hint: "Undo the last edit (⌘Z)") { model.undo() }
            speedButton("Redo", "arrow.uturn.forward", enabled: model.canRedo,
                        hint: "Redo (⇧⌘Z)") { model.redo() }
            speedDivider
            speedButton("Zebra", "line.3.horizontal", enabled: true, active: model.zebraMode,
                        hint: "Zebra curvature analysis (Z)") { model.zebraMode.toggle() }
            speedDivider
            speedButton("Export STL", "square.and.arrow.up", enabled: model.canExport,
                        hint: "Export the document as STL (⌘E)") {
                exportPanel(ext: "stl") { model.exportSTL(to: $0) }
            }
            speedButton("Export USDZ", "arkit", enabled: model.canExport,
                        hint: "Export as USDZ for Quick Look (⇧⌘E)") {
                exportPanel(ext: "usdz") { model.exportUSDZ(to: $0) }
            }
            Spacer(minLength: 0)
        }
    }

    private var speedDivider: some View {
        Rectangle().fill(.separator).frame(width: 1, height: 16).padding(.horizontal, 3)
    }

    // MARK: tabs

    private var tabStrip: some View {
        Picker("", selection: $model.toolTab) {
            ForEach(availableTabs) { t in Text(t.label).tag(t) }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
        // Number keys switch tabs, as they did on the studio palette. Zero-sized
        // rather than absent: a shortcut only fires while its button is in the
        // view hierarchy.
        .background {
            ForEach(Array(availableTabs.enumerated()), id: \.element) { idx, tab in
                Button("") { model.toolTab = tab }
                    .keyboardShortcut(KeyEquivalent(Character("\(idx + 1)")), modifiers: [])
                    .frame(width: 0, height: 0)
                    .opacity(0)
                    .accessibilityHidden(true)
            }
        }
    }

    // MARK: the palette proper

    private var toolGrid: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: tile), spacing: 4, alignment: .leading)],
                  alignment: .leading, spacing: 4) {
            ForEach(model.tools(for: activeTab)) { tool in
                toolTile(tool)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .animation(Motion.snappy, value: activeTab)
    }

    private func toolTile(_ tool: Tool) -> some View {
        Button(action: tool.action) {
            Image(systemName: tool.symbol)
                .font(.system(size: 15))
                .frame(width: tile, height: tile)
                .foregroundStyle(tool.isActive ? AnyShapeStyle(Color.accentColor)
                                 : tool.enabled ? AnyShapeStyle(.primary)
                                 : AnyShapeStyle(.tertiary))
                .background(background(active: tool.isActive, hovered: hovered == tool.id && tool.enabled),
                            in: RoundedRectangle(cornerRadius: 7, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .strokeBorder(tool.isActive ? Color.accentColor.opacity(0.55) : .clear,
                                  lineWidth: 1))
                // .plain buttons only hit-test opaque pixels; without this,
                // clicks in the transparent padding do nothing.
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!tool.enabled)
        .onHover { inside in
            hovered = inside ? tool.id : (hovered == tool.id ? nil : hovered)
        }
        .help(tool.hint.isEmpty ? tool.label : "\(tool.label) — \(tool.hint)")
        .accessibilityLabel(tool.label)
    }

    private func background(active: Bool, hovered: Bool) -> AnyShapeStyle {
        if active { return AnyShapeStyle(.selection) }
        if hovered { return AnyShapeStyle(.quaternary) }
        return AnyShapeStyle(.clear)
    }

    // MARK: hint line

    /// One line, always present (never collapsing — a palette that changes
    /// height as the pointer moves is a palette you cannot aim at).
    private var hintLine: some View {
        Text(hintText)
            .font(.system(size: 10))
            .foregroundStyle(hintIsBlocked ? AnyShapeStyle(Color.orange)
                             : model.armedShape != nil ? AnyShapeStyle(Color.accentColor)
                             : AnyShapeStyle(.secondary))
            .lineLimit(1)
            .truncationMode(.tail)
            .frame(width: paletteWidth, height: 13, alignment: .leading)
            .animation(nil, value: hintText)
    }

    private var hoveredTool: Tool? {
        model.tools(for: activeTab).first { $0.id == hovered }
    }

    private var hintIsBlocked: Bool {
        if let t = hoveredTool { return !t.enabled }
        return false
    }

    private var hintText: String {
        if let s = model.armedShape { return "Click in the scene to place \(s.label) · Esc" }
        if let a = hoveredAction { return a }
        guard let t = hoveredTool else { return activeTab.paletteHint }
        return t.hint.isEmpty ? t.label : "\(t.label) — \(t.hint)"
    }

    /// Combine only exists for documents (needs two selected parts).
    private var availableTabs: [ToolTab] {
        model.usesDocumentTree ? ToolTab.allCases : [.create, .modify]
    }
    private var activeTab: ToolTab {
        availableTabs.contains(model.toolTab) ? model.toolTab : .create
    }

    private func speedButton(_ label: String, _ symbol: String, enabled: Bool,
                             active: Bool = false, hint: String,
                             action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 12))
                .frame(width: 26, height: 22)
                .foregroundStyle(active ? AnyShapeStyle(Color.accentColor)
                                 : enabled ? AnyShapeStyle(.secondary)
                                 : AnyShapeStyle(.tertiary))
                .background(active ? AnyShapeStyle(.selection) : AnyShapeStyle(.clear),
                            in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
        .onHover { inside in
            hoveredAction = inside ? (enabled ? hint : "\(label) — unavailable") : nil
        }
        .help(hint)
        .accessibilityLabel(label)
    }

    private func exportPanel(ext: String, _ export: @escaping (URL) -> Bool) {
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "\(model.documentName).\(ext)"
        panel.begin { resp in
            guard resp == .OK, let url = panel.url else { return }
            _ = export(url)
        }
    }
}

/// The Object Inspector: what the selected feature is, its live parameters,
/// the document's named parameters, camera, and measurements.
struct ObjectInspectorWindow: View {
    @Bindable var model: EditorModel

    var body: some View {
        ToolWindow(title: "Object Inspector", onClose: { model.showsInspector = false }) {
            VStack(alignment: .leading, spacing: 8) {
                if model.usesDocumentTree {
                    documentSections
                } else {
                    sandboxSection
                }
                if !model.docParameters.isEmpty {
                    Divider()
                    header("Document Parameters")
                    Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                        docParameterRows
                    }
                }
                Divider()
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                    viewRows
                    Divider().gridCellUnsizedAxes(.horizontal)
                    measurementRows
                }
            }
            .frame(width: 236, alignment: .leading)
        }
    }

    // MARK: document (a .vcad's feature tree)

    /// The selected feature: what it is, what it is made of, and — for the ops
    /// that declare editable parameters — live scrub fields. This is the studio
    /// inspector's job, and it is the same `FeatureParamEditors` doing it, so a
    /// newly editable op shows up in both places at once.
    @ViewBuilder private var documentSections: some View {
        if let ii = model.selectedInstanceIndex {
            header(model.instanceName(ii) ?? "Instance \(ii)")
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                row("Kind", "Assembly instance")
                if let def = model.instancePartDefName(ii) { row("Part", def) }
            }
            if let node = model.instanceFeatureNode(ii) {
                Divider()
                // The geometry an instance draws lives in its part def, so the
                // editors act on the definition: change the link's cube here and
                // every instance of that link follows.
                header("\(node.name) — shared by every instance")
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                    row("Operation", DocumentGraph.label(node.opType))
                    if InspectorView.editableOps.contains(node.opType) {
                        FeatureParamEditors(model: model, node: node) { snapshot in
                            ReleaseWindowController.shared.refreshGeometry(model: model,
                                                                           collisions: snapshot)
                        }
                    }
                }
            }
        } else if let node = model.selectedFeatureNode {
            header(node.name)
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                row("Operation", DocumentGraph.label(node.opType))
                if let pi = node.partIndex {
                    GridRow {
                        Text("Material").font(.system(size: 11)).foregroundStyle(.secondary)
                        materialMenu(pi)
                    }
                    if !model.isPartVisible(pi) {
                        GridRow {
                            Text("Visibility").font(.system(size: 11)).foregroundStyle(.secondary)
                            Label("Hidden", systemImage: "eye.slash").font(.system(size: 11))
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
            if InspectorView.editableOps.contains(node.opType) {
                Divider()
                header("Parameters")
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                    // Edits re-solve the bound nodes; the released scene swaps
                    // meshes in place (collisions only on commit — per-tick
                    // regen stutters big documents).
                    FeatureParamEditors(model: model, node: node) { snapshot in
                        ReleaseWindowController.shared.refreshGeometry(model: model,
                                                                       collisions: snapshot)
                    }
                }
            } else if let d = node.detail {
                Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
                    row("Value", d)
                }
            }
        } else {
            Text("Select a feature in the tree")
                .font(.system(size: 11)).foregroundStyle(.tertiary)
        }
    }

    // MARK: sandbox (the built-in primitive + modifier)

    @ViewBuilder private var sandboxSection: some View {
        Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 7) {
            row("Shape", model.baseShape.label)
            row("Modifier", model.modifier.label)
            if model.modifier != .none && model.modifierEffective {
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
            } else if model.modifier != .none {
                GridRow {
                    Text("").font(.system(size: 11))
                    Label("No edges on a sphere", systemImage: "info.circle")
                        .font(.system(size: 11)).foregroundStyle(.secondary)
                }
            }
        }
    }

    @ViewBuilder private var docParameterRows: some View {
        ForEach(model.docParameters) { p in
            GridRow {
                Text(p.name).font(.system(size: 11)).foregroundStyle(.secondary)
                    .help(p.description ?? p.name)
                if let v = p.value {
                    ScrubField(label: "", value: v, unit: p.unit ?? "mm",
                               sensitivity: InspectorView.paramSensitivity(p),
                               minValue: p.min ?? -.greatestFiniteMagnitude) { v, s in
                        model.editParameter(p.name, value: v, snapshot: s)
                        // Collisions on typed commits / scrub start only —
                        // per-tick regen would stutter big docs.
                        ReleaseWindowController.shared.refreshGeometry(model: model, collisions: s)
                    }
                    .frame(width: 140)
                } else if let f = p.formula {
                    Text("= \(f)").font(.system(size: 11).monospacedDigit())
                        .foregroundStyle(.tertiary)
                }
            }
        }
    }

    @ViewBuilder private var viewRows: some View {
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
                .frame(width: 140)
        }
    }

    @ViewBuilder private var measurementRows: some View {
        row("Triangles", model.triangleCount.formatted())
        row("Bounds", String(format: "%.1f × %.1f × %.1f mm",
                             abs(model.sizeMM.x), abs(model.sizeMM.y), abs(model.sizeMM.z)))
        row("Solve", String(format: "%.1f ms", model.solveMillis))
        if let info = model.pickInfo {
            GridRow {
                Text("Picked").font(.system(size: 11)).foregroundStyle(.secondary)
                Text(info).font(.system(size: 11).monospacedDigit())
                    .foregroundStyle(.secondary)
                    .frame(width: 140, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func header(_ t: String) -> some View {
        Text(t.uppercased())
            .font(.system(size: 9, weight: .semibold))
            .tracking(0.6)
            .foregroundStyle(.tertiary)
    }

    /// Material assignment for a part — swatch + grouped presets, the studio
    /// inspector's control rather than a bare Picker (which showed a checkmark
    /// against "aluminum" for parts that had no material at all).
    private func materialMenu(_ pi: Int) -> some View {
        let current = model.materialName(forPart: pi) ?? "default"
        let resolved = model.resolvedMaterial(forPart: pi)
        return Menu {
            ForEach(MaterialPreset.grouped, id: \.category) { group in
                Section(group.category.capitalized) {
                    ForEach(group.items) { m in
                        Button { model.setPartMaterial(pi, m.key) } label: {
                            if m.key == current { Label(m.name, systemImage: "checkmark") }
                            else { Text(m.name) }
                        }
                    }
                }
            }
        } label: {
            HStack(spacing: 6) {
                Circle().fill(Color(portedColor: resolved.color)).frame(width: 10, height: 10)
                    .overlay(Circle().strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
                Text(MaterialPreset.byKey(current)?.name ?? current.capitalized)
                    .font(.system(size: 11))
                Image(systemName: "chevron.up.chevron.down").font(.system(size: 8)).opacity(0.5)
            }
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
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

/// The feature tree / history, as a closable tool window. It carried no title
/// or close button while every other panel had both.
struct FeatureTreeWindow: View {
    @Bindable var model: EditorModel

    var body: some View {
        ToolWindow(title: model.usesDocumentTree ? "Features" : "History",
                   onClose: { model.showsTree = false }) {
            ScrollViewReader { proxy in
                ScrollView {
                    FeatureTreeView(model: model)
                }
                // Clicking geometry selects a row that may be scrolled out of
                // sight — a sync you cannot see is not a sync.
                .onChange(of: model.selectedFeatureID) { _, id in
                    guard let id else { return }
                    withAnimation(Motion.snappy) { proxy.scrollTo(id, anchor: .center) }
                }
            }
            .frame(width: 230)
            .frame(maxHeight: 380)
            .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// The cross-domain Receipt, in the inspector's slot. The gripper is neither a
/// sandbox primitive nor a feature-tree document, so the Object Inspector has
/// nothing true to say about it — the studio swapped the whole inspector for
/// this ledger, and so does the released desktop.
struct ReceiptLedgerWindow: View {
    @Bindable var model: EditorModel

    var body: some View {
        ToolWindow(title: "Receipt", onClose: { model.showsInspector = false }) {
            ReceiptLedger(model: model)
                .frame(width: 264)
                .fixedSize(horizontal: false, vertical: true)
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


/// Opt-in click/frame timing (VCAD_PERF=1), read once rather than per event.
let perfLogging = ProcessInfo.processInfo.environment["VCAD_PERF"] == "1"

/// Which parts are hidden — an input to the released view, so a visibility
/// change repaints rather than rebuilds.
struct VisibilityState: Equatable {
    let hidden: Set<Int>
    let isolated: Int?
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
                       geometryDirty: model.geometryDirty,
                       // Selection and visibility are appearance, not geometry:
                       // they ride as their own inputs so `updateNSView` runs and
                       // repaints, instead of changing the key and rebuilding.
                       selection: model.highlightedParts,
                       selectedInstances: model.selectedInstances,
                       visibility: VisibilityState(hidden: model.hiddenParts,
                                                   isolated: model.isolatedPart),
                       geometryKey: geometryKey)
    }
}


/// The only chrome that is never closable: show/hide the tool windows. (It
/// replaces the old "Return to Studio" pill — there is no studio to return to,
/// release-to-desktop being the app's only mode.)
struct PanelsPill: View {
    @Bindable var model: EditorModel

    private var anyShown: Bool { model.showsPalette || model.showsTree || model.showsInspector }

    var body: some View {
        Button {
            withAnimation(Motion.panel) {
                model.setPanels(shown: !anyShown)
            }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: anyShown ? "sidebar.squares.left" : "sidebar.squares.leading")
                    .font(.system(size: 11, weight: .semibold))
                Text(anyShown ? "Hide Panels" : "Show Panels")
                    .font(.system(size: 12, weight: .medium))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(.ultraThinMaterial, in: Capsule())
        }
        .buttonStyle(.plain)
        .help("Show or hide the floating tool windows (⌘⇧Space)")
    }
}

// MARK: - Live Dock icon
//
// Release-to-desktop has no window to represent it in Mission Control and no
// thumbnail anywhere — the app IS the parts floating over your desktop. So the
// Dock tile becomes the viewport: whatever you are looking at, shrunk. Orbit
// the model and the icon orbits with it.

/// Keeps `NSApp.applicationIconImage` in step with the released viewport.
@MainActor
final class DockIcon {
    static let shared = DockIcon()

    /// The tile the app launched with, restored when there is nothing to show.
    private lazy var appIcon: NSImage? = NSImage(named: "NSApplicationIcon")
    private var timer: Timer?
    private var inFlight = false
    /// What the tile currently depicts. The Dock redraws on every assignment,
    /// so a still scene must stop feeding it identical frames.
    private var shownKey = ""

    /// Start following the viewport. Cheap when nothing changes: the capture
    /// only runs when the scene key moves, and never more than twice a second —
    /// a Dock tile is 128 points and nobody is reading it at 60 fps.
    func follow(model: EditorModel) {
        guard timer == nil else { return }
        let t = Timer.scheduledTimer(withTimeInterval: 0.5, repeats: true) { [weak self] _ in
            Task { @MainActor in self?.tick(model: model) }
        }
        // Common mode: the default run-loop mode stops firing while a menu is
        // open or a window is being dragged, and a frozen Dock icon during an
        // orbit is exactly when someone would look at it.
        RunLoop.main.add(t, forMode: .common)
        timer = t
    }

    func stop() {
        timer?.invalidate()
        timer = nil
        if let appIcon { NSApp.applicationIconImage = appIcon }
    }

    private func tick(model: EditorModel) {
        guard !inFlight, let ar = ReleaseWindowController.shared.arView else { return }
        // Everything that changes what the viewport looks like. Camera included:
        // an orbit changes nothing about the document and everything about the
        // picture.
        // Distance and pan are deliberately absent: the tile crops to the
        // model, so zooming and panning cannot change the picture. Only
        // rotation does.
        let key = [
            String(describing: model.source), String(model.triangleCount),
            String(format: "%.2f,%.2f", model.azimuth, model.elevation),
            String(describing: model.highlightedParts.sorted()),
            String(describing: model.selectedInstances.sorted()),
            String(model.poseTick),                 // a running simulation
        ].joined(separator: "|")
        guard key != shownKey else { return }
        guard model.triangleCount > 0 else { return }   // nothing to depict yet

        inFlight = true
        ar.snapshot(saveToHDR: false) { [weak self] image in
            // Reduce to a CGImage here, in the callback: NSImage is not
            // Sendable, so it must not cross into the main-actor hop.
            let cg = image?.cgImage(forProposedRect: nil, context: nil, hints: nil)
            Task { @MainActor in
                defer { self?.inFlight = false }
                guard let self, let cg, let tile = Self.tile(from: cg) else { return }
                NSApp.applicationIconImage = tile
                self.shownKey = key
            }
        }
    }

    /// Fit the capture into a square tile, framed on the MODEL rather than on
    /// the viewport.
    ///
    /// Cropping to the centre of the frame ties the icon to the camera: zoom out
    /// and the model becomes a speck in the tile, zoom in and it gets sliced.
    /// The icon should be a portrait of the document, not a screenshot — so this
    /// finds the drawn content and frames that. The viewport renders on a clear
    /// background, which makes the alpha channel an exact silhouette mask; the
    /// content bounds fall straight out of it.
    private static func tile(from cg: CGImage) -> NSImage? {
        guard let content = contentBounds(cg) else { return nil }
        // Square up around the content's centre, then breathe: a subject that
        // touches all four edges reads as cropped rather than composed.
        let side = max(content.width, content.height) * 1.18
        let square = CGRect(x: content.midX - side / 2, y: content.midY - side / 2,
                            width: side, height: side)
            .intersection(CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
        let cropped = cg.cropping(to: square.integral) ?? cg

        let size = NSSize(width: 512, height: 512)
        let out = NSImage(size: size)
        out.lockFocus()
        NSGraphicsContext.current?.imageInterpolation = .high
        // Aspect-fit, centred: `square` can come back non-square after the
        // clamp (a model against the edge of the viewport), and stretching it
        // to the tile would shear the part.
        let scale = min(size.width / CGFloat(cropped.width), size.height / CGFloat(cropped.height))
        let w = CGFloat(cropped.width) * scale, h = CGFloat(cropped.height) * scale
        NSImage(cgImage: cropped, size: .zero)
            .draw(in: NSRect(x: (size.width - w) / 2, y: (size.height - h) / 2, width: w, height: h),
                  from: .zero, operation: .copy, fraction: 1)
        out.unlockFocus()
        return out
    }

    /// The bounding box of everything the renderer drew, in `cg`'s pixel space.
    ///
    /// Scanned on a downsampled copy — a Dock tile does not need the silhouette
    /// to the pixel, and scanning six megapixels twice a second would cost more
    /// than the render it is framing.
    private static func contentBounds(_ cg: CGImage) -> CGRect? {
        let n = 96
        var alpha = [UInt8](repeating: 0, count: n * n)
        guard let ctx = CGContext(data: &alpha, width: n, height: n, bitsPerComponent: 8,
                                  bytesPerRow: n, space: CGColorSpaceCreateDeviceGray(),
                                  bitmapInfo: CGImageAlphaInfo.alphaOnly.rawValue)
        else { return nil }
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: n, height: n))

        var minX = n, minY = n, maxX = -1, maxY = -1
        for y in 0..<n {
            for x in 0..<n where alpha[y * n + x] > 12 {   // ignore antialiased dust
                if x < minX { minX = x }
                if x > maxX { maxX = x }
                if y < minY { minY = y }
                if y > maxY { maxY = y }
            }
        }
        guard maxX >= minX, maxY >= minY else { return nil }   // nothing drawn

        // MIND THE ORIGIN. `CGContext` draws bottom-up, so row 0 of the buffer
        // is the BOTTOM of the image, while `CGImage.cropping(to:)` measures
        // from the top. Using the scan rows directly would crop the mirror
        // image of the model — invisible on a centred subject, wrong the moment
        // it sits off centre.
        let top = n - 1 - maxY
        let bottom = n - 1 - minY
        let sx = CGFloat(cg.width) / CGFloat(n), sy = CGFloat(cg.height) / CGFloat(n)
        // The scan grid is coarse, so pad by one cell rather than clipping the
        // edge of the silhouette.
        return CGRect(x: CGFloat(minX - 1) * sx, y: CGFloat(top - 1) * sy,
                      width: CGFloat(maxX - minX + 3) * sx,
                      height: CGFloat(bottom - top + 3) * sy)
            .intersection(CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
    }
}
