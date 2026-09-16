import SwiftUI

/// Machine tools live in Manufacture's existing right-hand inspector.
struct CNCMachineInspector: View {
    @Bindable var cnc: CNCWorkspace
    @State private var accessoriesExpanded = false
    @State private var homeShown = false
    @State private var destination: String?
    private var machine: CNCController { cnc.machine }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Machine").font(.headline)
                Spacer()
                Text(machine.summary).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            if !machine.connected { CNCConnectionView(cnc: cnc) }
            HStack {
                Button("Home…") { homeShown = true }.disabled(!machine.canHome)
                Menu("Go to") {
                    Button("Work XY zero…") { destination = "XY zero" }
                    Button("Work Z zero…") { destination = "Z zero" }
                    Divider()
                    Button("Park…") { destination = "Park" }.disabled(machine.parkPosition == nil)
                    Button("Save current machine position as park") { machine.savePark() }
                }.disabled(!machine.canCommand)
            }.controlSize(.small)
            Divider()
            CNCOverrideControl(machine: machine, spindle: true)
            DisclosureGroup("Accessories", isExpanded: $accessoriesExpanded) {
                VStack(alignment: .leading, spacing: 8) {
                    Toggle("Flood coolant", isOn: Binding(get: { machine.status.flood }, set: { machine.setCoolant(flood: $0, mist: machine.status.mist) }))
                    Toggle("Mist coolant", isOn: Binding(get: { machine.status.mist }, set: { machine.setCoolant(flood: machine.status.flood, mist: $0) }))
                    Button("Stop spindle (M5)") { machine.stopSpindle() }
                }.padding(.top, 5).disabled(!machine.canCommand)
            }.font(.caption).controlSize(.small)
            Divider()
            Toggle("Tool, workholding, clearance and G54 zero checked", isOn: $cnc.setupConfirmed)
                .font(.caption).disabled(machine.active)
            if !machine.active, let blocker = cnc.runBlocker { Text(blocker).font(.caption).foregroundStyle(.secondary) }
            if let error = cnc.error ?? machine.error { Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled) }
        }
        .onChange(of: machine.connected) { _, _ in cnc.setupConfirmed = false }
        .onChange(of: machine.workspace) { _, _ in cnc.setupConfirmed = false }
        .confirmationDialog("Home the machine?", isPresented: $homeShown) {
            Button("Run homing cycle") { cnc.setupConfirmed = false; machine.home() }
        } message: { Text("The axes will move toward the configured homing switches.") }
        .confirmationDialog("Move to \(destination ?? "")?", isPresented: Binding(get: { destination != nil }, set: { if !$0 { destination = nil } })) {
            Button("Move") {
                if destination == "Park" { machine.park() }
                else { machine.returnToZero(xy: destination == "XY zero", clearance: cnc.setup.clearance) }
                destination = nil
            }
        } message: {
            Text(destination == "Park" ? "Retract to saved machine Z before XY travel, then return to saved Z. Verify the path is clear." : destination == "XY zero" ? "Retract to at least the CAM clearance height before moving to work X0 Y0." : "Move to work Z0 at 100 mm/min.")
        }
    }
}
