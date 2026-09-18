import SwiftUI

// The Manufacture menu.
//
// Friction-log item 45: "Import and Export job exist only inside a pull-down
// and a popover. Neither is in the menu bar … so neither a keyboard user nor
// an assistive tool can reach 'Export job…'."
//
// The rule this file exists to keep is narrower than "put them in a menu": a
// menu item must never be able to do what the button beside it refuses. So
// every one of these actions is named once, here, with one predicate and one
// body — and the buttons in the machine bar, the job outline and the readiness
// list call the same `isEnabled` / `run` pair. There is no second copy of the
// rule to fall out of step.

/// One thing the Manufacture workspace can be told to do.
enum CNCCommand: String, CaseIterable, Identifiable, Sendable {
    case outlineFromModel
    case importOutlineDXF
    case buildJob
    case verifyJob
    case exportJob
    case sendToNcSender
    case traceBounds
    case probe
    case connect
    case runJob

    var id: String { rawValue }

    /// What the menu item says. A few of them change with the job's state —
    /// "Build" becomes "Rebuild" once there is a job to rebuild — so the title
    /// takes the workspace.
    @MainActor func title(_ cnc: CNCWorkspace) -> String {
        switch self {
        case .outlineFromModel: return "Outline From Model"
        case .importOutlineDXF: return "Import Outline (DXF)…"
        case .buildJob: return cnc.generating ? "Building…" : cnc.jobCurrent ? "Rebuild Job" : "Generate Job"
        case .verifyJob: return "Verify"
        case .exportJob: return "Export Job…"
        case .sendToNcSender: return "Send to ncSender"
        case .traceBounds: return "Trace Bounds…"
        case .probe: return "Probe…"
        case .connect: return cnc.machine.connected || cnc.machine.connecting ? "Disconnect" : "Connect…"
        case .runJob: return "Run Job…"
        }
    }

    /// The key equivalent, or nothing. Every one is ⌘ with a second modifier:
    /// a bare-letter shortcut would fire while a number field has focus, and
    /// nothing in here may be one keystroke away. The full table, including
    /// what each one was checked against, is in the friction log under 45.
    var shortcut: (key: KeyEquivalent, modifiers: EventModifiers)? {
        switch self {
        case .outlineFromModel: return ("o", [.command, .option])
        case .importOutlineDXF: return ("i", [.command, .option])
        case .buildJob: return ("b", [.command, .option])
        case .verifyJob: return ("y", [.command, .option])
        case .exportJob: return ("e", [.command, .control])
        case .sendToNcSender: return ("n", [.command, .option])
        case .traceBounds: return ("g", [.command, .option])
        case .probe: return ("p", [.command, .option])
        case .connect: return ("k", [.command, .option])
        // Never Return, with or without modifiers: AppKit advertises any
        // button whose key is Return as the window's default button, and an
        // accessibility client asked to press "return" pressed Run Job
        // (friction-log item 44).
        case .runJob: return ("j", [.command, .option])
        }
    }

    /// Whether this may run right now. **This is the only answer**: the button
    /// and the menu item both ask it, so a menu item cannot outrun its button.
    @MainActor func isEnabled(_ cnc: CNCWorkspace) -> Bool {
        // Nothing here may touch a job while the controller is streaming one,
        // and nothing may touch it while the kernel is still building it.
        let editable = !cnc.machine.active && !cnc.generating
        switch self {
        case .outlineFromModel:
            return editable && cnc.hasModel
        case .importOutlineDXF:
            return editable
        case .buildJob:
            return editable
        case .verifyJob:
            // Verifying without an outline is the thing this whole pipeline
            // refuses to pretend to do: there would be nothing to replay the
            // job against.
            return editable && (cnc.outline != nil || cnc.usesImportedProgram)
        case .exportJob:
            // A refused job has no G-code at all, so there is nothing to write.
            return cnc.jobCode != nil
        case .sendToNcSender:
            return cncSenderBlockers(cnc).isEmpty && !cnc.ncSender.busy
        case .traceBounds:
            return cnc.machine.canCommand && cncJobEnvelope(cnc) != nil
        case .probe:
            return cnc.machine.canCommand
        case .connect:
            return !cnc.machine.active
        case .runJob:
            return cnc.runBlocker == nil
        }
    }

    /// Do it. Callers check `isEnabled` first; this checks again, because a
    /// menu item's enabled state is a frame behind the model at worst.
    @MainActor func run(_ cnc: CNCWorkspace) {
        guard isEnabled(cnc) else { return }
        switch self {
        case .outlineFromModel: cnc.importFromModel()
        case .importOutlineDXF: cnc.importOutlineFile()
        case .buildJob: cnc.build()
        case .verifyJob: cnc.verify()
        case .exportJob: cnc.export(job: true)
        case .sendToNcSender:
            // The panel is where the send is watched, so it comes forward
            // rather than the upload happening out of sight.
            cnc.senderPanelShown = true
            guard let code = cnc.jobCode else { return }
            let name = cncSenderFilename(cnc)
            let sender = cnc.ncSender
            Task { await sender.sendJob(gcode: code, filename: name) }
        case .traceBounds: cnc.traceShown = true
        case .probe: cnc.probeShown = true
        case .connect:
            if cnc.machine.connected || cnc.machine.connecting {
                cnc.machine.disconnect(); cnc.setupConfirmed = false
            } else {
                // Which controller — the Anolex or the Simulator — is a
                // choice, not a default, so this opens the popover that asks.
                cnc.connectionShown = true
            }
        case .runJob:
            // The same confirmation the bar's Run Job raises. Starting the
            // spindle is never one keystroke.
            cnc.runShown = true
        }
    }
}

// MARK: - The menu

/// Manufacture, in the menu bar. Shown always, disabled off-workspace, because
/// a menu that appears and disappears is one nobody can learn.
struct ManufactureCommands: Commands {
    @Bindable var model: EditorModel

    private static let groups: [[CNCCommand]] = [
        [.outlineFromModel, .importOutlineDXF],
        [.buildJob, .verifyJob],
        [.exportJob, .sendToNcSender],
        [.traceBounds, .probe, .connect],
        [.runJob],
    ]

    var body: some Commands {
        CommandMenu("Manufacture") {
            ForEach(Array(Self.groups.enumerated()), id: \.offset) { index, group in
                if index > 0 { Divider() }
                ForEach(group) { command in item(command) }
            }
            Divider()
            // `cnc` is a `let` on the model, so this is written out rather
            // than taken through `@Bindable`.
            Toggle("ncSender & Camera", isOn: Binding(
                get: { model.cnc.senderPanelShown },
                set: { model.cnc.senderPanelShown = $0 }))
                .disabled(model.workspace != .manufacture)
            Button("Job Readiness…") { model.cnc.readinessShown = true }
                .disabled(model.workspace != .manufacture)
        }
    }

    private func item(_ command: CNCCommand) -> some View {
        let cnc = model.cnc
        return Button(command.title(cnc)) {
            // Reaching a Manufacture action from the menu bar while another
            // workspace is on screen would act on a job nobody can see.
            model.workspace = .manufacture
            command.run(cnc)
        }
        .modifier(CNCShortcut(shortcut: command.shortcut))
        .disabled(model.workspace != .manufacture || !command.isEnabled(cnc))
    }
}

/// `keyboardShortcut` has no "no shortcut" form, so this is the optional one.
private struct CNCShortcut: ViewModifier {
    let shortcut: (key: KeyEquivalent, modifiers: EventModifiers)?
    func body(content: Content) -> some View {
        if let shortcut {
            content.keyboardShortcut(shortcut.key, modifiers: shortcut.modifiers)
        } else {
            content
        }
    }
}
