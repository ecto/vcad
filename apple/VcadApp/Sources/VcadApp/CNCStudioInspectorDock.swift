import SwiftUI
import AppKit

/// Additional tools share the existing lower inspector; the viewport and
/// transport never switch to a second machine-control screen.
struct CNCStudioInspectorDock: View {
    @Bindable var model: EditorModel
    private var cnc: CNCWorkspace { model.cnc }
    var body: some View {
        @Bindable var cnc = cnc
        VStack(spacing: 0) {
            HStack {
                Picker("Inspector section", selection: $cnc.inspectorTab) {
                    ForEach(CNCInspectorTab.allCases) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented).labelsHidden().frame(width: 340)
                Spacer()
                Text(cnc.usesImportedProgram ? cnc.importedName : cnc.selectedOperation.name)
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }.padding(.horizontal, 14).padding(.vertical, 8)
            switch cnc.inspectorTab {
            case .inspector:
                if cnc.usesImportedProgram {
                    VStack(alignment: .leading, spacing: 10) {
                        Label(cnc.importedName, systemImage: "doc.text").font(.headline)
                        Text("Imported G-code · G54 · one manually installed tool").font(.caption).foregroundStyle(.secondary)
                        Text("Preview assumes XYZ0 before the program establishes a position. Initial travel and fixtures are not verified.")
                            .font(.caption).foregroundStyle(.secondary)
                        Button("Edit generated operations") { cnc.useGeneratedJob(); cnc.select(.operation(cnc.selectedOperation.id)) }.disabled(cnc.machine.active)
                    }.padding(18).frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                } else { CNCStudioBottomInspector(model: model) }
            case .terminal: CNCStudioTerminal(cnc: cnc)
            case .gcode:
                ScrollView {
                    Text(cnc.jobCode ?? "Generate the job or import a G-code file.").font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, 18).padding(.vertical, 6)
                }
            case .macros: CNCStudioMacros(cnc: cnc)
            }
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct CNCStudioTerminal: View {
    @Bindable var cnc: CNCWorkspace
    @State private var input = ""
    @State private var autoScroll = true
    var body: some View {
        VStack(spacing: 6) {
            HStack {
                Toggle("Auto-scroll", isOn: $autoScroll)
                Spacer()
                Button("Copy") { NSPasteboard.general.clearContents(); NSPasteboard.general.setString(cnc.machine.log.joined(separator: "\n"), forType: .string) }
                Button("Clear") { cnc.machine.clearLog() }
            }.font(.caption).controlSize(.small)
            ScrollViewReader { proxy in
                ScrollView {
                    Text(cnc.machine.log.isEmpty ? "Controller commands and replies appear here." : cnc.machine.log.joined(separator: "\n"))
                        .font(.system(size: 11, design: .monospaced)).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                    Color.clear.frame(height: 1).id("end")
                }.frame(maxWidth: .infinity, maxHeight: .infinity)
                    .onChange(of: cnc.machine.log.last) { _, _ in if autoScroll { proxy.scrollTo("end", anchor: .bottom) } }
            }
            HStack {
                TextField("G-code command", text: $input).textFieldStyle(.roundedBorder).onSubmit(send)
                Button("Send", action: send).disabled(!CNCCommands.validManual(input))
            }.controlSize(.small).disabled(!cnc.machine.canCommand)
        }.padding(.horizontal, 18).padding(.bottom, 12)
    }
    private func send() {
        guard cnc.machine.canCommand, CNCCommands.validManual(input) else { return }
        cnc.setupConfirmed = false; cnc.machine.sendMDI(input); input = ""
    }
}

struct CNCStudioMacros: View {
    @Bindable var cnc: CNCWorkspace
    @State private var name = ""
    @State private var command = ""
    @State private var pending: CNCMacro?
    var body: some View {
        HStack(alignment: .top, spacing: 24) {
            ScrollView {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(cnc.macros) { macro in
                        HStack {
                            Button(macro.name) { pending = macro }.disabled(!cnc.machine.canCommand)
                            Text(macro.command).font(.caption.monospaced()).foregroundStyle(.secondary).lineLimit(1)
                            Spacer()
                            Button { cnc.removeMacro(macro.id) } label: { Image(systemName: "trash") }.accessibilityLabel("Delete \(macro.name)")
                        }
                    }
                    if cnc.macros.isEmpty { Text("Save a named command for repeated tasks.").font(.caption).foregroundStyle(.secondary) }
                }
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
            Divider()
            VStack(alignment: .leading, spacing: 8) {
                TextField("Name", text: $name)
                TextField("One G-code command", text: $command)
                Button("Save macro") { cnc.saveMacro(name: name, command: command); name = ""; command = "" }
                    .disabled(name.isEmpty || !CNCCommands.validManual(command))
            }.textFieldStyle(.roundedBorder).frame(width: 250)
        }.controlSize(.small).padding(.horizontal, 18).padding(.bottom, 12)
            .confirmationDialog("Run macro \(pending?.name ?? "")?", isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })) {
                if let macro = pending { Button("Send command") { cnc.setupConfirmed = false; cnc.machine.sendMDI(macro.command); pending = nil } }
            } message: { Text(pending?.command ?? "") }
    }
}
