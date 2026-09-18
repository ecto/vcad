import SwiftUI
import AppKit
import RealityKit

// Native CNC shell. Its viewport remains the existing ReleasedScene; these
// rails organize the same workspace and controller used in desktop mode.
private let cncAccent = Color.accentColor

struct WorkspaceHeader: View {
    @Bindable var model: EditorModel
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
                            Button { cnc.connectionShown.toggle() } label: {
                                HStack(spacing: 6) {
                                    Image(systemName: "circle.fill")
                                        .font(.caption2)
                                        .foregroundStyle(cnc.machine.connected && cnc.machine.status.isFresh ? Color.green : .secondary)
                                    Text(cnc.machine.demo ? "Simulator" : "Anolex")
                                    Text(cnc.machine.connected ? cnc.machine.status.state : "Disconnected").foregroundStyle(.secondary)
                                }.font(.caption).lineLimit(1)
                            }.buttonStyle(.borderless).help("Machine connection (⌥⌘K)")
                                .accessibilityLabel("Machine connection, \(cnc.machine.summary)")
                                .accessibilityIdentifier("cnc.machine.connection")
                                // The same view the machine bar shows: the
                                // header used to carry an older popover that
                                // knew nothing about travel, limits, or what
                                // had changed since the machine was last good.
                                .popover(isPresented: $cnc.connectionShown, arrowEdge: .bottom) {
                                    ScrollView { CNCMachineConnection(cnc: cnc).padding(18) }
                                        .frame(width: 330).frame(maxHeight: 520)
                                }
                        }
                        Menu {
                            switch model.workspace {
                            case .manufacture:
                                Toggle("Job Outline", isOn: $cnc.leftPanelShown)
                                Toggle("Inspector", isOn: $cnc.rightPanelShown)
                                Toggle("Drawer", isOn: $cnc.bottomPanelShown)
                                Toggle("ncSender & Camera", isOn: $cnc.senderPanelShown)
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
            // The Manufacture workspace takes its outline from the document on
            // screen, so it needs the same bytes the editor evaluates. Handing
            // it a closure rather than the model keeps the workspace testable
            // without an editor (items 16 and 37).
            .onAppear {
                model.cnc.modelDocument = { [weak model] in model?.camDocument() }
                // The tool list belongs to the job, not to the bench: the same
                // machine cuts one part with a Ø1 and the next with a Ø6. It
                // is keyed on the document, and read back when one opens.
                model.cnc.documentKey = { [weak model] in model?.documentURL?.path }
                model.cnc.loadTools()
            }
            .onChange(of: model.documentURL) { _, _ in model.cnc.loadTools() }
            // The outline comes from the part the user has selected, or the
            // first one when nothing is selected.
            .onChange(of: model.selectedPartIndex) { _, index in
                model.cnc.modelPartIndex = index ?? 0
                model.cnc.recheckOutlineAgainstModel()
            }
            // Item 16's caveat: a part edited after its outline was imported
            // never re-checked. A finished solve is the moment the part on
            // screen is a different part, so the comparison runs again — and
            // `build()` runs it too, so a job is never verified against an
            // outline the app has not just re-checked.
            .onChange(of: model.solving) { _, solving in
                if !solving { model.cnc.recheckOutlineAgainstModel() }
            }
    }
    private var documentLocation: String {
        if case let .document(path, _) = model.source { return path }
        return model.source.label
    }
}

/// The ncSender column: the blessed send path and the camera watching the cut,
/// side by side with the job.
///
/// Both models live on the workspace, so opening and closing this column never
/// costs a poller or a frame.
struct CNCStudioSenderColumn: View {
    @Bindable var model: EditorModel
    private var cnc: CNCWorkspace { model.cnc }

    var body: some View {
        VStack(spacing: Theme.Space.m) {
            CNCNcSenderPanel(cnc: cnc, sender: cnc.ncSender, camera: cnc.camera,
                             documentName: model.documentName,
                             onClose: { cnc.senderPanelShown = false })
                .frame(width: 330)
                .panelSurface()
            CNCCameraTile(camera: cnc.camera)
                .frame(width: 330)
                .panelSurface()
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
                    // From the part on screen first, and first for a reason:
                    // the outline that machines the part should come from the
                    // part, not from a file that may be a different revision
                    // (items 16 and 37).
                    if cnc.hasModel {
                        Button("From model…") { CNCCommand.outlineFromModel.run(cnc) }
                            .disabled(!CNCCommand.outlineFromModel.isEnabled(cnc))
                    }
                    Button("Outline (DXF)…") { CNCCommand.importOutlineDXF.run(cnc) }
                        .disabled(!CNCCommand.importOutlineDXF.isEnabled(cnc))
                    Button("G-code…") { cnc.importFile() }
                } label: { Label("Import", systemImage: "square.and.arrow.down") }
                    .menuStyle(.borderlessButton).fixedSize()
                    .disabled(cnc.machine.active || cnc.generating)
                    .help("Take the outline from the part on screen, import a DXF, or import a finished G-code program")
                // …and the same choices as plain buttons, because a pull-down
                // is not reachable from the keyboard or an assistive tool
                // (friction-log items 45 and 51). Both ask `CNCCommand`, which
                // is also what the Manufacture menu asks.
                if cnc.hasModel {
                    Button("From model") { CNCCommand.outlineFromModel.run(cnc) }
                        .disabled(!CNCCommand.outlineFromModel.isEnabled(cnc))
                        .accessibilityIdentifier("cnc.import.fromModel")
                        .help("Section the part on screen and machine that outline (⌥⌘O)")
                }
                Button("DXF…") { CNCCommand.importOutlineDXF.run(cnc) }
                    .disabled(!CNCCommand.importOutlineDXF.isEnabled(cnc))
                    .accessibilityIdentifier("cnc.import.dxf")
                    .help("Import a DXF outline (⌥⌘I)")
                Spacer(minLength: 0)
            }.font(.caption).buttonStyle(.borderless).padding(.horizontal, 12).padding(.bottom, 8)
            if let section = cnc.modelSection, cnc.modelRefusal == nil {
                VStack(alignment: .leading, spacing: 3) {
                    Label(section.prismaticVerdict, systemImage: section.prismatic?.prismatic == true
                          ? "square.stack.3d.up" : "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(section.prismatic?.prismatic == true ? Color.secondary : Color.orange)
                        .lineLimit(3)
                    Text("Z \(CNCVerdictText.mm(section.zRange.first ?? 0, 2))…\(CNCVerdictText.mm(section.zRange.last ?? 0, 2)) mm · suggested stock \(CNCVerdictText.mm(section.suggestedStockThickness, 2)) mm · \(section.meshSourceNote)")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(3)
                }.padding(.horizontal, 12).padding(.bottom, 8)
            }
            if let refusal = cnc.modelRefusal {
                // A torn solid is not healed away here: the tear is the thing
                // worth seeing, and the outline would be a guess.
                Label(refusal, systemImage: "xmark.octagon.fill")
                    .font(.caption).foregroundStyle(.red).lineLimit(4)
                    .padding(.horizontal, 12).padding(.bottom, 8)
            }
            if let mismatch = cnc.outlineMismatch {
                Label(mismatch, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption).foregroundStyle(.orange).lineLimit(5)
                    .padding(.horizontal, 12).padding(.bottom, 8)
            }
            if let outline = cnc.outline {
                VStack(alignment: .leading, spacing: 4) {
                    Label("\(outline.name) · \(counted(outline.holes.count, "hole"))", systemImage: "scribble.variable")
                        .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    // Item 49: which holes the installed cutter cannot make is
                    // answered again every time the tool changes, without a
                    // re-import, because it is derived and never stored.
                    ForEach(cnc.unmachinableHoles, id: \.index) { hole in
                        Label("Ø \(hole.diameter.formatted(.number.precision(.fractionLength(0...2)))) hole · no end mill fits and no drill is that size",
                              systemImage: "exclamationmark.triangle")
                            .font(.caption).foregroundStyle(.orange).lineLimit(2)
                    }
                }.padding(.horizontal, 12).padding(.bottom, 8)
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
            item("Stock", detail: "\(cnc.stockWidth.formatted()) × \(cnc.stockHeight.formatted()) × \(cnc.stockThickness.formatted()) mm · \(cnc.underStock.label)", symbol: "shippingbox", selection: .stock)
            item("Work origin", detail: "G54 · stock top", symbol: "move.3d", selection: .origin)
            item("Tools", detail: cnc.toolSequenceLabel, symbol: "wrench.adjustable", selection: .tool)
            Divider().padding(.horizontal, 16).padding(.vertical, 8)
            HStack(spacing: Theme.Space.xs) {
                Eyebrow("Operations")
                Spacer()
                // Reordering is on buttons, not only a drag or a context menu,
                // so it can be reached from the keyboard.
                Button { cnc.moveSelectedOperation(by: -1) } label: { Image(systemName: "arrow.up") }
                    .disabled(!cnc.canMoveSelectedOperation(by: -1))
                    .keyboardShortcut(.upArrow, modifiers: [.command, .option])
                    .help("Move the selected operation up (⌥⌘↑)").accessibilityLabel("Move operation up")
                Button { cnc.moveSelectedOperation(by: 1) } label: { Image(systemName: "arrow.down") }
                    .disabled(!cnc.canMoveSelectedOperation(by: 1))
                    .keyboardShortcut(.downArrow, modifiers: [.command, .option])
                    .help("Move the selected operation down (⌥⌘↓)").accessibilityLabel("Move operation down")
                // A menu for the choice, and a plain button for the common
                // one: a pull-down is not a path a keyboard or an assistive
                // tool can rely on (friction-log item 45).
                Button { cnc.addOperation(.pocket) } label: { Image(systemName: "plus") }
                    .help("Add a pocket operation").accessibilityLabel("Add operation")
                    .accessibilityIdentifier("cnc.operations.add")
                    .disabled(cnc.machine.active || cnc.generating)
                Menu {
                    ForEach([CNCOpKind.face, .pocket, .contourOutside], id: \.self) { kind in
                        Button(CNCWorkspace.manualLabel(kind)) { cnc.addOperation(kind) }
                    }
                } label: { Image(systemName: "chevron.down") }
                    .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
                    .help("Add another kind of operation").accessibilityLabel("Choose what to add")
                    .disabled(cnc.machine.active || cnc.generating)
            }.buttonStyle(.borderless).controlSize(.small)
                .padding(.horizontal, 18).padding(.bottom, 9)
            ScrollView {
                VStack(spacing: 3) {
                    ForEach(cnc.operations) { operation in
                        item(operation.name,
                             detail: operationDetail(operation),
                             symbol: operation.symbol, selection: .operation(operation.id),
                             status: status(of: operation))
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
                // The count only. What a tool change *asks of the operator* is
                // said in the readiness list and in the Tool panel, which are
                // the two places it is acted on; a third copy here put the
                // same two sentences on screen three times at once.
                Text("\(counted(cnc.tools.count, "tool")) · \(counted(cnc.operations.count, "operation"))")
                    .font(.caption).foregroundStyle(.secondary)
                if cnc.jobCurrent {
                    Text("~\(CNCWorkspace.durationLabel(cnc.jobDuration)) with acceleration")
                        .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                    Label(verdictLabel, systemImage: verdictSymbol)
                        .font(.caption).foregroundStyle(verdictColour).lineLimit(2)
                }
                Button(CNCCommand.buildJob.title(cnc)) { CNCCommand.buildJob.run(cnc) }
                    .frame(maxWidth: .infinity)
                    .disabled(!CNCCommand.buildJob.isEnabled(cnc))
                    .accessibilityIdentifier("cnc.job.build")
                    .help("Build the job and replay it against the part (⌥⌘B)")
            }.padding(16)
        }.frame(width: Theme.Width.cncOutline)
    }

    private func operationDetail(_ operation: CNCOperation) -> String {
        let tool = "T\(operation.setup.toolNumber)"
        guard cnc.jobCurrent, operation.seconds > 0 else { return "\(tool) · not built" }
        return "\(tool) · \(CNCWorkspace.durationLabel(operation.seconds))"
    }
    private var verdictLabel: String {
        if !cnc.blockers.isEmpty { return "Refused · \(counted(cnc.blockers.count, "reason"))" }
        if !cnc.verified { return "Not verified" }
        let pending = cnc.unacknowledgedWarnings.count
        return pending > 0 ? "\(counted(pending, "warning")) to acknowledge" : "Verified against the part"
    }
    private var verdictSymbol: String {
        if !cnc.blockers.isEmpty { return "xmark.octagon.fill" }
        if !cnc.verified { return "questionmark.circle" }
        return cnc.unacknowledgedWarnings.isEmpty ? "checkmark.seal.fill" : "exclamationmark.triangle.fill"
    }
    private var verdictColour: Color {
        if !cnc.blockers.isEmpty { return .red }
        if !cnc.verified { return .secondary }
        return cnc.unacknowledgedWarnings.isEmpty ? .green : .orange
    }
    /// What the tick on an operation row means. A refused job used to show a
    /// green check on every operation, which reads as "all good" beside a
    /// blocker — so the mark now says which cut the refusal is about.
    enum CNCRowStatus { case none, built, warned, refused }

    private func status(of operation: CNCOperation) -> CNCRowStatus {
        guard cnc.jobCurrent else { return .none }
        if cnc.blockers.contains(where: { $0.operationID == operation.id }) { return .refused }
        if !cnc.blockers.isEmpty && cnc.operations.count == 1 { return .refused }
        if cnc.warnings.contains(where: { $0.operationID == operation.id }) { return .warned }
        return operation.ranges.isEmpty ? .none : .built
    }

    private func item(_ title: String, detail: String, symbol: String, selection: CNCSelection,
                      status: CNCRowStatus = .none) -> some View {
        let selected = !cnc.usesImportedProgram && cnc.selection == selection && cnc.mode != .machine
        return Button { cnc.select(selection) } label: {
            HStack(spacing: 9) {
                Image(systemName: symbol).frame(width: 17).foregroundStyle(selected ? cncAccent : .secondary)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.callout.weight(.medium))
                    Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(2)
                }
                Spacer(minLength: 0)
                switch status {
                case .none: EmptyView()
                case .built:
                    Image(systemName: "checkmark").font(.caption).foregroundStyle(Color.green)
                        .accessibilityLabel("built")
                case .warned:
                    Image(systemName: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(Color.orange)
                        .accessibilityLabel("warning")
                case .refused:
                    Image(systemName: "xmark.octagon.fill").font(.caption).foregroundStyle(Color.red)
                        .accessibilityLabel("refused")
                }
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
                    Text(cnc.mode == .machine ? (cnc.machine.demo ? "Simulated telemetry" : "Reported position · G54") : cnc.toolSequenceLabel)
                        .font(.caption).foregroundStyle(.secondary)
                    if cnc.mode == .toolpaths && !cnc.jobCurrent { Text("Setup changed · showing the last job that was built").font(.caption).foregroundStyle(.orange) }
                    if cnc.mode == .toolpaths, cnc.jobCurrent, let first = cnc.blockers.first {
                        Text(first.text).font(.caption).foregroundStyle(.red).lineLimit(3).frame(maxWidth: 420, alignment: .leading)
                    }
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
                    // The Machine stage leads with the readiness checklist.
                    // It was rendered in a popover and nowhere else — item
                    // 45's narrow remainder: its actions and its headline
                    // sentence were reachable from the menu, the bar's caption
                    // and the status dump, but the *list* a screen reader
                    // would walk was not. Same view, same `runBlocker`, same
                    // `CNCCommand` predicates: one source of truth, shown in
                    // two places. Outside the group below on purpose, because
                    // that group is disabled while the machine streams and
                    // this list is exactly what has to stay readable then.
                    if cnc.mode == .machine {
                        CNCReadinessList(cnc: cnc, onTrace: { cnc.traceShown = true })
                            .accessibilityIdentifier("cnc.readiness.section")
                        Divider()
                    }
                    if cnc.usesImportedProgram { imported } else { selected }
                    if let error = cnc.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
                }
            }
        }
        .padding(Theme.Space.l).frame(width: Theme.Width.inspector)
    }

    private var title: String {
        if cnc.mode == .machine { return "Machine" }
        if cnc.usesImportedProgram { return cnc.importedName }
        switch cnc.selection {
        case .stock: return "Stock"
        case .origin: return "Work origin"
        case .tool: return "Tools"
        case .operation: return cnc.selectedOperation.name
        }
    }
    private var symbol: String {
        if cnc.mode == .machine { return "gauge.with.dots.needle.bottom.50percent" }
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
        Divider()
        CNCVerificationSection(cnc: cnc)
        Divider()
        Button("Edit generated operations") { cnc.useGeneratedJob(); cnc.select(.operation(cnc.selectedOperation.id)) }
            .disabled(cnc.machine.active)
    }

    @ViewBuilder private var selected: some View {
        @Bindable var cnc = cnc
        Group {
            switch cnc.selection {
            case .operation:
                // The verdict comes first. It was below thirty settings, which
                // is the same as not being there: on a refused job the only
                // thing on screen was a form that looked fine.
                CNCVerificationSection(cnc: cnc)
                Divider()
                HStack {
                    Button(cnc.generating ? "Building…" : cnc.jobCurrent ? "Rebuild and check" : "Build and check the job") {
                        CNCCommand.buildJob.run(cnc)
                    }
                    .disabled(!CNCCommand.buildJob.isEnabled(cnc))
                    .accessibilityIdentifier("cnc.job.buildAndCheck")
                    // Re-replaying without rebuilding is what an imported
                    // program needs when the outline it is judged against has
                    // moved under it.
                    Button("Verify") { CNCCommand.verifyJob.run(cnc) }
                        .disabled(!CNCCommand.verifyJob.isEnabled(cnc))
                        .accessibilityIdentifier("cnc.job.verify")
                        .help("Replay this job against the part it should make (⌥⌘Y)")
                }
                Divider()
                CNCOperationInspector(cnc: cnc)
                Divider()
                Toggle("Show clearance plane", isOn: $cnc.showClearance)
            case .stock:
                CNCSetupSummary(cnc: cnc)
                Divider()
                CNCMaterialSection(cnc: cnc)
                Divider()
                Eyebrow("Size")
                CNCNumber(label: "Width · X", value: $cnc.stockWidth, identifier: "cnc.stock.width")
                CNCNumber(label: "Length · Y", value: $cnc.stockHeight, identifier: "cnc.stock.height")
                CNCNumber(label: "Thickness · Z", value: $cnc.stockThickness, identifier: "cnc.stock.thickness")
                CNCNumber(label: "Margin round the part",
                          value: Binding(get: { cnc.effectiveMargin }, set: { cnc.stockMargin = $0 }),
                          help: "How far the blank stands proud of the part. The cutter runs a radius outside the profile, so it needs material to stand on.",
                          identifier: "cnc.stock.margin")
                if cnc.stockMargin != nil {
                    Button("Follow the cutter (\(cnc.automaticMargin.formatted()) mm)") { cnc.stockMargin = nil }
                }
                Divider()
                Eyebrow("What is under the stock")
                // Item 50: on a bare bed the only safe through-cut was one the
                // user shortened by hand. Now the app knows, and refuses.
                Picker("Under the stock", selection: Binding(
                    get: { cnc.underStock.thickness == nil ? 0 : 1 },
                    set: { cnc.underStock = $0 == 0 ? .machineBed : .spoilboard(cnc.underStock.thickness ?? 3) })) {
                        Text("Machine bed").tag(0)
                        Text("Spoilboard").tag(1)
                    }.pickerStyle(.segmented).labelsHidden()
                    .accessibilityLabel("What is under the stock")
                if let thickness = cnc.underStock.thickness {
                    CNCNumber(label: "Spoilboard thickness",
                              value: Binding(get: { thickness }, set: { cnc.underStock = .spoilboard($0) }),
                              identifier: "cnc.stock.spoilboard")
                } else {
                    Text("On a bare bed a cut may not break through, so an operation set to break through is refused.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Divider()
                Toggle("Show stock", isOn: $cnc.showStock)
                Toggle("Show cutter sweep", isOn: $cnc.showEnvelope)
                Toggle("Show part", isOn: $cnc.showPart)
                Button("Use model bounds") { placeFromModel(changeStock: true) }
                Button("Set work origin") { cnc.select(.origin) }
                Text("Stock top is Z0 and work zero is the part's lower-left corner. The blank is drawn with its margin.")
                    .font(.caption).foregroundStyle(.secondary)
            case .origin:
                CNCSetupSummary(cnc: cnc)
                Divider()
                CNCZeroSection(cnc: cnc)
                Divider()
                CNCPlacementSection(cnc: cnc)
                Divider()
                CNCClampSection(cnc: cnc)
                Divider()
                Eyebrow("G54 in the model")
                CNCNumber(label: "CAD X", value: $cnc.origin.x, identifier: "cnc.origin.x")
                CNCNumber(label: "CAD Y", value: $cnc.origin.y, identifier: "cnc.origin.y")
                CNCNumber(label: "CAD Z", value: $cnc.origin.z, identifier: "cnc.origin.z")
                if let outline = cnc.outline {
                    KeyValueRow("From the outline", "X \(Double(outline.origin.x).formatted()) · Y \(Double(outline.origin.y).formatted())")
                }
                Divider()
                Button("Place at model top") { placeFromModel(changeStock: false) }
                Toggle("Show clearance plane", isOn: $cnc.showClearance)
                Text("Places G54 in the model, so the toolpath is drawn on the part rather than beside it. Set machine zero in the machine bar.")
                    .font(.caption).foregroundStyle(.secondary)
            case .tool:
                CNCSetupSummary(cnc: cnc)
                Divider()
                CNCToolListSection(cnc: cnc)
                Divider()
                CNCFeedsSection(cnc: cnc)
                if !cnc.unmachinableHoles.isEmpty {
                    Text("\(counted(cnc.unmachinableHoles.count, "hole")) in this outline is too small for every end mill in the list, and no drill in it is that size. Fit a smaller cutter, or add the drill, and they come back on their own.")
                        .font(.caption).foregroundStyle(.orange)
                }
                Text("Feed and spindle speed are set per operation. This machine has no changer, so every tool change is an operator stop and a re-zero.")
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
