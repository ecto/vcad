import SwiftUI

// The machine bar: one persistent row along the bottom of Manufacture that
// carries everything the machine needs at a glance — state, work position,
// coordinate system, jog / zero / probe, overrides, job readiness, Run / Stop.
// Depth lives in popovers off the bar (connection, jog + homing, overrides +
// accessories, readiness checklist) and in the optional drawer above it
// (terminal, G-code, macros). Nothing on the bar is said twice.

struct CNCMachineBar: View {
    @Bindable var cnc: CNCWorkspace
    @State private var connectionShown = false
    @State private var jogShown = false
    @State private var overridesShown = false
    @State private var probeShown = false
    @State private var traceShown = false
    @State private var reviewShown = false
    @State private var runShown = false
    @State private var stopShown = false
    @State private var zeroAxes: String?
    private var machine: CNCController { cnc.machine }
    /// What the machine says about this job: travel, homing, alarms, limits.
    private var machineFindings: [CNCMachineFinding] { cncMachineFindings(cnc) }
    private var machineBlocker: String? { machineFindings.first(where: \.blocking)?.text }
    /// Everything Run waits on: the job's own gate first, then the machine's.
    /// The job side is the workspace's to answer; travel, homing and alarms
    /// are the machine's, and a job that leaves travel never reaches Run.
    private var blocker: String? { cnc.runBlocker ?? machineBlocker }
    private var held: Bool { machine.status.state.hasPrefix("Hold") }
    private var moving: Bool { machine.active || held || ["Run", "Jog", "Home"].contains(machine.status.state) }
    private var canResume: Bool { machine.connected && machine.status.isFresh && machine.status.state == "Hold:0" && !machine.faulted }

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { _ in
            HStack(spacing: Theme.Space.m) {
                drawerToggles
                Divider().frame(height: 28)
                machineChip
                Divider().frame(height: 28)
                position
                workspacePicker
                Divider().frame(height: 28)
                motion
                Divider().frame(height: 28)
                jobStatus
                Spacer(minLength: Theme.Space.s)
                transport
            }
            .controlSize(.small)
            .padding(.horizontal, Theme.Space.l).padding(.vertical, 10)
        }
        .onChange(of: machine.connected) { _, _ in cnc.setupConfirmed = false }
        .onChange(of: machine.workspace) { _, _ in cnc.setupConfirmed = false }
        .confirmationDialog("Set \(machine.workspace) \(zeroAxes ?? "") zero here?", isPresented: Binding(get: { zeroAxes != nil }, set: { if !$0 { zeroAxes = nil } })) {
            if let axes = zeroAxes {
                Button("Zero \(axes)") { cnc.setupConfirmed = false; machine.zero(axes: axes); zeroAxes = nil }
            }
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        }
        .sheet(isPresented: $probeShown) { CNCProbeSheet(cnc: cnc) }
        .sheet(isPresented: $traceShown) { CNCTraceSheet(cnc: cnc) }
        .confirmationDialog(machine.demo ? "Run the job in the simulator?" : "Start machining this job?", isPresented: $runShown) {
            Button(machine.demo ? "Run simulated job" : "Start machining") { cnc.startJob() }
            // Return answers Cancel: starting the spindle is a deliberate click,
            // never the key that also commits a number field.
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        } message: {
            let dial = cncSpindleInstruction(cnc).map { " " + $0 } ?? ""
            Text((cnc.usesImportedProgram ? "\(cnc.importedName) · G54. Verify the installed tool, program and initial travel." : "\(counted(cnc.operations.count, "operation")) · Ø \(cnc.toolDiameter.formatted()) mm tool · G54. The program starts the spindle and cuts to the configured depths.") + dial)
        }
        .confirmationDialog("Stop and reset the controller?", isPresented: $stopShown) {
            Button("Stop and reset", role: .destructive) { machine.reset(); cnc.setupConfirmed = false }
        } message: {
            Text("Feed hold has been requested. Reset aborts this job; reconnect before sending another. This is a software stop, not the machine’s emergency stop.")
        }
    }

    // MARK: drawer

    /// Terminal · G-code · Macros: one open at a time, click again to close.
    private var drawerToggles: some View {
        HStack(spacing: 2) {
            ForEach(CNCInspectorTab.allCases) { tab in
                let open = cnc.bottomPanelShown && cnc.inspectorTab == tab
                Button {
                    if open { cnc.bottomPanelShown = false } else { cnc.inspectorTab = tab; cnc.bottomPanelShown = true }
                } label: {
                    Image(systemName: tab.symbol).frame(width: 26, height: 22).contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(open ? AnyShapeStyle(Color.accentColor) : AnyShapeStyle(.secondary))
                .selectableRow(selected: open)
                .help(open ? "Hide \(tab.rawValue)" : "Show \(tab.rawValue)")
                .accessibilityLabel(tab.rawValue).accessibilityAddTraits(open ? .isSelected : [])
            }
        }
    }

    // MARK: machine

    /// State dot · machine name. Click for the connection.
    private var machineChip: some View {
        Button { connectionShown.toggle() } label: {
            HStack(spacing: 6) {
                Image(systemName: machine.faulted ? "exclamationmark.circle.fill" : "circle.fill")
                    .font(.caption2)
                    .foregroundStyle(machine.faulted ? Color.orange : machine.connected && machine.status.isFresh ? Color.green : Color.secondary)
                VStack(alignment: .leading, spacing: 1) {
                    Text(machine.connected ? (machine.status.isFresh ? machine.status.state : "Status stale") : "Disconnected")
                        .font(.callout.weight(.medium)).lineLimit(1)
                    Text(machine.demo ? "Simulator" : "Anolex 4030").font(.caption).foregroundStyle(.secondary)
                }
                Image(systemName: "chevron.down").font(.caption2).foregroundStyle(.tertiary)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .help("Machine connection")
        .accessibilityLabel("Machine connection, \(machine.summary)")
        .popover(isPresented: $connectionShown, arrowEdge: .top) {
            ScrollView { CNCMachineConnection(cnc: cnc).padding(18) }
                .frame(width: 330).frame(maxHeight: 520)
        }
    }

    /// Work position, one compact readout per axis; machine coordinates on hover.
    private var position: some View {
        HStack(spacing: Theme.Space.m) {
            axis("X", work: machine.status.work?.x, position: machine.status.machine?.x)
            axis("Y", work: machine.status.work?.y, position: machine.status.machine?.y)
            axis("Z", work: machine.status.work?.z, position: machine.status.machine?.z)
        }
    }
    private func axis(_ name: String, work: Double?, position: Double?) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 4) {
            Text(name).font(.caption).foregroundStyle(.secondary)
            Text(value(work)).font(.title3.weight(.medium).monospaced())
                .lineLimit(1).frame(minWidth: 62, alignment: .trailing).textSelection(.enabled)
        }
        .help("\(name) machine \(value(position)) mm")
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(name), work \(value(work)) millimetres, machine \(value(position)) millimetres")
    }
    private func value(_ number: Double?) -> String {
        guard machine.connected, machine.status.isFresh, let number, number.isFinite else { return "—" }
        return String(format: "%.3f", number)
    }

    private var workspacePicker: some View {
        Picker("Work coordinate system", selection: Binding(get: { machine.workspace }, set: { machine.selectWorkspace($0); cnc.setupConfirmed = false })) {
            if !CNCCommands.workspaces.contains(machine.workspace) { Text("—").tag(machine.workspace) }
            ForEach(CNCCommands.workspaces, id: \.self) { Text($0).tag($0) }
        }.labelsHidden().fixedSize().disabled(!machine.canCommand)
            .help("Work coordinate system · millimetres")
    }

    // MARK: motion

    private var motion: some View {
        HStack(spacing: 6) {
            Button("Jog", systemImage: "move.3d") { jogShown.toggle() }
                .disabled(!machine.canCommand && !machine.canHome && machine.status.state != "Jog")
                .popover(isPresented: $jogShown, arrowEdge: .top) { CNCJogControls(cnc: cnc).padding(18).frame(width: 290) }
            Menu("Zero", systemImage: "scope") {
                ForEach(["X", "Y", "Z", "XY", "XYZ"], id: \.self) { axes in
                    Button("Zero \(axes)…") { zeroAxes = axes }
                }
                Divider()
                Button("Use G54 work coordinates") { cnc.setupConfirmed = false; machine.selectG54() }
                    .disabled(machine.g54Active)
            }.disabled(!machine.canCommand)
            Button("Probe", systemImage: "arrow.down.to.line") { probeShown = true }.disabled(!machine.canCommand)
            // Item 53: the blank was 10° off and 5 mm out, and the only way
            // that was ever found was tracing the square by hand.
            Button("Trace", systemImage: "rectangle.dashed") { traceShown = true }
                .disabled(!machine.canCommand || cncJobEnvelope(cnc) == nil)
                .help("Walk the job's bounding rectangle at a safe height")
            Button {
                overridesShown.toggle()
            } label: {
                Label(overrideSummary, systemImage: "dial.medium")
            }
            .disabled(!machine.connected)
            .help("Feed and spindle overrides, coolant")
            .popover(isPresented: $overridesShown, arrowEdge: .top) { overrides.padding(18).frame(width: 300) }
        }
        .buttonStyle(.bordered)
    }

    private var overrideSummary: String {
        guard machine.status.isFresh, let f = machine.status.feedOverride, let s = machine.status.spindleOverride else { return "Overrides" }
        return "\(f)% · \(s)%"
    }

    /// Feed + spindle overrides and the accessories, in one place.
    private var overrides: some View {
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            Text("Overrides").font(.headline)
            CNCOverrideControl(machine: machine, spindle: false)
            CNCOverrideControl(machine: machine, spindle: true)
            Divider()
            Eyebrow("Accessories")
            Toggle("Flood coolant", isOn: Binding(get: { machine.status.flood }, set: { machine.setCoolant(flood: $0, mist: machine.status.mist) }))
            Toggle("Mist coolant", isOn: Binding(get: { machine.status.mist }, set: { machine.setCoolant(flood: machine.status.flood, mist: $0) }))
            Button("Stop spindle (M5)") { machine.stopSpindle() }
        }
        .controlSize(.small)
        .disabled(!machine.canCommand && !machine.canOverride)
    }

    // MARK: job

    private var jobStatus: some View {
        VStack(alignment: .leading, spacing: 2) {
            if machine.active {
                HStack(spacing: 6) {
                    Text(held ? "Feed held" : machine.stream.phase == .draining ? "Finishing queued motion" : "Machining")
                        .font(.callout.weight(.medium))
                    Text("Elapsed \(CNCWorkspace.durationLabel(machine.elapsed))").font(.caption).monospacedDigit().foregroundStyle(.secondary)
                }
                Text("\(machine.stream.acknowledged) / \(machine.stream.lines.count) lines accepted")
                    .font(.caption).foregroundStyle(.secondary)
            } else if moving {
                Text(held ? "Feed held" : machine.status.state == "Jog" ? "Jogging" : machine.status.state == "Home" ? "Homing" : "Controller motion")
                    .font(.callout.weight(.medium))
                Text("Manual controller activity").font(.caption).foregroundStyle(.secondary)
            } else {
                Button { reviewShown.toggle() } label: {
                    HStack(spacing: 5) {
                        Image(systemName: blocker == nil ? "checkmark.circle.fill" : "info.circle")
                            .foregroundStyle(blocker == nil ? Color.green : Color.secondary)
                        Text(blocker == nil ? "Ready to machine" : cnc.generating ? "Generating toolpaths…" : "Setup needs attention")
                            .font(.callout.weight(.medium))
                        Image(systemName: "chevron.down").font(.caption2).foregroundStyle(.tertiary)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help("Review the job setup")
                .popover(isPresented: $reviewShown, arrowEdge: .top) { readiness.padding(18).frame(width: 330) }
                Text(cnc.error ?? machine.error ?? blocker ?? (cnc.usesImportedProgram ? cnc.importedName : "\(counted(cnc.operations.count, "operation")) · est. \(CNCWorkspace.durationLabel(cnc.jobDuration))"))
                    .font(.caption).foregroundStyle((cnc.error ?? machine.error) == nil ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.red))
                    .lineLimit(1).truncationMode(.tail)
            }
        }
        .frame(minWidth: 120, alignment: .leading)
    }

    private var transport: some View {
        HStack(spacing: Theme.Space.s) {
            Button {
                if held { machine.resume() }
                else if moving { machine.hold() }
                else { runShown = true }
            } label: {
                Label(held ? "Resume" : moving ? "Hold" : "Run Job", systemImage: held ? "play.fill" : moving ? "pause.fill" : "play.fill")
                    .frame(width: 78)
            }.buttonStyle(.borderedProminent).tint(moving ? .orange : .accentColor)
                .disabled(held ? !canResume : moving ? !machine.connected : blocker != nil)
                // No Return-based key equivalent: AppKit advertises any button
                // whose key is Return as the window's default button, modifiers
                // or not, so accessibility clients pressed Run Job for "return".
                .keyboardShortcut("j", modifiers: [.command, .option])
            Button {
                machine.hold() // Request hold immediately; never leave motion running behind the reset dialog.
                stopShown = true
            } label: { Label("Stop", systemImage: "stop.fill").frame(width: 56) }
                .buttonStyle(.bordered).disabled(!machine.connected || (!moving && !machine.faulted))
                .help("Request feed hold, then confirm controller reset")
        }.controlSize(.regular).fixedSize()
    }

    /// The readiness checklist lives in its own view so the machine's half of
    /// it can be rendered — and looked at — without a window.
    private var readiness: some View {
        CNCReadinessList(cnc: cnc, onTrace: { reviewShown = false; traceShown = true })
    }
}

/// Jog pad, homing, and go-to moves — the machine's motion, in one popover.
struct CNCJogControls: View {
    @Bindable var cnc: CNCWorkspace
    @State private var homeShown = false
    @State private var destination: String?
    private var machine: CNCController { cnc.machine }
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Jog machine").font(.headline)
            Picker("Step", selection: $cnc.jogStep) {
                Text("0.1 mm").tag(0.1); Text("1 mm").tag(1.0); Text("10 mm").tag(10.0)
            }.pickerStyle(.segmented)
            HStack {
                Text("Feed")
                TextField("Jog feed", value: $cnc.jogFeed, format: .number).textFieldStyle(.roundedBorder)
                Text("mm/min").foregroundStyle(.secondary)
            }.font(.caption)
            Grid(horizontalSpacing: 8, verticalSpacing: 8) {
                GridRow { jog("↖", x: -1, y: 1); jog("Y+", y: 1); jog("↗", x: 1, y: 1); jog("Z+", z: 1) }
                GridRow {
                    jog("X−", x: -1)
                    Button { machine.cancelJog() } label: { Image(systemName: "stop.circle").frame(width: 32, height: 26) }
                        .accessibilityLabel("Cancel jog").disabled(!machine.connected)
                    jog("X+", x: 1); jog("Z−", z: -1)
                }
                GridRow { jog("↙", x: -1, y: -1); jog("Y−", y: -1); jog("↘", x: 1, y: -1) }
            }.buttonStyle(.bordered)
            Divider()
            HStack {
                Button("Home…") { homeShown = true }.disabled(!machine.canHome)
                Menu("Go to") {
                    Button("Work XY zero…") { destination = "XY zero" }
                    Button("Work Z zero…") { destination = "Z zero" }
                    Divider()
                    Button("Park…") { destination = "Park" }.disabled(machine.parkPosition == nil)
                }.disabled(!machine.canCommand)
                Spacer()
                Button("Cancel jog") { machine.cancelJog() }.disabled(!machine.connected)
            }
        }
        .confirmationDialog("Home the machine?", isPresented: $homeShown) {
            Button("Run homing cycle") { cnc.setupConfirmed = false; machine.home() }
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        } message: { Text("The axes will move toward the configured homing switches.") }
        .confirmationDialog("Move to \(destination ?? "")?", isPresented: Binding(get: { destination != nil }, set: { if !$0 { destination = nil } })) {
            Button("Move") {
                if destination == "Park" { machine.park() }
                else { machine.returnToZero(xy: destination == "XY zero", clearance: cnc.setup.clearance) }
                destination = nil
            }
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        } message: {
            Text(destination == "Park" ? "Retract to saved machine Z before XY travel, then return to saved Z. Verify the path is clear." : destination == "XY zero" ? "Retract to at least the CAM clearance height before moving to work X0 Y0." : "Move to work Z0 at 100 mm/min.")
        }
    }
    private func jog(_ title: String, x: Double = 0, y: Double = 0, z: Double = 0) -> some View {
        Button { machine.jog(x: x * cnc.jogStep, y: y * cnc.jogStep, z: z * cnc.jogStep, feed: cnc.jogFeed) } label: {
            Text(title).frame(width: 32, height: 26)
        }.disabled(!machine.canCommand).accessibilityLabel("Jog X \(x * cnc.jogStep) Y \(y * cnc.jogStep) Z \(z * cnc.jogStep) millimetres")
    }
}
