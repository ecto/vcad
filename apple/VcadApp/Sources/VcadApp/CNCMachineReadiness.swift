import SwiftUI

/// The readiness checklist: what Run Job is waiting on, and the one human
/// confirmation nothing can automate.
///
/// Its own view rather than a computed property on the bar, because the list
/// is where the machine's half of the gate is read — travel against this job's
/// envelope in machine coordinates, homing, alarms, the dial — and a list that
/// can only be seen inside a popover cannot be checked by anything but eyes.
struct CNCReadinessList: View {
    @Bindable var cnc: CNCWorkspace
    var onTrace: () -> Void = {}
    private var machine: CNCController { cnc.machine }
    private var findings: [CNCMachineFinding] { cncMachineFindings(cnc) }
    /// The workspace's gate now carries the machine's half too.
    private var blocker: String? { cnc.runBlocker }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            Text("Job readiness").font(.headline)
            // Item 48: "Controller connected" ticked for the Simulator too.
            Label(machine.connected
                    ? (machine.demo ? "Simulator connected · no machine" : "Controller connected")
                    : "Controller disconnected",
                  systemImage: machine.connected ? (machine.demo ? "desktopcomputer" : "checkmark.circle") : "circle")
            Label(cnc.jobCurrent ? "Toolpaths current" : "Toolpaths need generation",
                  systemImage: cnc.jobCurrent ? "checkmark.circle" : "circle")
            Label(machine.workspace == "G54" ? "G54 selected" : "G54 required",
                  systemImage: machine.workspace == "G54" ? "checkmark.circle" : "circle")
            machineSection
            Divider()
            Toggle("I checked the tool, workholding, clearance and G54 zero", isOn: $cnc.setupConfirmed)
                .disabled(machine.active).fixedSize(horizontal: false, vertical: true)
            if let reason = blocker {
                Text(reason).font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !cnc.jobCurrent && !cnc.usesImportedProgram {
                Button(CNCCommand.buildJob.title(cnc)) { CNCCommand.buildJob.run(cnc) }
                    .disabled(!CNCCommand.buildJob.isEnabled(cnc))
            }
            Divider()
            HStack {
                // The same predicate Manufacture ▸ Export Job… asks.
                Button("Export job…") { CNCCommand.exportJob.run(cnc) }
                    .disabled(!CNCCommand.exportJob.isEnabled(cnc))
                    .accessibilityIdentifier("cnc.job.export")
                Spacer()
                Button("Save park position") { machine.savePark() }.disabled(!machine.canCommand)
            }
            Text("No fixture collision check. Feed hold and soft reset are controller commands, not a hardware E-stop.")
                .font(.caption2).foregroundStyle(.tertiary)
        }.font(.callout)
    }

    /// What the machine has to say about this job, in machine coordinates.
    @ViewBuilder private var machineSection: some View {
        if machine.connected {
            if findings.isEmpty, cncJobEnvelope(cnc) != nil {
                Label("Job fits the machine's travel", systemImage: "checkmark.circle")
            }
            ForEach(findings) { finding in
                Label {
                    Text(finding.text).font(.caption).fixedSize(horizontal: false, vertical: true)
                } icon: {
                    Image(systemName: finding.blocking ? "exclamationmark.octagon.fill" : "exclamationmark.triangle")
                        .foregroundStyle(finding.blocking ? Color.red : Color.orange)
                }
            }
            if let spindle = cncSpindleInstruction(cnc) {
                Label { Text(spindle).font(.caption).fixedSize(horizontal: false, vertical: true) }
                    icon: { Image(systemName: "dial.medium") }
            }
            if cncJobEnvelope(cnc) != nil {
                Button("Trace bounds…", action: onTrace).controlSize(.small).disabled(!machine.canCommand)
            }
        }
    }
}

/// What the operator has to set the router's dial to for this job. The job's
/// own note wins when it carries one; otherwise the machine profile's dial
/// table answers from the commanded rpm.
@MainActor func cncSpindleInstruction(_ cnc: CNCWorkspace) -> String? {
    let rpm = cnc.operations.map(\.setup.rpm).filter { $0.isFinite && $0 > 0 }.max() ?? 0
    return CNCMachineCheck.spindleInstruction(rpm: rpm, profile: cnc.machine.profile,
                                              notes: cnc.jobNotes.map(\.text))
}
