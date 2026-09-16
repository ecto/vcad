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
                        Picker("Workspace", selection: Binding(get: { model.electronicsShown ? "Electronics" : cnc.shown ? "Manufacture" : "Design" }, set: { value in
                            model.electronicsShown = value == "Electronics"; cnc.shown = value == "Manufacture"
                        })) {
                            Text("Design").tag("Design")
                            Text("Electronics").tag("Electronics")
                            Text("Manufacture").tag("Manufacture")
                        }.pickerStyle(.menu).labelsHidden().frame(width: 132)
                            .accessibilityLabel("Workspace")
                            .help("Choose the design or manufacturing workspace")
                    }
                    Spacer(minLength: 0)
                    HStack(spacing: 14) {
                        if cnc.shown {
                            Button { connectionShown.toggle() } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: "circle.fill")
                                        .font(.system(size: 6))
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
                            if cnc.shown {
                                Toggle("Job Sidebar", isOn: $cnc.leftPanelShown)
                                Toggle("Inspector Drawer", isOn: $cnc.bottomPanelShown)
                                Toggle("Machine Inspector", isOn: $cnc.rightPanelShown)
                            } else {
                                Toggle("Model Navigator", isOn: $model.showsTree)
                                Toggle("Inspector", isOn: $model.showsInspector)
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
                    if case let .document(path, _) = model.source {
                        Button("Save") { model.saveDocument() }.disabled(!model.documentDirty)
                        Button("Show in Finder") { NSWorkspace.shared.activateFileViewerSelecting([URL(fileURLWithPath: path)]) }
                    } else {
                        Text("Unsaved document")
                    }
                } label: {
                VStack(spacing: 1) {
                    Text(model.source.label).font(.system(size: 13, weight: .semibold))
                        .lineLimit(1).truncationMode(.middle)
                    if model.documentDirty { Text("Edited").font(.system(size: 10)).foregroundStyle(.secondary) }
                }
                }.menuStyle(.borderlessButton).menuIndicator(.visible)
                .fixedSize()
                .frame(maxWidth: max(100, geometry.size.width - 2 * (model.isWindowed ? 326 : 250)))
                .help(documentLocation)
                .accessibilityElement(children: .combine)
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
        }.frame(height: 52)
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
            HStack {
                Button { cnc.importFile() } label: { Label("Import G-code…", systemImage: "doc.badge.plus") }
                    .disabled(cnc.machine.active || cnc.generating)
                Spacer(minLength: 0)
            }.font(.caption).padding(.horizontal, 12).padding(.bottom, 8)
            if cnc.importedProgram != nil {
                Button { cnc.useImportedJob() } label: {
                    Label(cnc.importedName, systemImage: "doc.text").font(.caption).lineLimit(2)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(8)
                        .background(cnc.usesImportedProgram ? Color.accentColor.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 6))
                }.buttonStyle(.plain).padding(.horizontal, 7).disabled(cnc.machine.active)
                Button("Use generated job") { cnc.useGeneratedJob(); cnc.select(.operation(cnc.selectedOperation.id)) }
                    .font(.caption).padding(.horizontal, 12).disabled(cnc.machine.active || !cnc.usesImportedProgram)
                Divider().padding(.vertical, 8)
            }
            Text("JOB SETUP").font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary).tracking(1).padding(12)
            item("Stock", detail: "\(cnc.stockWidth.formatted()) × \(cnc.stockHeight.formatted()) × \(cnc.stockThickness.formatted()) mm", symbol: "shippingbox", selection: .stock)
            item("Work origin", detail: "G54 · stock top", symbol: "move.3d", selection: .origin)
            item("Tool", detail: "T1 · Ø \(cnc.toolDiameter.formatted()) mm", symbol: "wrench.adjustable", selection: .tool)
            Divider().padding(.horizontal, 16).padding(.vertical, 8)
            HStack {
                Text("OPERATIONS").font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary).tracking(1)
                Spacer()
                Menu {
                    Button("Facing") { cnc.addOperation("face") }
                    Button("Pocket") { cnc.addOperation("pocket") }
                    Button("Outside profile") { cnc.addOperation("profile") }
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
                Text("1 tool · \(cnc.operations.count) operation\(cnc.operations.count == 1 ? "" : "s")").font(.caption).foregroundStyle(.secondary)
                if cnc.jobCurrent { Text("~\(CNCWorkspace.durationLabel(cnc.jobDuration)) estimated motion").font(.caption.monospacedDigit()).foregroundStyle(.secondary) }
                Button(cnc.generating ? "Generating…" : "Generate all") { cnc.generate(all: true) }
                    .frame(maxWidth: .infinity).disabled(cnc.generating || cnc.machine.active)
            }.padding(16)
        }.frame(width: 192)
    }
    private func item(_ title: String, detail: String, symbol: String, selection: CNCSelection, current: Bool? = nil) -> some View {
        let selected = !cnc.usesImportedProgram && cnc.selection == selection && cnc.mode != .machine
        return Button { cnc.select(selection) } label: {
            HStack(spacing: 9) {
                Image(systemName: symbol).frame(width: 17).foregroundStyle(selected ? cncAccent : .secondary)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.system(size: 12, weight: .medium))
                    Text(detail).font(.system(size: 10)).foregroundStyle(.secondary).lineLimit(2)
                }
                Spacer(minLength: 0)
                if let current { Image(systemName: current ? "checkmark" : "circle.dotted").font(.system(size: 10)).foregroundStyle(current ? Color.green : .secondary) }
            }.padding(.horizontal, 11).padding(.vertical, 7).frame(maxWidth: .infinity, alignment: .leading)
                .background(selected ? cncAccent.opacity(0.12) : Color.clear, in: RoundedRectangle(cornerRadius: 7))
        }.buttonStyle(.plain).padding(.horizontal, 7).accessibilityAddTraits(selected ? .isSelected : [])
    }
}

private struct CNCHeading: View {
    var eyebrow: String
    var title: String
    var detail: String
    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(eyebrow.uppercased()).font(.system(size: 10, weight: .medium)).tracking(1).foregroundStyle(.secondary)
            Text(title).font(.system(size: 19, weight: .semibold))
            Text(detail).font(.system(size: 11)).foregroundStyle(.secondary)
        }
    }
}

private struct CNCNumber: View {
    var label: String
    @Binding var value: Double
    var unit = "mm"
    var body: some View {
        HStack {
            Text(label).font(.system(size: 12))
            Spacer(minLength: 5)
            HStack(spacing: 5) {
                TextField(label, value: $value, format: .number.precision(.fractionLength(0...3)))
                    .multilineTextAlignment(.trailing).textFieldStyle(.roundedBorder).frame(width: 76)
                    .font(.system(size: 12, design: .monospaced)).accessibilityLabel(label)
                Text(unit).font(.system(size: 10)).foregroundStyle(.secondary).frame(minWidth: 18, alignment: .trailing)
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
                        .font(.system(size: 12, weight: .medium))
                    Text(cnc.mode == .machine ? (cnc.machine.demo ? "Simulated telemetry" : "Reported position · G54") : "T1 · Ø \(cnc.setup.diameter.formatted()) mm flat end mill")
                        .font(.system(size: 10)).foregroundStyle(.secondary)
                    if cnc.mode == .toolpaths && !cnc.current { Text("Setup changed · showing last generated path").font(.caption).foregroundStyle(.orange) }
                }.allowsHitTesting(false)
                Spacer()
                Text(cnc.mode == .toolpaths ? "PATH PREVIEW" : cnc.mode == .setup ? "SETUP" : cnc.machine.demo ? "SIMULATED" : "REPORTED")
                    .font(.system(size: 9, weight: .medium)).tracking(1).padding(.horizontal, 7).padding(.vertical, 4)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 4)).allowsHitTesting(false)
            }
            Spacer()
            HStack(spacing: 10) {
                Label("Cut", systemImage: "minus").foregroundStyle(.orange)
                Label("Rapid", systemImage: "ellipsis").foregroundStyle(.cyan)
                if cnc.mode == .toolpaths { Text("Orange tool: preview").foregroundStyle(.secondary) }
                Spacer(minLength: 0)
                Button("Iso") { model.stopSpin(); model.azimuth = .pi / 5; model.elevation = .pi / 7 }
                Button("Top") { model.stopSpin(); model.azimuth = 0; model.elevation = 1.45 }
                Button("Side") { model.stopSpin(); model.azimuth = 0; model.elevation = 0 }
                Menu {
                    Toggle("Auto-fit toolpath changes", isOn: $cnc.autoFit)
                    Toggle("Follow reported spindle", isOn: $cnc.followSpindle)
                } label: { Image(systemName: "ellipsis.circle") }.menuStyle(.borderlessButton).fixedSize()
                    .help("Viewport options").accessibilityLabel("Viewport options")
                Button { ReleaseWindowController.shared.frame(entityNamed: "cncRoot") } label: { Image(systemName: "viewfinder") }.help("Frame CNC setup").accessibilityLabel("Frame CNC setup")
                Button { cnc.showPart.toggle() } label: { Image(systemName: cnc.showPart ? "cube.fill" : "cube") }.help("Show or hide CAD part").accessibilityLabel("Toggle CAD part")
            }.font(.system(size: 10)).buttonStyle(.plain)
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
                    }.font(.system(size: 10)).frame(width: 57, alignment: .leading)
                    VStack(alignment: .leading, spacing: 3) {
                        Slider(value: $cnc.previewFraction, in: 0...1).tint(cncAccent).disabled(cnc.program == nil).accessibilityLabel("Toolpath preview position")
                        Text("\(cnc.previewTitle) · rapid time estimated").font(.system(size: 10)).foregroundStyle(.secondary)
                    }
                    Picker("Preview speed", selection: $cnc.previewSpeed) { Text("1×").tag(1.0); Text("5×").tag(5.0); Text("20×").tag(20.0) }
                        .labelsHidden().frame(width: 63)
                }.padding(.horizontal, 18).padding(.vertical, 8).controlSize(.small)
                Divider()
            }
            CNCMachineRail(cnc: cnc)
        }
    }
}

// Dojo's three inset panels share one surface over the uninterrupted scene.
extension View {
    @ViewBuilder
    func cncFloatingPanel() -> some View {
        if #available(macOS 26.0, *) {
            self.glassEffect(.regular, in: RoundedRectangle(cornerRadius: 12))
        } else {
            self.background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
                .clipShape(RoundedRectangle(cornerRadius: 12))
                .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(.primary.opacity(0.10)))
                .shadow(color: .black.opacity(0.20), radius: 18, y: 6)
        }
    }
}

struct CNCStudioMachinePanel: View {
    @Bindable var cnc: CNCWorkspace
    var body: some View {
        ScrollView {
            CNCMachineInspector(cnc: cnc).padding(16)
        }.frame(width: 276)
    }
}

struct CNCStudioBottomInspector: View {
    @Bindable var model: EditorModel
    private var cnc: CNCWorkspace { model.cnc }
    var body: some View {
        @Bindable var cnc = cnc
        ScrollView(.horizontal) {
            HStack(alignment: .top, spacing: 24) {
                VStack(alignment: .leading, spacing: 10) {
                    Text("INSPECTOR").font(.system(size: 10, weight: .medium)).tracking(1).foregroundStyle(.secondary)
                    Text(title).font(.system(size: 17, weight: .semibold))
                    Text("G54 · millimetres").font(.caption).foregroundStyle(.secondary)
                    if case .operation = cnc.selection {
                        Button(cnc.generating ? "Generating…" : "Update toolpath") { cnc.generate() }
                            .disabled(cnc.machine.active || cnc.generating)
                        Text(cnc.current ? "Toolpath up to date" : "Needs generation").font(.caption).foregroundStyle(.secondary)
                    }
                    if let error = cnc.error { Text(error).font(.caption).foregroundStyle(.red) }
                }.frame(width: 185, alignment: .leading)
                Divider()
                Group {
                    switch cnc.selection {
                    case .operation:
                        VStack(spacing: 12) {
                            CNCNumber(label: "Cut depth", value: $cnc.setup.depth)
                            CNCNumber(label: "Stepdown", value: $cnc.setup.stepdown)
                            CNCNumber(label: "Stepover", value: $cnc.setup.stepover)
                        }.frame(width: 230)
                        VStack(spacing: 12) {
                            CNCNumber(label: "Cutting feed", value: $cnc.setup.feed, unit: "mm/min")
                            CNCNumber(label: "Plunge", value: $cnc.setup.plunge, unit: "mm/min")
                            CNCNumber(label: "Spindle", value: $cnc.setup.rpm, unit: "RPM")
                        }.frame(width: 250)
                        VStack(alignment: .leading, spacing: 12) {
                            CNCNumber(label: "Clearance", value: $cnc.setup.clearance)
                            Toggle("Show clearance plane", isOn: $cnc.showClearance)
                            Text("Vertical plunge · retract before XY travel").foregroundStyle(.secondary)
                            if cnc.setup.operation == "profile" { Text("No holding tabs").foregroundStyle(.secondary) }
                        }.font(.caption).frame(width: 235)
                    case .stock:
                        VStack(spacing: 12) {
                            CNCNumber(label: "Width · X", value: $cnc.stockWidth)
                            CNCNumber(label: "Length · Y", value: $cnc.stockHeight)
                            CNCNumber(label: "Thickness · Z", value: $cnc.stockThickness)
                        }.frame(width: 250)
                        VStack(alignment: .leading, spacing: 12) {
                            Toggle("Show stock", isOn: $cnc.showStock)
                            Toggle("Show part", isOn: $cnc.showPart)
                            Button("Use model bounds") { placeFromModel(changeStock: true) }
                            Button("Set work origin") { cnc.select(.origin) }
                        }.font(.caption).frame(width: 200)
                        Text("Stock top is Z0. Facing and profiles extend beyond the rectangle by the cutter radius.")
                            .font(.caption).foregroundStyle(.secondary).frame(width: 220)
                    case .origin:
                        VStack(spacing: 12) {
                            CNCNumber(label: "CAD X", value: $cnc.origin.x)
                            CNCNumber(label: "CAD Y", value: $cnc.origin.y)
                            CNCNumber(label: "CAD Z", value: $cnc.origin.z)
                        }.frame(width: 250)
                        VStack(alignment: .leading, spacing: 12) {
                            Button("Place at model top") { placeFromModel(changeStock: false) }
                            Toggle("Show clearance plane", isOn: $cnc.showClearance)
                            Text("Places G54 in the model. Set machine zero in the machine panel.").foregroundStyle(.secondary)
                        }.font(.caption).frame(width: 270)
                    case .tool:
                        VStack(alignment: .leading, spacing: 12) {
                            CNCNumber(label: "Diameter", value: $cnc.toolDiameter)
                            Text("T1 · flat end mill").font(.caption).foregroundStyle(.secondary)
                        }.frame(width: 250)
                        Text("One installed centre-cutting tool for the entire job. Set feed and spindle speed per operation.")
                            .font(.caption).foregroundStyle(.secondary).frame(width: 280)
                    }
                }.disabled(cnc.machine.active || cnc.generating)
                Spacer(minLength: 0)
            }.padding(18)
        }
    }
    private var title: String {
        switch cnc.selection {
        case .stock: "Stock definition"
        case .origin: "Work origin"
        case .tool: "Flat end mill"
        case .operation: cnc.selectedOperation.name
        }
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
