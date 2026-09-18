import SwiftUI
import AppKit

/// The drawer above the machine bar: the controller terminal, the job's
/// G-code, or saved macros — one at a time, opened from the bar.
struct CNCStudioDrawer: View {
    @Bindable var model: EditorModel
    private var cnc: CNCWorkspace { model.cnc }
    var body: some View {
        @Bindable var cnc = cnc
        VStack(spacing: 0) {
            HStack {
                Eyebrow(cnc.inspectorTab.rawValue)
                Spacer()
                Text(cnc.usesImportedProgram ? cnc.importedName : cnc.selectedOperation.name)
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }.padding(.horizontal, 18).padding(.vertical, Theme.Space.s)
            switch cnc.inspectorTab {
            case .terminal: CNCStudioTerminal(cnc: cnc)
            case .gcode:
                ScrollView {
                    if let code = cnc.jobCode {
                        CNCGcodeListing(code: code)
                    } else {
                        gcodeAbsence
                    }
                }
            case .macros: CNCStudioMacros(cnc: cnc)
            }
        }.frame(maxWidth: .infinity, maxHeight: .infinity)
    }

    /// Why there is no G-code.
    ///
    /// "Generate the job or import a G-code file." was said for a *refused*
    /// job too — the one case where the absence is the whole point. A job the
    /// oracle refused has no `gcode` key at all, which is the gate working;
    /// reading it as "you haven't pressed Generate yet" invites pressing
    /// Generate again and wondering why nothing changes.
    @ViewBuilder private var gcodeAbsence: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            if cnc.jobCurrent, !cnc.blockers.isEmpty {
                Label("This job was refused, so it has no G-code.", systemImage: "xmark.octagon.fill")
                    .font(.callout.weight(.medium)).foregroundStyle(Color.red)
                ForEach(cnc.blockers) { blocker in
                    Text("• " + blocker.text).font(.callout)
                        .fixedSize(horizontal: false, vertical: true)
                }
                Text("There is nothing here to export or send: a refused job never becomes a file. Fix the reasons above and build it again.")
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else if cnc.jobCurrent {
                // Blocked, but every reason is on the imported program's side.
                Text("This program produced no G-code.").font(.callout)
            } else {
                Text("Generate the job or import a G-code file.").font(.callout)
            }
        }
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 18).padding(.vertical, 6)
    }
}

/// The job's G-code, with the operator stops marked.
///
/// A tool change on this machine is an `M0`: the controller stops dead and
/// waits for a cycle start. Item 19 — a job that pauses is a job that asks
/// something of the operator, and reading three thousand lines to find out
/// where is not a way to learn it. The marked lines are also what makes the
/// count in the readiness list checkable against the program itself.
struct CNCGcodeListing: View {
    var code: String

    /// The stop lines, by their 1-based number in the program.
    static func stopLines(in code: String) -> [Int] {
        code.split(separator: "\n", omittingEmptySubsequences: false).enumerated()
            .compactMap { index, line in isStop(String(line)) ? index + 1 : nil }
    }

    /// `M0` and `M1`, with or without a leading line number, and never `M0…`
    /// as a prefix of something else (`M03` is a spindle start, which would be
    /// a bad thing to call a pause).
    static func isStop(_ line: String) -> Bool {
        let bare = line.split(separator: "(").first.map(String.init) ?? line
        for word in bare.split(whereSeparator: { $0 == " " || $0 == "\t" }) {
            let w = word.uppercased()
            guard w.hasPrefix("M") else { continue }
            let digits = w.dropFirst()
            guard !digits.isEmpty, digits.allSatisfy(\.isNumber), let code = Int(digits) else { continue }
            if code == 0 || code == 1 { return true }
        }
        return false
    }

    var body: some View {
        let lines = code.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        let stops = Set(Self.stopLines(in: code))
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
                let isStop = stops.contains(index + 1)
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    if isStop {
                        Image(systemName: "pause.circle.fill").font(.caption)
                            .foregroundStyle(.orange)
                            .accessibilityHidden(true)
                    }
                    Text(line).font(.subheadline.monospaced())
                        .foregroundStyle(isStop ? Color.orange : .primary)
                        .fontWeight(isStop ? .semibold : .regular)
                    Spacer(minLength: 0)
                }
                .padding(.horizontal, 18)
                .padding(.vertical, isStop ? 3 : 0)
                .background(isStop ? Color.orange.opacity(0.12) : .clear)
                .accessibilityLabel(isStop ? "Tool change stop: \(line)" : line)
            }
        }
        .textSelection(.enabled)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.vertical, 6)
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
                        .font(.subheadline.monospaced()).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
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
                Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
            } message: { Text(pending?.command ?? "") }
    }
}
