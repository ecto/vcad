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
                .overlay(alignment: .topLeading) {
                    if !compact {
                        FeatureTreeView(model: model)
                            .frame(width: 206)
                            .padding(14)
                            .transition(.move(edge: .leading).combined(with: .opacity))
                    }
                }
                .overlay(alignment: compact ? .leading : .top) {
                    if model.source.isSandbox || model.usesDocumentTree {
                        ToolPaletteView(model: model, axis: compact ? .vertical : .horizontal)
                            .padding(compact ? .leading : .top, 14)
                    }
                }
                .overlay(alignment: .top) {
                    if model.source.isGripper {
                        GripperReceiptPill(model: model).padding(.top, 14)
                    }
                }
                .overlay(alignment: .topTrailing) {
                    if model.source.isGripper {
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
                    VStack(spacing: 10) {
                        if model.source.isSandbox && intent.draft.isEmpty && !intent.isThinking {
                            ExampleChips(intent: intent)
                                .transition(.opacity.combined(with: .move(edge: .bottom)))
                        }
                        StatusBarView(model: model)
                    }
                    .padding(.bottom, 14)
                    .animation(.smooth(duration: 0.3), value: model.source.isSandbox)
                    .animation(.smooth(duration: 0.25), value: intent.draft.isEmpty)
                }
                .toolbar {
                    ToolbarItem(placement: .navigation) { DocumentMenu(model: model) }
                    ToolbarItem(placement: .principal) { CommandBar(engine: intent, model: model) }
                }
                .navigationTitle("vcad")
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

// MARK: tool palette (native Borland-model)

struct ToolPaletteView: View {
    @Bindable var model: EditorModel
    var axis: Axis = .horizontal
    private var vertical: Bool { axis == .vertical }
    @Namespace private var paletteNS

    var body: some View {
        let outer = vertical ? AnyLayout(VStackLayout(spacing: 6)) : AnyLayout(HStackLayout(spacing: 10))
        let tabsLayout = vertical ? AnyLayout(VStackLayout(spacing: 4)) : AnyLayout(HStackLayout(spacing: 3))
        let toolsLayout = vertical ? AnyLayout(VStackLayout(spacing: 5)) : AnyLayout(HStackLayout(spacing: 6))
        return outer {
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
        }
        .padding(vertical ? 6 : 8)
        .glassCard(vertical ? 16 : 13)
        .animation(.snappy(duration: 0.26), value: model.toolTab)
        .animation(.snappy(duration: 0.22), value: model.baseShape)
        .animation(.snappy(duration: 0.22), value: model.modifier)
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
        .animation(.snappy(duration: 0.18), value: model.selectedFeatureID)
        .animation(.snappy(duration: 0.2), value: model.expandedFeatureIDs)
        .animation(.snappy(duration: 0.2), value: model.hiddenParts)
        .animation(.snappy(duration: 0.2), value: model.isolatedPart)
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
    private var rowBackground: Color {
        if selected { return Color.accentColor.opacity(0.20) }
        return hovering ? Color.white.opacity(0.06) : .clear
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
                        if let pi = node.partIndex, let mat = model.materialName(forPart: pi) {
                            row("Material", mat.capitalized)
                        }
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

struct ViewportView: View {
    let model: EditorModel

    var body: some View {
        _ = (model.azimuth, model.elevation, model.distance, model.modifierValue,
             model.source, model.selectedFeatureID, model.baseShape, model.modifier,
             model.pickDirty, model.connectorX, model.hoveredHandle, model.panOffset,
             model.copperDirty, model.copperStale, model.visibilityDirty, model.selectionDirty,
             model.docParamDirty)

        return GeometryReader { geo in
          RealityView { content in
            if let env = makeStudioEnvironment() { content.environment = .skybox(env) }
            setupScene(content)
            rebuildGeometry(content)
            model.geometryDirty = false
          } update: { content in
            if let camera = content.entities.first(where: { $0.name == "camera" }) {
                camera.position = model.cameraPosition
                camera.look(at: model.panOffset, from: model.cameraPosition, relativeTo: nil)
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
                            (centering.findEntity(named: "part\(i)") as? ModelEntity)?.model?.mesh = item.mesh
                        }
                        centering.findEntity(named: "connectorHandle")?.position =
                            model.connectorHandlePosition()
                    }
                    // Copper now lags the connector — dim it until the drag settles.
                    if model.copperStale { setCopperDimmed(content, true) }
                } else {
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

            // Live parameter edit: re-evaluate the doc and swap part meshes in
            // place (no entity rebuild → smooth scrub, no materialize-pop). Part
            // count is unchanged for scalar/vec/int edits; materials/highlight
            // ride along untouched.
            if model.docParamDirty {
                if let meshes = model.reevalDocumentMeshes(),
                   let root = content.entities.first(where: { $0.name == "geomRoot" }),
                   let centering = root.findEntity(named: "centering") {
                    for (i, m) in meshes.enumerated() {
                        (centering.findEntity(named: "part\(i)") as? ModelEntity)?.model?.mesh = m
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
            if model.selectionDirty {
                applySelectionHighlight(content)
                model.selectionDirty = false
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
                            (centering.findEntity(named: "part\(i)") as? ModelEntity)?.model?.mesh = mesh
                        }
                    }
                    redrawCopper(content, solve.copper)
                }
                model.copperDirty = false
            }

            // Keep the handles' world positions current + apply the hover pop.
            if let root = content.entities.first(where: { $0.name == "geomRoot" }),
               let centering = root.findEntity(named: "centering") {
                if let h = centering.findEntity(named: "connectorHandle") {
                    model.connectorHandleWorld = h.position(relativeTo: nil)
                    applyHover(h, hovered: model.hoveredHandle == "connectorHandle")
                }
                if let h = centering.findEntity(named: "filletHandle") {
                    model.filletHandleWorld = h.position(relativeTo: nil)
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
        }
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
        key.light.intensity = 5500
        key.shadow = DirectionalLightComponent.Shadow(maximumDistance: 4, depthBias: 2)
        key.look(at: .zero, from: [0.7, 1.1, 0.85], relativeTo: nil)
        content.add(key)

        // Cool rim for silhouette separation.
        let rim = DirectionalLight()
        rim.light.intensity = 2600
        rim.look(at: .zero, from: [-0.9, 0.35, -1.0], relativeTo: nil)
        content.add(rim)

        // Soft front-low fill to lift shadow detail without flattening.
        let fill = DirectionalLight()
        fill.light.intensity = 1400
        fill.look(at: .zero, from: [-0.2, 0.5, 1.2], relativeTo: nil)
        content.add(fill)
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
            // The gripper's parts get intentional materials (glass enclosure,
            // green FR4 board, brushed-metal bracket) so each domain reads as what
            // it is; everything else falls back to the index palette.
            let mat: PhysicallyBasedMaterial =
                model.source.isGripper ? gripperMaterial(i) : material(item.color)
            let entity = ModelEntity(mesh: item.mesh, materials: [mat])
            entity.name = "part\(i)"
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

    // MARK: feature-tree sync (visibility + selection highlight)

    /// Enable/disable part entities to honor the tree's eye toggles + isolate.
    private func applyVisibility(_ content: RealityViewCameraContent) {
        guard let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let centering = root.findEntity(named: "centering") else { return }
        for i in 0..<model.partCount {
            centering.findEntity(named: "part\(i)")?.isEnabled = model.isPartVisible(i)
        }
    }

    /// Tint the selected part's emissive brand-pink so the tree selection reads
    /// in the viewport. Non-selected parts are restored to their base material.
    private func applySelectionHighlight(_ content: RealityViewCameraContent) {
        guard model.usesDocumentTree,
              let root = content.entities.first(where: { $0.name == "geomRoot" }),
              let centering = root.findEntity(named: "centering") else { return }
        let sel = model.highlightedParts
        for i in 0..<model.partCount {
            guard let e = centering.findEntity(named: "part\(i)") as? ModelEntity else { continue }
            var m = material(model.documentBaseColor(i))
            if sel.contains(i) {
                m.emissiveColor = .init(color: EditorModel.brandPink)
                m.emissiveIntensity = 0.5
            }
            e.model?.materials = [m]
        }
    }

    // MARK: gestures

    private var handleDrag: some Gesture {
        DragGesture()
            .targetedToAnyEntity()
            .onChanged { value in
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
                guard value.entity.name == "filletHandle" else { return }
                if !model.draggingHandle {
                    model.draggingHandle = true
                    model.handleBaseline = model.modifierValue
                }
                let delta = Double(-value.translation.height) * 0.03
                model.modifierValue = max(0, min(12, model.handleBaseline + delta))
            }
            .onEnded { _ in
                if model.draggingHandle { NSCursor.arrow.set() }
                model.draggingHandle = false
                // Connector settled → re-route the copper once (gripper only).
                if model.source.isGripper { model.copperDirty = true }
            }
    }

    private var orbitGesture: some Gesture {
        // Drag orbits; ⇧-drag pans the look-at target.
        DragGesture()
            .onChanged { value in
                guard !model.draggingHandle else { return }
                let dx = Float(value.translation.width - model.lastDrag.width)
                let dy = Float(value.translation.height - model.lastDrag.height)
                if NSEvent.modifierFlags.contains(.shift) {
                    model.panBy(dx: dx, dy: dy)
                } else {
                    model.azimuth -= dx * 0.01
                    model.elevation = max(-1.45, min(1.45, model.elevation + dy * 0.01))
                }
                model.lastDrag = value.translation
                NSCursor.closedHand.set()        // grabbing to orbit/pan
            }
            .onEnded { _ in
                model.lastDrag = .zero
                if !model.draggingHandle { NSCursor.arrow.set() }
            }
    }

    private var zoomGesture: some Gesture {
        MagnifyGesture()
            .onChanged { value in
                model.distance = max(0.45, min(8.0, model.pinchBaseline / Float(value.magnification)))
            }
            .onEnded { _ in model.pinchBaseline = model.distance }
    }

    // MARK: picking (#7)

    private func pick(at p: CGPoint, viewSize: CGSize) {
        guard viewSize.width > 1, viewSize.height > 1 else { return }
        let cam = model.cameraPosition
        let forward = normalize(model.panOffset - cam)
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

        // Documents: tap a part → select its row (⌘-tap toggles multi-select, for
        // booleans); tap empty space → deselect. Kernel-space AABBs make it cheap.
        if model.usesDocumentTree {
            let cmd = NSEvent.modifierFlags.contains(.command)
            if let pi = model.pickDocumentPart(originKernel: camKernel, dirKernel: dirKernel),
               pi < model.featureNodes.count {
                if cmd { model.toggleMultiSelect(part: pi, featureID: model.featureNodes[pi].id) }
                else { model.selectFeature(model.featureNodes[pi].id) }
            } else if !cmd {
                model.deselectAll()
            }
            return
        }

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

    // MARK: hover (cursor + a scale pop on the draggable handles)

    private func hover(_ phase: HoverPhase, viewSize: CGSize) {
        switch phase {
        case .active(let point):
            let hit = hitHandle(at: point, viewSize: viewSize)
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

    /// Nearest visible handle whose projected screen position is within reach of
    /// the pointer, or nil.
    private func hitHandle(at p: CGPoint, viewSize: CGSize) -> String? {
        var best: (name: String, dist: CGFloat)?
        func consider(_ name: String, _ world: SIMD3<Float>) {
            guard let s = worldToScreen(world, viewSize) else { return }
            let d = hypot(s.x - p.x, s.y - p.y)
            if d < 24, best == nil || d < best!.dist { best = (name, d) }
        }
        if model.showsConnectorHandle { consider("connectorHandle", model.connectorHandleWorld) }
        if model.showsHandle { consider("filletHandle", model.filletHandleWorld) }
        return best?.name
    }

    /// Forward pinhole projection — the inverse of `pick`'s ray build: world → screen.
    private func worldToScreen(_ p: SIMD3<Float>, _ viewSize: CGSize) -> CGPoint? {
        guard viewSize.width > 1, viewSize.height > 1 else { return nil }
        let cam = model.cameraPosition
        let forward = normalize(model.panOffset - cam)
        let right = normalize(cross(forward, SIMD3<Float>(0, 1, 0)))
        let up = cross(right, forward)
        let rel = p - cam
        let z = dot(rel, forward)
        guard z > 0.0001 else { return nil }
        let tanHalf = Float(tan(Double.pi / 6.0))
        let aspect = Float(viewSize.width / viewSize.height)
        let ndcX = (dot(rel, right) / z) / (tanHalf * aspect)
        let ndcY = (dot(rel, up) / z) / tanHalf
        return CGPoint(x: Double((ndcX + 1) / 2) * viewSize.width,
                       y: Double((1 - ndcY) / 2) * viewSize.height)
    }

    private func applyHover(_ e: Entity, hovered: Bool) {
        let target: Float = hovered ? 1.4 : 1.0
        if abs(e.scale.x - target) > 0.001 {
            e.scale = SIMD3<Float>(repeating: target)
        }
    }
}
