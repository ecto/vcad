import SwiftUI

/// The job's tool list, and the tool-change sentence that follows from it
/// (friction-log item 19).
///
/// One row per cutter, the selected one expanded into its numbers. The fields
/// carry per-tool identifiers (`cnc.tool.2.diameter`), and the *selected*
/// tool's numbers are also reachable under the plain names the field registry
/// has always used — so a script that says `cnc.tool.diameter` still means the
/// primary end mill, and nothing that already worked stops working.
struct CNCToolListSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        @Bindable var cnc = cnc
        Group {
            HStack(spacing: Theme.Space.xs) {
                Eyebrow("Tools")
                Spacer()
                Button { cnc.addTool(kind: .drill, diameter: 2.5) } label: { Image(systemName: "plus") }
                    .help("Add a drill or a second cutter")
                    .accessibilityLabel("Add tool").accessibilityIdentifier("cnc.tools.add")
                Button { cnc.removeTool(number: cnc.selectedToolNumber) } label: { Image(systemName: "minus") }
                    .disabled(cnc.tools.count < 2)
                    .help("Remove the selected tool")
                    .accessibilityLabel("Remove tool").accessibilityIdentifier("cnc.tools.remove")
            }.buttonStyle(.borderless).controlSize(.small)

            ForEach(cnc.tools) { tool in row(tool) }

            if let tool = cnc.tool(number: cnc.selectedToolNumber) {
                Divider()
                Eyebrow(tool.label)
                Picker("Kind", selection: binding(tool.number, \.kind)) {
                    ForEach(CNCToolKind.allCases) { Text($0.label).tag($0) }
                }.pickerStyle(.segmented).labelsHidden()
                    .accessibilityLabel("What this tool is")
                    .accessibilityIdentifier("cnc.tool.\(tool.number).kind")
                CNCNumber(label: "Diameter", value: binding(tool.number, \.diameter),
                          help: tool.kind == .drill
                            ? "A drill makes exactly this hole, so it is matched to a hole of the same size — not to anything it would fit inside."
                            : "The cutting diameter. Holes narrower than the smallest end mill have to be drilled.",
                          identifier: "cnc.tool.\(tool.number).diameter")
                Stepper(value: binding(tool.number, \.flutes), in: 1...6) {
                    KeyValueRow("Flutes", "\(tool.flutes)")
                }
                .accessibilityLabel("Flutes").accessibilityValue("\(tool.flutes)")
                .accessibilityIdentifier("cnc.tool.\(tool.number).flutes")
                CNCNumber(label: "Flute length", value: binding(tool.number, \.fluteLength),
                          help: "Usable cutting length. Zero means undeclared, and the job cannot check the cut against it.",
                          identifier: "cnc.tool.\(tool.number).fluteLength")
                CNCNumber(label: "Stickout", value: binding(tool.number, \.stickout),
                          help: "How far the tool stands out of the collet. Zero means undeclared, and holder clearance over the stock cannot be checked.",
                          identifier: "cnc.tool.\(tool.number).stickout")
                Toggle("Cuts on its centre", isOn: binding(tool.number, \.centreCutting))
                    .help("A cutter that does not cut across its own centre cannot plunge; the job checks every entry against this.")
                    .accessibilityIdentifier("cnc.tool.\(tool.number).centreCutting")
                    .disabled(tool.kind == .drill)
            }

            if let warning = cnc.toolChangeWarning {
                Divider()
                CNCToolSequenceNote(cnc: cnc, warning: warning)
            }
        }
    }

    private func row(_ tool: CNCTool) -> some View {
        let selected = tool.number == cnc.selectedToolNumber
        let symbol = tool.kind == .drill ? "circle.bottomhalf.filled" : "wrench.adjustable"
        return Button { cnc.selectedToolNumber = tool.number } label: {
            HStack(spacing: 8) {
                Image(systemName: symbol).frame(width: 16)
                    .foregroundStyle(selected ? Color.accentColor : Color.secondary)
                VStack(alignment: .leading, spacing: 2) {
                    Text(tool.label).font(.callout).lineLimit(1)
                    Text(usage(of: tool)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                }
                Spacer(minLength: 0)
            }
            .padding(7)
            .selectableRow(selected: selected)
        }
        .buttonStyle(.plain)
        .accessibilityIdentifier("cnc.tool.row.\(tool.number)")
    }

    /// Which operations this tool cuts, for the row's second line.
    private func usage(of tool: CNCTool) -> String {
        let users = cnc.operations.filter { $0.setup.toolNumber == tool.number }
        guard !users.isEmpty else { return "not used by any operation" }
        if users.count == 1 { return users[0].name }
        return counted(users.count, "operation")
    }

    /// A binding into one tool, going through `updateTool` so the list's own
    /// `didSet` fires once per edit — which is what stales the job and
    /// re-decides the machinable holes.
    private func binding<V>(_ number: Int, _ path: WritableKeyPath<CNCTool, V>) -> Binding<V> {
        Binding(
            get: { cnc.tool(number: number)?[keyPath: path] ?? CNCTool()[keyPath: path] },
            set: { value in cnc.updateTool(number: number) { $0[keyPath: path] = value } })
    }
}

/// What a multi-tool job asks of the operator, said in one place so the
/// readiness list, the Machine stage and the tool panel cannot word it
/// differently.
struct CNCToolSequenceNote: View {
    @Bindable var cnc: CNCWorkspace
    var warning: String

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Label { Text(cnc.toolSequenceLabel).font(.caption.monospacedDigit()) }
                icon: { Image(systemName: "arrow.triangle.swap") }
                .accessibilityIdentifier("cnc.tools.sequence")
                .accessibilityLabel("Tool sequence")
                .accessibilityValue(cnc.toolSequenceLabel)
            Label { Text(warning).font(.caption).fixedSize(horizontal: false, vertical: true) }
                icon: { Image(systemName: "pause.circle") }
                .foregroundStyle(.orange)
                .accessibilityIdentifier("cnc.tools.changeWarning")
        }
    }
}

/// The drill settings, shown on a drill operation only.
struct CNCDrillSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        @Bindable var cnc = cnc
        Group {
            Eyebrow("Drilling")
            KeyValueRow("Holes", counted(cnc.setup.bores.count, "hole"))
            Picker("Cycle", selection: $cnc.setup.drillCycle) {
                ForEach(CNCDrillCycle.allCases) { Text($0.label).tag($0) }
            }.pickerStyle(.menu).labelsHidden()
                .accessibilityLabel("Drill cycle")
                .accessibilityIdentifier("cnc.op.drillCycle")
            if cnc.setup.drillCycle.needsPeckDepth {
                CNCNumber(label: "Peck depth", value: $cnc.setup.peckDepth,
                          help: "How deep each peck goes. Zero follows the roughing stepdown.",
                          identifier: "cnc.op.peckDepth")
            }
            CNCNumber(label: "Dwell at the bottom", value: $cnc.setup.drillDwell, unit: "s",
                      help: "Seconds to pause at full depth, for a flatter-bottomed hole.",
                      identifier: "cnc.op.drillDwell")
            if cnc.setup.drillCycle == .peck {
                // Item 63. Said here rather than only in the refusal, because
                // the refusal arrives after a build and this is the moment the
                // choice is made.
                Text("A full-retract peck rapids back down into the hole between pecks. The verifier refuses that — it replays a prismatic job and cannot tell that the hole is already open — so this job will be refused. Chip break does the same work without leaving the hole.")
                    .font(.caption).foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }
}
