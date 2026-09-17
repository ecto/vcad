import SwiftUI

struct CNCOverrideControl: View {
    var machine: CNCController
    var spindle: Bool
    @State private var draft = 100.0
    @State private var editing = false
    private var reported: Int? { spindle ? machine.status.spindleOverride : machine.status.feedOverride }
    var body: some View {
        VStack(spacing: 5) {
            HStack {
                Button { machine.setOverride(spindle: spindle, percent: 100) } label: { Label(spindle ? "Spindle" : "Feed", systemImage: "arrow.counterclockwise") }
                    .buttonStyle(.plain).foregroundStyle(Color.accentColor)
                Spacer()
                Text(machine.status.isFresh ? "\((spindle ? machine.status.rpm : machine.status.feed).formatted()) \(spindle ? "rpm" : "mm/min")" : "—").monospacedDigit()
            }.font(.caption)
            HStack {
                Text(reported.map { "\($0)%" } ?? "—").font(.caption.monospaced()).foregroundStyle(Color.accentColor).frame(width: 38, alignment: .leading)
                Slider(value: $draft, in: 10...200, step: 1) { changing in
                    editing = changing
                    if !changing { machine.setOverride(spindle: spindle, percent: Int(draft)) }
                }.accessibilityLabel(spindle ? "Spindle override" : "Feed override")
            }
            if reported == nil { Text("Waiting for controller override report").font(.caption2).foregroundStyle(.secondary) }
        }.disabled(!machine.canOverride || reported == nil || machine.changingOverride)
            .onAppear { draft = Double(reported ?? 100) }
            .onChange(of: reported) { _, value in if !editing { draft = Double(value ?? 100) } }
    }
}


struct CNCProbeSheet: View {
    @Bindable var cnc: CNCWorkspace
    @Environment(\.dismiss) private var dismiss
    @State private var distance = 10.0
    @State private var feed = 50.0
    @State private var thickness = 1.0
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Probe Z · \(cnc.machine.workspace)").font(.title2)
            Text("Connect the probe, position above the plate, then probe downward. Apply the plate thickness after a successful contact.").font(.callout).foregroundStyle(.secondary)
            field("Maximum travel (mm)", value: $distance)
            field("Probe feed (mm/min)", value: $feed)
            field("Plate thickness (mm)", value: $thickness)
            Text(cnc.machine.status.probeTriggered ? "Probe input is already triggered" : "Probe input is open").foregroundStyle(cnc.machine.status.probeTriggered ? Color.orange : .secondary)
            if let p = cnc.machine.probePosition { Text(String(format: "Contact at machine Z %.3f mm", p.z)).font(.callout.monospaced()) }
            if let error = cnc.machine.error { Text(error).foregroundStyle(.red).font(.caption) }
            HStack {
                Button("Probe downward") { cnc.setupConfirmed = false; cnc.machine.probeZ(distance: distance, feed: feed) }
                    .buttonStyle(.borderedProminent).disabled(!cnc.machine.canCommand || cnc.machine.status.work == nil || cnc.machine.status.probeTriggered || !(0.01...50).contains(distance) || !(1...500).contains(feed))
                Button("Apply Z zero") { cnc.setupConfirmed = false; cnc.machine.applyProbe(thickness: thickness) }
                    .disabled(!cnc.machine.canApplyProbe || !(0...100).contains(thickness))
                Button("Hold") { cnc.machine.hold() }.disabled(!cnc.machine.connected)
                Spacer()
                Button("Done") { dismiss() }
            }
        }.padding(24).frame(width: 560)
    }
    private func field(_ title: String, value: Binding<Double>) -> some View {
        HStack { Text(title); Spacer(); TextField(title, value: value, format: .number).textFieldStyle(.roundedBorder).frame(width: 100) }
    }
}
