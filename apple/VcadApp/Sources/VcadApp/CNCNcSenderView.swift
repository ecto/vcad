import SwiftUI

// The ncSender panel: the blessed send path, end to end, in one column.
//
// Reading order is the order the work happens in — where the sender is, what
// the machine says, whether the job fits the machine's travel, the send, the
// transport, and what the sender's own log recorded. Nothing here duplicates
// the machine bar: that bar drives the *native* sender, this panel drives
// ncSender, and only one of the two may hold the controller at a time.

struct CNCNcSenderPanel: View {
    @Bindable var cnc: CNCWorkspace
    @Bindable var sender: NcSenderSession
    @Bindable var camera: CNCCameraModel
    var documentName: String = "untitled"
    /// Injected so a snapshot renders a fixed name; the app passes `Date()`.
    var now: Date = Date()
    var onClose: (() -> Void)?

    @State private var startShown = false
    @State private var stopShown = false
    @State private var confirmedForSender = false

    private var filename: String {
        NcSenderJobPlan.filename(document: documentName, toolDiameter: cnc.toolDiameter, date: now)
    }
    private var envelope: NcSenderEnvelopeCheck? {
        guard let verification = cnc.verification else { return nil }
        return sender.envelopeCheck(workMin: verification.envelope.workMin,
                                    workMax: verification.envelope.workMax)
    }
    private var blockers: [String] {
        sender.sendBlockers(jobCurrent: cnc.jobCurrent,
                            jobBlocked: !cnc.blockers.isEmpty,
                            hasCode: cnc.jobCode != nil,
                            envelope: envelope)
    }
    private var loadedHere: Bool {
        if case .loaded(let name, _) = sender.send { return name == sender.job?.filename }
        return false
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.l) {
                PanelHeader(title: "Send to ncSender", systemImage: "paperplane", onClose: onClose)
                connection
                Divider()
                machine
                Divider()
                travel
                Divider()
                job
                Divider()
                logTail
            }
            .padding(Theme.Space.l)
        }
        .frame(minWidth: 320)
        .confirmationDialog("Start this job on ncSender?", isPresented: $startShown) {
            Button("Start machining") {
                let name = filename
                Task { await sender.startLoadedJob(filename: name) }
            }
            // Same rule as every other dialog that starts motion: Return
            // answers Cancel (friction-log item 44).
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        } message: {
            Text("\(filename) · \(counted(cnc.operations.count, "operation")) · Ø \(cnc.toolDiameter.formatted()) mm tool · G54. ncSender starts the spindle and cuts to the configured depths.")
        }
        .confirmationDialog("Stop the job on ncSender?", isPresented: $stopShown) {
            Button("Stop job", role: .destructive) { Task { await sender.stop() } }
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        } message: {
            Text("ncSender aborts the program. This is a software stop, not the machine's emergency stop; the spindle may keep turning until the controller acts.")
        }
    }

    // MARK: connection

    private var connection: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Eyebrow("Sender")
            HStack {
                TextField("http://pika:8090", text: $sender.baseURLText)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityLabel("ncSender address")
                Button("Test") {
                    Task { await sender.testConnection(nativeHost: cnc.machine.host, nativePort: cnc.machine.port) }
                }.disabled(sender.busy)
            }
            if let probe = sender.probe {
                Label(probe.detail, systemImage: probe.reachable ? "checkmark.circle" : "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(probe.reachable ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
                if let version = probe.version { KeyValueRow("ncSender", version) }
                if let probeType = probe.probeType { KeyValueRow("Probe type", probeType) }
                if let contention = probe.contention { warning(contention) }
            }
            if let error = sender.lastError {
                Text(error).font(.caption).foregroundStyle(Color.red).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    // MARK: machine

    private var machine: some View {
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            Eyebrow("Machine")
            if let state = sender.machine {
                KeyValueRow("Status", state.status + (state.spindleActive ? " · spindle on" : ""))
                positions(work: state.wpos, machine: state.mpos)
                if !sender.assertedPins.isEmpty {
                    KeyValueRow("Pins asserted", sender.assertedPins.joined(separator: ", "))
                }
                if let alarm = sender.alarmExplanation { warning(alarm) }
                if let notHomed = sender.notHomedWarning { warning(notHomed) }
            } else {
                EmptyPanelState(title: "No state from ncSender",
                                systemImage: "antenna.radiowaves.left.and.right.slash",
                                detail: "Test the connection to start polling.")
            }
        }
    }

    /// Work and machine coordinates, one row per axis. Three numbers on one
    /// line is how the first draft truncated Z out of the panel entirely.
    private func positions(work: NcSenderVector?, machine: NcSenderVector?) -> some View {
        Grid(alignment: .trailing, horizontalSpacing: Theme.Space.m, verticalSpacing: 2) {
            GridRow {
                Text("").gridColumnAlignment(.leading)
                Text("work").font(.caption2).foregroundStyle(.secondary)
                Text("machine").font(.caption2).foregroundStyle(.secondary)
            }
            ForEach(0..<3, id: \.self) { axis in
                GridRow {
                    Text(NcSenderTravel.names[axis]).font(.caption).foregroundStyle(.secondary)
                        .gridColumnAlignment(.leading)
                    Text(number(work, axis)).font(.callout.monospacedDigit())
                    Text(number(machine, axis)).font(.callout.monospacedDigit()).foregroundStyle(.secondary)
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("\(NcSenderTravel.names[axis]), work \(number(work, axis)), machine \(number(machine, axis))")
            }
        }
    }
    private func number(_ vector: NcSenderVector?, _ axis: Int) -> String {
        guard let vector else { return "—" }
        return String(format: "%.3f", vector[axis])
    }

    // MARK: travel

    private var travel: some View {
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            HStack {
                Eyebrow("Travel · ncSender's $$")
                Spacer()
                Button("Read $$") { Task { await sender.refreshFirmware() } }
                    .buttonStyle(.borderless).font(.caption).disabled(sender.busy)
            }
            if let firmware = sender.firmware {
                switch sender.travel ?? .unknown("") {
                case .known(let travel):
                    KeyValueRow("Travel ($130–132)", travel.summary)
                case .unknown(let why):
                    warning(why)
                }
                KeyValueRow("Soft limits ($20)", flag(firmware.softLimits))
                KeyValueRow("Hard limits ($21)", flag(firmware.hardLimits))
            } else {
                Text("Not read yet.").font(.caption).foregroundStyle(.secondary)
            }
            if let envelope {
                if let travel = envelope.travel {
                    ForEach(0..<3, id: \.self) { axis in
                        KeyValueRow("\(NcSenderTravel.names[axis]) in machine",
                                    "\(NcSenderJobPlan.mm(envelope.machineMin[axis], 1))…\(NcSenderJobPlan.mm(envelope.machineMax[axis], 1)) of \(NcSenderJobPlan.mm(travel.axes[axis].lower, 0))…\(NcSenderJobPlan.mm(travel.axes[axis].upper, 0))")
                    }
                }
                if let blocker = envelope.blocker { warning(blocker, level: .red) }
                else { Label("The job fits inside travel.", systemImage: "checkmark.circle").font(.caption).foregroundStyle(.secondary) }
            } else {
                Text("No verified job envelope to place yet.").font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private func flag(_ value: Bool?) -> String {
        guard let value else { return "not reported" }
        return value ? "on" : "off"
    }

    // MARK: the job

    private var job: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Eyebrow("Job")
            // The filename runs to about 30 characters, so it gets its own
            // line rather than being cut in half by a key/value row.
            VStack(alignment: .leading, spacing: 1) {
                Text(filename).font(.callout.monospaced()).textSelection(.enabled)
                    .lineLimit(1).truncationMode(.middle)
                Text(cnc.jobCode.map { "\(counted(NcSenderJobPlan.fileLineCount($0), "line")) · uploaded as this name every time" }
                    ?? "No G-code to send.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Uploads as \(filename)")
            HStack {
                Button(sending ? "Sending…" : "Send to ncSender") {
                    guard let code = cnc.jobCode else { return }
                    let name = filename
                    Task { await sender.sendJob(gcode: code, filename: name) }
                }
                .buttonStyle(.borderedProminent)
                .disabled(!blockers.isEmpty || sender.busy)
                Spacer()
            }
            if let first = blockers.first {
                Text(blockers.count == 1 ? first : "\(first) (\(blockers.count - 1) more)")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            sendState
            if loadedHere || sender.job != nil { progress }
            Divider()
            Toggle("I checked the tool, workholding, clearance and G54 zero", isOn: $confirmedForSender)
                .font(.callout)
            transport
            Text("ncSender holds the controller; the built-in sender must be disconnected. Feed hold and stop are ncSender commands, not the machine's emergency stop.")
                .font(.caption2).foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private var sending: Bool { if case .uploading = sender.send { return true }; return false }

    @ViewBuilder private var sendState: some View {
        switch sender.send {
        case .idle, .uploading:
            EmptyView()
        case .loaded(let name, let lines):
            Label("ncSender is holding \(name) · \(counted(lines, "line"))", systemImage: "checkmark.seal")
                .font(.caption).foregroundStyle(.secondary)
        case .failed(let detail):
            warning(detail, level: .red)
        }
    }

    private var progress: some View {
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            let job = sender.job
            ProgressView(value: min(1, max(0, (job?.progressPercent ?? 0) / 100)))
                .accessibilityLabel("Job progress")
            KeyValueRow("Progress", "\(job?.currentLine ?? 0) / \(job?.totalLines ?? 0) · \(NcSenderJobPlan.mm(job?.progressPercent ?? 0, 1))%")
            if let times = sender.progressTimes {
                KeyValueRow("Elapsed", NcSenderJobPlan.duration(times.elapsed))
                KeyValueRow("Remaining", times.remaining.map(NcSenderJobPlan.duration) ?? "—")
            }
            if let loaded = job?.filename, !loaded.isEmpty { KeyValueRow("File", loaded) }
        }
    }

    private var transport: some View {
        HStack(spacing: Theme.Space.s) {
            Button("Start") { startShown = true }
                .buttonStyle(.borderedProminent)
                .disabled(!confirmedForSender || !loadedHere || sender.busy)
            Button("Pause") { Task { await sender.pause() } }.disabled(sender.job?.isRunning != true)
            Button("Resume") { Task { await sender.resume() } }.disabled(sender.job?.isPaused != true)
            Button("Stop") { stopShown = true }.disabled(sender.job == nil)
            Spacer()
        }.controlSize(.small)
    }

    // MARK: log

    private var logTail: some View {
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            HStack {
                Eyebrow("ncSender log · commands, alarms, probes")
                Spacer()
                Button("Refresh") { Task { await sender.refreshLog() } }
                    .buttonStyle(.borderless).font(.caption).disabled(sender.busy)
            }
            if let error = sender.logError {
                Text(error).font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else if sender.logLines.isEmpty {
                Text("Nothing read yet. This is the view that diagnosed the 2026-09-17 probe failure.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            } else {
                ForEach(sender.logLines) { line in
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: symbol(line.kind)).font(.caption2)
                            .foregroundStyle(line.kind == .alarm ? AnyShapeStyle(Color.orange) : AnyShapeStyle(.tertiary))
                        Text(line.text).font(.caption.monospaced()).lineLimit(2)
                            .textSelection(.enabled)
                    }
                }
            }
        }
    }

    private func symbol(_ kind: NcSenderLogFilter.Kind) -> String {
        switch kind {
        case .command: return "arrow.right"
        case .alarm: return "exclamationmark.triangle"
        case .probe: return "arrow.down.to.line"
        }
    }

    // MARK: bits

    private enum Level { case amber, red }
    private func warning(_ text: String, level: Level = .amber) -> some View {
        Label(text, systemImage: level == .red ? "exclamationmark.octagon" : "exclamationmark.triangle")
            .font(.caption)
            .foregroundStyle(level == .red ? Color.red : Color.orange)
            .fixedSize(horizontal: false, vertical: true)
    }
}
