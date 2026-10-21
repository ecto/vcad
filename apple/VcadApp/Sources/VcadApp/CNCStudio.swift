import SwiftUI
import AppKit
import RealityKit

// Native CNC shell. Its viewport remains the existing ReleasedScene; these
// rails organize the same workspace and controller used in desktop mode.
private let cncAccent = Color.accentColor

struct WorkspaceHeader: View {
    @Bindable var model: EditorModel
    @State private var connectionShown = false
    private var cnc: CNCWorkspace { model.cnc }
    var body: some View {
        @Bindable var cnc = cnc
        GeometryReader { geometry in
            ZStack {
                HStack {
                    HStack(spacing: 14) {
                        BrandMark().accessibilityLabel("vcad")
                        Picker("Workspace", selection: $model.workspace) {
                            ForEach(Workspace.allCases) { Label($0.label, systemImage: $0.symbol).tag($0) }
                        }.pickerStyle(.menu).labelsHidden().fixedSize()
                            .accessibilityLabel("Workspace")
                            .help("Design, Electronics, or Manufacture (⌃⌘1–3)")
                    }
                    Spacer(minLength: 0)
                    HStack(spacing: 14) {
                        if model.workspace == .manufacture {
                            Button { connectionShown.toggle() } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: "circle.fill")
                                        .font(.caption2)
                                        .foregroundStyle(cnc.machine.connected && cnc.machine.status.isFresh ? Color.green : .secondary)
                                    Text(cnc.machine.demo ? "Simulator" : "Anolex")
                                    Text(cnc.machine.connected ? cnc.machine.status.state : "Disconnected").foregroundStyle(.secondary)
                                }.font(.caption).lineLimit(1)
                            }.buttonStyle(.borderless).help("Machine connection")
                                .accessibilityLabel("Machine connection, \(cnc.machine.summary)")
                                .popover(isPresented: $connectionShown, arrowEdge: .bottom) {
                                    CNCConnectionView(cnc: cnc).padding(18).frame(width: 320)
                                }
                        }
                        Menu {
                            switch model.workspace {
                            case .manufacture:
                                Toggle("Job Outline", isOn: $cnc.leftPanelShown)
                                Toggle("Inspector", isOn: $cnc.rightPanelShown)
                                Toggle("Drawer", isOn: $cnc.bottomPanelShown)
                            case .design:
                                Toggle("Model Navigator", isOn: $model.showsTree)
                                Toggle("Inspector", isOn: $model.showsInspector)
                            case .electronics:
                                Text("Electronics panels are always shown")
                            }
                            Divider()
                            Button(model.isWindowed ? "Release to Desktop" : "Open in Window") {
                                ReleaseWindowController.shared.setWindowed(!model.isWindowed)
                            }
                        } label: { Image(systemName: "rectangle.split.3x1") }
                            .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                            .help("View options").accessibilityLabel("View options")
                    }
                }.padding(.leading, model.isWindowed ? 92 : 16).padding(.trailing, 18)
                Menu {
                    if model.documentURL != nil {
                        Button("Save") { model.saveDocument() }.disabled(!model.documentDirty)
                        Button("Save As…") { saveAsPanel(model) }
                        Button("Revert to Saved") { model.revertDocument() }.disabled(!model.documentDirty)
                        Divider()
                        Button("Show in Finder") { model.showInFinder() }
                    } else if model.usesDocumentTree {
                        Button("Save…") { saveAsPanel(model) }
                    } else {
                        Text("Sandbox — add geometry to save")
                    }
                } label: {
                VStack(spacing: 1) {
                    HStack(spacing: 5) {
                        Image(systemName: model.documentURL == nil ? "cube" : "doc")
                            .font(.caption).foregroundStyle(.secondary)
                        Text(model.source.label).font(.body.weight(.semibold))
                            .lineLimit(1).truncationMode(.middle)
                    }
                    if model.documentDirty { Text("Edited").font(.caption).foregroundStyle(.secondary) }
                }
                }.menuStyle(.borderlessButton).menuIndicator(.visible)
                .fixedSize()
                .frame(maxWidth: max(100, geometry.size.width - 2 * (model.isWindowed ? 326 : 250)))
                .help(documentLocation)
                .accessibilityElement(children: .combine)
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
        }.frame(height: Theme.Height.header)
    }
    private var documentLocation: String {
        if case let .document(path, _) = model.source { return path }
        return model.source.label
    }
}

struct CNCConnectionView: View {
    @Bindable var cnc: CNCWorkspace
    private var machine: CNCController { cnc.machine }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Anolex 4030 Ultra 2").font(.headline)
            Text(machine.firmware).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            HStack {
                TextField("Host", text: Binding(get: { machine.host }, set: { machine.host = $0 }))
                TextField("Port", text: Binding(get: { machine.port }, set: { machine.port = $0 })).frame(width: 55)
            }.textFieldStyle(.roundedBorder).disabled(machine.connected || machine.connecting)
            HStack {
                if machine.connected || machine.connecting {
                    Button("Disconnect") { machine.disconnect(); cnc.setupConfirmed = false }
                } else {
                    Button("Connect") { machine.connect(); cnc.setupConfirmed = false }
                    Button("Simulator") { machine.connect(simulated: true); cnc.setupConfirmed = false }
                }
            }
            Text(machine.summary).font(.caption)
            if let error = machine.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
        }
    }
}

struct CNCStudioOutline: View {
    @Bindable var cnc: CNCWorkspace
    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Picker("Manufacturing stage", selection: $cnc.mode) {
                ForEach(CNCMode.allCases) { Text($0.rawValue).tag($0) }
            }.pickerStyle(.segmented).labelsHidden().controlSize(.small).padding(10)
            HStack(spacing: Theme.Space.s) {
                Menu {
                    Button("Outline (DXF)…") { cnc.importOutlineFile() }
                    Button("G-code…") { cnc.importFile() }
                } label: { Label("Import", systemImage: "square.and.arrow.down") }
                    .menuStyle(.borderlessButton).fixedSize()
                    .disabled(cnc.machine.active || cnc.generating)
                    .help("Import a DXF outline to machine, or a finished G-code program")
                Spacer(minLength: 0)
            }.font(.caption).padding(.horizontal, 12).padding(.bottom, 8)
            if let outline = cnc.outline {
                Label("\(outline.name) · \(counted(outline.holes.count, "hole"))", systemImage: "scribble.variable")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    .padding(.horizontal, 12).padding(.bottom, 8)
            }
            if cnc.importedProgram != nil {
                Button { cnc.useImportedJob() } label: {
                    Label(cnc.importedName, systemImage: "doc.text").font(.caption).lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(8)
                        .selectableRow(selected: cnc.usesImportedProgram)
                }.buttonStyle(.plain).padding(.horizontal, 7).disabled(cnc.machine.active)
                Button("Use generated job") { cnc.useGeneratedJob(); cnc.select(.operation(cnc.selectedOperation.id)) }
                    .font(.caption).padding(.horizontal, 12).disabled(cnc.machine.active || !cnc.usesImportedProgram)
                Divider().padding(.vertical, 8)
            }
            Eyebrow("Job setup").padding(Theme.Space.m)
            item("Stock", detail: "\(cnc.stockWidth.formatted()) × \(cnc.stockHeight.formatted()) × \(cnc.stockThickness.formatted()) mm", symbol: "shippingbox", selection: .stock)
            item("Work origin", detail: "G54 · stock top", symbol: "move.3d", selection: .origin)
            item("Tool", detail: "T1 · Ø \(cnc.toolDiameter.formatted()) mm", symbol: "wrench.adjustable", selection: .tool)
            Divider().padding(.horizontal, 16).padding(.vertical, 8)
            HStack {
                Eyebrow("Operations")
                Spacer()
                Menu {
                    ForEach(["face", "pocket", "profile"], id: \.self) { kind in
                        Button(CNCOperation.name(kind)) { cnc.addOperation(kind) }
                    }
                } label: { Image(systemName: "plus") }
                    .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                    .help("Add operation").accessibilityLabel("Add operation")
                    .disabled(cnc.machine.active || cnc.generating)
            }.padding(.horizontal, 18).padding(.bottom, 9)
            ScrollView {
                VStack(spacing: 3) {
                    ForEach(cnc.operations) { operation in
                        item(operation.name,
                             detail: operation.current ? "T1 · ~\(CNCWorkspace.durationLabel(operation.preview.duration))" : "T1 · needs generation",
                             symbol: operation.symbol, selection: .operation(operation.id), current: operation.current)
                            .contextMenu {
                                Button("Move up") { cnc.select(.operation(operation.id)); cnc.moveSelectedOperation(by: -1) }
                                Button("Move down") { cnc.select(.operation(operation.id)); cnc.moveSelectedOperation(by: 1) }
                                Divider()
                                Button("Remove operation", role: .destructive) { cnc.select(.operation(operation.id)); cnc.removeSelectedOperation() }
                                    .disabled(cnc.operations.count == 1)
                            }.disabled(cnc.generating)
                    }
                }
            }
            VStack(alignment: .leading, spacing: 8) {
                Text("1 tool · \(counted(cnc.operations.count, "operation"))").font(.caption).foregroundStyle(.secondary)
                if cnc.jobCurrent { Text("~\(CNCWorkspace.durationLabel(cnc.jobDuration)) estimated motion").font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
                Button(cnc.generating ? "Generating…" : "Generate all") { cnc.generate(all: true) }
                    .frame(maxWidth: .infinity).disabled(cnc.generating || cnc.machine.active)
            }.padding(16)
        }.frame(width: Theme.Width.cncOutline)
    }
    private func item(_ title: String, detail: String, symbol: String, selection: CNCSelection, current: Bool? = nil) -> some View {
        let selected = !cnc.usesImportedProgram && cnc.selection == selection && cnc.mode != .machine
        return Button { cnc.select(selection) } label: {
            HStack(spacing: 9) {
                Image(systemName: symbol).frame(width: 17).foregroundStyle(selected ? cncAccent : .secondary)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.callout.weight(.medium))
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
                Spacer(minLength: 0)
                if let current { Image(systemName: current ? "checkmark" : "circle.dotted").font(.caption).foregroundStyle(current ? Color.green : .secondary) }
            }.padding(.horizontal, 11).padding(.vertical, 7).frame(maxWidth: .infinity, alignment: .leading)
                .selectableRow(selected: selected)
        }.buttonStyle(.plain).padding(.horizontal, 7).accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private struct CNCHeading: View {
    var eyebrow: String
    var title: String
    var detail: String
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Eyebrow(eyebrow)
            Text(title).font(.title2.weight(.semibold))
            Text(detail).font(.subheadline).foregroundStyle(.secondary)
        }
    }
}

private struct CNCNumber: View {
    var label: String
    @Binding var value: Double
    var unit = "mm"
    var body: some View {
        HStack {
            Text(label).font(.callout)
            Spacer(minLength: 5)
            HStack(spacing: 5) {
                TextField(label, value: $value, format: .number.precision(.fractionLength(0...3)))
                    .multilineTextAlignment(.trailing).textFieldStyle(.roundedBorder).frame(width: 76)
                    .font(.callout.monospaced()).accessibilityLabel(label)
                Text(unit).font(.caption).foregroundStyle(.secondary).frame(minWidth: 18, alignment: .trailing)
            }
        }
    }
}

struct CNCStudioViewportChrome: View {
    @Bindable var model: EditorModel
    private var cnc: CNCWorkspace { model.cnc }
    var body: some View {
        @Bindable var cnc = cnc
        VStack {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: 5) {
                    Text(cnc.mode == .setup ? "Stock & work origin" : cnc.mode == .machine ? "Machine position" : cnc.previewTitle)
                        .font(.callout.weight(.medium))
                    Text(cnc.mode == .machine ? (cnc.machine.demo ? "Simulated telemetry" : "Reported position · G54") : "T1 · Ø \(cnc.setup.diameter.formatted()) mm flat end mill")
                        .font(.caption).foregroundStyle(.secondary)
                    if cnc.mode == .toolpaths && !cnc.current { Text("Setup changed · showing last generated path").font(.caption).foregroundStyle(.orange) }
                }.allowsHitTesting(false)
                Spacer()
                Eyebrow(cnc.mode == .toolpaths ? "Path preview" : cnc.mode == .setup ? "Setup" : cnc.machine.demo ? "Simulated" : "Reported")
                    .padding(.horizontal, 7).padding(.vertical, 4)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous))
                    .allowsHitTesting(false)
            }
            Spacer()
            HStack(spacing: 10) {
                Label("Cut", systemImage: "minus").foregroundStyle(.orange)
                Label("Rapid", systemImage: "ellipsis").foregroundStyle(.cyan)
                if cnc.mode == .toolpaths { Text("Orange tool: preview").foregroundStyle(.secondary) }
                Spacer(minLength: 0)
                Button("Iso") { model.animateCamera(to: .isometric) }
                Button("Top") { model.animateCamera(to: .top) }
                Button("Front") { model.animateCamera(to: .front) }
                Menu {
                    Toggle("Auto-fit toolpath changes", isOn: $cnc.autoFit)
                    Toggle("Follow reported spindle", isOn: $cnc.followSpindle)
                } label: { Image(systemName: "ellipsis.circle") }.menuStyle(.borderlessButton).fixedSize()
                    .help("Viewport options").accessibilityLabel("Viewport options")
                Button { ReleaseWindowController.shared.frame(entityNamed: "cncRoot") } label: { Image(systemName: "viewfinder") }.help("Frame CNC setup").accessibilityLabel("Frame CNC setup")
                Button { cnc.showPart.toggle() } label: { Image(systemName: cnc.showPart ? "cube.fill" : "cube") }.help("Show or hide CAD part").accessibilityLabel("Toggle CAD part")
            }.font(.caption).buttonStyle(.plain)
        }.padding(16)
            .onChange(of: cnc.machine.tick) { _, _ in ReleaseWindowController.shared.followCNCTool() }
            .onChange(of: cnc.followSpindle) { _, _ in ReleaseWindowController.shared.followCNCTool() }
            .onChange(of: cnc.autoFit) { _, enabled in if enabled { ReleaseWindowController.shared.frame(entityNamed: "cncRoot") } }
    }
}

struct CNCStudioTransport: View {
    @Bindable var cnc: CNCWorkspace
    var body: some View {
        VStack(spacing: 0) {
            if cnc.mode == .toolpaths && !cnc.machine.active {
                HStack(spacing: 12) {
                    Text("Simulation").font(.caption).foregroundStyle(.secondary)
                    Button { cnc.togglePreview() } label: { Image(systemName: cnc.previewPlaying ? "pause.fill" : "play.fill").frame(width: 20, height: 20) }
                        .buttonStyle(.bordered).clipShape(Circle()).disabled(cnc.program == nil)
                        .accessibilityLabel(cnc.previewPlaying ? "Pause preview" : "Play preview")
                    VStack(alignment: .leading, spacing: 3) {
                        Text(CNCWorkspace.durationLabel(cnc.preview.duration * cnc.previewFraction)).monospacedDigit()
                        Text("/ ~\(CNCWorkspace.durationLabel(cnc.preview.duration))").foregroundStyle(.secondary).monospacedDigit()
                    }.font(.caption).frame(width: 57, alignment: .leading)
                    VStack(alignment: .leading, spacing: 3) {
                        Slider(value: $cnc.previewFraction, in: 0...1).tint(cncAccent).disabled(cnc.program == nil).accessibilityLabel("Toolpath preview position")
                        Text("\(cnc.previewTitle) · rapid time estimated").font(.caption).foregroundStyle(.secondary)
                    }
                    Picker("Preview speed", selection: $cnc.previewSpeed) { Text("1×").tag(1.0); Text("5×").tag(5.0); Text("20×").tag(20.0) }
                        .labelsHidden().frame(width: 63)
                }.padding(.horizontal, 18).padding(.vertical, 8).controlSize(.small)
                Divider()
            }
            CNCMachineBar(cnc: cnc)
        }
    }
}

/// The right rail: the inspector for whatever the job outline selected —
/// stock, work origin, tool, or an operation — plus the imported program when
/// one is in use. Selecting an outline row changes this panel.
struct CNCStudioInspector: View {
    @Bindable var model: EditorModel
    private var cnc: CNCWorkspace { model.cnc }
    var body: some View {
        @Bindable var cnc = cnc
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            PanelHeader(title: title, systemImage: symbol, onClose: { cnc.rightPanelShown = false })
            ScrollView {
                VStack(alignment: .leading, spacing: Theme.Space.m) {
                    if cnc.usesImportedProgram { imported } else { selected }
                    if let error = cnc.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
                }
            }
        }
        .padding(Theme.Space.l).frame(width: Theme.Width.inspector)
    }

    private var title: String {
        if cnc.usesImportedProgram { return cnc.importedName }
        switch cnc.selection {
        case .stock: return "Stock"
        case .origin: return "Work origin"
        case .tool: return "Tool"
        case .operation: return cnc.selectedOperation.name
        }
    }
    private var symbol: String {
        if cnc.usesImportedProgram { return "doc.text" }
        switch cnc.selection {
        case .stock: return "shippingbox"
        case .origin: return "move.3d"
        case .tool: return "wrench.adjustable"
        case .operation: return cnc.selectedOperation.symbol
        }
    }

    @ViewBuilder private var imported: some View {
        Eyebrow("Imported G-code")
        KeyValueRow("Coordinates", "G54")
        KeyValueRow("Tool", "one, installed by hand")
        Text("Preview assumes XYZ0 before the program establishes a position. Initial travel and fixtures are not verified.")
            .font(.caption).foregroundStyle(.secondary)
        Button("Edit generated operations") { cnc.useGeneratedJob(); cnc.select(.operation(cnc.selectedOperation.id)) }
            .disabled(cnc.machine.active)
    }

    @ViewBuilder private var selected: some View {
        @Bindable var cnc = cnc
        Group {
            switch cnc.selection {
            case .operation:
                if cnc.setup.isContour {
                    Eyebrow("Contour")
                    KeyValueRow("Points", "\(cnc.setup.contour.count)")
                    KeyValueRow("Side", cnc.setup.operation == "contour_outside" ? "Outside" : "Inside")
                    if cnc.setup.operation == "contour_outside" {
                        Stepper(value: $cnc.setup.tabs, in: 0...12) { KeyValueRow("Holding tabs", "\(cnc.setup.tabs)") }
                        if cnc.setup.tabs > 0 {
                            CNCNumber(label: "Tab width", value: $cnc.setup.tabWidth)
                            CNCNumber(label: "Tab height", value: $cnc.setup.tabHeight)
                        }
                    }
                    Divider()
                }
                Eyebrow("Cut")
                CNCNumber(label: "Cut depth", value: $cnc.setup.depth)
                CNCNumber(label: "Stepdown", value: $cnc.setup.stepdown)
                CNCNumber(label: "Stepover", value: $cnc.setup.stepover)
                CNCNumber(label: "Clearance", value: $cnc.setup.clearance)
                Divider()
                Eyebrow("Feeds and speeds")
                CNCNumber(label: "Cutting feed", value: $cnc.setup.feed, unit: "mm/min")
                CNCNumber(label: "Plunge", value: $cnc.setup.plunge, unit: "mm/min")
                CNCNumber(label: "Spindle", value: $cnc.setup.rpm, unit: "RPM")
                Divider()
                Toggle("Show clearance plane", isOn: $cnc.showClearance)
                Text("Vertical plunge · retract before XY travel\(cnc.setup.operation == "profile" ? " · no holding tabs" : "")")
                    .font(.caption).foregroundStyle(.secondary)
                Divider()
                Button(cnc.generating ? "Generating…" : "Update toolpath") { cnc.generate() }
                    .disabled(cnc.machine.active || cnc.generating)
                Text(cnc.current ? "Toolpath up to date" : "Needs generation").font(.caption).foregroundStyle(.secondary)
            case .stock:
                Eyebrow("Size")
                CNCNumber(label: "Width · X", value: $cnc.stockWidth)
                CNCNumber(label: "Length · Y", value: $cnc.stockHeight)
                CNCNumber(label: "Thickness · Z", value: $cnc.stockThickness)
                Divider()
                Toggle("Show stock", isOn: $cnc.showStock)
                Toggle("Show part", isOn: $cnc.showPart)
                Button("Use model bounds") { placeFromModel(changeStock: true) }
                Button("Set work origin") { cnc.select(.origin) }
                Text("Stock top is Z0. Facing and profiles extend beyond the rectangle by the cutter radius.")
                    .font(.caption).foregroundStyle(.secondary)
            case .origin:
                Eyebrow("G54 in the model")
                CNCNumber(label: "CAD X", value: $cnc.origin.x)
                CNCNumber(label: "CAD Y", value: $cnc.origin.y)
                CNCNumber(label: "CAD Z", value: $cnc.origin.z)
                Divider()
                Button("Place at model top") { placeFromModel(changeStock: false) }
                Toggle("Show clearance plane", isOn: $cnc.showClearance)
                Text("Places G54 in the model. Set machine zero in the Machine dock.")
                    .font(.caption).foregroundStyle(.secondary)
            case .tool:
                Eyebrow("T1 · flat end mill")
                CNCNumber(label: "Diameter", value: $cnc.toolDiameter)
                Text("One installed centre-cutting tool for the entire job. Set feed and spindle speed per operation.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .controlSize(.small)
        .disabled(cnc.machine.active || cnc.generating)
    }

    private func placeFromModel(changeStock: Bool) {
        guard let parent = ReleaseWindowController.shared.centeringEntity else { return }
        let parts = parent.children.filter { $0.name.hasPrefix("part") || $0.name.hasPrefix("inst") }
        guard let first = parts.first else { return }
        var bounds = first.visualBounds(relativeTo: parent)
        for part in parts.dropFirst() { bounds = bounds.union(part.visualBounds(relativeTo: parent)) }
        let lo = bounds.min, hi = bounds.max
        guard hi.x > lo.x, hi.y > lo.y, hi.z > lo.z else { return }
        cnc.origin = .init(x: Double(lo.x), y: Double(lo.y), z: Double(hi.z))
        if changeStock {
            cnc.stockWidth = Double(hi.x - lo.x); cnc.stockHeight = Double(hi.y - lo.y)
            cnc.stockThickness = Double(hi.z - lo.z)
        }
    }
}
