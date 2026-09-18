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

// MARK: - Probing

/// Probing that knows what it is touching: a plate of a declared thickness, a
/// slip of paper, or the metal itself; an edge, with the cutter's radius taken
/// off; and two touches on one edge, which is how a blank that is 10° out
/// stops being a surprise halfway through a cut.
struct CNCProbeSheet: View {
    @Bindable var cnc: CNCWorkspace
    @Environment(\.dismiss) private var dismiss
    @State private var settings = CNCProbeSettings.load()
    @State private var edge = CNCEdgeProbe()
    @State private var pending: Motion?
    private var machine: CNCController { cnc.machine }

    /// Every motion in this sheet goes through one confirmation, and Return
    /// always answers Cancel.
    private enum Motion: Identifiable, Equatable {
        case probeZ, applyZ, setPaper
        case probeEdge, applyEdge
        case skewFirst, skewStation, skewSecond
        var id: String { String(describing: self) }
        var title: String {
            switch self {
            case .probeZ: return "Probe down to the surface?"
            case .applyZ: return "Set Z zero from this touch?"
            case .setPaper: return "Set Z here from the paper?"
            case .probeEdge: return "Probe toward the edge?"
            case .applyEdge: return "Set this edge as zero?"
            case .skewFirst: return "Probe the first touch?"
            case .skewStation: return "Move to the second station?"
            case .skewSecond: return "Probe the second touch?"
            }
        }
        var action: String {
            switch self {
            case .probeZ, .skewFirst, .skewSecond, .probeEdge: return "Probe"
            case .applyZ, .applyEdge: return "Set zero"
            case .setPaper: return "Set Z"
            case .skewStation: return "Move"
            }
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.l) {
            Text("Probe · \(machine.workspace)").font(.title2)
            zeroZ
            Divider()
            edgeFinding
            Divider()
            skew
            if let error = machine.error { Text(error).foregroundStyle(.red).font(.caption).fixedSize(horizontal: false, vertical: true) }
            HStack {
                Button("Hold") { machine.hold() }.disabled(!machine.connected)
                Spacer()
                Button("Done") { settings.save(); dismiss() }
            }
        }
        .padding(24).frame(width: 600)
        .onChange(of: settings) { _, value in value.save() }
        .confirmationDialog(pending?.title ?? "", isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })) {
            if let pending {
                Button(pending.action) { perform(pending); self.pending = nil }
                Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
            }
        } message: { Text(message(for: pending)) }
    }

    // MARK: Z

    private var zeroZ: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Eyebrow("Z zero")
            Picker("Touching", selection: Binding(get: { settings.kind }, set: { settings.touchOff = $0.rawValue })) {
                ForEach(CNCTouchOff.allCases) { Text($0.title).tag($0) }
            }.pickerStyle(.segmented).labelsHidden()
            Text(settings.kind.explanation).font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: Theme.Space.m) {
                if settings.kind == .plate { field("Plate thickness", value: $settings.plateThickness) }
                if settings.kind == .paper { field("Paper", value: $settings.paperThickness) }
                field("Travel", value: $settings.travel)
                field("Feed", value: $settings.feed)
            }
            Text(machine.status.probeTriggered ? "Probe input is already triggered" : "Probe input is open")
                .font(.caption).foregroundStyle(machine.status.probeTriggered ? Color.orange : .secondary)
            if let p = machine.probePosition {
                Text(String(format: "Contact at machine Z %.3f mm · work Z0 lands %.3f mm below it",
                            p.z, settings.standoff)).font(.callout.monospaced())
            }
            HStack {
                Button("Probe down") { pending = .probeZ }
                    .buttonStyle(.borderedProminent)
                    .disabled(!machine.canCommand || machine.status.work == nil || machine.status.probeTriggered)
                Button("Apply Z zero") { pending = .applyZ }.disabled(!machine.canApplyProbe)
                if settings.kind == .paper {
                    Button("Set Z \(settings.paperThickness.formatted()) at the paper") { pending = .setPaper }
                        .disabled(!machine.canCommand)
                }
            }
        }
    }

    // MARK: edges

    private var edgeFinding: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Eyebrow("Edge")
            Text("Touch the side of the blank and set X0 or Y0 from it, allowing for the Ø \(cnc.toolDiameter.formatted()) mm cutter — rather than eyeing a centre mark.")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            HStack(spacing: Theme.Space.m) {
                Picker("Axis", selection: $edge.axis) { Text("X").tag("X"); Text("Y").tag("Y") }
                    .pickerStyle(.segmented).frame(width: 90).labelsHidden()
                Picker("Direction", selection: $edge.direction) {
                    Text("toward +").tag(1.0); Text("toward −").tag(-1.0)
                }.frame(width: 140).labelsHidden()
                Button("Probe edge") { pending = .probeEdge }
                    .disabled(!machine.canCommand || machine.status.probeTriggered)
                Button("Set \(edge.axis)0 here") { pending = .applyEdge }
                    .disabled(machine.probeWorkPosition == nil || !machine.canCommand)
            }
            if let contact = machine.probeWorkPosition {
                Text(String(format: "Contact at work X %.3f  Y %.3f", contact.x, contact.y)).font(.caption.monospaced())
            }
        }
    }

    // MARK: skew

    private var skew: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            Eyebrow("Skew")
            Text(edge.label).font(.callout)
            HStack(spacing: Theme.Space.m) {
                field("Station spacing", value: $settings.spacing)
                Button("First touch") { pending = .skewFirst }.disabled(!machine.canCommand)
                Button("Move \(settings.spacing.formatted()) mm") { pending = .skewStation }
                    .disabled(!machine.canCommand || edge.phase != .first)
                Button("Second touch") { pending = .skewSecond }
                    .disabled(!machine.canCommand || edge.phase != .first)
            }
            if let measured = machine.skewDegrees {
                HStack {
                    Text(String(format: "Blank sits %+.2f° from the machine axes.", measured)).font(.callout)
                    Spacer()
                    Button("Clear") { machine.setSkew(nil); edge.reset() }.controlSize(.small)
                }
                Text("Offered as the job's placement rotation: the job turns the same way as the blank, so a +10° blank takes a +10° job.")
                    .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private func field(_ title: String, value: Binding<Double>) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption2).foregroundStyle(.secondary)
            TextField(title, value: value, format: .number).textFieldStyle(.roundedBorder).frame(width: 90)
        }
    }

    private func message(for motion: Motion?) -> String {
        switch motion {
        case .probeZ: return "The tool feeds down up to \(settings.travel.formatted()) mm at \(settings.feed.formatted()) mm/min until the probe closes. Check the clip is on the tool."
        case .applyZ: return "Work Z0 will be set \(settings.standoff.formatted()) mm below the contact (\(settings.kind.title))."
        case .setPaper: return "Work Z here becomes \(settings.paperThickness.formatted()) mm — the slip of paper under the tool."
        case .probeEdge: return "The tool feeds \(edge.direction > 0 ? "+" : "−")\(edge.axis) up to \(settings.travel.formatted()) mm until it touches the blank."
        case .applyEdge: return "The edge becomes \(edge.axis)0, one cutter radius beyond the contact."
        case .skewFirst, .skewSecond: return "A probe move along \(edge.axis) of up to \(settings.travel.formatted()) mm."
        case .skewStation: return "Back off \(settings.backOff.formatted()) mm and travel \(settings.spacing.formatted()) mm along \(edge.stationAxis)."
        case nil: return ""
        }
    }

    private func perform(_ motion: Motion) {
        cnc.setupConfirmed = false
        switch motion {
        case .probeZ: machine.probeZ(distance: settings.travel, feed: settings.feed)
        case .applyZ: machine.applyProbe(thickness: settings.standoff)
        case .setPaper: machine.setWork(axis: "Z", value: settings.paperThickness)
        case .probeEdge:
            machine.probe(axis: edge.axis, distance: (edge.direction > 0 ? 1 : -1) * settings.travel, feed: settings.feed)
        case .applyEdge:
            machine.applyEdgeZero(axis: edge.axis, target: 0, toolDiameter: cnc.toolDiameter, direction: edge.direction)
        case .skewFirst:
            edge.reset(); edge.probe(on: machine, settings: settings)
        case .skewStation: edge.moveToSecondStation(on: machine, settings: settings)
        case .skewSecond: edge.probe(on: machine, settings: settings)
        }
    }
}

// MARK: - Trace bounds

/// Walk the job's bounding rectangle at a safe height, stopping at each corner
/// (friction-log item 53). Nothing here cuts: the lowest this goes is the dip
/// height, and only where the operator asks for it.
struct CNCTraceSheet: View {
    @Bindable var cnc: CNCWorkspace
    @Environment(\.dismiss) private var dismiss
    @State private var safeZ = 5.0
    @State private var dipZ = 1.0
    @State private var pending: Motion?
    private var machine: CNCController { cnc.machine }
    private var trace: CNCTrace { machine.trace }
    private var box: CNCEnvelopeBox? { cncJobEnvelope(cnc) }

    private enum Motion: Identifiable, Equatable {
        case start, next, dip
        var id: String { String(describing: self) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.l) {
            Text("Trace bounds").font(.title2)
            if let box {
                Text(String(format: "The cutter sweeps X %.1f…%.1f, Y %.1f…%.1f mm in work coordinates. Tracing it shows where the blank, the clamps and the router actually are.",
                            box.min.x, box.max.x, box.min.y, box.max.y))
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            } else {
                Text("Build the job first: there is no swept rectangle to trace.").font(.callout).foregroundStyle(.secondary)
            }
            HStack(spacing: Theme.Space.l) {
                field("Trace height Z", value: $safeZ)
                field("Dip height Z", value: $dipZ)
                field("Feed", value: Binding(get: { trace.feed }, set: { trace.feed = $0 }))
            }
            Divider()
            Text(trace.label).font(.headline)
            if trace.running, let corner = trace.currentCorner, trace.corners.indices.contains(corner) {
                Text(String(format: "Standing at X %.1f  Y %.1f, Z %.1f.",
                            trace.corners[corner].x, trace.corners[corner].y, trace.safeZ))
                    .font(.callout.monospaced())
                if !trace.dipped.isEmpty {
                    Text("Dipped at \(trace.dipped.sorted().map { String($0 + 1) }.joined(separator: ", ")).")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            HStack {
                if trace.running {
                    Button("Continue") { pending = .next }
                        .buttonStyle(.borderedProminent).disabled(!machine.canCommand)
                    Button("Dip to Z \(dipZ.formatted())") { pending = .dip }.disabled(!machine.canCommand)
                    Button("Hold") { machine.hold() }.disabled(!machine.connected)
                    Button("Stop tracing") { trace.cancel() }
                } else {
                    Button("Start trace") { pending = .start }
                        .buttonStyle(.borderedProminent)
                        .disabled(box == nil || !machine.canCommand || safeZ <= dipZ)
                    Button("Hold") { machine.hold() }.disabled(!machine.connected)
                }
                Spacer()
                Button("Done") { dismiss() }
            }
        }
        .padding(24).frame(width: 560)
        .confirmationDialog(title(for: pending), isPresented: Binding(get: { pending != nil }, set: { if !$0 { pending = nil } })) {
            if let pending {
                Button(pending == .dip ? "Dip" : "Move") { perform(pending); self.pending = nil }
                // Cancel answers Return here too: this moves the machine.
                Button("Cancel", role: .cancel) {}.keyboardShortcut(.defaultAction)
            }
        } message: { Text(message(for: pending)) }
    }

    private func field(_ title: String, value: Binding<Double>) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.caption2).foregroundStyle(.secondary)
            TextField(title, value: value, format: .number).textFieldStyle(.roundedBorder).frame(width: 90)
        }
    }
    private func title(for motion: Motion?) -> String {
        switch motion {
        case .start: return "Move to the first corner?"
        case .next: return trace.currentCorner.map { $0 + 1 < trace.corners.count ? "Move to corner \($0 + 2)?" : "Retract and finish?" } ?? "Move?"
        case .dip: return "Dip to Z \(dipZ.formatted())?"
        case nil: return ""
        }
    }
    private func message(for motion: Motion?) -> String {
        switch motion {
        case .start, .next:
            return "The machine retracts to Z \(safeZ.formatted()) and travels at \(trace.feed.formatted()) mm/min. Nothing cuts. Feed hold stops it."
        case .dip:
            return "The tool feeds down to Z \(dipZ.formatted()) at this corner and comes straight back up. Watch the clamps."
        case nil: return ""
        }
    }
    private func perform(_ motion: Motion) {
        cnc.setupConfirmed = false
        switch motion {
        case .start:
            guard let box else { return }
            trace.begin(box: box, on: machine, safeZ: safeZ, dipZ: dipZ)
        case .next: trace.advance(on: machine)
        case .dip: trace.dip(on: machine)
        }
    }
}
