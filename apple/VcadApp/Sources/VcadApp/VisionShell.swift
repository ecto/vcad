import SwiftUI

// Views the visionOS volume still composes. The Mac shell replaced these with
// the released-desktop panels (DesignShell.swift / ReleasedDesktop.swift); they
// are compiled out on macOS so the Mac binary carries no dead chrome.

#if !os(macOS)

/// The volume's inspector: the selected feature, its live parameters, the
/// document's named parameters, and measurements.
struct InspectorView: View {
    @Bindable var model: EditorModel
    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.l) {
            if model.usesDocumentTree {
                if let node = model.selectedFeatureNode {
                    section(node.name) {
                        KeyValueRow("Operation", DocumentGraph.label(node.opType))
                        if let pi = node.partIndex { materialPicker(pi) }
                        if let pi = node.partIndex, !model.isPartVisible(pi) {
                            Label("Hidden", systemImage: "eye.slash").font(.callout).foregroundStyle(.secondary)
                        }
                    }
                    if FeatureParamEditors.editableOps.contains(node.opType) {
                        section("Parameters") { FeatureParamEditors(model: model, node: node) }
                    } else if let d = node.detail {
                        section("Parameters") { KeyValueRow("Value", d) }
                    }
                }
            } else if let f = model.selectedFeature {
                section(f.name) {
                    switch f.kind {
                    case .base:
                        KeyValueRow("Shape", model.baseShape.label)
                    case .modifier:
                        if model.modifier == .none {
                            Text("No modifier").font(.callout).foregroundStyle(.secondary)
                        } else if !model.modifierEffective {
                            Label("No edges on a sphere", systemImage: "info.circle")
                                .font(.callout).foregroundStyle(.secondary)
                        } else {
                            VStack(alignment: .leading, spacing: Theme.Space.s) {
                                KeyValueRow(model.modifier.paramLabel, String(format: "%.1f mm", model.modifierValue))
                                Slider(value: $model.modifierValue, in: 0...12)
                            }
                        }
                    case .part:
                        KeyValueRow("Type", "Solid")
                    }
                }
            }
            if model.usesDocumentTree, !model.docParameters.isEmpty {
                section("Document Parameters") {
                    ForEach(model.docParameters) { p in
                        if let v = p.value {
                            ScrubField(label: p.name, value: v, unit: p.unit ?? "mm",
                                       sensitivity: FeatureParamEditors.paramSensitivity(p),
                                       minValue: p.min ?? -.greatestFiniteMagnitude) { v, s in
                                model.editParameter(p.name, value: v, snapshot: s)
                            }
                            .help(p.description ?? p.name)
                        } else if let f = p.formula {
                            KeyValueRow(p.name, "= \(f)").help(p.description ?? p.name)
                        }
                    }
                }
            }
            section("Measurements") {
                KeyValueRow("Triangles", model.triangleCount.formatted())
                KeyValueRow("Bounds", boundsText)
                KeyValueRow("Solve", String(format: "%.1f ms", model.solveMillis))
            }
            if let info = model.pickInfo {
                section("Picked") {
                    Text(info).font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                }
            }
            if model.canSimulate {
                Divider()
                SimInspector(model: model).padding(.top, 2)
            }
        }
        .padding(Theme.Space.l)
        .panelSurface()
    }

    @ViewBuilder private func section<C: View>(_ title: String, @ViewBuilder _ content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            Eyebrow(title)
            content()
        }
    }

    private func materialPicker(_ pi: Int) -> some View {
        let current = model.materialName(forPart: pi) ?? "default"
        let resolved = model.resolvedMaterial(forPart: pi)
        return HStack {
            Text("Material").font(.callout).foregroundStyle(.secondary)
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
                    Circle().fill(Color(portedColor: resolved.color)).frame(width: 11, height: 11)
                        .overlay(Circle().strokeBorder(.separator, lineWidth: 0.5))
                    Text(MaterialPreset.byKey(current)?.name ?? current.capitalized).font(.callout)
                }
            }
            .fixedSize()
        }
    }

    private var boundsText: String {
        let s = model.sizeMM
        return String(format: "%.1f × %.1f × %.1f mm", abs(s.x), abs(s.y), abs(s.z))
    }
}

/// The volume's tool palette: tabs, then the tools of the active tab.
struct ToolPaletteView: View {
    @Bindable var model: EditorModel
    var axis: Axis = .horizontal
    private var vertical: Bool { axis == .vertical }

    var body: some View {
        let outer = vertical ? AnyLayout(VStackLayout(spacing: 6)) : AnyLayout(HStackLayout(spacing: 10))
        let tabsLayout = vertical ? AnyLayout(VStackLayout(spacing: 4)) : AnyLayout(HStackLayout(spacing: 3))
        let toolsLayout = vertical ? AnyLayout(VStackLayout(spacing: 5)) : AnyLayout(HStackLayout(spacing: 6))
        outer {
            tabsLayout {
                ForEach(model.availableTabs) { tab in tabButton(tab) }
            }
            if vertical { Divider().frame(width: 24) } else { Divider().frame(height: 18) }
            toolsLayout {
                ForEach(model.tools(for: model.toolTab)) { tool in toolButton(tool) }
            }
            .id(model.toolTab)
            .transition(.opacity)
        }
        .animation(Motion.snappy, value: model.toolTab)
        .padding(vertical ? 6 : 8)
        .panelSurface()
    }

    @ViewBuilder private func tabButton(_ tab: ToolTab) -> some View {
        let active = model.toolTab == tab
        Button { model.toolTab = tab } label: {
            Group {
                if vertical {
                    Image(systemName: tab.symbol).font(.title3).frame(width: 42, height: 42)
                } else {
                    Label(tab.label, systemImage: tab.symbol).font(.callout.weight(.medium))
                        .padding(.horizontal, 10).padding(.vertical, 5)
                }
            }
            .selectableRow(selected: active)
        }
        .buttonStyle(.plain)
        .foregroundStyle(active ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
        .help(tab.label)
    }

    @ViewBuilder private func toolButton(_ tool: Tool) -> some View {
        Button { if tool.enabled { tool.action() } } label: {
            Group {
                if vertical {
                    Image(systemName: tool.symbol).font(.title3).frame(width: 42, height: 42)
                } else {
                    Label(tool.label, systemImage: tool.symbol).font(.callout)
                        .padding(.horizontal, 9).padding(.vertical, 5)
                }
            }
            .selectableRow(selected: tool.isActive)
        }
        .buttonStyle(.plain)
        .foregroundStyle(tool.isActive ? AnyShapeStyle(.primary) : AnyShapeStyle(.secondary))
        .opacity(tool.enabled ? 1 : 0.32)
        .disabled(!tool.enabled)
        .help(tool.enabled ? tool.label : "\(tool.label) — \(tool.hint)")
    }
}

/// Seed prompts over an untouched volume — tap to load one into the command bar.
struct ExampleChips: View {
    @Bindable var intent: IntentEngine
    var body: some View {
        HStack(spacing: Theme.Space.s) {
            Text("Try").font(.subheadline.weight(.medium)).foregroundStyle(.tertiary)
            ForEach(IntentEngine.examplePrompts.prefix(3), id: \.self) { prompt in
                Button {
                    intent.draft = prompt
                    intent.focusRequested = true
                } label: {
                    Text(prompt).font(.subheadline)
                        .padding(.horizontal, 11).padding(.vertical, 5)
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .pillSurface()
            }
        }
    }
}

/// The volume's composer: a `+` quick-start menu beside the AI command field.
struct ComposerBar: View {
    @Bindable var engine: IntentEngine
    let model: EditorModel
    var body: some View {
        HStack(spacing: Theme.Space.s) {
            Menu {
                Button("New") { model.newDocument() }
                if !model.examples.isEmpty {
                    Menu("Examples") {
                        ForEach(model.examples, id: \.path) { ex in
                            Button(ex.name) { model.openDocument(URL(fileURLWithPath: ex.path)) }
                        }
                    }
                }
            } label: {
                Image(systemName: "plus")
                    .font(.body.weight(.medium))
                    .foregroundStyle(.secondary)
                    .frame(width: 30, height: 30)
            }
            .menuIndicator(.hidden)
            .fixedSize()
            CommandBar(engine: engine, model: model)
        }
    }
}

#endif
