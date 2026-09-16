import SwiftUI

/// Stable glass surfaces around the existing RealityKit canvas.
struct DesignModelNavigator: View {
    @Bindable var model: EditorModel
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Model").font(.headline)
                Spacer()
                Button { model.showsTree = false } label: { Image(systemName: "sidebar.left") }
                    .buttonStyle(.borderless).help("Hide model navigator").accessibilityLabel("Hide model navigator")
            }
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    FeatureTreeView(model: model, embedded: true)
                }.frame(maxHeight: 330).fixedSize(horizontal: false, vertical: true)
                    .onChange(of: model.selectedFeatureID) { _, id in
                        if let id { withAnimation { proxy.scrollTo(id, anchor: .center) } }
                    }
            }
            Divider()
            Text("\(model.usesDocumentTree ? model.featureNodes.count : model.features.count) features")
                .font(.caption).foregroundStyle(.secondary)
        }.padding(16).frame(width: 236).cncFloatingPanel()
    }
}

struct DesignModelingTools: View {
    @Bindable var model: EditorModel
    var body: some View {
        HStack(spacing: 14) {
            Picker("Tool group", selection: $model.toolTab) {
                ForEach(model.availableTabs) { Text($0.label).tag($0) }
            }.labelsHidden().frame(width: 125)
            Divider().frame(height: 22)
            ScrollView(.horizontal) {
                HStack(spacing: 14) {
                    ForEach(model.tools(for: model.toolTab)) { tool in
                        Button(action: tool.action) { Label(tool.label, systemImage: tool.symbol) }
                            .disabled(!tool.enabled).help(tool.hint.isEmpty ? tool.label : tool.hint)
                    }
                }
            }.scrollIndicators(.hidden).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
            Button { model.undo() } label: { Image(systemName: "arrow.uturn.backward") }.disabled(!model.canUndo).help("Undo").accessibilityLabel("Undo")
            Button { model.redo() } label: { Image(systemName: "arrow.uturn.forward") }.disabled(!model.canRedo).help("Redo").accessibilityLabel("Redo")
        }.buttonStyle(.borderless).controlSize(.small)
            .disabled(model.cnc.machine.active)
            .onChange(of: model.source) { _, _ in if !model.availableTabs.contains(model.toolTab) { model.toolTab = .create } }
            .background {
                Button("") { model.disarm() }.keyboardShortcut(.cancelAction)
                    .frame(width: 0, height: 0).opacity(0).accessibilityHidden(true)
                    .disabled(model.armedShape == nil)
            }
    }
}

struct DesignBreadcrumb: View {
    @Bindable var model: EditorModel
    var body: some View {
        HStack(spacing: 8) {
            Text(model.source.label).lineLimit(1).truncationMode(.middle)
            if let title = selectedTitle {
                Image(systemName: "chevron.right").font(.system(size: 9))
                Text(title).foregroundStyle(.primary).lineLimit(1)
            }
        }.font(.caption).foregroundStyle(.secondary)
            .padding(.horizontal, 12).padding(.vertical, 7).cncFloatingPanel()
            .frame(maxWidth: 260).allowsHitTesting(false)
    }
    private var selectedTitle: String? {
        model.selectedFeatureNode?.name ?? model.features.first { $0.id == model.selectedFeatureID }?.name
    }
}

struct DesignViewportTools: View {
    @Bindable var model: EditorModel
    var body: some View {
        HStack(spacing: 12) {
            Menu("View") {
                Button("Isometric") { camera(.pi / 5, .pi / 7) }
                Button("Front") { camera(0, 0) }
                Button("Right") { camera(.pi / 2, 0) }
                Button("Top") { camera(0, 1.45) }
                Divider()
                Toggle("Zebra analysis", isOn: $model.zebraMode)
                Button("Show all parts") { model.showAllParts() }.disabled(!model.hasHiddenParts)
            }.menuStyle(.borderlessButton).fixedSize()
            Button { model.stopSpin(); model.distance = 1.7; model.panOffset = .zero } label: {
                Image(systemName: "viewfinder")
            }.buttonStyle(.borderless).help("Reset camera").accessibilityLabel("Reset camera")
            Text("mm").font(.caption).foregroundStyle(.secondary)
        }.padding(10).cncFloatingPanel()
    }
    private func camera(_ azimuth: Float, _ elevation: Float) {
        model.stopSpin(); model.azimuth = azimuth; model.elevation = elevation
    }
}

struct DesignTreeSurface: ViewModifier {
    var embedded: Bool
    @ViewBuilder func body(content: Content) -> some View {
        if embedded { content } else { content.cncFloatingPanel() }
    }
}
