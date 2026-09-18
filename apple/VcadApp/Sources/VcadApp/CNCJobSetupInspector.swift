import SwiftUI

// The Setup stage's panels: what the blank is made of, what to set on the
// router, where zero is, where the part sits on the metal, and what is clamped
// on top of it.
//
// Every pull-down here has a plain control beside or behind it, and every
// number field carries a label, an identifier and its value in the
// accessibility tree (friction-log items 45 and 51).

// MARK: - Material

struct CNCMaterialSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        Eyebrow("Material")
        // Item 54: the first real cut was made with feeds typed by hand for a
        // material the plate turned out not to be.
        Picker("Material", selection: Binding(
            get: { cnc.materialID ?? "" },
            set: { cnc.materialID = $0.isEmpty ? nil : $0 })) {
                Text("Not chosen").tag("")
                ForEach(cnc.materials) { material in
                    Text(material.name).tag(material.id)
                }
            }
            .labelsHidden()
            .accessibilityLabel("Stock material")
            .accessibilityIdentifier("cnc.stock.material")
            .onAppear { cnc.loadMaterials() }
        if let material = cnc.material {
            if let advice = material.coolantAdvice {
                Text("\(material.name) \(advice).")
                    .font(.caption).foregroundStyle(.secondary)
            }
        } else {
            Text("Nothing is assumed: with no material chosen the app offers no feeds, because numbers for the wrong material are worse than none.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}

// MARK: - Feeds, and the dial to set

/// "Recommend feeds" and what it came back with. The rpm is not the answer on
/// this machine — the dial is — so the dial is what this shows first.
struct CNCFeedsSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        Eyebrow("Feeds and speeds")
        if cnc.material == nil {
            Text("Choose a material on Stock and this fills in feed, plunge, stepdown, stepover and the dial to set.")
                .font(.caption).foregroundStyle(.secondary)
        }
        Toggle("First cut on this machine: use 60 %", isOn: $cnc.firstCutDerate)
            .help("Derates the recommended feed and stepdown to 60 %. The first cut on an unknown machine is the one that breaks cutters.")
            .accessibilityIdentifier("cnc.feeds.firstCut")
        Button("Recommend feeds") { cnc.applyRecommendedFeeds() }
            .disabled(cnc.material == nil || cnc.machine.active || cnc.generating)
            .accessibilityIdentifier("cnc.feeds.recommend")
            .help("Fills feed, plunge, stepdown, stepover and spindle on every operation, and says which dial position to set.")
        if let advice = cnc.recommendation {
            // The S word does nothing on a router with a manual dial, and a
            // recommendation that only said "13 500 rpm" would be unusable.
            Label(advice.spindleAdvice, systemImage: "dial.medium")
                .font(.caption).foregroundStyle(.primary).fixedSize(horizontal: false, vertical: true)
            KeyValueRow("Chip per tooth", "\(CNCVerdictText.mm(advice.recommendation.chiploadMm, 4)) mm")
            if cnc.firstCutDerate {
                Text("Derated to 60 %: feed \(CNCVerdictText.mm(advice.values(derated: true).feed, 0)) instead of \(CNCVerdictText.mm(advice.recommendation.feedMmMin, 0)) mm/min, stepdown \(CNCVerdictText.mm(advice.values(derated: true).stepdown, 3)) instead of \(CNCVerdictText.mm(advice.recommendation.stepdownMm, 3)) mm.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            ForEach(advice.recommendation.notes.filter { $0.level > .info }) { note in
                Label(note.text, systemImage: note.level.symbol)
                    .font(.caption)
                    .foregroundStyle(note.level >= .warning ? Color.orange : .secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        // …and a second opinion on numbers that were typed rather than
        // recommended: "0.018 mm/tooth: rubbing" is the note that would have
        // caught the first job's feeds.
        ForEach(cnc.feedNotes.filter { $0.level > .info }) { note in
            Label(note.text, systemImage: note.level.symbol)
                .font(.caption)
                .foregroundStyle(note.level >= .warning ? Color.orange : .secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

// MARK: - Where zero is

struct CNCZeroSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        Eyebrow("Work zero")
        // Item 41: zero was the lower-left of the outline's bounding box,
        // which is *inside* the blank and nothing showed where it sat on it.
        Picker("Where zero is", selection: $cnc.zeroLocation) {
            ForEach(CNCZeroLocation.allCases) { Text($0.label).tag($0) }
        }
        .pickerStyle(.radioGroup).labelsHidden()
        .accessibilityLabel("Where the operator sets zero")
        .accessibilityIdentifier("cnc.zero.location")
        Text(cnc.zeroLocation.detail).font(.caption).foregroundStyle(.secondary)
        KeyValueRow("Blank corner from zero",
                    "X \(CNCVerdictText.mm(cnc.stockCornerFromZero[0], 2)) · Y \(CNCVerdictText.mm(cnc.stockCornerFromZero[1], 2)) mm")
        KeyValueRow("Part corner from zero",
                    "X \(CNCVerdictText.mm(cnc.effectivePlacement.dx, 2)) · Y \(CNCVerdictText.mm(cnc.effectivePlacement.dy, 2)) mm")
    }
}

// MARK: - Where the part sits on the metal

struct CNCPlacementSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        Eyebrow("Placement")
        CNCNumber(label: "Shift X", value: $cnc.placement.dx,
                  help: "Moves the whole job on the blank, and the part it is verified against with it.",
                  identifier: "cnc.placement.dx")
        CNCNumber(label: "Shift Y", value: $cnc.placement.dy, identifier: "cnc.placement.dy")
        CNCNumber(label: "Rotation", value: $cnc.placement.rotationDeg, unit: "°",
                  help: "Turns the job about work zero — for a blank that is clamped crooked.",
                  identifier: "cnc.placement.rotation")
        // Read-only: the machine bar measures skew, this only says what it
        // found. Copying it into the rotation is the operator's decision.
        if let skew = cnc.machineProfile.skewDegrees {
            KeyValueRow("Measured skew", "\(CNCVerdictText.mm(skew, 2))°")
            Button("Use the measured skew") { cnc.placement.rotationDeg = skew }
                .disabled(cnc.machine.active || cnc.generating)
                .accessibilityIdentifier("cnc.placement.useSkew")
        } else {
            Text("A two-point probe on the machine bar measures how far the blank is off the axes; it appears here when it has.")
                .font(.caption).foregroundStyle(.secondary)
        }
        if !cnc.placement.isIdentity {
            Text("The job and the part it is checked against move together, so a placed job is still verified against the metal it cuts.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}

// MARK: - Clamps

struct CNCClampSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        HStack {
            Eyebrow("Clamps and keep-outs")
            Spacer()
            Button { cnc.addClamp() } label: { Image(systemName: "plus") }
                .buttonStyle(.borderless).controlSize(.small)
                .disabled(cnc.machine.active || cnc.generating)
                .accessibilityLabel("Add a clamp").accessibilityIdentifier("cnc.clamp.add")
        }
        if cnc.clamps.isEmpty {
            Text("Rectangles the cutter may not sweep through, measured from work zero. The job request has no clamp field — this is the app checking the sweep against what you type, and it says so in the warning.")
                .font(.caption).foregroundStyle(.secondary)
        }
        ForEach(Array(cnc.clamps.enumerated()), id: \.element.id) { index, clamp in
            VStack(alignment: .leading, spacing: 4) {
                HStack {
                    Text(clamp.name).font(.callout.weight(.medium))
                    Spacer()
                    if cnc.clampsInTheWay.contains(where: { $0.id == clamp.id }) {
                        Label("in the sweep", systemImage: "exclamationmark.triangle.fill")
                            .font(.caption).foregroundStyle(.orange)
                    }
                    Button { cnc.removeClamp(clamp.id) } label: { Image(systemName: "trash") }
                        .buttonStyle(.borderless).controlSize(.small)
                        .accessibilityLabel("Remove \(clamp.name)")
                        .accessibilityIdentifier("cnc.clamp.remove.\(index)")
                }
                CNCNumber(label: "X", value: binding(index, \.x), identifier: "cnc.clamp.\(index).x")
                CNCNumber(label: "Y", value: binding(index, \.y), identifier: "cnc.clamp.\(index).y")
                CNCNumber(label: "Width", value: binding(index, \.width), identifier: "cnc.clamp.\(index).width")
                CNCNumber(label: "Depth", value: binding(index, \.height), identifier: "cnc.clamp.\(index).height")
            }
            .padding(.vertical, 4)
        }
    }

    private func binding(_ index: Int, _ path: WritableKeyPath<CNCClamp, Double>) -> Binding<Double> {
        Binding(get: { cnc.clamps.indices.contains(index) ? cnc.clamps[index][keyPath: path] : 0 },
                set: { if cnc.clamps.indices.contains(index) { cnc.clamps[index][keyPath: path] = $0 } })
    }
}

// MARK: - Tabs

/// Where the tabs are, as numbers — the keyboard-reachable twin of dragging
/// them in the viewport (item 21, and item 51 for reaching them at all).
struct CNCTabSection: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        let operation = cnc.selectedOperation
        let declared = cnc.declaredTabPositions(of: operation)
        let landings = cnc.tabLandings(of: operation)
        Eyebrow("Where the tabs are")
        if declared.isEmpty {
            Text("No tabs on this cut.").font(.caption).foregroundStyle(.secondary)
        }
        ForEach(Array(declared.enumerated()), id: \.offset) { index, _ in
            CNCNumber(label: "Tab \(index + 1) round the loop",
                      value: Binding(
                        get: { (cnc.tabFraction(of: operation, index: index) * 1000).rounded() / 1000 },
                        set: { cnc.setTabPosition(index: index, to: $0) }),
                      unit: "of 1",
                      help: "0 is the start of the loop, 0.5 half way round. Dragging a tab in the viewport writes the same number.",
                      identifier: "cnc.tab.\(index)")
        }
        if !landings.isEmpty {
            // Where they *are*, not where they were asked for: the kernel
            // settles each tab onto the nearest straight stretch.
            ForEach(Array(landings.enumerated()), id: \.offset) { index, tab in
                KeyValueRow("Tab \(index + 1) as cut",
                            "\(CNCVerdictText.mm(tab.metalWidth)) mm of metal, \(CNCVerdictText.mm(tab.height)) mm tall, \(CNCVerdictText.mm(tab.gapToNextMm, 0)) mm to the next")
            }
        }
    }
}
