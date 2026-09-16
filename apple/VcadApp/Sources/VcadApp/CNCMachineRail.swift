import SwiftUI

/// Persistent physical-machine controls; simulation never replaces this rail.
struct CNCMachineRail: View {
    @Bindable var cnc: CNCWorkspace
    @State private var jogShown = false
    @State private var probeShown = false
    @State private var reviewShown = false
    @State private var runShown = false
    @State private var stopShown = false
    @State private var zeroAxes: String?
    private var machine: CNCController { cnc.machine }
    private var held: Bool { machine.status.state.hasPrefix("Hold") }
    private var moving: Bool { machine.active || held || ["Run", "Jog", "Home"].contains(machine.status.state) }
    private var canResume: Bool { machine.connected && machine.status.isFresh && machine.status.state == "Hold:0" && !machine.faulted }

    var body: some View {
        VStack(spacing: 0) {
            TimelineView(.periodic(from: .now, by: 0.5)) { _ in
                HStack(spacing: 20) {
                    identity.frame(width: 122, alignment: .leading)
                    Divider()
                    coordinates.frame(maxWidth: .infinity)
                    Divider()
                    adjustments.frame(width: 260)
                }.padding(.horizontal, 18).padding(.vertical, 14).fixedSize(horizontal: false, vertical: true)
            }
            Divider()
            HStack(spacing: 16) {
                Button {
                    if cnc.bottomPanelShown && cnc.inspectorTab == .terminal { cnc.bottomPanelShown = false }
                    else { cnc.inspectorTab = .terminal; cnc.bottomPanelShown = true }
                } label: {
                    Label("Terminal", systemImage: cnc.bottomPanelShown && cnc.inspectorTab == .terminal ? "chevron.down" : "chevron.right")
                }.buttonStyle(.borderless).help("Show or hide the machine terminal")
                Menu {
                    Button("G-code") { cnc.inspectorTab = .gcode; cnc.bottomPanelShown = true }
                    Button("Macros") { cnc.inspectorTab = .macros; cnc.bottomPanelShown = true }
                    Divider()
                    Button("Export job…") { cnc.export(job: true) }.disabled(cnc.jobCode == nil)
                } label: { Image(systemName: "ellipsis.circle") }
                    .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize().accessibilityLabel("Job tools")
                Divider().frame(height: 24)
                jobStatus.frame(maxWidth: .infinity, alignment: .leading)
                transport
            }.padding(.horizontal, 18).padding(.vertical, 12)
        }
        .onChange(of: machine.connected) { _, _ in cnc.setupConfirmed = false }
        .onChange(of: machine.workspace) { _, _ in cnc.setupConfirmed = false }
        .confirmationDialog("Set \(machine.workspace) \(zeroAxes ?? "") zero here?", isPresented: Binding(get: { zeroAxes != nil }, set: { if !$0 { zeroAxes = nil } })) {
            if let axes = zeroAxes {
                Button("Zero \(axes)") { cnc.setupConfirmed = false; machine.zero(axes: axes); zeroAxes = nil }
            }
        }
        .sheet(isPresented: $probeShown) { CNCProbeSheet(cnc: cnc) }
        .confirmationDialog(machine.demo ? "Run the job in the simulator?" : "Start machining this job?", isPresented: $runShown) {
            Button(machine.demo ? "Run simulated job" : "Start machining") { cnc.startJob() }
        } message: {
            Text(cnc.usesImportedProgram ? "\(cnc.importedName) · G54. Verify the installed tool, program and initial travel." : "\(cnc.operations.count) operations · Ø \(cnc.toolDiameter.formatted()) mm tool · G54. The program starts the spindle and cuts to the configured depths.")
        }
        .confirmationDialog("Stop and reset the controller?", isPresented: $stopShown) {
            Button("Stop and reset", role: .destructive) { machine.reset(); cnc.setupConfirmed = false }
        } message: {
            Text("Feed hold has been requested. Reset aborts this job; reconnect before sending another. This is a software stop, not the machine’s emergency stop.")
        }
    }

    private var identity: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(machine.connected ? (machine.status.isFresh ? machine.status.state : "Status stale") : "Disconnected", systemImage: machine.faulted ? "exclamationmark.circle.fill" : "circle.fill")
                .font(.headline).foregroundStyle(machine.faulted ? Color.orange : .primary)
                .lineLimit(1).minimumScaleFactor(0.8)
            Text(machine.demo ? "Simulator" : "Anolex 4030").font(.caption).foregroundStyle(.secondary)
            HStack {
                Picker("Work coordinate system", selection: Binding(get: { machine.workspace }, set: { machine.selectWorkspace($0); cnc.setupConfirmed = false })) {
                    if !CNCCommands.workspaces.contains(machine.workspace) { Text("—").tag(machine.workspace) }
                    ForEach(CNCCommands.workspaces, id: \.self) { Text($0).tag($0) }
                }.labelsHidden().frame(width: 80).disabled(!machine.canCommand)
                Text("mm").font(.caption).foregroundStyle(.secondary)
            }.controlSize(.small)
        }
    }

    private var coordinates: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("WORK POSITION").font(.system(size: 10, weight: .medium)).foregroundStyle(.secondary)
            HStack(spacing: 16) {
                axis("X", work: machine.status.work?.x, position: machine.status.machine?.x)
                axis("Y", work: machine.status.work?.y, position: machine.status.machine?.y)
                axis("Z", work: machine.status.work?.z, position: machine.status.machine?.z)
            }
        }
    }
    private func axis(_ name: String, work: Double?, position: Double?) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(name).font(.caption).foregroundStyle(.secondary)
                Text(value(work)).font(.system(size: 28, weight: .medium, design: .monospaced))
                    .lineLimit(1).minimumScaleFactor(0.65).textSelection(.enabled)
            }
            Text("Machine \(value(position))").font(.system(size: 10, design: .monospaced))
                .foregroundStyle(.secondary).lineLimit(1).minimumScaleFactor(0.75)
        }.frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(name), work \(value(work)) millimetres, machine \(value(position)) millimetres")
    }
    private func value(_ number: Double?) -> String {
        guard machine.connected, machine.status.isFresh, let number, number.isFinite else { return "—" }
        return String(format: "%.3f", number)
    }

    private var adjustments: some View {
        VStack(spacing: 9) {
            HStack {
                Button("Jog", systemImage: "move.3d") { jogShown.toggle() }
                    .disabled(!machine.canCommand && machine.status.state != "Jog")
                    .popover(isPresented: $jogShown, arrowEdge: .bottom) { CNCJogControls(cnc: cnc).padding(18).frame(width: 290) }
                Menu("Zero", systemImage: "scope") {
                    ForEach(["X", "Y", "Z", "XY", "XYZ"], id: \.self) { axes in
                        Button("Zero \(axes)…") { zeroAxes = axes }
                    }
                }.disabled(!machine.canCommand)
                Button("Probe", systemImage: "arrow.down.to.line") { probeShown = true }.disabled(!machine.canCommand)
            }.buttonStyle(.bordered).controlSize(.small)
            CNCRailFeedOverride(machine: machine)
        }
    }

    private var jobStatus: some View {
        VStack(alignment: .leading, spacing: 3) {
            if machine.active {
                HStack {
                    Text(held ? "Feed held" : machine.stream.phase == .draining ? "Finishing queued motion" : "Machining")
                        .fontWeight(.medium)
                    TimelineView(.periodic(from: .now, by: 1)) { _ in
                        Text("Elapsed \(CNCWorkspace.durationLabel(machine.elapsed))").monospacedDigit().foregroundStyle(.secondary)
                    }
                }
                Text("\(machine.stream.acknowledged) / \(machine.stream.lines.count) lines accepted")
                    .font(.caption).foregroundStyle(.secondary)
            } else if moving {
                Text(held ? "Feed held" : machine.status.state == "Jog" ? "Jogging" : machine.status.state == "Home" ? "Homing" : "Controller motion").fontWeight(.medium)
                Text("Manual controller activity").font(.caption).foregroundStyle(.secondary)
            } else {
                HStack {
                    Image(systemName: cnc.runBlocker == nil ? "checkmark.circle.fill" : "info.circle")
                        .foregroundStyle(cnc.runBlocker == nil ? Color.green : .secondary)
                    Text(cnc.runBlocker == nil ? "Ready to machine" : cnc.generating ? "Generating toolpaths…" : "Setup needs attention").fontWeight(.medium)
                    Button("Review setup") { reviewShown.toggle() }.buttonStyle(.borderless)
                        .popover(isPresented: $reviewShown, arrowEdge: .bottom) { readiness.padding(18).frame(width: 330) }
                }
                Text(cnc.runBlocker ?? (cnc.usesImportedProgram ? cnc.importedName : "\(cnc.operations.count) operations · Est. \(CNCWorkspace.durationLabel(cnc.jobDuration))"))
                    .font(.caption).foregroundStyle(.secondary).lineLimit(2)
            }
            if let error = cnc.error ?? machine.error { Text(error).font(.caption).foregroundStyle(.red).lineLimit(2).help(error) }
        }.font(.callout)
    }

    private var transport: some View {
        HStack(spacing: 10) {
            Button {
                if held { machine.resume() }
                else if moving { machine.hold() }
                else { runShown = true }
            } label: {
                Label(held ? "Resume" : moving ? "Hold" : "Run Job", systemImage: held ? "play.fill" : moving ? "pause.fill" : "play.fill")
                    .frame(width: 86)
            }.buttonStyle(.borderedProminent).tint(moving ? .orange : .accentColor)
                .disabled(held ? !canResume : moving ? !machine.connected : cnc.runBlocker != nil)
            Button {
                machine.hold() // Request hold immediately; never leave motion running behind the reset dialog.
                stopShown = true
            } label: { Label("Stop", systemImage: "stop.fill").frame(width: 70) }
                .buttonStyle(.bordered).disabled(!machine.connected || (!moving && !machine.faulted))
                .help("Request feed hold, then confirm controller reset")
        }.controlSize(.large).fixedSize()
    }

    private var readiness: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Job readiness").font(.headline)
            Label(machine.connected ? "Controller connected" : "Controller disconnected", systemImage: machine.connected ? "checkmark.circle" : "circle")
            Label(cnc.jobCurrent ? "Toolpaths current" : "Toolpaths need generation", systemImage: cnc.jobCurrent ? "checkmark.circle" : "circle")
            Label(machine.workspace == "G54" ? "G54 selected" : "G54 required", systemImage: machine.workspace == "G54" ? "checkmark.circle" : "circle")
            Divider()
            Toggle("I checked the tool, workholding, clearance and G54 zero", isOn: $cnc.setupConfirmed).disabled(machine.active)
            if let blocker = cnc.runBlocker { Text(blocker).font(.caption).foregroundStyle(.secondary) }
            if !cnc.jobCurrent && !cnc.usesImportedProgram {
                Button(cnc.generating ? "Generating…" : "Generate toolpaths") { cnc.generate(all: true) }.disabled(cnc.generating || machine.active)
            }
        }.font(.callout)
    }
}

struct CNCJogControls: View {
    @Bindable var cnc: CNCWorkspace
    @State private var homeShown = false
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
            HStack {
                Button("Home…") { homeShown = true }.disabled(!machine.canHome)
                Spacer()
                Button("Cancel jog") { machine.cancelJog() }.disabled(!machine.connected)
            }
        }.confirmationDialog("Home the machine?", isPresented: $homeShown) {
            Button("Run homing cycle") { cnc.setupConfirmed = false; machine.home() }
        } message: { Text("The axes will move toward the configured homing switches.") }
    }
    private func jog(_ title: String, x: Double = 0, y: Double = 0, z: Double = 0) -> some View {
        Button { machine.jog(x: x * cnc.jogStep, y: y * cnc.jogStep, z: z * cnc.jogStep, feed: cnc.jogFeed) } label: {
            Text(title).frame(width: 32, height: 26)
        }.disabled(!machine.canCommand).accessibilityLabel("Jog X \(x * cnc.jogStep) Y \(y * cnc.jogStep) Z \(z * cnc.jogStep) millimetres")
    }
}

private struct CNCRailFeedOverride: View {
    var machine: CNCController
    @State private var draft = 100.0
    @State private var editing = false
    private var reported: Int? { machine.status.feedOverride }
    private var enabled: Bool { machine.canOverride && reported != nil && !machine.changingOverride }
    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text("Feed override")
                Spacer()
                Text(machine.status.isFresh ? reported.map { "\($0)%" } ?? "—" : "—").monospacedDigit()
            }.font(.caption)
            HStack(spacing: 8) {
                Slider(value: $draft, in: 10...200, step: 1) { changing in
                    editing = changing
                    if !changing { machine.setOverride(spindle: false, percent: Int(draft)) }
                }.disabled(!enabled).accessibilityLabel("Feed override")
                Button { machine.setOverride(spindle: false, percent: 100) } label: {
                    Image(systemName: "arrow.counterclockwise")
                }.disabled(!enabled).help("Reset feed override to 100%")
                    .accessibilityLabel("Reset feed override to 100 percent")
            }.controlSize(.small)
            Text(machine.status.isFresh ? "\(machine.status.feed.formatted()) mm/min · \(machine.status.rpm.formatted()) rpm" : "Waiting for machine telemetry")
                .font(.caption2).monospacedDigit().foregroundStyle(.secondary)
        }.onAppear { draft = Double(reported ?? 100) }
            .onChange(of: reported) { _, value in if !editing { draft = Double(value ?? 100) } }
    }
}
