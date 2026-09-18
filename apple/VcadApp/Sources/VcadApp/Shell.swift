import SwiftUI
import RealityKit
#if canImport(AppKit)
import AppKit
#endif
import simd
import CoreGraphics
import UniformTypeIdentifiers

// The Mac shell's entry point, menu bar, file panels, and the SwiftUI pieces
// every workspace shares: the feature tree, the parameter editors, the scrub
// field, the sketch bars and the playback transport. The window itself is
// built in ReleasedDesktop.swift; the Design workspace's panels live in
// DesignShell.swift. Styling comes from Theme.swift, scene assets from
// SceneAssets.swift.

#if os(macOS)  // mac window root + document menu
/// SwiftUI lifecycle host. The visible editor lives in a single AppKit window
/// that can switch between a desktop overlay and a normal resizable window.
struct EditorView: View {
    let model: EditorModel
    let intent: IntentEngine

    var body: some View {
        Color.clear
            .frame(width: 1, height: 1)
            .background(HostWindowHider {
                ReleaseWindowController.shared.show(model: model, intent: intent)
            })
            .task {
                    // Dev hook: VCAD_GRIPPER=1 [VCAD_CONNECTOR_X=n] launches into
                    // the cross-domain gripper (used to verify without driving the UI).
                    let env = ProcessInfo.processInfo.environment
                    AppInstance.currentModel = model
                    // `kill -USR1 <pid>` writes what the app is doing to a
                    // file: the editor window is borderless, so there is
                    // nothing to query from outside (friction-log item 31).
                    // Installed here rather than at launch because this is
                    // where the model first exists.
                    VcadStatus.installSignalHandler(model)
                    // …and its write counterpart, which exists only when
                    // `VCAD_SET` names a request file (item 51). The dump is a
                    // report and stays one; this is the one way in, it writes
                    // named numbers and nothing else, and it refuses while the
                    // machine is streaming.
                    VcadFieldWriter.installSignalHandler(model)
                    // Offline native CNC smoke hook. Never contacts hardware.
                    if env["VCAD_CNC_DEMO"] == "1" {
                        model.workspace = .manufacture
                        model.cnc.select(.operation(model.cnc.selectedOperation.id))
                        model.cnc.generate()
                        model.cnc.machine.connect(simulated: true)
                    }
                    // Pick up documents other instances opened while this one
                    // was already running.
                    model.refreshRecents()
                    // The document this instance was launched for: the argument
                    // a spawning instance passed, the VCAD_OPEN dev hook, or a
                    // path on the command line.
                    if let url = AppInstance.launchDocument() {
                        model.openDocument(url)
                    }
                    // Files that arrived before this view existed: the first
                    // document takes this (scratch) instance, outlines join it,
                    // any further documents get instances of their own.
                    for url in AppInstance.pendingOpen { AppInstance.opening(url, from: model) }
                    AppInstance.pendingOpen = []
                    // Dev hook: VCAD_SIM=1 builds the physics simulation for the
                    // opened document and starts it running, so the whole path
                    // (enableSimulation -> prepare -> stepping engine -> instance
                    // transforms -> renderer) can be verified without driving the
                    // UI. Same purpose as VCAD_GRIPPER/VCAD_ROUTE below.
                    if env["VCAD_SIM"] == "1" {
                        // After the geometry rebuild, or there is no resident
                        // assembly scene to bind against yet.
                        try? await Task.sleep(for: .seconds(2))
                        model.enableSimulation()
                        if model.sim.isReady {
                            model.sim.run()
                        }
                        // VCAD_POLICY=<path> drives with a trained policy.
                        if let pol = env["VCAD_POLICY"], !pol.isEmpty {
                            model.sim.loadPolicy(from: URL(fileURLWithPath: pol))
                        }
                        // VCAD_TRAIN=1 additionally kicks off a short ARS run,
                        // so the training console can be seen working.
                        if env["VCAD_TRAIN"] == "1" {
                            model.sim.trainSpec.ars.iterations = 12
                            model.sim.trainSpec.ars.n_directions = 4
                            model.sim.trainSpec.ars.top_k = 2
                            model.sim.trainSpec.held_out_seeds = 3
                            model.sim.trainSpec.held_out_every = 1
                            model.sim.spec.max_steps = 60
                            model.sim.startTraining()
                        }
                        let line = "[VCAD_SIM] available=\(model.sim.isAvailable) "
                            + "ready=\(model.sim.isReady) actionDim=\(model.sim.actionDim) "
                            + "error=\(model.sim.errorMessage ?? "none")\n"
                        FileHandle.standardError.write(Data(line.utf8))
                    }
                    guard env["VCAD_GRIPPER"] == "1" else { return }
                    model.openGripper()
                    if let x = env["VCAD_CONNECTOR_X"].flatMap(Double.init) { model.connectorX = x }
                    if env["VCAD_ROUTE"] == "1" {
                        let segs = model.routeGripperCopper()
                        // stderr is unbuffered → survives the kill that a buffered
                        // stdout print would lose; a quick end-to-end FFI smoke test.
                        let line = "[VCAD_ROUTE] connector_x=\(Int(model.connectorX)) "
                            + "segments=\(segs.count) unrouted=\(model.copperUnrouted)\n"
                        FileHandle.standardError.write(Data(line.utf8))
                    }
            }
    }
}

/// Hides the WindowGroup's window and opens the released overlay in its place.
/// Done from an NSViewRepresentable rather than a `.task` because the overlay
/// controller needs the host window to exist (it hides it), and a view's window
/// is nil until AppKit has attached it — the old code waited out that race with
/// a 1s sleep, which is exactly the studio flash this removes.
struct HostWindowHider: NSViewRepresentable {
    /// Called once, on the main actor, as soon as the host window exists.
    let onAttach: () -> Void

    func makeNSView(context: Context) -> NSView { HiderView(onAttach: onAttach) }
    func updateNSView(_ view: NSView, context: Context) {}

    final class HiderView: NSView {
        private let onAttach: () -> Void
        private var fired = false
        init(onAttach: @escaping () -> Void) {
            self.onAttach = onAttach
            super.init(frame: .zero)
        }
        required init?(coder: NSCoder) { fatalError("unused") }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let w = window, !fired else { return }
            fired = true
            // Zero alpha first: orderOut alone can still flash a frame if the
            // window was ordered in earlier in this pass.
            w.alphaValue = 0
            w.isRestorable = false
            w.orderOut(nil)
            onAttach()
        }
    }
}

// MARK: - Menu bar

/// The app's real menu bar. Every action the app can take is here — so the
/// Help menu's search finds it, every key equivalent works no matter which
/// floating panel has focus, and nothing hides behind a bare key.
///
/// Key equivalents keep clear of the system's: ⌘W closes, ⇧⌘W is not
/// re-bound, ⌃-digits stay with Mission Control, and nothing fires on an
/// unmodified letter.
struct DocumentCommands: Commands {
    @Bindable var model: EditorModel
    @Bindable var intent: IntentEngine

    var body: some Commands {
        CommandGroup(replacing: .newItem) {
            // ⌘N opens another COPY OF THE APP, so the new document gets its own
            // Dock tile and its own ⌘-Tab entry (see AppInstance for why that
            // cannot be a second window).
            Button("New Document") { AppInstance.open() }.keyboardShortcut("n")
            Divider()
            Button("Open…") { openPanel(model) }.keyboardShortcut("o")
            Menu("Open Recent") {
                ForEach(model.recents, id: \.self) { url in
                    Button(url.deletingPathExtension().lastPathComponent) {
                        AppInstance.opening(url, from: model)
                    }
                }
                if !model.recents.isEmpty {
                    Divider()
                    Button("Clear Menu") { model.clearRecents() }
                }
            }
            .disabled(model.recents.isEmpty)
            if !model.examples.isEmpty {
                Menu("Open Example") {
                    ForEach(model.examples, id: \.path) { ex in
                        Button(ex.name) {
                            AppInstance.opening(URL(fileURLWithPath: ex.path), from: model)
                        }
                    }
                }
            }
        }
        CommandGroup(replacing: .saveItem) {
            Button("Close") { ReleaseWindowController.shared.requestClose() }
                .keyboardShortcut("w")
            Divider()
            Button("Save") { model.saveDocument() }
                .keyboardShortcut("s").disabled(!model.usesDocumentTree || !model.documentDirty)
            Button("Save As…") { saveAsPanel(model) }
                .keyboardShortcut("s", modifiers: [.command, .shift])
                .disabled(!model.usesDocumentTree)
            Button("Revert to Saved") { model.revertDocument() }
                .disabled(!model.usesDocumentTree || !model.documentDirty)
            Button("Show in Finder") { model.showInFinder() }
                .disabled(model.documentURL == nil)
            Divider()
            Menu("Export") {
                Button("STL…") { exportSTLPanel(model) }
                    .keyboardShortcut("e").disabled(!model.canExport)
                Button("USDZ…") { exportUSDZPanel(model) }
                    .keyboardShortcut("e", modifiers: [.command, .shift]).disabled(!model.canExport)
            }
        }
        CommandGroup(replacing: .undoRedo) {
            Button(model.undoActionName.map { "Undo \($0)" } ?? "Undo") { model.undo() }
                .keyboardShortcut("z").disabled(!model.canUndo)
            Button(model.redoActionName.map { "Redo \($0)" } ?? "Redo") { model.redo() }
                .keyboardShortcut("z", modifiers: [.command, .shift]).disabled(!model.canRedo)
        }
        // The standard Edit ▸ Delete and Select All items reach the model
        // through the responder chain (KeyableWindow), so a text field keeps
        // them while it is first responder and the parts get them otherwise.
        CommandGroup(after: .pasteboard) {
            Button("Deselect All") { model.deselectAll() }
                .keyboardShortcut("a", modifiers: [.command, .shift]).disabled(!model.hasSelection)
            Divider()
            Button("Rename") { model.beginRenamingSelection() }
                .disabled(model.selectedFeatureNode == nil)
            Divider()
            Button(model.selectedPartIndex.map { model.isPartVisible($0) } ?? true ? "Hide" : "Show") {
                model.toggleSelectedVisibility()
            }
            .keyboardShortcut("h", modifiers: [.command, .shift]).disabled(model.selectedPartIndex == nil)
            Button(model.selectedPartIndex.map { model.isolatedPart == $0 } ?? false ? "Exit Isolate" : "Isolate") {
                model.toggleSelectedIsolate()
            }
            .keyboardShortcut("i", modifiers: [.command, .shift]).disabled(model.selectedPartIndex == nil)
            Button("Show All Parts") { model.showAllParts() }
                .keyboardShortcut("h", modifiers: [.command, .shift, .option]).disabled(!model.hasHiddenParts)
            Divider()
            Button("Describe a Part…") { intent.focusRequested = true }
                .keyboardShortcut("k")
        }
        CommandGroup(after: .sidebar) {
            Divider()
            ForEach(Workspace.allCases) { ws in
                Toggle(ws.label, isOn: Binding(
                    get: { model.workspace == ws },
                    set: { if $0 { model.workspace = ws } }))
                    .keyboardShortcut(ws.keyEquivalent, modifiers: [.command, .control])
            }
            Divider()
            Toggle("Model Navigator", isOn: $model.showsTree)
                .keyboardShortcut("1", modifiers: [.command, .option])
            Toggle("Inspector", isOn: $model.showsInspector)
                .keyboardShortcut("2", modifiers: [.command, .option])
            Button(model.anyPanelShown ? "Hide All Panels" : "Show All Panels") {
                model.setPanels(shown: !model.anyPanelShown)
            }
            .keyboardShortcut("0", modifiers: [.command, .option])
            Divider()
            Toggle("Zebra Analysis", isOn: $model.zebraMode)
                .keyboardShortcut("z", modifiers: [.command, .option])
            Toggle("Ray-Traced Preview", isOn: Binding(
                get: { model.raytraceEnabled },
                set: { model.setRaytrace($0) }))
                .keyboardShortcut("r", modifiers: [.command, .option])
                .disabled(!model.usesDocumentTree)
            Divider()
            Button(model.isWindowed ? "Release to Desktop" : "Open in Window") {
                ReleaseWindowController.shared.setWindowed(!model.isWindowed)
            }
            .keyboardShortcut("d", modifiers: [.command, .shift])
        }
        CommandMenu("Camera") {
            Button("Isometric") { model.animateCamera(to: .isometric) }.keyboardShortcut("1")
            Button("Front") { model.animateCamera(to: .front) }.keyboardShortcut("2")
            Button("Right") { model.animateCamera(to: .right) }.keyboardShortcut("3")
            Button("Top") { model.animateCamera(to: .top) }.keyboardShortcut("4")
            Divider()
            Button("Frame All") { model.resetCamera(animated: true) }.keyboardShortcut("0")
            // Item 55: this and View ▸ Show/Hide All Panels both claimed ⌥⌘0.
            // AppKit shows both and only one fires. Frame Selection is the one
            // that moved, on two grounds: it is the less-used of the two (it
            // needs a selection at all, and is disabled without one), and it
            // has a near neighbour here — ⌘0 Frame All — so ⇧⌘0 reads as
            // "frame, but narrower" beside it. Show/Hide All Panels keeps
            // ⌥⌘0, where it sits with ⌥⌘1 and ⌥⌘2, the other two panel keys.
            Button("Frame Selection") { ReleaseWindowController.shared.frameSelection() }
                .keyboardShortcut("0", modifiers: [.command, .shift]).disabled(!model.hasSelection)
        }
        CommandGroup(replacing: .help) {
            Button("vcad Help") { NSWorkspace.shared.open(URL(string: "https://vcad.io/docs")!) }
            Button("Keyboard Shortcuts") { NSWorkspace.shared.open(URL(string: "https://vcad.io/docs/native/shortcuts")!) }
            Divider()
            // The editor window is borderless — no accessibility window, no
            // Window-menu entry, no title to query — so this and `kill -USR1`
            // are how anything outside the app finds out what it is doing
            // (friction-log item 31).
            Menu("Debug") {
                Button("Dump Status") { _ = VcadStatus.dump(model) }
                Button("Dump Status and Reveal") {
                    if let url = VcadStatus.dump(model) { NSWorkspace.shared.activateFileViewerSelecting([url]) }
                }
            }
            Divider()
            Button("Report an Issue…") { NSWorkspace.shared.open(URL(string: "https://github.com/ecto/vcad/issues/new")!) }
        }
    }
}

// MARK: document panels (shared by the menu bar and the workspaces)

/// The `.vcad` document type, declared in the bundle's Info.plist and used by
/// every open/save panel. Falls back to a dynamic type when run as a bare
/// executable (swift run) with no bundle.
extension UTType {
    static let vcadDocument: UTType =
        UTType("io.vcad.document") ?? UTType(filenameExtension: "vcad") ?? .json
    static let loonSource: UTType =
        UTType("io.vcad.loon") ?? UTType(filenameExtension: "loon") ?? .plainText
}

@MainActor func exportSTLPanel(_ model: EditorModel) {
    savePanel(name: "\(model.documentName).stl", type: UTType(filenameExtension: "stl") ?? .data,
              prompt: "Export") {
        _ = model.exportSTL(to: $0)
    }
}

@MainActor func exportUSDZPanel(_ model: EditorModel) {
    savePanel(name: "\(model.documentName).usdz", type: .usdz, prompt: "Export") {
        _ = model.exportUSDZ(to: $0)
    }
}

@MainActor func saveAsPanel(_ model: EditorModel) {
    savePanel(name: "\(model.documentName).vcad", type: .vcadDocument, prompt: "Save") {
        model.saveDocumentAs($0)
    }
}

@MainActor func openPanel(_ model: EditorModel) {
    let panel = NSOpenPanel()
    panel.allowedContentTypes = [.vcadDocument, .loonSource]
    // Several documents at once are several instances, so a multiple selection
    // is a perfectly good thing to ask for.
    panel.allowsMultipleSelection = true
    panel.canChooseDirectories = false
    panel.prompt = "Open"
    present(panel) {
        for (i, url) in panel.urls.enumerated() {
            // The first one may claim this instance if it is a scratch; the rest
            // always get their own.
            if i == 0 { AppInstance.opening(url, from: model) } else { AppInstance.open(document: url) }
        }
    }
}

@MainActor private func savePanel(name: String, type: UTType, prompt: String,
                                  _ done: @escaping (URL) -> Void) {
    let panel = NSSavePanel()
    panel.allowedContentTypes = [type]
    panel.nameFieldStringValue = name
    panel.prompt = prompt
    panel.canCreateDirectories = true
    present(panel) { if let url = panel.url { done(url) } }
}

/// Run a file panel as a sheet on the editor window when there is one, and
/// app-modal only when released over the desktop (no window to hang it on).
@MainActor func present(_ panel: NSSavePanel, onOK: @escaping () -> Void) {
    if let host = ReleaseWindowController.shared.hostWindow,
       host.styleMask.contains(.titled) {
        panel.beginSheetModal(for: host) { resp in
            if resp == .OK { onOK() }
        }
    } else if panel.runModal() == .OK {
        onOK()
    }
}
#endif  // os(macOS) — mac window root + document menu

/// The leading wordmark.
struct BrandMark: View {
    var body: some View {
        Text("vcad")
            .font(.system(.body, design: .rounded, weight: .semibold))
            .foregroundStyle(.secondary)
            .padding(.trailing, 2)
    }
}

// MARK: - Sketch bars

/// The sketch-mode toolbar: plane · tools · extrude depth · Finish / Cancel.
/// Replaces the Create/Modify/Combine palette while drawing a profile.
struct SketchPaletteView: View {
    @Bindable var model: EditorModel
    var body: some View {
        HStack(spacing: Theme.Space.m) {
            Picker("Sketch plane", selection: Binding(get: { model.sketchPlane }, set: { model.setSketchPlane($0) })) {
                ForEach(SketchPlane.allCases) { Text($0.label).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden().controlSize(.small).fixedSize()
            Divider().frame(height: 18)
            Picker("Sketch tool", selection: Binding(get: { model.sketchTool }, set: { model.setSketchTool($0) })) {
                ForEach(SketchTool.allCases) { Label($0.label, systemImage: $0.symbol).tag($0) }
            }
            .pickerStyle(.segmented).labelsHidden().controlSize(.small).fixedSize()
            Divider().frame(height: 18)
            HStack(spacing: 6) {
                Image(systemName: "arrow.up.to.line").font(.subheadline).foregroundStyle(.secondary)
                    .help("Extrude depth")
                ScrubField(label: "", value: model.sketchExtrudeDepth, sensitivity: 0.1, minValue: 0.1) { v, _ in
                    model.sketchExtrudeDepth = v
                }.frame(width: 96)
            }
            Divider().frame(height: 18)
            Button { model.finishSketch() } label: { Label("Extrude", systemImage: "checkmark") }
                .buttonStyle(.borderedProminent).controlSize(.small)
                .disabled(!model.canFinishSketch)
                .help("Extrude the closed profile")
            Button { model.exitSketch() } label: { Image(systemName: "xmark") }
                .buttonStyle(.bordered).controlSize(.small)
                .keyboardShortcut(.cancelAction)
                .help("Cancel sketch (Esc)")
                .accessibilityLabel("Cancel sketch")
        }
        .padding(Theme.Space.s)
        .panelSurface()
        .animation(Motion.snappy, value: model.sketchTool)
        .animation(Motion.snappy, value: model.sketchPlane)
    }
}

/// A one-line prompt at the bottom telling the user what to click next.
struct SketchHintBar: View {
    let model: EditorModel
    var body: some View {
        HStack(spacing: Theme.Space.m) {
            HStack(spacing: 6) {
                Image(systemName: "hand.point.up.left")
                Text(hint)
            }
            if let c = model.sketchCursor {
                Divider().frame(height: 11)
                Text(String(format: "%.1f, %.1f mm", c.x, c.y))
                    .monospacedDigit()
                    .foregroundStyle(model.sketchSnapToStart ? AnyShapeStyle(Color.green) : AnyShapeStyle(.secondary))
            }
        }
        .font(.subheadline)
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14).padding(.vertical, 7)
        .pillSurface()
    }
    private var hint: String {
        if model.canFinishSketch { return "Profile closed — set a depth and hit Extrude" }
        switch model.sketchTool {
        case .line:
            return model.sketchVerts.isEmpty
                ? "Line: click to place the first point"
                : "Line: keep clicking · click the first point to close"
        case .rectangle:
            return model.sketchAnchor == nil ? "Rectangle: click the first corner" : "Rectangle: click the opposite corner"
        case .circle:
            return model.sketchAnchor == nil ? "Circle: click the center" : "Circle: click to set the radius"
        }
    }
}

// MARK: - Feature tree

struct FeatureTreeView: View {
    @Bindable var model: EditorModel
    var embedded = false
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            if !embedded { header }
            if model.usesDocumentTree {
                if model.featureNodes.isEmpty {
                    EmptyPanelState(title: "No features yet", systemImage: "cube.transparent",
                                    detail: "Add a primitive or sketch a profile to start.")
                } else {
                    ForEach(model.featureNodes) { node in
                        FeatureRowView(model: model, node: node, depth: 0)
                    }
                }
            } else {
                ForEach(model.features) { f in
                    let selected = model.selectedFeatureID == f.id
                    Button { model.selectedFeatureID = f.id } label: {
                        HStack(spacing: Theme.Space.s) {
                            Image(systemName: f.symbol).frame(width: 16)
                            Text(f.name)
                            Spacer(minLength: 0)
                        }
                        .font(.callout)
                        .padding(.horizontal, Theme.Space.s).padding(.vertical, 6)
                        .selectableRow(selected: selected)
                        .foregroundStyle(selected ? Color.primary : Color.secondary)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .accessibilityAddTraits(selected ? .isSelected : [])
                }
            }
        }
        .padding(embedded ? 0 : 6)
        .modifier(DesignTreeSurface(embedded: embedded))
        .animation(Motion.snappy, value: model.selectedFeatureID)
        .animation(Motion.snappy, value: model.expandedFeatureIDs)
        .animation(Motion.snappy, value: model.hiddenParts)
        .animation(Motion.snappy, value: model.isolatedPart)
    }

    private var header: some View {
        HStack(spacing: 6) {
            Eyebrow(model.usesDocumentTree ? "Features" : "History")
            Spacer(minLength: 0)
            if model.hasHiddenParts {
                Button { model.showAllParts() } label: {
                    Label("Show all", systemImage: "eye").font(.caption.weight(.medium))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Show all hidden parts")
            }
        }
        .padding(.horizontal, Theme.Space.s).padding(.top, 6).padding(.bottom, 4)
    }
}

/// One row of the hierarchical document feature tree (recurses into operands
/// when expanded). Root rows carry an eye toggle + a context menu acting on the
/// part they produce.
struct FeatureRowView: View {
    @Bindable var model: EditorModel
    let node: FeatureNode
    let depth: Int
    @State private var hovering = false

    private var expanded: Bool { model.expandedFeatureIDs.contains(node.id) }
    private var selected: Bool {
        if model.selectedFeatureID == node.id { return true }
        if let pi = node.partIndex { return model.multiSelectedParts.contains(pi) }
        return false
    }
    private var dimmed: Bool {
        guard let pi = node.partIndex else { return false }
        return !model.isPartVisible(pi)
    }
    /// Hovered via the tree row itself OR via the viewport (bidirectional).
    /// `hoveredFeatureID` covers assembly instances, whose rows are part defs
    /// and therefore have no part index of their own.
    private var hovered: Bool {
        hovering
            || model.hoveredFeatureID == node.id
            || (node.partIndex != nil && node.partIndex == model.hoveredPartIndex)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            row
                // Addressable by the ScrollViewReader that keeps the selected
                // row in view when the selection comes from the viewport.
                .id(node.id)
            if expanded {
                ForEach(node.children) { child in
                    FeatureRowView(model: model, node: child, depth: depth + 1)
                }
            }
        }
    }

    private var row: some View {
        HStack(spacing: 6) {
            if node.hasChildren {
                Button { model.toggleExpanded(node.id) } label: {
                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.semibold))
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                        .foregroundStyle(.tertiary)
                        .frame(width: 12, height: 12)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(expanded ? "Collapse" : "Expand")
            } else {
                Color.clear.frame(width: 12, height: 12)
            }
            Image(systemName: node.symbol)
                .font(.callout).frame(width: 16)
                .foregroundStyle(selected ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
            VStack(alignment: .leading, spacing: 0) {
                if model.renamingFeatureID == node.id {
                    RenameField(initial: node.name,
                                commit: { model.renameFeature(node.nodeId, to: $0); model.renamingFeatureID = nil },
                                cancel: { model.renamingFeatureID = nil })
                } else {
                    Text(node.name).font(.callout).lineLimit(1)
                }
                if let d = node.detail {
                    Text(d).font(.caption.monospacedDigit())
                        .foregroundStyle(.tertiary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let pi = node.partIndex { eyeButton(pi) }
        }
        .padding(.leading, CGFloat(depth) * Theme.Space.m + Theme.Space.xs)
        .padding(.trailing, 5).padding(.vertical, 4)
        .selectableRow(selected: selected, hovered: hovered)
        .foregroundStyle(selected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .opacity(dimmed ? 0.45 : 1)
        .contentShape(Rectangle())
        .onHover { inside in
            hovering = inside
            // Row → viewport: light the geometry this row drew.
            if inside { model.hoverFeature(node) }
            else if model.hoveredFeatureID == node.id { model.hoverFeature(nil) }
        }
        // ⌘-click a part row → toggle it in the multi-selection (for booleans).
        // The modifier comes from the gesture, not NSEvent's hardware state.
        .gesture(TapGesture().modifiers(.command).onEnded {
            guard model.renamingFeatureID != node.id else { return }
            if let pi = node.partIndex { model.toggleMultiSelect(part: pi, featureID: node.id) }
            else { model.selectFeature(node.id) }
        })
        .gesture(TapGesture().onEnded {
            guard model.renamingFeatureID != node.id else { return }
            model.selectFeature(node.id)
        })
        .contextMenu { menu }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(node.detail.map { "\(node.name), \($0)" } ?? node.name)
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    private func eyeButton(_ pi: Int) -> some View {
        let vis = model.isPartVisible(pi)
        return Button { model.toggleVisibility(part: pi) } label: {
            Image(systemName: vis ? "eye" : "eye.slash")
                .font(.subheadline)
                .foregroundStyle(vis ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.secondary))
                .frame(width: 18, height: 18)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        // Visible parts keep the toggle quiet until the row is hovered; a
        // hidden part always shows its state.
        .opacity(vis && !hovered && !selected ? 0 : 1)
        .help(vis ? "Hide part" : "Show part")
        .accessibilityLabel(vis ? "Hide part" : "Show part")
    }

    @ViewBuilder private var menu: some View {
        if let pi = node.partIndex {
            Button { model.isolate(part: pi) } label: {
                Label(model.isolatedPart == pi ? "Exit Isolate" : "Isolate",
                      systemImage: "scope")
            }
            Button { model.toggleVisibility(part: pi) } label: {
                Label(model.isPartVisible(pi) ? "Hide" : "Show",
                      systemImage: model.isPartVisible(pi) ? "eye.slash" : "eye")
            }
            if model.hasHiddenParts {
                Button { model.showAllParts() } label: { Label("Show All", systemImage: "eye") }
            }
            Divider()
        }
        if node.hasChildren {
            Button { model.toggleExpanded(node.id) } label: {
                Label(expanded ? "Collapse" : "Expand", systemImage: "list.bullet.indent")
            }
        }
        Button { model.renamingFeatureID = node.id } label: { Label("Rename", systemImage: "pencil") }
        Button {
            #if os(macOS)
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(node.name, forType: .string)
            #else
            UIPasteboard.general.string = node.name
            #endif
        } label: { Label("Copy Name", systemImage: "doc.on.doc") }
        if let pi = node.partIndex {
            Divider()
            Button(role: .destructive) { model.deletePart(pi) } label: {
                Label("Delete Part", systemImage: "trash")
            }
        }
    }
}

/// The tree's surface when it is not embedded in a navigator panel (visionOS).
struct DesignTreeSurface: ViewModifier {
    var embedded: Bool
    @ViewBuilder func body(content: Content) -> some View {
        if embedded { content } else { content.panelSurface() }
    }
}

// MARK: - Parameter editors

/// Scrub/stepper editors for a feature node's parameters — each writes back
/// into the live document and re-evaluates (parity with the web app's scrub
/// inputs). Shared by every inspector so a new editable op appears everywhere
/// at once.
struct FeatureParamEditors: View {
    @Bindable var model: EditorModel
    let node: FeatureNode
    /// Called after every edit, so the released overlay can mesh-swap in place.
    /// `snapshot` is true on a typed commit or the first tick of a scrub.
    var onEdit: (Bool) -> Void = { _ in }

    /// Op types that expose live-editable parameters in the inspector.
    static let editableOps: Set<String> = [
        "Cube", "Cylinder", "Sphere", "Cone", "Fillet", "Chamfer", "Shell",
        "Translate", "Rotate", "Scale", "Revolve", "LinearPattern", "CircularPattern",
    ]

    /// Scrub sensitivity for a document parameter: span-derived when the doc
    /// declares a range (≈200 ticks across it), else the default 0.1 mm/pt.
    static func paramSensitivity(_ p: DocParameter) -> Double {
        if let lo = p.min, let hi = p.max, hi > lo { return (hi - lo) / 200 }
        return 0.1
    }

    var body: some View { editors }

    @ViewBuilder private var editors: some View {
        let op = model.opDict(nodeId: node.nodeId) ?? [:]
        let id = node.nodeId
        switch node.opType {
        case "Cube":
            axisField(op, id, "Width", "size", "x", minV: 0.1)
            axisField(op, id, "Depth", "size", "y", minV: 0.1)
            axisField(op, id, "Height", "size", "z", minV: 0.1)
        case "Cylinder":
            scalarField(op, id, "Radius", "radius"); scalarField(op, id, "Height", "height")
        case "Sphere":
            scalarField(op, id, "Radius", "radius")
        case "Cone":
            scalarField(op, id, "Radius 1", "radius1", minV: 0)
            scalarField(op, id, "Radius 2", "radius2", minV: 0)
            scalarField(op, id, "Height", "height")
        case "Fillet":
            scalarField(op, id, "Radius", "radius", sens: 0.05, minV: 0)
        case "Chamfer":
            scalarField(op, id, "Distance", "distance", sens: 0.05, minV: 0)
        case "Shell":
            scalarField(op, id, "Thickness", "thickness", sens: 0.05, minV: 0.1)
        case "Translate":
            axisField(op, id, "X", "offset", "x"); axisField(op, id, "Y", "offset", "y")
            axisField(op, id, "Z", "offset", "z")
        case "Rotate":
            axisField(op, id, "X", "angles", "x", unit: "°", sens: 0.5)
            axisField(op, id, "Y", "angles", "y", unit: "°", sens: 0.5)
            axisField(op, id, "Z", "angles", "z", unit: "°", sens: 0.5)
        case "Scale":
            axisField(op, id, "X", "factor", "x", unit: "", sens: 0.01, minV: 0.01)
            axisField(op, id, "Y", "factor", "y", unit: "", sens: 0.01, minV: 0.01)
            axisField(op, id, "Z", "factor", "z", unit: "", sens: 0.01, minV: 0.01)
        case "Revolve":
            scalarField(op, id, "Angle", "angle_deg", unit: "°", sens: 0.5, minV: 0)
        case "LinearPattern":
            countStepper(id, (op["count"] as? NSNumber)?.intValue ?? 0)
        case "CircularPattern":
            countStepper(id, (op["count"] as? NSNumber)?.intValue ?? 0)
            scalarField(op, id, "Span", "angle_deg", unit: "°", sens: 0.5, minV: 0)
        default:
            EmptyView()
        }
    }

    private func scalarField(_ op: [String: Any], _ id: Int, _ label: String, _ key: String,
                             unit: String = "mm", sens: Double = 0.1, minV: Double = 0.1) -> some View {
        let value = (op[key] as? NSNumber)?.doubleValue ?? 0
        return ScrubField(label: label, value: value, unit: unit, sensitivity: sens, minValue: minV) { v, s in
            model.editScalar(nodeId: id, key: key, value: v, snapshot: s, name: "Change \(label)")
            onEdit(s)
        }
    }

    private func axisField(_ op: [String: Any], _ id: Int, _ label: String, _ key: String, _ a: String,
                           unit: String = "mm", sens: Double = 0.1,
                           minV: Double = -.greatestFiniteMagnitude) -> some View {
        let value = ((op[key] as? [String: Any])?[a] as? NSNumber)?.doubleValue ?? 0
        return ScrubField(label: label, value: value, unit: unit, sensitivity: sens, minValue: minV) { v, s in
            model.editVec(nodeId: id, key: key, axis: a, value: v, snapshot: s, name: "Change \(label)")
            onEdit(s)
        }
    }

    private func countStepper(_ id: Int, _ count: Int) -> some View {
        Stepper(value: Binding(
            get: { count },
            set: {
                model.editInt(nodeId: id, key: "count", value: max(1, $0), snapshot: true, name: "Change Count")
                onEdit(true)
            }
        ), in: 1...200) {
            KeyValueRow("Count", "\(count)")
        }
    }
}

/// A numeric field — the native take on the web app's scrub inputs. Drag the
/// value horizontally to scrub (⌥ for fine, ⇧ for coarse), or double-click to
/// type an exact number. The first tick of a scrub snapshots for undo; reads
/// top-down each render so live re-eval stays in sync.
struct ScrubField: View {
    let label: String
    let value: Double
    var unit: String = "mm"
    var sensitivity: Double = 0.1
    var minValue: Double = -.greatestFiniteMagnitude
    let onChange: (_ value: Double, _ snapshotFirst: Bool) -> Void
    @State private var base: Double?
    @State private var typing: String?
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: Theme.Space.s) {
            if !label.isEmpty {
                Text(label).font(.callout).foregroundStyle(.secondary)
                Spacer(minLength: Theme.Space.s)
            }
            if typing != nil { editor } else { pill }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label.isEmpty ? "Value" : label)
        .accessibilityValue(formatted)
        .accessibilityAdjustableAction { direction in
            let step = sensitivity * 10
            onChange(max(minValue, value + (direction == .increment ? step : -step)), true)
        }
    }

    private var pill: some View {
        Text(formatted).font(.callout.monospacedDigit())
            .padding(.horizontal, Theme.Space.s).padding(.vertical, 3)
            .frame(minWidth: 70, alignment: .trailing)
            .controlWell(active: base != nil)
            .contentShape(Rectangle())
            .onHover { hovering in
                #if os(macOS)
                (hovering ? NSCursor.resizeLeftRight : NSCursor.arrow).set()
                #endif
            }
            .help("Drag to scrub (⌥ fine · ⇧ coarse) · double-click to type")
            .onTapGesture(count: 2) { typing = String(format: "%g", value) }
            .gesture(
                DragGesture(minimumDistance: 2)
                    .onChanged { v in
                        let first = base == nil
                        let b = base ?? value
                        if base == nil { base = b }
                        onChange(max(minValue, b + Double(v.translation.width) * sensitivity * Self.modifierScale), first)
                    }
                    .onEnded { _ in base = nil }
            )
    }

    /// ⌥ scrubs at a tenth of the rate, ⇧ at ten times — the Xcode/Motion idiom.
    private static var modifierScale: Double {
        #if os(macOS)
        let flags = NSEvent.modifierFlags
        if flags.contains(.option) { return 0.1 }
        if flags.contains(.shift) { return 10 }
        #endif
        return 1
    }

    private var editor: some View {
        TextField("", text: Binding(get: { typing ?? "" }, set: { typing = $0 }))
            .textFieldStyle(.plain)
            .font(.callout.monospacedDigit())
            .multilineTextAlignment(.trailing)
            .frame(width: 70)
            .focused($focused)
            .onAppear { focused = true }
            .onSubmit { commit() }
            .onEscape { typing = nil }
            .onChange(of: focused) { _, now in if !now { commit() } }
            .padding(.horizontal, Theme.Space.s).padding(.vertical, 3)
            .controlWell(active: true)
    }

    private func commit() {
        defer { typing = nil }
        guard let t = typing, let v = Self.parse(t) else { return }
        onChange(max(minValue, v), true)
    }

    /// Accept the user's locale decimal separator as well as a plain dot, and
    /// ignore a trailing unit the user may have typed along with the number.
    static func parse(_ text: String) -> Double? {
        var s = text.trimmingCharacters(in: .whitespaces)
        if let firstNonNumber = s.firstIndex(where: { !"0123456789.,-+eE".contains($0) }) {
            s = String(s[..<firstNonNumber]).trimmingCharacters(in: .whitespaces)
        }
        if let v = Double(s) { return v }
        let f = NumberFormatter()
        f.locale = .current
        f.numberStyle = .decimal
        return f.number(from: s)?.doubleValue
    }

    private var formatted: String {
        let s = abs(value - value.rounded()) < 0.001 ? String(Int(value.rounded())) : String(format: "%.2f", value)
        return unit.isEmpty ? s : "\(s) \(unit)"
    }
}

/// Inline rename field for a feature-tree row — commits on Return/blur, cancels
/// on Escape.
struct RenameField: View {
    let initial: String
    let commit: (String) -> Void
    let cancel: () -> Void
    @State private var text: String
    @FocusState private var focused: Bool

    init(initial: String, commit: @escaping (String) -> Void, cancel: @escaping () -> Void) {
        self.initial = initial
        self.commit = commit
        self.cancel = cancel
        _text = State(initialValue: initial)
    }

    var body: some View {
        TextField("", text: $text)
            .textFieldStyle(.plain)
            .font(.callout)
            .focused($focused)
            .onAppear { focused = true }
            .onSubmit { commit(text) }
            .onEscape { cancel() }
            .onChange(of: focused) { _, now in if !now { commit(text) } }
            .padding(.horizontal, Theme.Space.xs).padding(.vertical, 1)
            .controlWell(active: true)
            .accessibilityLabel("Rename")
    }
}

// MARK: - Transports and pills

/// The slice-1 Receipt: the live cross-domain verdict. Drag the connector and
/// the min-wall check flips green→red as the cutout threatens the housing.
struct GripperReceiptPill: View {
    let model: EditorModel
    var body: some View {
        let ok = model.connectorOK
        return HStack(spacing: 11) {
            HStack(spacing: 5) {
                Image(systemName: "bolt.fill")
                Text("connector \(Int(model.connectorX.rounded())) mm")
            }
            .foregroundStyle(.secondary)
            Divider().frame(height: 12)
            HStack(spacing: 5) {
                Image(systemName: ok ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                Text(String(format: "min-wall %.1f mm", max(0, model.connectorMinWall)))
            }
            .foregroundStyle(ok ? Color.green : Color.orange)
        }
        .font(.callout.monospacedDigit())
        .padding(.horizontal, 14).padding(.vertical, 8)
        .pillSurface()
        .overlay(Capsule(style: .continuous)
            .strokeBorder((ok ? Color.green : Color.orange).opacity(0.35), lineWidth: 1))
        .animation(Motion.snappy, value: ok)
    }
}

/// Play/pause + scrub transport for kinematic joint playback. Scrubbing
/// pauses (direct control beats a fighting timer); play loops the timeline.
struct PlaybackBar: View {
    let model: EditorModel

    private var duration: Double { model.timeline?.durationS ?? 1 }

    var body: some View {
        HStack(spacing: Theme.Space.m) {
            Button {
                model.togglePlayback()
            } label: {
                Image(systemName: model.isPlaying ? "pause.fill" : "play.fill")
                    .font(.body.weight(.semibold))
                    .frame(width: 22, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(model.isPlaying ? "Pause" : "Play")
            .accessibilityLabel(model.isPlaying ? "Pause" : "Play")

            Slider(
                value: Binding(
                    get: { model.playbackTime },
                    set: { model.setPlaybackTime($0) }
                ),
                in: 0...max(duration, 0.001),
                onEditingChanged: { began in
                    if began { model.pausePlayback() }
                }
            )
            .controlSize(.small)
            .frame(width: 220)
            .accessibilityLabel("Timeline position")

            Text(String(format: "%.2f / %.2f s", model.playbackTime, duration))
                .font(.caption.weight(.medium).monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(minWidth: 86, alignment: .trailing)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .pillSurface()
    }
}

// MARK: - Sketch preview overlay (shared by every viewport)

/// Build the in-progress sketch: committed segments + vertex dots in brand
/// pink, plus a cyan rubber-band from the last point / anchor to the cursor.
/// Every viewport parents it under its `centering` entity, so kernel coords
/// line up.
@MainActor
func buildSketchRoot(model: EditorModel) -> Entity {
    let root = Entity(); root.name = "sketchRoot"
    let ink = NSColor(srgbRed: 0.98, green: 0.15, blue: 0.45, alpha: 1.0)   // brand pink
    let live = NSColor(srgbRed: 0.55, green: 0.92, blue: 1.0, alpha: 1.0)   // cyan
    let verts = model.sketchVerts

    func seg(_ a2: SIMD2<Float>, _ b2: SIMD2<Float>, _ color: NSColor) {
        let a = model.sketchWorld(a2), b = model.sketchWorld(b2)
        let d = b - a; let len = simd_length(d)
        guard len > 1e-4 else { return }
        let e = ModelEntity(mesh: .generateBox(size: SIMD3(len, 0.7, 0.7)),
                            materials: [UnlitMaterial(color: color)])
        e.position = (a + b) / 2
        e.orientation = simd_quatf(from: SIMD3(1, 0, 0), to: d / len)
        root.addChild(e)
    }
    func dot(_ v2: SIMD2<Float>, _ color: NSColor, _ r: Float = 1.1) {
        let e = ModelEntity(mesh: .generateSphere(radius: r), materials: [UnlitMaterial(color: color)])
        e.position = model.sketchWorld(v2)
        root.addChild(e)
    }

    // Committed profile.
    if model.sketchTool == .line && !model.sketchClosed {
        for i in 0..<max(0, verts.count - 1) { seg(verts[i], verts[i + 1], ink) }
        for v in verts { dot(v, ink) }
        if let last = verts.last, let c = model.sketchCursor { seg(last, c, live) }
    } else if !verts.isEmpty {
        for i in 0..<verts.count { seg(verts[i], verts[(i + 1) % verts.count], ink) }
        for v in verts { dot(v, ink) }
    }

    // Two-click rect/circle preview from the anchor to the cursor.
    if let a = model.sketchAnchor, let c = model.sketchCursor {
        switch model.sketchTool {
        case .rectangle:
            let corners = [a, SIMD2(c.x, a.y), c, SIMD2(a.x, c.y)]
            for i in 0..<4 { seg(corners[i], corners[(i + 1) % 4], live) }
        case .circle:
            let r = simd_distance(a, c)
            let n = 48
            let pts = (0..<n).map { i -> SIMD2<Float> in
                let t = 2 * Float.pi * Float(i) / Float(n)
                return SIMD2(a.x + r * cos(t), a.y + r * sin(t))
            }
            for i in 0..<n { seg(pts[i], pts[(i + 1) % n], live) }
            dot(a, live)
        case .line:
            break
        }
    }

    // Landing-point marker: a bright dot where the next click lands, so you
    // can always see where you're clicking. Turns green + snaps to the first
    // vertex when a click would close the loop.
    if let c = model.sketchCursor, !model.sketchClosed {
        if model.sketchSnapToStart, let f = model.sketchVerts.first {
            dot(f, NSColor.systemGreen, 1.9)
        } else {
            dot(c, live, 1.4)
        }
    }
    return root
}

// MARK: - Transform gizmo overlay (shared by every viewport)

/// Gizmo handle ink: X red / Y green / Z blue.
enum GizmoInk {
    static let x = NSColor(srgbRed: 0.95, green: 0.30, blue: 0.34, alpha: 1)
    static let y = NSColor(srgbRed: 0.42, green: 0.80, blue: 0.44, alpha: 1)
    static let z = NSColor(srgbRed: 0.32, green: 0.58, blue: 0.98, alpha: 1)

    static func brighten(_ c: NSColor) -> NSColor {
        let s = c.usingColorSpace(.sRGB) ?? c
        return NSColor(srgbRed: min(1, s.redComponent * 1.2 + 0.18),
                       green: min(1, s.greenComponent * 1.2 + 0.18),
                       blue: min(1, s.blueComponent * 1.2 + 0.18), alpha: 1)
    }
}

/// Scale factor that keeps the gizmo a constant fraction of the view
/// height: target world arm length is proportional to orbit distance, so
/// zooming in/out never changes its apparent size.
@MainActor
func gizmoScreenScale(model: EditorModel) -> Float {
    let armKernel = model.gizmoArmLength()
    let armWorld = armKernel * model.displayScale
    guard armWorld > 1e-6 else { return 1 }
    return (0.14 * model.distance) / armWorld
}

/// A refined translate gizmo: cylinder shafts + cone arrowheads (axis drag),
/// corner squares (plane drag), a pearl hub, and invisible full-length grab
/// proxies. The hovered handle brightens + thickens. X red / Y green / Z blue.
/// Every viewport parents it under its `centering` entity (kernel coords).
@MainActor
func buildGizmo(model: EditorModel) -> Entity {
    func brighten(_ c: NSColor) -> NSColor { GizmoInk.brighten(c) }
    let root = Entity(); root.name = "gizmoRoot"
    guard let c = model.gizmoCenterKernel() else { return root }
    // Children live in gizmo-local coords; the root carries the center so
    // the whole gizmo can be scaled per-frame for constant screen size.
    root.position = c
    root.scale = SIMD3<Float>(repeating: gizmoScreenScale(model: model))
    let len = model.gizmoArmLength()
    let shaftR = max(0.3, len * 0.013)
    let headLen = len * 0.2
    let headR = shaftR * 2.6
    let shaftLen = len - headLen
    let hov = model.hoveredGizmoHandle

    let axes: [(name: String, dir: SIMD3<Float>, color: NSColor)] = [
        ("gizmoX", SIMD3(1, 0, 0), GizmoInk.x),
        ("gizmoY", SIMD3(0, 1, 0), GizmoInk.y),
        ("gizmoZ", SIMD3(0, 0, 1), GizmoInk.z),
    ]
    for a in axes {
        let on = hov == a.name
        let mat = UnlitMaterial(color: on ? brighten(a.color) : a.color)
        let rot = simd_quatf(from: SIMD3(0, 1, 0), to: a.dir)
        let k: Float = on ? 1.3 : 1.0

        let shaft = ModelEntity(mesh: .generateCylinder(height: shaftLen, radius: shaftR * k), materials: [mat])
        shaft.orientation = rot; shaft.position = a.dir * (shaftLen / 2)
        root.addChild(shaft)
        let head = ModelEntity(mesh: .generateCone(height: headLen, radius: headR * k), materials: [mat])
        head.orientation = rot; head.position = a.dir * (shaftLen + headLen / 2)
        root.addChild(head)

        let hit = ModelEntity()
        hit.name = a.name; hit.orientation = rot; hit.position = a.dir * (len / 2)
        hit.components.set(CollisionComponent(shapes: [.generateBox(size: SIMD3(headR * 2.6, len, headR * 2.6))]))
        hit.components.set(InputTargetComponent())
        root.addChild(hit)
    }

    // Plane handles — a square in the corner of each axis pair (normal colored).
    let pOff = model.gizmoPlaneOffset, pSize = model.gizmoPlaneSize
    let planes: [(name: String, a: SIMD3<Float>, b: SIMD3<Float>, color: NSColor)] = [
        ("planeXY", SIMD3(1, 0, 0), SIMD3(0, 1, 0), GizmoInk.z),
        ("planeYZ", SIMD3(0, 1, 0), SIMD3(0, 0, 1), GizmoInk.x),
        ("planeXZ", SIMD3(1, 0, 0), SIMD3(0, 0, 1), GizmoInk.y),
    ]
    for p in planes {
        let on = hov == p.name
        let n = simd_normalize(simd_cross(p.a, p.b))
        let rot = simd_quatf(from: SIMD3(0, 0, 1), to: n)
        let center = (p.a + p.b) * pOff
        let mat = UnlitMaterial(color: (on ? brighten(p.color) : p.color).withAlphaComponent(on ? 0.7 : 0.4))
        let sq = ModelEntity(mesh: .generateBox(size: SIMD3(pSize, pSize, max(0.2, pSize * 0.05))), materials: [mat])
        sq.orientation = rot; sq.position = center
        root.addChild(sq)
        let hit = ModelEntity()
        hit.name = p.name; hit.orientation = rot; hit.position = center
        hit.components.set(CollisionComponent(shapes: [.generateBox(size: SIMD3(pSize * 1.25, pSize * 1.25, pSize * 0.6))]))
        hit.components.set(InputTargetComponent())
        root.addChild(hit)
    }

    // Rotate rings — a circle of grabbable segments around each axis.
    let ringR = model.gizmoRingRadius
    let tube = max(0.25, len * 0.013)
    let segN = 40
    let rings: [(name: String, axis: SIMD3<Float>, color: NSColor)] = [
        ("rotX", SIMD3(1, 0, 0), GizmoInk.x),
        ("rotY", SIMD3(0, 1, 0), GizmoInk.y),
        ("rotZ", SIMD3(0, 0, 1), GizmoInk.z),
    ]
    for r in rings {
        let on = hov == r.name
        let mat = UnlitMaterial(color: (on ? brighten(r.color) : r.color).withAlphaComponent(on ? 1 : 0.85))
        let (u1, u2) = EditorModel.ringBasis(r.axis)
        let k: Float = on ? 1.5 : 1.0
        var prev = u1 * ringR
        for i in 1...segN {
            let ang = 2 * Float.pi * Float(i) / Float(segN)
            let pt = (u1 * cos(ang) + u2 * sin(ang)) * ringR
            let mid = (prev + pt) / 2
            let seg = pt - prev
            let l = simd_length(seg)
            let e = ModelEntity(mesh: .generateBox(size: SIMD3(l * 1.06, tube * 2 * k, tube * 2 * k)),
                                materials: [mat])
            e.name = r.name
            e.position = mid
            e.orientation = simd_quatf(from: SIMD3(1, 0, 0), to: seg / l)
            e.components.set(CollisionComponent(shapes: [.generateBox(size: SIMD3(l, tube * 6, tube * 6))]))
            e.components.set(InputTargetComponent())
            root.addChild(e)
            prev = pt
        }
    }

    let hub = ModelEntity(mesh: .generateSphere(radius: shaftR * 2.4),
                          materials: [UnlitMaterial(color: NSColor(white: 0.95, alpha: 1))])
    root.addChild(hub)
    return root
}

extension Color {
    /// `Color(nsColor:)` / `Color(uiColor:)` under one spelling for shared code.
    init(portedColor c: NSColor) {
        #if os(macOS)
        self.init(nsColor: c)
        #else
        self.init(uiColor: c)
        #endif
    }
}
