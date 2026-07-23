import SwiftUI
import RealityKit
import AppKit
import simd
import CoreGraphics
import UniformTypeIdentifiers

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
    @State private var intent = IntentEngine()

    var body: some View {
        GeometryReader { geo in
            let compact = geo.size.width < 760
            ViewportView(model: model)
                .ignoresSafeArea()
                .background(ReleaseWindowConfigurator(release: model.releaseMode))
                .containerBackground(
                    model.releaseMode ? AnyShapeStyle(.clear) : AnyShapeStyle(.background),
                    for: .window)
                .overlay(alignment: .topLeading) {
                    if !compact && !model.releaseMode {
                        FeatureTreeView(model: model)
                            .frame(width: 206)
                            .padding(14)
                            .transition(.move(edge: .leading).combined(with: .opacity))
                    }
                }
                .overlay(alignment: .top) {
                    if showsTools && !model.releaseMode && model.toolPlacement == .header {
                        toolStrip(header: true)
                            .transition(.move(edge: .top).combined(with: .opacity))
                    }
                }
                .overlay(alignment: .top) {
                    if model.source.isGripper && !model.releaseMode {
                        GripperReceiptPill(model: model).padding(.top, 14)
                    }
                }
                .overlay(alignment: .topTrailing) {
                    if model.releaseMode {
                        ReleaseReturnPill(model: model).padding(14)
                    } else if model.source.isGripper {
                        // The cross-domain Receipt takes the inspector slot — the
                        // adaptive inspector adapting to a multi-domain part.
                        ReceiptLedger(model: model)
                            .frame(width: compact ? 248 : 300)
                            .padding(14)
                            .transition(.move(edge: .trailing).combined(with: .opacity))
                    } else {
                        InspectorView(model: model)
                            .frame(width: compact ? 224 : 280)
                            .padding(14)
                    }
                }
                .overlay(alignment: .bottom) {
                  if !model.releaseMode {
                    VStack(spacing: 10) {
                        if model.sketching {
                            SketchHintBar(model: model)
                                .transition(.opacity.combined(with: .move(edge: .bottom)))
                        } else if model.source.isSandbox && intent.draft.isEmpty && !intent.isThinking {
                            ExampleChips(intent: intent)
                                .transition(.opacity.combined(with: .move(edge: .bottom)))
                        }
                        ComposerBar(engine: intent, model: model)
                        if showsTools && model.toolPlacement == .footer {
                            toolStrip(header: false)
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }
                    }
                    .padding(.bottom, 14)
                    .animation(Motion.smooth, value: model.source.isSandbox)
                    .animation(Motion.smooth, value: intent.draft.isEmpty)
                    .animation(Motion.panel, value: model.sketching)
                    .animation(Motion.panel, value: model.toolPlacement)
                  }
                }
                .toolbar {
                    ToolbarItem(placement: .navigation) { BrandMark() }
                    ToolbarItem(placement: .principal) { IdentityStatusBar(model: model) }
                    ToolbarItem(placement: .primaryAction) { CollabAvatars() }
                }
                .navigationTitle(model.documentName)
                .onChange(of: model.releaseMode) { _, released in
                    if released {
                        ReleaseWindowController.shared.show(model: model, intent: intent)
                    } else {
                        ReleaseWindowController.shared.hide()
                    }
                }
                .animation(.smooth(duration: 0.3), value: compact)
                .task {
                    // Dev hook: VCAD_GRIPPER=1 [VCAD_CONNECTOR_X=n] launches into
                    // the cross-domain gripper (used to verify without driving the UI).
                    let env = ProcessInfo.processInfo.environment
                    // Dev hook: VCAD_OPEN=<path> opens a .vcad on launch (handy
                    // for verifying the feature tree against a real document).
                    if let path = env["VCAD_OPEN"], !path.isEmpty {
                        model.openDocument(URL(fileURLWithPath: path))
                    }
                    // Release-to-desktop is the default launch mode; VCAD_RELEASE=0
                    // opts out (studio window on launch, handy for dev/debugging).
                    // The 1s delay lets the window exist before the release
                    // controller reparents it.
                    if env["VCAD_RELEASE"] != "0" {
                        try? await Task.sleep(for: .seconds(1))
                        model.releaseMode = true
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

    private var showsTools: Bool {
        model.sketching || model.source.isSandbox || model.usesDocumentTree
    }

    /// The active tool strip — the sketch palette while drawing, otherwise the
    /// tool palette, rendered as a docked header bar or a floating footer card.
    @ViewBuilder private func toolStrip(header: Bool) -> some View {
        if model.sketching {
            SketchPaletteView(model: model)
                .padding(.top, header ? 8 : 0)
        } else if header {
            ToolPaletteView(model: model, axis: .horizontal, docked: true)
        } else {
            ToolPaletteView(model: model, axis: .horizontal)
        }
    }
}

struct DocumentMenu: View {
    @Bindable var model: EditorModel
    var body: some View {
        Menu {
            Button("New Sandbox") { model.newDocument() }.keyboardShortcut("n")
            Button("Cross-domain Gripper") { model.openGripper() }
            Divider()
            Button("Open…") { openPanel() }.keyboardShortcut("o")
            if !model.recents.isEmpty {
                Menu("Open Recent") {
                    ForEach(model.recents, id: \.self) { url in
                        Button(url.deletingPathExtension().lastPathComponent) { model.openDocument(url) }
                    }
                }
            }
            Menu("Examples") {
                ForEach(model.examples, id: \.path) { ex in
                    Button(ex.name) { model.openDocument(URL(fileURLWithPath: ex.path)) }
                }
            }
            Divider()
            Button("Undo") { model.undo() }.keyboardShortcut("z").disabled(!model.canUndo)
            Button("Redo") { model.redo() }
                .keyboardShortcut("z", modifiers: [.command, .shift]).disabled(!model.canRedo)
            Divider()
            if model.usesDocumentTree {
                Button("Save") { model.saveDocument() }
                    .keyboardShortcut("s").disabled(!model.documentDirty)
                Button("Save As…") { saveAsPanel() }
                    .keyboardShortcut("s", modifiers: [.command, .shift])
                if model.documentDirty {
                    Button("Revert Changes") { model.revertDocument() }
                }
            }
            Button("Export STL…") { exportPanel() }.keyboardShortcut("e").disabled(!model.canExport)
            Button("Export USDZ…") { exportUSDZPanel() }
                .keyboardShortcut("e", modifiers: [.command, .shift]).disabled(!model.canExport)
            Divider()
            Picker("Tool Palette", selection: $model.toolPlacement) {
                ForEach(ToolPlacement.allCases) { p in Text(p.label).tag(p) }
            }
            Button("Toggle Tool Palette") { model.cycleToolPlacement() }.keyboardShortcut("t")
            Button(model.zebraMode ? "Zebra Analysis ✓" : "Zebra Analysis") { model.zebraMode.toggle() }
                .keyboardShortcut("z", modifiers: [])
            Button(model.releaseMode ? "Return to Studio" : "Release to Desktop") {
                withAnimation(Motion.panel) { model.releaseMode.toggle() }
            }
            .keyboardShortcut(.space, modifiers: [.command, .shift])
        } label: {
            HStack(spacing: 6) {
                Image(systemName: model.source.isSandbox ? "cube" : "doc").font(.system(size: 12))
                Text(model.documentName).font(.system(size: 13, weight: .medium))
                if model.documentDirty {
                    Circle().fill(Color.accentColor).frame(width: 5, height: 5)
                }
                Image(systemName: "chevron.down").font(.system(size: 9)).opacity(0.55)
            }
        }
        .menuStyle(.borderlessButton)
        .fixedSize()
    }

    private func exportPanel() {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "stl") ?? .data]
        panel.nameFieldStringValue = "\(model.documentName).stl"
        panel.prompt = "Export"
        if panel.runModal() == .OK, let url = panel.url { _ = model.exportSTL(to: url) }
    }

    private func exportUSDZPanel() {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.usdz]
        panel.nameFieldStringValue = "\(model.documentName).usdz"
        panel.prompt = "Export"
        if panel.runModal() == .OK, let url = panel.url { _ = model.exportUSDZ(to: url) }
    }

    private func saveAsPanel() {
        let panel = NSSavePanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "vcad") ?? .json]
        panel.nameFieldStringValue = "\(model.documentName).vcad"
        panel.prompt = "Save"
        if panel.runModal() == .OK, let url = panel.url { model.saveDocumentAs(url) }
    }

    private func openPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "vcad") ?? .json]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.prompt = "Open"
        if panel.runModal() == .OK, let url = panel.url { model.openDocument(url) }
    }
}

// MARK: top chrome — brand · centered identity + status · presence

/// The leading wordmark.
struct BrandMark: View {
    var body: some View {
        Text("vcad")
            .font(.system(size: 14, weight: .semibold, design: .rounded))
            .foregroundStyle(.secondary)
            .padding(.trailing, 2)
    }
}

/// Centered identity (the document menu) + a compact live status readout.
struct IdentityStatusBar: View {
    @Bindable var model: EditorModel
    var body: some View {
        HStack(spacing: 10) {
            DocumentMenu(model: model)
            Divider().frame(height: 13)
            StatusStrip(model: model)
        }
    }
}

/// kernel · tris · bounds · solve — the dense status, inline in the title bar.
struct StatusStrip: View {
    let model: EditorModel
    var body: some View {
        HStack(spacing: 9) {
            HStack(spacing: 4) {
                Circle().fill(.green).frame(width: 5, height: 5)
                Text("kernel")
            }
            bar
            Text("\(model.triangleCount.formatted()) tris")
            bar
            Text(String(format: "%.0f×%.0f×%.0f", abs(model.sizeMM.x), abs(model.sizeMM.y), abs(model.sizeMM.z)))
            bar
            Text(String(format: "%.0f ms", model.solveMillis))
            bar
            // Pixel-perfect mode: settle-triggered direct-BRep ray trace.
            Button {
                model.raytraceEnabled.toggle()
                if !model.raytraceEnabled { model.raytraceImage = nil }
            } label: {
                Text("RT")
                    .foregroundStyle(model.raytraceEnabled ? Color.green : Color.secondary)
            }
            .buttonStyle(.plain)
            .help("Ray-traced still when the camera settles — rays vs the exact BRep, no tessellation")
        }
        .font(.system(size: 11, design: .monospaced))
        .foregroundStyle(.secondary)
        .lineLimit(1)
        .fixedSize()
    }
    private var bar: some View { Rectangle().fill(.secondary.opacity(0.25)).frame(width: 1, height: 10) }
}

/// Presence + share — a share affordance and the current user's avatar.
struct CollabAvatars: View {
    var body: some View {
        HStack(spacing: 9) {
            Button {} label: { Image(systemName: "person.badge.plus").font(.system(size: 13)) }
                .buttonStyle(.plain).foregroundStyle(.secondary).help("Share")
            Circle()
                .fill(LinearGradient(
                    colors: [Color(red: 0.98, green: 0.15, blue: 0.45), Color(red: 0.72, green: 0.09, blue: 0.32)],
                    startPoint: .top, endPoint: .bottom))
                .frame(width: 22, height: 22)
                .overlay(Text("C").font(.system(size: 11, weight: .semibold)).foregroundStyle(.white))
                .overlay(Circle().strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
                .help("You")
        }
    }
}

/// The bottom composer: a `+` quick-start menu beside the AI command field.
struct ComposerBar: View {
    @Bindable var engine: IntentEngine
    let model: EditorModel
    var body: some View {
        HStack(spacing: 8) {
            Menu {
                Button("New") { model.newDocument() }
                Button("Open…") { openPanel() }
                if !model.examples.isEmpty {
                    Menu("Examples") {
                        ForEach(model.examples, id: \.path) { ex in
                            Button(ex.name) { model.openDocument(URL(fileURLWithPath: ex.path)) }
                        }
                    }
                }
            } label: {
                Image(systemName: "plus")
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(.secondary)
                    .frame(width: 30, height: 30)
                    .background(.regularMaterial, in: Circle())
                    .overlay(Circle().strokeBorder(.white.opacity(0.10), lineWidth: 0.5))
                    .shadow(color: .black.opacity(0.35), radius: 10, y: 5)
            }
            .menuStyle(.borderlessButton)
            .menuIndicator(.hidden)
            .fixedSize()
            CommandBar(engine: engine, model: model)
        }
    }

    private func openPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "vcad") ?? .json]
        panel.allowsMultipleSelection = false
        panel.canChooseDirectories = false
        panel.prompt = "Open"
        if panel.runModal() == .OK, let url = panel.url { model.openDocument(url) }
    }
}

// MARK: tool palette (native Borland-model)

struct ToolPaletteView: View {
    @Bindable var model: EditorModel
    var axis: Axis = .horizontal
    /// Docked (Borland-style) full-width top strip vs a floating glass card.
    var docked: Bool = false
    private var vertical: Bool { axis == .vertical }
    @Namespace private var paletteNS

    var body: some View {
        let outer = vertical ? AnyLayout(VStackLayout(spacing: 6)) : AnyLayout(HStackLayout(spacing: 10))
        let tabsLayout = vertical ? AnyLayout(VStackLayout(spacing: 4)) : AnyLayout(HStackLayout(spacing: 3))
        let toolsLayout = vertical ? AnyLayout(VStackLayout(spacing: 5)) : AnyLayout(HStackLayout(spacing: 6))
        let content = outer {
            tabsLayout {
                ForEach(Array(model.availableTabs.enumerated()), id: \.element.id) { idx, tab in
                    tabButton(tab, idx: idx)
                }
            }
            if vertical { Divider().frame(width: 24) } else { Divider().frame(height: 18) }
            toolsLayout {
                ForEach(model.tools(for: model.toolTab)) { tool in
                    toolButton(tool)
                }
            }
            .id(model.toolTab)
            .transition(.opacity)
            if docked { Spacer(minLength: 0) }
        }
        .animation(Motion.snappy, value: model.toolTab)
        .animation(Motion.snappy, value: model.baseShape)
        .animation(Motion.snappy, value: model.modifier)

        return Group {
            if docked {
                content
                    .padding(.horizontal, 14).padding(.vertical, 7)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(.regularMaterial)
                    .overlay(alignment: .bottom) {
                        Rectangle().fill(.white.opacity(0.08)).frame(height: 0.5)
                    }
            } else {
                content
                    .padding(vertical ? 6 : 8)
                    .glassCard(vertical ? 16 : 13)
            }
        }
    }

    @ViewBuilder private func tabButton(_ tab: ToolTab, idx: Int) -> some View {
        let active = model.toolTab == tab
        Button { model.toolTab = tab } label: {
            Group {
                if vertical {
                    Image(systemName: tab.symbol).font(.system(size: 17)).frame(width: 42, height: 42)
                } else {
                    HStack(spacing: 5) {
                        Image(systemName: tab.symbol).font(.system(size: 12))
                        Text(tab.label).font(.system(size: 12, weight: .medium))
                        Text("\(idx + 1)").font(.system(size: 9, design: .monospaced)).opacity(0.5)
                    }
                    .padding(.horizontal, 10).padding(.vertical, 5)
                }
            }
            .background {
                if active {
                    RoundedRectangle(cornerRadius: vertical ? 12 : 7, style: .continuous)
                        .fill(Color.accentColor.opacity(0.18))
                        .matchedGeometryEffect(id: "tabSel", in: paletteNS)
                }
            }
        }
        .buttonStyle(.plain)
        .foregroundStyle(active ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
        .help(tab.label)
        .keyboardShortcut(KeyEquivalent(Character(String(idx + 1))), modifiers: [])
    }

    @ViewBuilder private func toolButton(_ tool: Tool) -> some View {
        Button { if tool.enabled { tool.action() } } label: {
            if vertical {
                Image(systemName: tool.symbol).font(.system(size: 16))
                    .frame(width: 42, height: 42)
                    .background(tool.isActive ? Color.white.opacity(0.10) : .clear,
                                in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .strokeBorder(tool.isActive ? Color.accentColor.opacity(0.65) : .clear, lineWidth: 1))
            } else {
                HStack(spacing: 5) {
                    Image(systemName: tool.symbol).font(.system(size: 12))
                    Text(tool.label).font(.system(size: 12))
                }
                .padding(.horizontal, 9).padding(.vertical, 5)
                .background(tool.isActive ? Color.white.opacity(0.10) : .clear,
                            in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .strokeBorder(tool.isActive ? Color.accentColor.opacity(0.65) : .clear, lineWidth: 1))
            }
        }
        .buttonStyle(.plain)
        .foregroundStyle(tool.isActive ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .opacity(tool.enabled ? 1 : 0.32)
        .disabled(!tool.enabled)
        .help(tool.enabled ? tool.label : "\(tool.label) — \(tool.hint)")
    }
}

/// The sketch-mode toolbar: plane · tools · extrude depth · Finish / Cancel.
/// Replaces the Create/Modify/Combine palette while drawing a profile.
struct SketchPaletteView: View {
    @Bindable var model: EditorModel
    var body: some View {
        HStack(spacing: 10) {
            segmented(SketchPlane.allCases, get: { model.sketchPlane }, label: { $0.label }) {
                model.setSketchPlane($0)
            }
            Divider().frame(height: 18)
            HStack(spacing: 6) {
                ForEach(SketchTool.allCases) { tool in
                    let active = model.sketchTool == tool
                    Button { model.setSketchTool(tool) } label: {
                        HStack(spacing: 5) {
                            Image(systemName: tool.symbol).font(.system(size: 12))
                            Text(tool.label).font(.system(size: 12))
                        }
                        .padding(.horizontal, 9).padding(.vertical, 5)
                        .background(active ? Color.accentColor.opacity(0.18) : .clear,
                                    in: RoundedRectangle(cornerRadius: 6, style: .continuous))
                        .foregroundStyle(active ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
                    }
                    .buttonStyle(.plain)
                }
            }
            Divider().frame(height: 18)
            HStack(spacing: 6) {
                Image(systemName: "arrow.up.to.line").font(.system(size: 11)).foregroundStyle(.secondary)
                ScrubField(label: "", value: model.sketchExtrudeDepth, sensitivity: 0.1, minValue: 0.1) { v, _ in
                    model.sketchExtrudeDepth = v
                }.frame(width: 96)
            }
            Divider().frame(height: 18)
            Button { model.finishSketch() } label: {
                Label("Extrude", systemImage: "checkmark")
                    .font(.system(size: 12, weight: .medium))
                    .padding(.horizontal, 9).padding(.vertical, 5)
            }
            .buttonStyle(.plain)
            .foregroundStyle(model.canFinishSketch ? AnyShapeStyle(Color.green) : AnyShapeStyle(.tertiary))
            .disabled(!model.canFinishSketch)
            Button { model.exitSketch() } label: {
                Image(systemName: "xmark").font(.system(size: 12)).foregroundStyle(.secondary)
                    .padding(6)
            }
            .buttonStyle(.plain)
            .help("Cancel sketch (Esc)")
        }
        .padding(8)
        .glassCard(13)
        .animation(.snappy(duration: 0.2), value: model.sketchTool)
        .animation(.snappy(duration: 0.2), value: model.sketchPlane)
    }

    private func segmented<T: Identifiable & Equatable>(
        _ items: [T], get: () -> T, label: @escaping (T) -> String, set: @escaping (T) -> Void
    ) -> some View {
        let current = get()
        return HStack(spacing: 2) {
            ForEach(items) { item in
                let active = item == current
                Button { set(item) } label: {
                    Text(label(item)).font(.system(size: 11, weight: .medium))
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .background(active ? Color.accentColor.opacity(0.18) : .clear,
                                    in: RoundedRectangle(cornerRadius: 5, style: .continuous))
                        .foregroundStyle(active ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
                }
                .buttonStyle(.plain)
            }
        }
    }
}

/// A one-line prompt at the bottom telling the user what to click next.
struct SketchHintBar: View {
    let model: EditorModel
    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 6) {
                Image(systemName: "hand.point.up.left").font(.system(size: 11))
                Text(hint)
            }
            if let c = model.sketchCursor {
                Rectangle().fill(.secondary.opacity(0.25)).frame(width: 1, height: 11)
                Text(String(format: "%.1f, %.1f mm", c.x, c.y))
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(model.sketchSnapToStart ? AnyShapeStyle(Color.green) : AnyShapeStyle(.secondary))
            }
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14).padding(.vertical, 7)
        .glassCard(11)
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

struct FeatureTreeView: View {
    @Bindable var model: EditorModel
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            header
            if model.usesDocumentTree {
                ForEach(model.featureNodes) { node in
                    FeatureRowView(model: model, node: node, depth: 0)
                }
            } else {
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
        }
        .padding(6)
        .glassCard()
        .animation(Motion.snappy, value: model.selectedFeatureID)
        .animation(Motion.snappy, value: model.expandedFeatureIDs)
        .animation(Motion.snappy, value: model.hiddenParts)
        .animation(Motion.snappy, value: model.isolatedPart)
    }

    private var header: some View {
        HStack(spacing: 6) {
            Text(model.usesDocumentTree ? "FEATURES" : "HISTORY")
                .font(.system(size: 10, weight: .semibold)).tracking(0.6)
                .foregroundStyle(.tertiary)
            Spacer(minLength: 0)
            if model.hasHiddenParts {
                Button { model.showAllParts() } label: {
                    Label("Show all", systemImage: "eye")
                        .font(.system(size: 10, weight: .medium))
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .help("Show all hidden parts")
            }
        }
        .padding(.horizontal, 8).padding(.top, 6).padding(.bottom, 4)
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
    private var hovered: Bool {
        hovering || (node.partIndex != nil && node.partIndex == model.hoveredPartIndex)
    }
    private var rowBackground: Color {
        if selected { return Color.accentColor.opacity(0.20) }
        return hovered ? Color.white.opacity(0.06) : .clear
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            row
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
                    Image(systemName: expanded ? "chevron.down" : "chevron.right")
                        .font(.system(size: 9, weight: .semibold))
                        .foregroundStyle(.tertiary)
                        .frame(width: 12, height: 12)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            } else {
                Color.clear.frame(width: 12, height: 12)
            }
            Image(systemName: node.symbol)
                .font(.system(size: 12)).frame(width: 16)
                .foregroundStyle(selected ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
            VStack(alignment: .leading, spacing: 0) {
                if model.renamingFeatureID == node.id {
                    RenameField(initial: node.name,
                                commit: { model.renameFeature(node.nodeId, to: $0); model.renamingFeatureID = nil },
                                cancel: { model.renamingFeatureID = nil })
                } else {
                    Text(node.name).font(.system(size: 12)).lineLimit(1)
                }
                if let d = node.detail {
                    Text(d).font(.system(size: 10).monospacedDigit())
                        .foregroundStyle(.tertiary).lineLimit(1)
                }
            }
            Spacer(minLength: 0)
            if let pi = node.partIndex { eyeButton(pi) }
        }
        .padding(.leading, CGFloat(depth) * 13 + 4)
        .padding(.trailing, 5).padding(.vertical, 4)
        .background(rowBackground, in: RoundedRectangle(cornerRadius: 7, style: .continuous))
        .foregroundStyle(selected ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .opacity(dimmed ? 0.45 : 1)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture {
            guard model.renamingFeatureID != node.id else { return }
            // ⌘-click a part row → toggle it in the multi-selection (for booleans).
            if NSEvent.modifierFlags.contains(.command), let pi = node.partIndex {
                model.toggleMultiSelect(part: pi, featureID: node.id)
            } else {
                model.selectFeature(node.id)
            }
        }
        .contextMenu { menu }
    }

    private func eyeButton(_ pi: Int) -> some View {
        let vis = model.isPartVisible(pi)
        return Button { model.toggleVisibility(part: pi) } label: {
            Image(systemName: vis ? "eye" : "eye.slash")
                .font(.system(size: 11))
                .foregroundStyle(vis ? AnyShapeStyle(.tertiary) : AnyShapeStyle(Color.accentColor))
                .frame(width: 18, height: 18)
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help(vis ? "Hide part" : "Show part")
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
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(node.name, forType: .string)
        } label: { Label("Copy Name", systemImage: "doc.on.doc") }
        if let pi = node.partIndex {
            Divider()
            Button(role: .destructive) { model.deletePart(pi) } label: {
                Label("Delete Part", systemImage: "trash")
            }
        }
    }
}

struct InspectorView: View {
    @Bindable var model: EditorModel
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            if model.usesDocumentTree {
                if let node = model.selectedFeatureNode {
                    section(node.name) {
                        row("Operation", DocumentGraph.label(node.opType))
                        if let pi = node.partIndex { materialPicker(pi) }
                        if let pi = node.partIndex, !model.isPartVisible(pi) {
                            Label("Hidden", systemImage: "eye.slash")
                                .font(.system(size: 12)).foregroundStyle(.secondary)
                        }
                    }
                    if Self.editableOps.contains(node.opType) {
                        section("Parameters") { paramEditors(node) }
                    } else if let d = node.detail {
                        section("Parameters") { row("Value", d) }
                    }
                }
            } else if let f = model.selectedFeature {
                section(f.name) {
                    switch f.kind {
                    case .base:
                        row("Shape", model.baseShape.label)
                    case .modifier:
                        if model.modifier == .none {
                            Text("No modifier").font(.system(size: 12)).foregroundStyle(.secondary)
                        } else if !model.modifierEffective {
                            Label("No edges on a sphere", systemImage: "info.circle")
                                .font(.system(size: 12)).foregroundStyle(.secondary)
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
            if model.usesDocumentTree, !model.docParameters.isEmpty {
                // Document-level named parameters — the parametric scrub,
                // generalized: any .vcad that declares `parameters` gets live
                // handles here, bindings re-solving every driven node together.
                section("Document Parameters") {
                    ForEach(model.docParameters) { p in
                        if let v = p.value {
                            ScrubField(label: p.name, value: v, unit: p.unit ?? "mm",
                                       sensitivity: Self.paramSensitivity(p),
                                       minValue: p.min ?? -.greatestFiniteMagnitude) { v, s in
                                model.editParameter(p.name, value: v, snapshot: s)
                            }
                            .help(p.description ?? p.name)
                        } else if let f = p.formula {
                            row(p.name, "= \(f)").help(p.description ?? p.name)
                        }
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

    /// Material assignment for a part — a swatch + grouped preset menu.
    private func materialPicker(_ pi: Int) -> some View {
        let current = model.materialName(forPart: pi) ?? "default"
        let resolved = model.resolvedMaterial(forPart: pi)
        return HStack {
            Text("Material").font(.system(size: 12))
            Spacer()
            Menu {
                ForEach(MaterialPreset.grouped, id: \.category) { group in
                    Section(group.category.capitalized) {
                        ForEach(group.items) { p in
                            Button { model.setPartMaterial(pi, p.key) } label: {
                                if p.key == current { Label(p.name, systemImage: "checkmark") }
                                else { Text(p.name) }
                            }
                        }
                    }
                }
            } label: {
                HStack(spacing: 6) {
                    Circle().fill(Color(nsColor: resolved.color)).frame(width: 11, height: 11)
                        .overlay(Circle().strokeBorder(.white.opacity(0.25), lineWidth: 0.5))
                    Text(MaterialPreset.byKey(current)?.name ?? current.capitalized)
                        .font(.system(size: 12))
                    Image(systemName: "chevron.up.chevron.down").font(.system(size: 8)).opacity(0.5)
                }
                .foregroundStyle(.secondary)
            }
            .menuStyle(.borderlessButton)
            .fixedSize()
        }
    }

    private var boundsText: String {
        let s = model.sizeMM
        return String(format: "%.1f × %.1f × %.1f mm", abs(s.x), abs(s.y), abs(s.z))
    }

    /// Scrub sensitivity for a document parameter: span-derived when the doc
    /// declares a range (≈200 ticks across it), else the default 0.1 mm/pt.
    static func paramSensitivity(_ p: DocParameter) -> Double {
        if let lo = p.min, let hi = p.max, hi > lo { return (hi - lo) / 200 }
        return 0.1
    }

    /// Op types that expose live-editable parameters in the inspector.
    static let editableOps: Set<String> = [
        "Cube", "Cylinder", "Sphere", "Cone", "Fillet", "Chamfer", "Shell",
        "Translate", "Rotate", "Scale", "Revolve", "LinearPattern", "CircularPattern",
    ]

    /// Scrub/stepper editors for the selected feature — each writes back into the
    /// live document and re-evaluates (parity with the web app's scrub inputs).
    @ViewBuilder private func paramEditors(_ node: FeatureNode) -> some View {
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
            model.editScalar(nodeId: id, key: key, value: v, snapshot: s)
        }
    }

    private func axisField(_ op: [String: Any], _ id: Int, _ label: String, _ key: String, _ a: String,
                           unit: String = "mm", sens: Double = 0.1,
                           minV: Double = -.greatestFiniteMagnitude) -> some View {
        let value = ((op[key] as? [String: Any])?[a] as? NSNumber)?.doubleValue ?? 0
        return ScrubField(label: label, value: value, unit: unit, sensitivity: sens, minValue: minV) { v, s in
            model.editVec(nodeId: id, key: key, axis: a, value: v, snapshot: s)
        }
    }

    private func countStepper(_ id: Int, _ count: Int) -> some View {
        Stepper(value: Binding(
            get: { count },
            set: { model.editInt(nodeId: id, key: "count", value: max(1, $0), snapshot: true) }
        ), in: 1...200) {
            HStack {
                Text("Count").font(.system(size: 12))
                Spacer()
                Text("\(count)").font(.system(size: 12).monospacedDigit()).foregroundStyle(.secondary)
            }
        }
    }
}

/// A numeric field — the native take on the web app's scrub inputs. Drag the
/// value horizontally to scrub, or double-click to type an exact number. The
/// first tick of a scrub snapshots for undo; reads top-down each render so live
/// re-eval stays in sync.
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
        HStack(spacing: 8) {
            Text(label).font(.system(size: 12)).foregroundStyle(.secondary)
            Spacer(minLength: 8)
            if typing != nil { editor } else { pill }
        }
    }

    private var pill: some View {
        Text(formatted).font(.system(size: 12).monospacedDigit())
            .padding(.horizontal, 8).padding(.vertical, 3)
            .frame(minWidth: 70, alignment: .trailing)
            .background(.white.opacity(base != nil ? 0.12 : 0.06),
                        in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                .strokeBorder(base != nil ? Color.accentColor.opacity(0.6) : .white.opacity(0.10), lineWidth: 0.5))
            .contentShape(Rectangle())
            .onHover { (($0 ? NSCursor.resizeLeftRight : NSCursor.arrow)).set() }
            .help("Drag to scrub · double-click to type")
            .onTapGesture(count: 2) { typing = String(format: "%g", value) }
            .gesture(
                DragGesture(minimumDistance: 2)
                    .onChanged { v in
                        let first = base == nil
                        let b = base ?? value
                        if base == nil { base = b }
                        onChange(max(minValue, b + Double(v.translation.width) * sensitivity), first)
                    }
                    .onEnded { _ in base = nil }
            )
    }

    private var editor: some View {
        TextField("", text: Binding(get: { typing ?? "" }, set: { typing = $0 }))
            .textFieldStyle(.plain)
            .font(.system(size: 12).monospacedDigit())
            .multilineTextAlignment(.trailing)
            .frame(width: 70)
            .focused($focused)
            .onAppear { focused = true }
            .onSubmit { commit() }
            .onExitCommand { typing = nil }
            .onChange(of: focused) { _, now in if !now { commit() } }
            .padding(.horizontal, 8).padding(.vertical, 3)
            .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 6, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 6, style: .continuous)
                .strokeBorder(Color.accentColor.opacity(0.7), lineWidth: 1))
    }

    private func commit() {
        defer { typing = nil }
        guard let t = typing, let v = Double(t.trimmingCharacters(in: .whitespaces)) else { return }
        onChange(max(minValue, v), true)
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
            .font(.system(size: 12))
            .focused($focused)
            .onAppear { focused = true }
            .onSubmit { commit(text) }
            .onExitCommand { cancel() }
            .onChange(of: focused) { _, now in if !now { commit(text) } }
            .padding(.horizontal, 4).padding(.vertical, 1)
            .background(.white.opacity(0.10), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous)
                .strokeBorder(Color.accentColor.opacity(0.7), lineWidth: 1))
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

/// The slice-1 Receipt: the live cross-domain verdict. Drag the connector and
/// the min-wall check flips green→red as the cutout threatens the housing.
struct GripperReceiptPill: View {
    let model: EditorModel
    var body: some View {
        let ok = model.connectorOK
        return HStack(spacing: 11) {
            HStack(spacing: 5) {
                Image(systemName: "bolt.fill").font(.system(size: 11))
                Text("connector \(Int(model.connectorX.rounded())) mm")
            }
            .foregroundStyle(.secondary)
            bar
            HStack(spacing: 5) {
                Image(systemName: ok ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                Text(String(format: "min-wall %.1f mm", max(0, model.connectorMinWall)))
            }
            .foregroundStyle(ok ? Color.green : Color.orange)
        }
        .font(.system(size: 12, design: .monospaced))
        .padding(.horizontal, 14).padding(.vertical, 8)
        .glassCard(11)
        .overlay(
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .strokeBorder((ok ? Color.green : Color.orange).opacity(0.35), lineWidth: 1)
        )
        .animation(.snappy(duration: 0.2), value: ok)
    }
    private var bar: some View { Rectangle().fill(.secondary.opacity(0.25)).frame(width: 1, height: 12) }
}

/// Seed prompts over an untouched studio — tap to load one into the command bar.
struct ExampleChips: View {
    @Bindable var intent: IntentEngine
    var body: some View {
        HStack(spacing: 8) {
            Text("Try").font(.system(size: 11, weight: .medium)).foregroundStyle(.tertiary)
            ForEach(IntentEngine.examplePrompts.prefix(3), id: \.self) { prompt in
                Button {
                    intent.draft = prompt
                    intent.focusRequested = true
                } label: {
                    Text(prompt).font(.system(size: 11))
                        .padding(.horizontal, 11).padding(.vertical, 5)
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .background(.regularMaterial, in: Capsule(style: .continuous))
                .overlay(Capsule(style: .continuous).strokeBorder(.white.opacity(0.10), lineWidth: 0.5))
            }
        }
    }
}

/// Play/pause + scrub transport for kinematic joint playback. Scrubbing
/// pauses (direct control beats a fighting timer); play loops the timeline.
struct PlaybackBar: View {
    let model: EditorModel

    private var duration: Double { model.timeline?.durationS ?? 1 }

    var body: some View {
        HStack(spacing: 10) {
            Button {
                model.togglePlayback()
            } label: {
                Image(systemName: model.isPlaying ? "pause.fill" : "play.fill")
                    .font(.system(size: 13, weight: .semibold))
                    .frame(width: 22, height: 22)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(model.isPlaying ? "Pause" : "Play")

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

            Text(String(format: "%.2f / %.2f s", model.playbackTime, duration))
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(.secondary)
                .frame(minWidth: 86, alignment: .trailing)
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glassCard(22)
    }
}

/// Applies/reverts the transparent-window styling for release-to-desktop mode.
/// Lives as an invisible background NSView so it can reach the hosting NSWindow.
struct ReleaseWindowConfigurator: NSViewRepresentable {
    var release: Bool

    func makeNSView(context: Context) -> NSView { NSView() }

    func updateNSView(_ view: NSView, context: Context) {
        let release = release
        DispatchQueue.main.async {
            guard let w = view.window else { return }
            w.isOpaque = !release
            w.backgroundColor = release ? .clear : .windowBackgroundColor
            w.hasShadow = !release
            w.titlebarAppearsTransparent = release
            w.titleVisibility = release ? .hidden : .visible
            w.toolbar?.isVisible = !release
            if release {
                w.styleMask.insert(.fullSizeContentView)
                w.level = .floating
            } else {
                w.styleMask.remove(.fullSizeContentView)
                w.level = .normal
            }
            // RealityKit's drawable clears opaque — punch through every metal
            // layer in the hierarchy so the desktop shows behind the parts.
            if let content = w.contentView { Self.setMetalLayersOpaque(!release, in: content) }
        }
    }

    private static func setMetalLayersOpaque(_ opaque: Bool, in view: NSView) {
        if let layer = view.layer { walkLayers(layer, opaque: opaque) }
        for sub in view.subviews { setMetalLayersOpaque(opaque, in: sub) }
    }

    private static func walkLayers(_ layer: CALayer, opaque: Bool) {
        if layer is CAMetalLayer {
            layer.isOpaque = opaque
            layer.backgroundColor = opaque ? nil : NSColor.clear.cgColor
        }
        for sub in layer.sublayers ?? [] { walkLayers(sub, opaque: opaque) }
    }
}

/// The only chrome left in release mode: a small pill to come back inside.
struct ReleaseReturnPill: View {
    @Bindable var model: EditorModel

    var body: some View {
        Button {
            withAnimation(Motion.panel) { model.releaseMode = false }
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "arrow.down.forward.and.arrow.up.backward")
                    .font(.system(size: 11, weight: .semibold))
                Text("Return to Studio").font(.system(size: 12, weight: .medium))
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 7)
            .background(.ultraThinMaterial, in: Capsule())
        }
        .buttonStyle(.plain)
        .help("Bring vcad back into its window (⌘⇧Space)")
    }
}

struct ViewportView: View {
    let model: EditorModel

    var body: some View {
        _ = (model.azimuth, model.elevation, model.distance, model.modifierValue,
             model.source, model.selectedFeatureID, model.baseShape, model.modifier,
             model.pickDirty, model.connectorX, model.hoveredHandle, model.panOffset,
             model.copperDirty, model.copperStale, model.visibilityDirty, model.selectionDirty,
             model.docParamDirty, model.sketchDirty, model.hoverDirty, model.gizmoDirty,
             model.playbackTime, model.playbackDirty, model.zebraDirty, model.releaseDirty)

        return GeometryReader { geo in
          RealityView { content in
            if model.releaseMode {
                content.environment = .default
            } else if let env = model.zebraMode ? Self.zebraEnvironment : Self.studioEnvironment {
                content.environment = .skybox(env)
            }
            setupScene(content)
            rebuildGeometry(content)
            model.geometryDirty = false
          } update: { content in
            if let camera = content.entities.first(where: { $0.name == "camera" }) {
                camera.position = model.cameraPosition
                camera.look(at: model.panOffset, from: model.cameraPosition, relativeTo: nil)
            }
            // Constant on-screen gizmo size: rescale with camera distance, the
            // way every desktop CAD tool does (zooming never balloons it).
            if let root = content.entities.first(where: { $0.name == "geomRoot" }),
               let gizmo = root.findEntity(named: "gizmoRoot") {
                gizmo.scale = SIMD3<Float>(repeating: gizmoScreenScale(model: model))
            }
            if model.zebraDirty {
                var mutableContent = content
                applyZebra(&mutableContent, on: model.zebraMode)
                model.zebraDirty = false
            }
            if model.releaseDirty {
                var mutableContent = content
                applyRelease(&mutableContent, on: model.releaseMode)
                model.releaseDirty = false
            }
            if model.geometryDirty {
                rebuildGeometry(content)
                model.geometryDirty = false
                model.parameterDirty = false
            } else if model.parameterDirty {
                if model.source.isGripper {
                    // Live re-solve, CHEAP roots only: the enclosure cutout + board
                    // connector follow the finger every frame (~15 ms). The
                    // expensive sheet-metal fold + copper are deferred to settle, so
                    // the drag stays smooth (heeding the tween lesson). Reassign
                    // meshes in place (no re-pop) and ride the handle along.
                    let scene = model.gripperSceneCheap()
                    if let root = content.entities.first(where: { $0.name == "geomRoot" }),
                       let centering = root.findEntity(named: "centering") {
                        for (i, item) in scene.meshes.enumerated() {
                            if let pe = centering.findEntity(named: "part\(i)") as? ModelEntity {
                                pe.model?.mesh = item.mesh
                                Self.applyPartPicking(pe, mesh: item.mesh, model: model)
                            }
                        }
                        centering.findEntity(named: "connectorHandle")?.position =
                            model.connectorHandlePosition()
                    }
                    // Copper now lags the connector — dim it until the drag settles.
                    if model.copperStale { setCopperDimmed(content, true) }
                } else {
                    let recreated = model.streamSandbox()
                    if let root = content.entities.first(where: { $0.name == "geomRoot" }) {
                        if let res = model.streaming.resource,
                           let entity = root.findEntity(named: "part0") as? ModelEntity {
                            if recreated { entity.model?.mesh = res }
                            // Streaming mutates the buffers in place — refresh
                            // the collider either way so picks track the solve.
                            Self.applyPartPicking(entity, mesh: res, model: model)
                        }
                        if model.showsHandle {
                            root.findEntity(named: "filletHandle")?.position =
                                model.handlePosition(radius: model.modifierValue)
                        }
                    }
                }
                model.parameterDirty = false
            }
            // Kinematic playback: re-pose the instance entities from the
            // latest FK solve (transforms only — meshes never change).
            if model.playbackDirty {
                if let root = content.entities.first(where: { $0.name == "geomRoot" }),
                   let centering = root.findEntity(named: "centering") {
                    for (i, m) in model.instanceTransforms.enumerated() {
                        centering.findEntity(named: "inst\(i)")?.transform = Transform(matrix: m)
                    }
                }
                model.playbackDirty = false
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

            // Live parameter edit: re-evaluate the doc and swap part meshes in
            // place (no entity rebuild → smooth scrub, no materialize-pop). Part
            // count is unchanged for scalar/vec/int edits; materials/highlight
            // ride along untouched.
            if model.docParamDirty {
                if let meshes = model.reevalDocumentMeshes(),
                   let root = content.entities.first(where: { $0.name == "geomRoot" }),
                   let centering = root.findEntity(named: "centering") {
                    for (i, m) in meshes.enumerated() {
                        if let pe = centering.findEntity(named: "part\(i)") as? ModelEntity {
                            syncPartEntity(pe, index: i, mesh: m)
                        }
                        if i < model.docPartEdges.count,
                           let ribbon = EdgeOverlay.ribbonResource(
                               segments: model.docPartEdges[i],
                               width: max(model.displayedSceneSize * 0.0016, 0.02),
                               name: "edges\(i)") {
                            (centering.findEntity(named: "edges\(i)") as? ModelEntity)?.model?.mesh = ribbon
                        }
                        // Ride the selection highlight ribbon along the scrub.
                        if let o = centering.findEntity(named: "outline\(i)") as? ModelEntity,
                           let ribbon = EdgeOverlay.ribbonResource(
                               segments: i < model.docPartEdges.count ? model.docPartEdges[i] : [],
                               width: outlineWidth(selected: model.highlightedParts.contains(i)),
                               name: "outline\(i)") {
                            o.model?.mesh = ribbon
                        }
                    }
                    // Slide the gizmo with the part (don't rebuild — that would
                    // destroy the arm the drag is holding).
                    if let c = model.gizmoCenterKernel() {
                        centering.findEntity(named: "gizmoRoot")?.position = c + model.gizmoLiveOffset
                    }
                }
                model.docParamDirty = false
            }

            // Feature-tree → viewport: hide/show parts and highlight the
            // selected one (documents only; cheap entity-property writes).
            if model.visibilityDirty {
                applyVisibility(content)
                model.visibilityDirty = false
            }
            if model.selectionDirty || model.hoverDirty {
                applySelectionHighlight(content)
                if model.selectionDirty { rebuildGizmo(content) }   // move to new selection
                model.selectionDirty = false
                model.hoverDirty = false
            }
            if model.gizmoDirty {
                rebuildGizmo(content)
                model.gizmoDirty = false
            }

            // Sketch overlay: rebuild the in-progress profile + rubber-band on
            // every tap / cursor move (tiny geometry, so a full rebuild is fine).
            if model.sketchDirty {
                if let root = content.entities.first(where: { $0.name == "geomRoot" }),
                   let centering = root.findEntity(named: "centering") {
                    centering.findEntity(named: "sketchRoot")?.removeFromParent()
                    if model.sketching { centering.addChild(buildSketchRoot(model: model)) }
                }
                model.sketchDirty = false
            }

            // Settle: the connector stopped moving — run the EXPENSIVE domains
            // once and snap them crisp. Re-fold the sheet-metal bracket (full
            // solve, all roots) and re-route the copper. The cheap mechanical
            // re-solve already tracked the finger per frame; these heavy passes
            // run a single time per gesture, never in the drag hot loop.
            if model.copperDirty {
                // ONE solve returns every domain: meshes (incl. the sheet-metal
                // fold) + copper, both from the same resolved connector_x.
                if model.source.isGripper, let solve = model.gripperSolve() {
                    if let root = content.entities.first(where: { $0.name == "geomRoot" }),
                       let centering = root.findEntity(named: "centering") {
                        for (i, mesh) in solve.meshes.enumerated() {
                            if let pe = centering.findEntity(named: "part\(i)") as? ModelEntity {
                                pe.model?.mesh = mesh
                                Self.applyPartPicking(pe, mesh: mesh, model: model)
                            }
                        }
                    }
                    redrawCopper(content, solve.copper)
                }
                model.copperDirty = false
            }

            // Apply the hover pop on the draggable handles.
            if let root = content.entities.first(where: { $0.name == "geomRoot" }),
               let centering = root.findEntity(named: "centering") {
                if let h = centering.findEntity(named: "connectorHandle") {
                    applyHover(h, hovered: model.hoveredHandle == "connectorHandle")
                }
                if let h = centering.findEntity(named: "filletHandle") {
                    applyHover(h, hovered: model.hoveredHandle == "filletHandle")
                }
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
          .onContinuousHover(coordinateSpace: .local) { phase in
              hover(phase, viewSize: geo.size)
          }
          .onAppear { model.installScrollZoom(); model.installKeyMonitor() }
          // Ray-traced still overlay: fades in over the rasterized view once
          // the camera settles; any camera/edit motion drops it instantly.
          .overlay {
              if model.raytraceEnabled, let img = model.raytraceImage {
                  Image(nsImage: img)
                      .resizable()
                      .interpolation(.high)
                      .transition(.opacity.animation(.easeIn(duration: 0.25)))
                      .allowsHitTesting(false)
              }
          }
          // Kinematic playback transport — shown only when the document has
          // an animation timeline with joint tracks (and instances to move).
          .overlay(alignment: .bottom) {
              if model.hasPlayback {
                  PlaybackBar(model: model)
                      .padding(.bottom, 14)
              }
          }
          .overlay(alignment: .bottomTrailing) {
              if model.raytraceEnabled {
                  Text(model.raytraceImage == nil ? "RT …" : "RT")
                      .font(.system(size: 10, weight: .semibold, design: .monospaced))
                      .foregroundStyle(model.raytraceImage == nil ? .secondary : Color.green)
                      .padding(.horizontal, 8).padding(.vertical, 4)
                      .background(.regularMaterial, in: Capsule(style: .continuous))
                      .padding(10)
                      .allowsHitTesting(false)
              }
          }
          .task(id: raytraceKey(size: geo.size)) {
              guard model.raytraceEnabled, model.usesDocumentTree else { return }
              model.raytraceImage = nil
              model.raytraceToken += 1
              let token = model.raytraceToken
              // Settle debounce: skip while the camera is still moving.
              try? await Task.sleep(nanoseconds: 350_000_000)
              guard !Task.isCancelled, token == model.raytraceToken else { return }
              // 1x-point resolution: the parallel tracer holds this in the
              // sub-second range, and analytic edges upscale cleanly.
              let w = max(64, Int(geo.size.width))
              let h = max(64, Int(geo.size.height))
              let image = await model.raytraceStillAsync(width: w, height: h)
              guard !Task.isCancelled, token == model.raytraceToken else { return }
              model.raytraceImage = image
          }
        }
    }

    /// Anything that should invalidate the ray-traced still: camera orbit,
    /// pan, zoom, parameter edits, geometry, selection tint, and view size.
    private func raytraceKey(size: CGSize) -> String {
        guard model.raytraceEnabled else { return "off" }
        let c = model.cameraPosition, p = model.panOffset
        return "\(c.x),\(c.y),\(c.z)|\(p.x),\(p.y),\(p.z)|\(model.solveMillis)|\(Int(size.width))x\(Int(size.height))"
    }

    // MARK: scene

    private func setupScene(_ content: RealityViewCameraContent) {
        let camera = Entity()
        camera.name = "camera"
        camera.components.set(PerspectiveCameraComponent())
        camera.position = model.cameraPosition
        camera.look(at: model.panOffset, from: model.cameraPosition, relativeTo: nil)
        content.add(camera)

        // Key light with a soft grounding shadow.
        let key = DirectionalLight()
        key.light.intensity = 2300
        key.shadow = DirectionalLightComponent.Shadow(maximumDistance: 4, depthBias: 2)
        key.look(at: .zero, from: [0.7, 1.1, 0.85], relativeTo: nil)
        content.add(key)

        // Cool rim for silhouette separation.
        let rim = DirectionalLight()
        rim.light.intensity = 1900
        rim.look(at: .zero, from: [-0.9, 0.35, -1.0], relativeTo: nil)
        content.add(rim)

        // Soft front-low fill to lift shadow detail without flattening.
        let fill = DirectionalLight()
        fill.light.intensity = 900
        fill.look(at: .zero, from: [-0.2, 0.5, 1.2], relativeTo: nil)
        content.add(fill)
    }

    /// A dark studio environment drawn procedurally (no bundled HDR), used for
    /// both the skybox backdrop and image-based reflections on the geometry.
    /// A soft ceiling, a horizon band, and three softbox highlights at varied
    /// azimuths give metals real reflection structure as the camera orbits.
    static let studioEnvironment: EnvironmentResource? = makeStudioEnvironment()
    static let zebraEnvironment: EnvironmentResource? = makeZebraEnvironment()

    /// Equirect of bold horizontal bands: the classic zebra-analysis light box.
    /// Reflected off a chromed surface, stripe kinks expose G1/G2 breaks.
    private static func makeZebraEnvironment() -> EnvironmentResource? {
        let w = 2048, h = 1024
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        ctx.setFillColor(CGColor(gray: 0.03, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        // Latitude bands: even count, mirrored about the horizon so stripes stay
        // readable at any camera elevation. 16 white bands ≈ classic light-tube rig.
        let bands = 32
        let bandH = CGFloat(h) / CGFloat(bands)
        ctx.setFillColor(CGColor(gray: 0.98, alpha: 1))
        for i in stride(from: 0, to: bands, by: 2) {
            ctx.fill(CGRect(x: 0, y: CGFloat(i) * bandH, width: CGFloat(w), height: bandH))
        }
        guard let img = ctx.makeImage() else { return nil }
        return try? EnvironmentResource(equirectangular: img)
    }

    private static func makeStudioEnvironment() -> EnvironmentResource? {
        let w = 2048, h = 1024
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        let fw = CGFloat(w), fh = CGFloat(h)
        // Base: soft ceiling (top) → horizon → near-black floor (bottom).
        let base = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0.20, green: 0.21, blue: 0.25, alpha: 1),   // zenith
            CGColor(red: 0.13, green: 0.14, blue: 0.17, alpha: 1),
            CGColor(red: 0.07, green: 0.075, blue: 0.09, alpha: 1),  // horizon
            CGColor(red: 0.025, green: 0.025, blue: 0.032, alpha: 1),
            CGColor(red: 0.008, green: 0.008, blue: 0.011, alpha: 1), // nadir
        ] as CFArray, locations: [0, 0.34, 0.52, 0.74, 1])!
        ctx.drawLinearGradient(base, start: CGPoint(x: 0, y: fh), end: CGPoint(x: 0, y: 0), options: [])

        ctx.setBlendMode(.plusLighter)
        // A faint horizon band so reflective edges catch a bright line.
        let horizon = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0.16, green: 0.18, blue: 0.22, alpha: 0),
            CGColor(red: 0.16, green: 0.18, blue: 0.22, alpha: 0.7),
            CGColor(red: 0.16, green: 0.18, blue: 0.22, alpha: 0),
        ] as CFArray, locations: [0, 0.5, 1])!
        ctx.drawLinearGradient(horizon,
            start: CGPoint(x: 0, y: fh * 0.40), end: CGPoint(x: 0, y: fh * 0.56), options: [])

        // Softbox highlights — bright soft blobs the metal reflects.
        func softbox(_ cx: CGFloat, _ cy: CGFloat, _ rad: CGFloat, _ c: CGColor) {
            let g = CGGradient(colorsSpace: cs, colors: [c, c.copy(alpha: 0)!] as CFArray, locations: [0, 1])!
            ctx.drawRadialGradient(g, startCenter: CGPoint(x: cx, y: cy), startRadius: 0,
                                   endCenter: CGPoint(x: cx, y: cy), endRadius: rad, options: [])
        }
        softbox(fw * 0.20, fh * 0.95, fh * 0.30, CGColor(red: 0.20, green: 0.23, blue: 0.28, alpha: 1)) // cool key
        softbox(fw * 0.56, fh * 0.97, fh * 0.22, CGColor(red: 0.22, green: 0.20, blue: 0.17, alpha: 1)) // warm
        softbox(fw * 0.83, fh * 0.93, fh * 0.24, CGColor(red: 0.15, green: 0.18, blue: 0.22, alpha: 1)) // cool fill
        guard let img = ctx.makeImage() else { return nil }
        return try? EnvironmentResource(equirectangular: img)
    }

    /// Pick a round millimeter grid step (1/2/5 decade) so ~10–25 minor
    /// cells span the part regardless of its size.
    static func gridStepMM(forPartSize size: Float) -> Float {
        guard size.isFinite, size > 0 else { return 10 }
        let target = size / 12
        let decade = pow(10, floor(log10(target)))
        for m in [1 as Float, 2, 5, 10] where m * decade >= target {
            return m * decade
        }
        return 10 * decade
    }

    /// One minor grid cell, tiling: hairline right/top borders with a radial
    /// fade handled by the plane's texture as a whole. Major lines every 10
    /// cells come from a brighter line baked at the tile edge — the texture is
    /// a full 48×48-cell atlas so major/minor hierarchy and the radial fade
    /// are exact (no shader needed).
    static let gridTexture: TextureResource? = makeGridTexture()
    private static func makeGridTexture() -> TextureResource? {
        let cells = 48, px = 32                  // 1536×1536: crisp hairlines
        let sz = cells * px
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: sz, height: sz, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        let center = CGFloat(sz) / 2
        let maxR = center * 0.98
        func lineAlpha(_ x: CGFloat, _ y: CGFloat, major: Bool) -> CGFloat {
            let d = hypot(x - center, y - center)
            let fade = max(0, 1 - d / maxR)
            return (major ? 0.72 : 0.34) * fade * fade
        }
        ctx.setLineWidth(1)
        for i in 0...cells {
            let major = i % 10 == 0 || i == cells
            let v = CGFloat(i * px)
            // Sample fade at several points along each line for a cheap
            // radial falloff without per-pixel work.
            let steps = 24
            for sIdx in 0..<steps {
                let t0 = CGFloat(sIdx) / CGFloat(steps) * CGFloat(sz)
                let t1 = CGFloat(sIdx + 1) / CGFloat(steps) * CGFloat(sz)
                let mid = (t0 + t1) / 2
                let aV = lineAlpha(v, mid, major: major)
                if aV > 0.003 {
                    ctx.setStrokeColor(CGColor(gray: 1.0, alpha: aV))
                    ctx.move(to: CGPoint(x: v, y: t0)); ctx.addLine(to: CGPoint(x: v, y: t1))
                    ctx.strokePath()
                }
                let aH = lineAlpha(mid, v, major: major)
                if aH > 0.003 {
                    ctx.setStrokeColor(CGColor(gray: 1.0, alpha: aH))
                    ctx.move(to: CGPoint(x: t0, y: v)); ctx.addLine(to: CGPoint(x: t1, y: v))
                    ctx.strokePath()
                }
            }
        }
        guard let img = ctx.makeImage() else { return nil }
        return try? TextureResource(image: img, options: .init(semantic: .color))
    }

    /// A soft radial alpha disc (dark center → clear edge) for the pooled contact
    /// shadow. Resolution-independent, so it's built once and stretched per part.
    private static let contactTexture: TextureResource? = makeContactTexture()
    private static func makeContactTexture() -> TextureResource? {
        let s = 256
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: s, height: s, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        let g = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0, green: 0, blue: 0, alpha: 0.5),
            CGColor(red: 0, green: 0, blue: 0, alpha: 0.32),
            CGColor(red: 0, green: 0, blue: 0, alpha: 0.0),
        ] as CFArray, locations: [0, 0.45, 1])!
        let c = CGFloat(s) / 2
        ctx.drawRadialGradient(g, startCenter: CGPoint(x: c, y: c), startRadius: 0,
                               endCenter: CGPoint(x: c, y: c), endRadius: c, options: [])
        guard let img = ctx.makeImage() else { return nil }
        return try? TextureResource(image: img, options: .init(semantic: .color))
    }

    private func rebuildGeometry(_ content: RealityViewCameraContent) {
        content.entities.filter { $0.name == "geomRoot" }.forEach { $0.removeFromParent() }

        let scene = model.buildScene()
        let sceneScale = 0.6 / max(scene.size, 0.0001)

        let centering = Entity()
        centering.name = "centering"
        centering.position = -scene.center
        model.centeringEntity = centering        // scene handle for native raycasts
        for (i, item) in scene.meshes.enumerated() {
            // The gripper's parts get intentional materials (glass enclosure,
            // green FR4 board, brushed-metal bracket) so each domain reads as what
            // it is; everything else falls back to the index palette.
            let mat: PhysicallyBasedMaterial =
                model.zebraMode ? Self.zebraChrome
                : model.source.isGripper ? gripperMaterial(i)
                : model.usesDocumentTree ? pbrMaterial(model.resolvedMaterial(forPart: i))
                : material(item.color)
            let entity = ModelEntity()
            entity.name = "part\(i)"
            if let inst = i < scene.instancing.count ? scene.instancing[i] : nil {
                // Pattern root: one shared MeshResource, N instance entities
                // with per-copy rigid transforms (kernel-space, under centering).
                for (j, t) in inst.transforms.enumerated() {
                    let child = ModelEntity(mesh: item.mesh, materials: [mat])
                    child.name = "part\(i)_inst\(j)"
                    child.transform = Transform(matrix: t)
                    Self.applyPartPicking(child, mesh: item.mesh, model: model)
                    entity.addChild(child)
                }
            } else {
                entity.model = ModelComponent(mesh: item.mesh, materials: [mat])
                Self.applyPartPicking(entity, mesh: item.mesh, model: model)
            }
            centering.addChild(entity)
            // CAD edge overlay: crisp feature edges over the shading. Width is
            // proportional to the scene so it stays visually constant across
            // part sizes.
            if i < scene.edges.count,
               let ribbon = EdgeOverlay.ribbonResource(
                   segments: scene.edges[i],
                   width: max(scene.size * 0.0016, 0.02),
                   name: "edges\(i)") {
                let edgeEntity = ModelEntity(mesh: ribbon, materials: [Self.edgeMaterial])
                edgeEntity.name = "edges\(i)"
                edgeEntity.isEnabled = !model.zebraMode
                centering.addChild(edgeEntity)
            }
        }
        // Assembly instances: local mesh + world transform per entity, so
        // kinematic playback re-poses transforms without touching meshes.
        for inst in scene.instances {
            let entity = ModelEntity(mesh: inst.mesh, materials: [pbrMaterial(inst.material)])
            entity.name = "inst\(inst.index)"
            entity.transform = Transform(matrix: inst.transform)
            centering.addChild(entity)
        }
        if model.showsHandle {
            centering.addChild(makeHandle(radius: model.modifierValue))
        }
        if model.showsConnectorHandle {
            centering.addChild(makeConnectorHandle(at: model.connectorHandlePosition()))
        }
        if model.source.isGripper {
            centering.addChild(buildCopperRoot(model.routeGripperCopper()))
        }
        if model.showsGizmo {
            centering.addChild(buildGizmo(model: model))
        }

        let zUp = Entity()
        zUp.addChild(centering)
        zUp.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])

        let geomRoot = Entity()
        geomRoot.name = "geomRoot"
        geomRoot.addChild(zUp)
        content.add(geomRoot)
        if model.suppressMaterializePop {
            // An edit re-eval: snap to final size (no pop) so scrubs feel direct.
            geomRoot.scale = SIMD3<Float>(repeating: sceneScale)
            model.suppressMaterializePop = false
        } else {
            // Subtle "materialize" pop when the geometry changes (load / new part).
            geomRoot.scale = SIMD3<Float>(repeating: sceneScale * 0.9)
            var grown = geomRoot.transform
            grown.scale = SIMD3<Float>(repeating: sceneScale)
            geomRoot.move(to: grown, relativeTo: geomRoot.parent, duration: 0.3, timingFunction: .easeOut)
        }

        // Grounding floor + pooled contact shadow under the part.
        content.entities.filter { $0.name == "floor" || $0.name == "contactShadow" }
            .forEach { $0.removeFromParent() }
        let floorY = -(model.sizeMM.z * 0.5 * sceneScale) - 0.004
        var floorMat = PhysicallyBasedMaterial()
        floorMat.baseColor = .init(tint: NSColor(white: 0.025, alpha: 1.0))
        floorMat.roughness = 0.42            // a touch glossy → catches the IBL sheen
        floorMat.metallic = 0.0
        let floor = ModelEntity(mesh: .generatePlane(width: 8, depth: 8), materials: [floorMat])
        floor.name = "floor"
        floor.position = [0, floorY, 0]
        floor.isEnabled = !model.releaseMode
        content.add(floor)

        // Adaptive reference grid: minor cells snap to a 1/2/5-decade of the
        // part size so the spacing always reads as round millimeters, with a
        // radial fade so it grounds the part without stretching to the horizon.
        let cellMM = Self.gridStepMM(forPartSize: model.displayedSceneSize)
        let cellWorld = cellMM * sceneScale
        if cellWorld > 1e-5, let gridTex = Self.gridTexture {
            var gm = UnlitMaterial()
            gm.color = .init(tint: NSColor(white: 1.0, alpha: 0.85), texture: .init(gridTex))
            gm.blending = .transparent(opacity: .init(floatLiteral: 1.0))
            let cells = 48                        // 48×48 minor cells around origin
            let sizeW = cellWorld * Float(cells)
            var gd = MeshDescriptor(name: "grid")
            let h = sizeW / 2
            gd.positions = MeshBuffers.Positions([[-h, 0, -h], [h, 0, -h], [h, 0, h], [-h, 0, h]])
            gd.normals = MeshBuffers.Normals([[0, 1, 0], [0, 1, 0], [0, 1, 0], [0, 1, 0]])
            gd.textureCoordinates = MeshBuffers.TextureCoordinates([[0, 0], [1, 0], [1, 1], [0, 1]])
            gd.primitives = .triangles([0, 2, 1, 0, 3, 2, 0, 1, 2, 0, 2, 3])
            if let gmesh = try? MeshResource.generate(from: [gd]) {
                let grid = ModelEntity(mesh: gmesh, materials: [gm])
                grid.name = "grid"
                grid.position = [0, floorY + 0.0008, 0]
                grid.isEnabled = !model.releaseMode
                content.add(grid)
            }
        }

        // Soft AO blob right under the part (footprint: kernel XY → world XZ).
        if let tex = Self.contactTexture {
            var sm = UnlitMaterial()
            sm.color = .init(tint: .white, texture: .init(tex))
            sm.blending = .transparent(opacity: .init(floatLiteral: 1.0))
            let fwd = max(0.05, model.sizeMM.x * sceneScale * 1.8)
            let dpt = max(0.05, model.sizeMM.y * sceneScale * 1.8)
            let blob = ModelEntity(mesh: .generatePlane(width: fwd, depth: dpt), materials: [sm])
            blob.name = "contactShadow"
            blob.position = [0, floorY + 0.0015, 0]
            blob.isEnabled = !model.releaseMode
            content.add(blob)
        }
    }

    private func material(_ color: NSColor) -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: color)
        m.roughness = 0.34
        m.metallic = 0.55
        return m
    }

    /// Mirror-chrome for zebra analysis: near-zero roughness so the striped
    /// environment reflects as sharp bands.
    static let zebraChrome: PhysicallyBasedMaterial = {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: .white)
        m.metallic = 1.0
        m.roughness = 0.02
        return m
    }()

    /// Toggle zebra analysis: swap the environment and chrome/restore the part
    /// materials in place. Restore goes through a (pop-suppressed) rebuild so
    /// gripper/document/palette materials come back exactly as built.
    /// Release-to-desktop: keep the studio environment for LIGHTING only (no
    /// skybox → the view composites transparent over the desktop) and hide the
    /// grounding set (floor, grid, contact shadow). Window transparency is
    /// handled by ReleaseWindowConfigurator.
    private func applyRelease(_ content: inout RealityViewCameraContent, on: Bool) {
        if on {
            content.environment = .default
        } else if let env = model.zebraMode ? Self.zebraEnvironment : Self.studioEnvironment {
            content.environment = .skybox(env)
        }
        for name in ["floor", "grid", "contactShadow"] {
            content.entities.filter { $0.name == name }.forEach { $0.isEnabled = !on }
        }
    }

    private func applyZebra(_ content: inout RealityViewCameraContent, on: Bool) {
        if on {
            if let env = Self.zebraEnvironment { content.environment = .skybox(env) }
            guard let root = content.entities.first(where: { $0.name == "geomRoot" }),
                  let centering = root.findEntity(named: "centering") else { return }
            var i = 0
            while let e = centering.findEntity(named: "part\(i)") as? ModelEntity {
                for h in Self.partModelEntities(e) { h.model?.materials = [Self.zebraChrome] }
                centering.findEntity(named: "edges\(i)")?.isEnabled = false
                i += 1
            }
        } else {
            if let env = Self.studioEnvironment { content.environment = .skybox(env) }
            model.suppressMaterializePop = true
            rebuildGeometry(content)
        }
    }

    /// Build a PBR material from a resolved document material (color + metallic +
    /// roughness, with transmission rendered as translucency).
    private func pbrMaterial(_ r: ResolvedMaterial) -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: r.color)
        m.roughness = .init(floatLiteral: r.roughness)
        m.metallic = .init(floatLiteral: r.metallic)
        if r.transmission > 0.02 {
            m.blending = .transparent(opacity: .init(floatLiteral: max(0.14, 1 - r.transmission * 0.85)))
            m.faceCulling = .none
        }
        return m
    }

    /// Intentional materials for the gripper's known parts (enclosure 0, board 1,
    /// bracket 2) — so each domain reads as what it physically is.
    private func gripperMaterial(_ index: Int) -> PhysicallyBasedMaterial {
        switch index {
        case 0: return glassMaterial()  // enclosure — see inside
        case 2: return brushedMetal()   // sheet-metal bracket
        default: return pcbGreen()       // PCB board
        }
    }

    /// Brushed aluminium for the sheet-metal bracket — semi-matte so it reads as
    /// metal without mirroring the dark studio to black.
    private func brushedMetal() -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: NSColor(srgbRed: 0.56, green: 0.59, blue: 0.64, alpha: 1.0))
        m.roughness = 0.42
        m.metallic = 0.7
        return m
    }

    /// Dark green FR4 for the PCB.
    private func pcbGreen() -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: NSColor(srgbRed: 0.09, green: 0.40, blue: 0.20, alpha: 1.0))
        m.roughness = 0.5
        m.metallic = 0.0
        return m
    }

    /// A frosted-glass enclosure so the interior (board, connector, handle)
    /// reads through it.
    private func glassMaterial() -> PhysicallyBasedMaterial {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: NSColor(red: 0.78, green: 0.84, blue: 0.92, alpha: 1.0))
        m.roughness = 0.12
        m.metallic = 0.0
        m.blending = .transparent(opacity: .init(floatLiteral: 0.20))
        m.faceCulling = .none
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

    /// The connector handle — drag it along the board edge to drive `connector_x`,
    /// which re-solves the enclosure cutout and the board connector together.
    private func makeConnectorHandle(at pos: SIMD3<Float>) -> ModelEntity {
        let handle = ModelEntity(
            mesh: .generateSphere(radius: 2.4),
            materials: [UnlitMaterial(color: NSColor(red: 1.0, green: 0.62, blue: 0.12, alpha: 1.0))]
        )
        handle.name = "connectorHandle"
        handle.position = pos
        handle.components.set(CollisionComponent(shapes: [ShapeResource.generateSphere(radius: 3.6)]))
        handle.components.set(InputTargetComponent())
        return handle
    }

    // MARK: copper (slice 2)

    /// Route the slice-2 board at the current `connector_x` and build a fresh
    /// `copperRoot` of ribbon traces — copper for SIG, tin-grey for GND. The
    /// router gives straight board-local segments; each becomes a thin flat box
    /// oriented along the segment. Tiny counts, so plain ModelEntities suffice.
    private func buildCopperRoot(_ segs: [EditorModel.CopperSeg]) -> Entity {
        let copperRoot = Entity()
        copperRoot.name = "copperRoot"
        // Brighter copper / tin, and drawn ~2.4x the electrical width so the
        // re-route reads at demo distance (the routed width is still 0.25 mm).
        let cu = NSColor(srgbRed: 0.96, green: 0.60, blue: 0.22, alpha: 1.0)
        let gnd = NSColor(srgbRed: 0.74, green: 0.78, blue: 0.84, alpha: 1.0)
        for (i, s) in segs.enumerated() {
            let d = s.b - s.a
            let len = simd_length(d)
            guard len > 1e-4 else { continue }
            let ribbon = ModelEntity(
                mesh: .generateBox(size: SIMD3<Float>(len, max(s.width * 2.4, 0.6), 0.12)),
                materials: [UnlitMaterial(color: s.net == 1 ? gnd : cu)]
            )
            ribbon.name = "copper\(i)"
            ribbon.position = (s.a + s.b) / 2
            ribbon.orientation = simd_quatf(from: SIMD3<Float>(1, 0, 0), to: d / len)
            copperRoot.addChild(ribbon)
        }
        model.copperStale = false
        return copperRoot
    }

    /// Swap in a freshly-routed copperRoot from the given segments (drag-settle).
    private func redrawCopper(_ content: RealityViewCameraContent, _ segs: [EditorModel.CopperSeg]) {
        guard let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let centering = root.findEntity(named: "centering") else { return }
        centering.findEntity(named: "copperRoot")?.removeFromParent()
        centering.addChild(buildCopperRoot(segs))
    }

    /// Dim (or restore) the copper while it lags a moving connector.
    private func setCopperDimmed(_ content: RealityViewCameraContent, _ dim: Bool) {
        guard let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let copperRoot = root.findEntity(named: "copperRoot") else { return }
        copperRoot.components.set(OpacityComponent(opacity: dim ? 0.3 : 1.0))
    }

    // MARK: transform gizmo overlay

    /// Near-black unlit material for the feature-edge overlay — reads as ink
    /// lines over the shaded surfaces, the classic CAD look.
    private static let edgeMaterial: UnlitMaterial = {
        UnlitMaterial(color: NSColor(white: 0.09, alpha: 1.0))
    }()
}

// MARK: sketch preview overlay (shared: studio viewport + released desktop)

/// Build the in-progress sketch: committed segments + vertex dots in brand
/// pink, plus a cyan rubber-band from the last point / anchor to the cursor.
/// Shared between the studio viewport and the released-desktop ARView — both
/// parent it under their `centering` entity, so kernel coords line up.
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

// MARK: transform gizmo overlay (shared: studio viewport + released desktop)

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
/// Shared between the studio viewport and the released desktop — both parent
/// it under their `centering` entity (kernel coords).
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

extension ViewportView {
    private func rebuildGizmo(_ content: RealityViewCameraContent) {
        guard let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let centering = root.findEntity(named: "centering") else { return }
        centering.findEntity(named: "gizmoRoot")?.removeFromParent()
        if model.showsGizmo { centering.addChild(buildGizmo(model: model)) }
    }

    // MARK: pattern instancing helpers

    /// The ModelEntities that carry a part's geometry: the part entity itself,
    /// or its per-instance children when the part is a pattern rendered as
    /// shared-mesh instances.
    private static func partModelEntities(_ e: ModelEntity) -> [ModelEntity] {
        if e.model != nil { return [e] }
        return e.children.compactMap { $0 as? ModelEntity }
            .filter { $0.name.hasPrefix("\(e.name)_inst") }
    }

    /// In-place scrub update for one part: swap the mesh (and, for instanced
    /// patterns, sync the instance count + transforms) without rebuilding
    /// entities — materials, outlines, and colliders ride along. Handles the
    /// part flipping between plain and instanced mid-scrub (count 1 ↔ N).
    private func syncPartEntity(_ pe: ModelEntity, index i: Int, mesh m: MeshResource) {
        let inst = i < model.docPartInstancing.count ? model.docPartInstancing[i] : nil
        if let inst {
            var children = pe.children.compactMap { $0 as? ModelEntity }
                .filter { $0.name.hasPrefix("part\(i)_inst") }
            if pe.model != nil {          // was plain last frame → become container
                pe.model = nil
                pe.components.remove(CollisionComponent.self)
                pe.components.remove(InputTargetComponent.self)
            }
            let mat: any RealityKit.Material = children.first?.model?.materials.first
                ?? pbrMaterial(model.resolvedMaterial(forPart: i))
            while children.count > inst.transforms.count {
                children.removeLast().removeFromParent()
            }
            for (j, t) in inst.transforms.enumerated() {
                if j < children.count {
                    children[j].model?.mesh = m
                    children[j].transform = Transform(matrix: t)
                    Self.applyPartPicking(children[j], mesh: m, model: model)
                } else {
                    let child = ModelEntity(mesh: m, materials: [mat])
                    child.name = "part\(i)_inst\(j)"
                    child.transform = Transform(matrix: t)
                    Self.applyPartPicking(child, mesh: m, model: model)
                    pe.addChild(child)
                }
            }
        } else {
            for c in pe.children.filter({ $0.name.hasPrefix("part\(i)_inst") }) {
                c.removeFromParent()
            }
            if pe.model == nil {          // was instanced last frame → plain again
                pe.model = ModelComponent(
                    mesh: m, materials: [pbrMaterial(model.resolvedMaterial(forPart: i))])
            } else {
                pe.model?.mesh = m
            }
            Self.applyPartPicking(pe, mesh: m, model: model)
        }
    }

    // MARK: feature-tree sync (visibility + selection highlight)

    /// Enable/disable part entities to honor the tree's eye toggles + isolate.
    private func applyVisibility(_ content: RealityViewCameraContent) {
        guard let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let centering = root.findEntity(named: "centering") else { return }
        for i in 0..<model.partCount {
            centering.findEntity(named: "part\(i)")?.isEnabled = model.isPartVisible(i)
        }
    }

    /// Selection reads as brand-orange feature-edge ribbons: the same crisp
    /// edge polylines the CAD overlay already draws, rebuilt a touch wider in
    /// accent orange for the selected part (hover = thinner + dimmer). This is
    /// the classic CAD idiom — every silhouette, hole rim, and rib edge lights
    /// up exactly where the geometry is, with none of the inflated-hull
    /// artifacts. The part's dark edge ribbon is hidden while highlighted so
    /// the two coplanar ribbons don't z-fight.
    private func applySelectionHighlight(_ content: RealityViewCameraContent) {
        guard model.usesDocumentTree,
              let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let centering = root.findEntity(named: "centering") else { return }
        let sel = model.highlightedParts
        let hov = model.hoveredPartIndex
        for i in 0..<model.partCount {
            centering.findEntity(named: "outline\(i)")?.removeFromParent()
            let edges = centering.findEntity(named: "edges\(i)")
            let selected = sel.contains(i)
            guard selected || i == hov else { edges?.isEnabled = true; continue }
            guard let outline = outlineRibbon(index: i, selected: selected) else { continue }
            centering.addChild(outline)
            edges?.isEnabled = false
        }
    }

    /// Build the highlight ribbon for one part from its feature-edge segments
    /// (already aggregated across pattern instances). Nil if there are none.
    private func outlineRibbon(index i: Int, selected: Bool) -> ModelEntity? {
        guard i < model.docPartEdges.count,
              let ribbon = EdgeOverlay.ribbonResource(
                  segments: model.docPartEdges[i],
                  width: outlineWidth(selected: selected),
                  name: "outline\(i)") else { return nil }
        let color = selected
            ? EditorModel.brandOrange
            : NSColor(srgbRed: 0.62, green: 0.32, blue: 0.10, alpha: 1.0)   // dim hover
        let e = ModelEntity(mesh: ribbon, materials: [UnlitMaterial(color: color)])
        e.name = "outline\(i)"
        return e
    }

    /// Highlight ribbon width in kernel mm — a bit heavier than the standard
    /// edge overlay (0.0016×scene) so the accent reads, scaled with the scene
    /// so it stays visually constant across part sizes.
    private func outlineWidth(selected: Bool) -> Float {
        max(model.displayedSceneSize * (selected ? 0.0042 : 0.0026), selected ? 0.05 : 0.03)
    }

    // MARK: gestures

    private var handleDrag: some Gesture {
        DragGesture()
            .targetedToAnyEntity()
            .onChanged { value in
                // Transform gizmo: drag an axis arm or plane handle to translate
                // the part. Kernel ray from the live cursor → closest-point math.
                if model.gizmoHandle(for: value.entity.name) != nil {
                    NSCursor.closedHand.set()
                    if !model.draggingHandle {
                        model.draggingHandle = true
                        if let ray = kernelRay(at: value.startLocation, viewSize: model.viewSize) {
                            model.beginGizmoDrag(handle: value.entity.name, ray: ray)
                        }
                    }
                    if let ray = kernelRay(at: value.location, viewSize: model.viewSize) {
                        model.gizmoDragTo(ray: ray)
                    }
                    return
                }
                let isHandle = value.entity.name == "connectorHandle" || value.entity.name == "filletHandle"
                if isHandle { NSCursor.closedHand.set() }
                if value.entity.name == "connectorHandle" {
                    if !model.draggingHandle {
                        model.draggingHandle = true
                        model.beginConnectorDrag()
                    }
                    let delta = Double(value.translation.width) * 0.14
                    model.setConnectorX(model.connectorDragBaseline + delta)
                    return
                }
                if value.entity.name == "filletHandle" {
                    if !model.draggingHandle {
                        model.draggingHandle = true
                        model.handleBaseline = model.modifierValue
                    }
                    let delta = Double(-value.translation.height) * 0.03
                    model.modifierValue = max(0, min(12, model.handleBaseline + delta))
                    return
                }
                // Parts are input targets too (for native picking) — a drag
                // that starts on one is still an orbit/pan, same as empty space.
                orbitChanged(value.translation)
            }
            .onEnded { _ in
                if model.draggingHandle {
                    NSCursor.arrow.set()
                    model.draggingHandle = false
                    model.endGizmoDrag()
                    // Connector settled → re-route the copper once (gripper only).
                    if model.source.isGripper { model.copperDirty = true }
                } else {
                    orbitEnded()
                }
            }
    }

    // Drag orbits (with flick-to-spin momentum); ⇧-drag pans the look-at.
    // Shared by the plain background gesture and entity drags on parts.
    private func orbitChanged(_ translation: CGSize) {
        guard !model.draggingHandle else { return }
        if model.lastDrag == .zero { model.beginOrbit() }   // grab → stop coasting
        let dx = Float(translation.width - model.lastDrag.width)
        let dy = Float(translation.height - model.lastDrag.height)
        if NSEvent.modifierFlags.contains(.shift) {
            model.panBy(dx: dx, dy: dy)
        } else {
            model.orbitDrag(dx: dx, dy: dy)
        }
        model.lastDrag = translation
        NSCursor.closedHand.set()        // grabbing to orbit/pan
    }

    private func orbitEnded() {
        model.lastDrag = .zero
        model.endOrbit()                 // coast if the flick was fast
        if !model.draggingHandle { NSCursor.arrow.set() }
    }

    private var orbitGesture: some Gesture {
        DragGesture()
            .onChanged { orbitChanged($0.translation) }
            .onEnded { _ in orbitEnded() }
    }

    private var zoomGesture: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                model.stopSpin()
                model.distance = max(0.45, min(8.0, model.pinchBaseline / Float(value.magnification)))
            }
            .onEnded { _ in model.pinchBaseline = model.distance }
    }

    // MARK: picking (#7)

    /// Screen point → ray in RealityKit world space (origin, normalized dir).
    /// Inverse of the pinhole projection; the basis for both native raycasts
    /// and the kernel-space conversion below.
    private func worldRay(at p: CGPoint, viewSize: CGSize) -> (o: SIMD3<Float>, d: SIMD3<Float>)? {
        guard viewSize.width > 1, viewSize.height > 1 else { return nil }
        let cam = model.cameraPosition
        let forward = normalize(model.panOffset - cam)
        let right = normalize(cross(forward, SIMD3<Float>(0, 1, 0)))
        let up = cross(right, forward)
        let tanHalf = Float(tan(Double.pi / 6.0))   // 60° vertical FOV / 2
        let aspect = Float(viewSize.width / viewSize.height)
        let ndcX = Float(2 * p.x / viewSize.width - 1)
        let ndcY = Float(1 - 2 * p.y / viewSize.height)
        return (cam, normalize(forward + ndcX * tanHalf * aspect * right + ndcY * tanHalf * up))
    }

    /// Screen point → ray in kernel space — still needed for sketch-plane taps
    /// and the gizmo's closest-point drag math.
    private func kernelRay(at p: CGPoint, viewSize: CGSize) -> (o: SIMD3<Float>, d: SIMD3<Float>)? {
        guard let (cam, dirWorld) = worldRay(at: p, viewSize: viewSize) else { return nil }
        let s = model.displayScale
        return (rxPlus90(cam / s) + model.displayCenter, normalize(rxPlus90(dirWorld)))
    }

    private func pick(at p: CGPoint, viewSize: CGSize) {
        model.viewSize = viewSize                 // keep fresh for gizmo drags
        model.stopSpin()                          // a tap settles the camera

        // Sketch mode: a tap drops a point on the sketch plane.
        if model.sketching {
            if let (o, d) = kernelRay(at: p, viewSize: viewSize),
               let pt = model.sketchPlanePoint(originKernel: o, dirKernel: d) {
                model.sketchTap(pt)
            }
            return
        }

        let hits = raycastHits(at: p, viewSize: viewSize)

        // Documents: tap a part → select its row (⌘-tap toggles multi-select, for
        // booleans); tap empty space → deselect. Native collision raycast.
        if model.usesDocumentTree {
            let cmd = NSEvent.modifierFlags.contains(.command)
            if let pi = hits.compactMap({ Self.partIndex($0.entity) }).first,
               pi < model.featureNodes.count {
                if cmd { model.toggleMultiSelect(part: pi, featureID: model.featureNodes[pi].id) }
                else { model.selectFeature(model.featureNodes[pi].id) }
            } else if !cmd,
                      !hits.contains(where: { model.gizmoHandle(for: $0.entity.name) != nil }) {
                model.deselectAll()        // empty space (but not a tap on the gizmo)
            }
            return
        }

        // Sandbox: surface probe — nearest part hit, converted world → kernel
        // through the live centering entity (handles Z-up rotation + scale).
        if model.source.isSandbox,
           let hit = hits.first(where: { Self.partIndex($0.entity) != nil }),
           let centering = model.centeringEntity {
            let pt = centering.convert(position: hit.position, from: nil)
            let n = normalize(centering.convert(direction: hit.normal, from: nil))
            model.pickPoint = pt
            model.pickInfo = describe(EditorModel.PickHit(point: pt, normal: n))
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

    // MARK: hover (cursor + a scale pop on the draggable handles)

    private func hover(_ phase: HoverPhase, viewSize: CGSize) {
        model.viewSize = viewSize                 // keep fresh for gizmo drags
        // Sketch mode: track the cursor on the plane for the rubber-band preview.
        if model.sketching {
            if case .active(let point) = phase,
               let (o, d) = kernelRay(at: point, viewSize: viewSize),
               let pt = model.sketchPlanePoint(originKernel: o, dirKernel: d) {
                model.sketchCursor = pt
                model.sketchDirty = true
                NSCursor.crosshair.set()
            } else if case .ended = phase {
                NSCursor.arrow.set()
            }
            return
        }
        // Documents: hover-highlight the part under the cursor (native collision
        // raycast), and show a pointing cursor to signal it's clickable.
        if model.usesDocumentTree {
            switch phase {
            case .active(let point):
                let hits = raycastHits(at: point, viewSize: viewSize)
                // Gizmo handles win over part hover (they sit on top). Never
                // re-highlight mid-drag — that would rebuild the grabbed arm.
                let gh = (model.showsGizmo && !model.draggingHandle)
                    ? hits.first(where: { model.gizmoHandle(for: $0.entity.name) != nil })?.entity.name
                    : nil
                if gh != model.hoveredGizmoHandle { model.hoveredGizmoHandle = gh; model.gizmoDirty = true }
                if gh != nil {
                    if model.hoveredPartIndex != nil { model.hoveredPartIndex = nil; model.hoverDirty = true }
                    NSCursor.openHand.set()
                    return
                }
                let pi = hits.compactMap { Self.partIndex($0.entity) }.first
                if pi != model.hoveredPartIndex { model.hoveredPartIndex = pi; model.hoverDirty = true }
                (pi != nil ? NSCursor.pointingHand : NSCursor.arrow).set()
            case .ended:
                if model.hoveredGizmoHandle != nil { model.hoveredGizmoHandle = nil; model.gizmoDirty = true }
                if model.hoveredPartIndex != nil { model.hoveredPartIndex = nil; model.hoverDirty = true }
                NSCursor.arrow.set()
            }
            return
        }

        switch phase {
        case .active(let point):
            // The draggable handles' colliders answer hover directly; handle
            // hits win over any part in front (the glass enclosure encloses
            // the gripper's connector handle).
            let hit = raycastHits(at: point, viewSize: viewSize)
                .first(where: { $0.entity.name == "connectorHandle" || $0.entity.name == "filletHandle" })?
                .entity.name
            if hit != nil, !model.draggingHandle {
                NSCursor.openHand.set()
            } else if hit == nil, model.hoveredHandle != nil {
                NSCursor.arrow.set()
            }
            if hit != model.hoveredHandle { model.hoveredHandle = hit }
        case .ended:
            if model.hoveredHandle != nil {
                model.hoveredHandle = nil
                NSCursor.arrow.set()
            }
        }
    }

    private func applyHover(_ e: Entity, hovered: Bool) {
        let target: Float = hovered ? 1.4 : 1.0
        if abs(e.scale.x - target) > 0.001 {
            e.scale = SIMD3<Float>(repeating: target)
        }
    }

    // MARK: native picking (collision raycasts)

    /// Make a part entity natively pickable: an input target plus a static-mesh
    /// collider generated from the render mesh, so scene raycasts hit exactly
    /// what is drawn. Collider generation is async; the token guards against an
    /// older build landing after a newer mesh swap.
    static func applyPartPicking(_ entity: ModelEntity, mesh: MeshResource, model: EditorModel) {
        entity.components.set(InputTargetComponent())
        let key = entity.name
        let token = (model.colliderTokens[key] ?? 0) + 1
        model.colliderTokens[key] = token
        Task { @MainActor in
            guard let shape = try? await ShapeResource.generateStaticMesh(from: mesh),
                  model.colliderTokens[key] == token else { return }
            entity.components.set(CollisionComponent(shapes: [shape]))
        }
    }

    /// All collision hits under a screen point, nearest first — parts, gizmo
    /// grab proxies, and drag handles all carry colliders.
    private func raycastHits(at p: CGPoint, viewSize: CGSize) -> [CollisionCastHit] {
        guard let scene = model.centeringEntity?.scene,
              let (o, d) = worldRay(at: p, viewSize: viewSize) else { return [] }
        return scene.raycast(origin: o, direction: d, length: 100,
                             query: .all, mask: .all, relativeTo: nil)
            .sorted { $0.distance < $1.distance }
    }

    /// "part7" → 7 (instance children "part7_inst2" also → 7); else nil.
    private static func partIndex(_ e: Entity) -> Int? {
        guard e.name.hasPrefix("part") else { return nil }
        let digits = e.name.dropFirst(4).prefix(while: \.isNumber)
        return digits.isEmpty ? nil : Int(digits)
    }
}
