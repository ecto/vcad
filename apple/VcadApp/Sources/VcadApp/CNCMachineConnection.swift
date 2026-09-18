import SwiftUI

// The connection popover. It used to be a host field, two buttons and a
// status line; what was missing was the machine itself. `$$` comes back on
// every connect, so the travel, the limit switches and — the point of the
// whole thing — what has changed since this machine was last known good are
// all there to be read before anything moves.

struct CNCMachineConnection: View {
    @Bindable var cnc: CNCWorkspace
    @State private var confirmBaseline = false
    private var machine: CNCController { cnc.machine }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            Text(machine.demo ? "Simulator" : "Anolex 4030 Ultra 2").font(.headline)
            Text(machine.firmware).font(.caption).foregroundStyle(.secondary)
                .textSelection(.enabled).lineLimit(2)
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
            if let error = machine.error {
                Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled)
            }
            if let profile = machine.profile {
                Divider()
                travel(profile)
                Divider()
                baseline(profile)
            } else if machine.connected {
                Divider()
                Label("Reading the controller's settings…", systemImage: "clock")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .confirmationDialog("Save these settings as the baseline for \(machine.baselineMachine)?",
                            isPresented: $confirmBaseline) {
            Button("Save as baseline") { machine.saveBaseline() }
            Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
        } message: {
            Text("Every later connection is compared against this listing. Save it when the machine is known good, not when it is being fixed.")
        }
    }

    /// What the controller says about itself, in the units a machinist uses.
    private func travel(_ profile: CNCMachineProfile) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Eyebrow("Travel")
            ForEach(profile.travels, id: \.axis) { axis in
                KeyValueRow(axis.axis, "\(number(axis.min))…\(number(axis.max)) mm")
            }
            HStack(spacing: Theme.Space.s) {
                flag("Soft limits", on: profile.softLimits)
                flag("Hard limits", on: profile.hardLimits)
                flag("Homed", on: profile.homed)
            }.padding(.top, 2)
            if profile.homingEnabled && !profile.homed {
                Text("Soft limits mean nothing until the machine is homed.")
                    .font(.caption2).foregroundStyle(.orange)
            }
        }
    }
    private func flag(_ title: String, on: Bool) -> some View {
        Label(title, systemImage: on ? "checkmark.circle.fill" : "xmark.circle")
            .font(.caption2)
            .foregroundStyle(on ? AnyShapeStyle(.secondary) : AnyShapeStyle(Color.orange))
            .labelStyle(.titleAndIcon)
    }

    /// The diff. This is the row that would have said `$132 · 130 → 100`.
    @ViewBuilder private func baseline(_ profile: CNCMachineProfile) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Eyebrow("Settings")
            if let name = profile.baselineName {
                if profile.baselineDiff.isEmpty {
                    Label("Settings match the baseline", systemImage: "checkmark.circle")
                        .font(.caption).foregroundStyle(.secondary)
                } else {
                    Label("\(counted(profile.baselineDiff.count, "setting")) changed since baseline",
                          systemImage: "exclamationmark.triangle.fill")
                        .font(.caption.weight(.medium)).foregroundStyle(Color.orange)
                    ForEach(profile.baselineDiff) { delta in
                        Text(delta.summary).font(.caption.monospaced())
                            .textSelection(.enabled).lineLimit(2)
                            .accessibilityLabel("Setting \(delta.number), \(delta.label), was \(delta.baseline ?? 0), now \(delta.current ?? 0)")
                    }
                }
                Text(name).font(.caption2).foregroundStyle(.tertiary).lineLimit(1)
            } else {
                Text("No baseline saved for this machine, so nothing can say what has changed.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Button(profile.baselineName == nil ? "Save as baseline…" : "Replace baseline…") { confirmBaseline = true }
                .controlSize(.small).disabled(profile.settings.isEmpty)
        }
    }

    /// Millimetres with the same typographic minus the findings use.
    private func number(_ value: Double) -> String {
        let text = value == value.rounded() ? String(Int(abs(value))) : String(format: "%.1f", abs(value))
        return (value < 0 ? "−" : "") + text
    }
}

/// The job's swept envelope in work coordinates: the oracle's own box when the
/// job was verified, otherwise the moves grown by the cutter's radius.
///
/// A free function rather than an extension so that the machine side owns it
/// outright and nothing in the job side has to know it exists.
@MainActor func cncJobEnvelope(_ cnc: CNCWorkspace) -> CNCEnvelopeBox? {
    if let envelope = cnc.verification?.envelope,
       let box = CNCEnvelopeBox.verified(workMin: envelope.workMin, workMax: envelope.workMax) {
        return box
    }
    let points = cnc.jobMoves.map(\.to)
    return CNCEnvelopeBox.around(moves: points, toolRadius: cnc.toolDiameter / 2)
}

/// Everything the machine has to say about the job that is loaded now.
@MainActor func cncMachineFindings(_ cnc: CNCWorkspace) -> [CNCMachineFinding] {
    guard cnc.machine.connected else { return [] }
    return CNCMachineCheck.findings(job: cncJobEnvelope(cnc), profile: cnc.machine.profile)
}

/// The first reason the machine will not run this job, or nothing.
@MainActor func cncMachineBlocker(_ cnc: CNCWorkspace) -> String? {
    cncMachineFindings(cnc).first(where: \.blocking)?.text
}
