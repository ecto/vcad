import SwiftUI

// The right rail's job panels: what one operation does, what the oracle made
// of the whole job, and the summary the Setup stage owed the user after an
// import or a build (friction-log items 23 and 24).

/// A labelled number field. Values are typed, not scrubbed: a feed is a
/// decision, not a slider.
///
/// Every one of these carries a label, an identifier and its current value in
/// the accessibility tree, so a script can find it by name and set it without
/// the window being frontmost (friction-log item 51: the sidebar summary was
/// the only confirmation that a typed value had taken).
struct CNCNumber: View {
    var label: String
    @Binding var value: Double
    var unit = "mm"
    var help: String?
    /// A stable name for scripts and tests. Defaults to the label.
    var identifier: String?
    var body: some View {
        HStack {
            Text(label).font(.callout)
            Spacer(minLength: 5)
            HStack(spacing: 5) {
                TextField(label, value: $value, format: .number.precision(.fractionLength(0...3)))
                    .multilineTextAlignment(.trailing).textFieldStyle(.roundedBorder).frame(width: 76)
                    .font(.callout.monospaced())
                    .accessibilityLabel(label)
                    .accessibilityIdentifier(identifier ?? label)
                    .accessibilityValue("\(value.formatted(.number.precision(.fractionLength(0...3)))) \(unit)")
                Text(unit).font(.caption).foregroundStyle(.secondary).frame(minWidth: 18, alignment: .trailing)
            }
        }
        .help(help ?? label)
    }
}

// MARK: - One operation

struct CNCOperationInspector: View {
    @Bindable var cnc: CNCWorkspace

    var body: some View {
        @Bindable var cnc = cnc
        let kind = cnc.setup.kind
        Group {
            Eyebrow("Cut")
            KeyValueRow("Shape", shapeSummary)
            // Which cutter runs this operation (item 19). The kernel keeps a
            // tool's operations together inside a phase, so this is also what
            // decides where the job stops for a tool change.
            if cnc.tools.count > 1 {
                Picker("Tool", selection: $cnc.setup.toolNumber) {
                    ForEach(cnc.tools) { Text($0.label).tag($0.number) }
                }.pickerStyle(.menu).labelsHidden()
                    .accessibilityLabel("Which tool cuts this operation")
                    .accessibilityIdentifier("cnc.op.tool")
                if kind.isContour || kind == .pocket || kind == .face,
                   cnc.tool(number: cnc.setup.toolNumber)?.kind == .drill {
                    // A drill cuts on its point; it cannot walk a wall. The
                    // kernel refuses this too, but after a build — saying it
                    // here is the difference between a choice and a mystery.
                    Text("A drill cuts on its point only and cannot follow a wall. This operation needs an end mill.")
                        .font(.caption).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            if kind == .drill {
                Divider()
                CNCDrillSection(cnc: cnc)
                Divider()
                Eyebrow("Cut")
            }
            CNCNumber(label: "Cut depth", value: $cnc.setup.depth, identifier: "cnc.op.depth")
            CNCNumber(label: "Roughing stepdown", value: $cnc.setup.stepdown, identifier: "cnc.op.stepdown")
            if kind == .face || kind == .pocket {
                CNCNumber(label: "Stepover", value: $cnc.setup.stepover, identifier: "cnc.op.stepover")
            }
            CNCNumber(label: "Clearance", value: $cnc.setup.clearance, identifier: "cnc.op.clearance")

            if kind == .contourInside || kind == .pocket {
                Divider()
                Eyebrow("Waste")
                Picker("What happens to the middle", selection: Binding(
                    get: { cnc.setup.kind },
                    set: { cnc.setup.kind = $0 })) {
                        Text("Cut out (waste drops free)").tag(CNCOpKind.contourInside)
                        Text("Pocket (nothing comes loose)").tag(CNCOpKind.pocket)
                    }
                    .labelsHidden().pickerStyle(.radioGroup)
                    .accessibilityLabel("What happens to the waste in this opening")
                    .accessibilityIdentifier("cnc.op.waste")
                if kind == .pocket {
                    // Material inside the opening that the pocket keeps: the
                    // kernel clears around it and the answer says how close
                    // the cutter came.
                    Toggle("Keep the features inside this opening",
                           isOn: Binding(get: { !cnc.setup.islands.isEmpty },
                                         set: { cnc.keepIslands($0) }))
                        .accessibilityIdentifier("cnc.op.islands")
                        .help("Anything of the outline that lies inside this pocket is left standing, and the job reports how close the cutter came to it.")
                    if !cnc.setup.islands.isEmpty {
                        KeyValueRow("Kept", counted(cnc.setup.islands.count, "island"))
                        ForEach(Array(cnc.islandClearances(of: cnc.selectedOperation).enumerated()), id: \.offset) { index, island in
                            KeyValueRow("Island \(index + 1)",
                                        island.kept
                                        ? "untouched · cutter stayed \(CNCVerdictText.mm(island.centreClearanceMm ?? 0)) mm off"
                                        : "cut into by \(CNCVerdictText.mm(island.cutIntoMm, 3)) mm")
                        }
                    } else if cnc.islandCandidates(for: cnc.selectedOperation).isEmpty {
                        Text("No part of this outline lies inside this opening, so there is nothing to keep.")
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
            }

            if kind.isContour {
                Divider()
                Eyebrow("Roughing and finishing")
                CNCNumber(label: "Leave on the wall", value: $cnc.setup.stockToLeave,
                          help: "Metal the roughing passes leave for the finish pass. Zero means no separate roughing phase.",
                          identifier: "cnc.op.stockToLeave")
                Stepper(value: $cnc.setup.finishStepdowns, in: 1...20) {
                    KeyValueRow("Finish stepdowns", "\(cnc.setup.finishStepdowns)")
                }
                Toggle("Spring pass", isOn: $cnc.setup.springPass)
                    .help("One more pass at the same offset, to take the deflection the last one left.")
                CNCNumber(label: "Finish feed", value: $cnc.setup.finishFeed, unit: "mm/min",
                          help: "Zero uses the cutting feed.",
                          identifier: "cnc.op.finishFeed")
                Divider()
                Eyebrow("How it cuts")
                Picker("Direction", selection: $cnc.setup.direction) {
                    ForEach(CNCCutDirection.allCases, id: \.self) { Text($0.label).tag($0) }
                }.pickerStyle(.segmented).labelsHidden().accessibilityLabel("Cut direction")
                Picker("Entry", selection: $cnc.setup.entry) {
                    ForEach(CNCEntry.allCases, id: \.self) { Text($0.label).tag($0) }
                }.labelsHidden().accessibilityLabel("How the cutter gets to depth")
                if cnc.setup.entry == .ramp {
                    CNCNumber(label: "Ramp angle", value: $cnc.setup.rampAngle, unit: "°",
                              identifier: "cnc.op.rampAngle")
                }
                Toggle("Tangential lead-in and out", isOn: $cnc.setup.leadIn)
                    .help("Arc onto the wall instead of stepping onto it, so the entry does not leave a witness mark.")
            }

            if kind == .helicalBore {
                Divider()
                Eyebrow("Bore")
                KeyValueRow("Diameter", "Ø \(cnc.setup.boreDiameter.formatted()) mm")
                KeyValueRow("Holes", "\(cnc.setup.bores.count)")
                KeyValueRow("Pitch per turn", "\(cnc.setup.stepdown.formatted()) mm")
            }

            if kind.isContour {
                Divider()
                Eyebrow("Holding tabs")
                Stepper(value: Binding(get: { cnc.setup.tabs }, set: { cnc.setTabCount($0) }), in: 0...12) {
                    KeyValueRow("Tabs", cnc.setup.tabs == 0 ? "none" : "\(cnc.setup.tabs)")
                }
                .accessibilityLabel("Tabs").accessibilityIdentifier("cnc.tabs.count")
                .accessibilityValue("\(cnc.setup.tabs)")
                if cnc.setup.tabs > 0 {
                    CNCNumber(label: "Metal left", value: $cnc.setup.tabWidth,
                              help: "Width means metal, not the run the cutter is lifted over.",
                              identifier: "cnc.tabs.width")
                    CNCNumber(label: "Tab height", value: $cnc.setup.tabHeight,
                              identifier: "cnc.tabs.height")
                    CNCTabSection(cnc: cnc)
                }
                if kind == .contourInside && cnc.setup.tabs == 0 {
                    Text("Nothing holds the slug on the last pass. Add tabs or leave a skin.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }

            Divider()
            Eyebrow("The floor")
            // Item 50: breaking through is only a choice when something under
            // the blank can take the cut. On the bare bed it is not offered at
            // all, and the reason names what is down there.
            let hasSpoilboard = cnc.underStock.thickness != nil
            Picker("Bottom", selection: Binding(
                get: { cnc.setup.bottomAllowance < 0 ? 1 : 0 },
                set: { cnc.setup.bottomAllowance = $0 == 1 ? -min(0.3, abs(cnc.setup.bottomAllowance)) : max(0, cnc.setup.bottomAllowance) })) {
                    Text("Leave a skin").tag(0)
                    if hasSpoilboard { Text("Break through").tag(1) }
                }.pickerStyle(.segmented).labelsHidden()
                .accessibilityLabel("What the cut does at the bottom")
                .accessibilityIdentifier("cnc.op.floor")
            CNCNumber(label: cnc.setup.bottomAllowance < 0 ? "Break through by" : "Skin left",
                      value: Binding(get: { abs(cnc.setup.bottomAllowance) },
                                     set: { cnc.setup.bottomAllowance = cnc.setup.bottomAllowance < 0 ? -abs($0) : abs($0) }),
                      identifier: "cnc.op.bottomAllowance")
            if !hasSpoilboard {
                Text("The blank is on the \(cnc.underStock.label.lowercased()), which is not sacrificial: a cut that goes past the underside would be cut into the machine. Declare a spoilboard on Stock to break through.")
                    .font(.caption)
                    .foregroundStyle(cnc.setup.bottomAllowance < 0 ? Color.orange : Color.secondary)
            }

            if kind.isContour {
                Divider()
                Eyebrow("Slots as narrow as the cutter")
                Picker("Thin slots", selection: $cnc.setup.thinSlot) {
                    ForEach(CNCThinSlot.allCases, id: \.self) { Text($0.label).tag($0) }
                }.labelsHidden().accessibilityLabel("What to do where the cutter is as wide as the opening")
                if cnc.setup.thinSlot == .centreLine {
                    CNCNumber(label: "Allowed wall error", value: $cnc.setup.thinSlotTolerance,
                              identifier: "cnc.op.thinSlotTolerance")
                    if let report = cnc.selectedOperation.contourReport, report.maxWallError > 0 {
                        KeyValueRow("Reported wall error", "\(CNCVerdictText.mm(report.maxWallError, 3)) mm")
                    }
                }
            }

            Divider()
            CNCFeedsSection(cnc: cnc)
            CNCNumber(label: "Cutting feed", value: $cnc.setup.feed, unit: "mm/min",
                      identifier: "cnc.op.feed")
            CNCNumber(label: "Plunge", value: $cnc.setup.plunge, unit: "mm/min",
                      identifier: "cnc.op.plunge")
            CNCNumber(label: "Spindle", value: $cnc.setup.rpm, unit: "RPM",
                      help: "On a router with a speed dial this number does nothing: set the dial by hand.",
                      identifier: "cnc.op.rpm")
            Button("Apply these feeds to all operations") { cnc.applyFeedsToAllOperations() }
                .disabled(cnc.operations.count < 2)
                .accessibilityIdentifier("cnc.feeds.applyToAll")

            if kind == .contourOutside {
                Divider()
                Toggle("Cut in list order, before the part is free", isOn: $cnc.setup.forceOrder)
                    .help("The profile that frees the part normally runs last whatever the list says.")
            }

            if let report = cnc.selectedOperation.contourReport {
                Divider()
                Eyebrow("What it generated")
                KeyValueRow("Depth cut", "\(CNCVerdictText.mm(report.finalDepth, 3)) mm")
                KeyValueRow("Passes", "\(report.roughPasses) rough · \(report.finishPasses) finish\(report.springPass ? " · spring" : "")")
                KeyValueRow("Entries", "\(report.rampEntries) ramp · \(report.leadEntries) lead · \(report.plungeEntries) plunge")
            }
            if let fit = cnc.selectedOperation.fitReport, !fit.fits || fit.unreachable.count > 0 {
                Divider()
                Eyebrow("Cutter fit")
                KeyValueRow("Corners it cannot reach", "\(fit.unreachable.count)")
                KeyValueRow("Metal left there", "\(CNCVerdictText.mm(fit.unreachable.totalArea, 1)) mm², up to \(CNCVerdictText.mm(fit.unreachable.maxStandoff)) mm proud")
                if let largest = fit.largestToolDiameter {
                    KeyValueRow("Largest tool that fits", "Ø \(CNCVerdictText.mm(largest)) mm")
                }
            }
            if cnc.selectedOperation.seconds > 0 {
                Divider()
                KeyValueRow("Time for this operation", CNCWorkspace.durationLabel(cnc.selectedOperation.seconds))
            }
        }
    }

    private var shapeSummary: String {
        let s = cnc.setup
        switch s.kind {
        case .face: return "\(cnc.stockWidth.formatted()) × \(cnc.stockHeight.formatted()) mm"
        case .helicalBore: return "\(s.bores.count) × Ø\(s.boreDiameter.formatted()) mm"
        case .drill: return "\(counted(s.bores.count, "hole")) drilled Ø\(s.boreDiameter.formatted()) mm"
        case .pocket, .contourInside, .contourOutside:
            return s.contour.isEmpty ? "the whole blank" : "\(counted(s.contour.count, "point")) closed loop"
        }
    }
}

// MARK: - Verification

/// Every check the oracle ran, with its number and its verdict. This is the
/// section the first real cut did not have: every defect that day was found by
/// a script outside the app, none of them by looking at the preview.
struct CNCVerificationSection: View {
    @Bindable var cnc: CNCWorkspace
    /// The stock, origin and tool panels are where a job is *set up*, and a
    /// full list of eight checks there pushed the settings off the bottom of
    /// the panel. They get the verdict and anything wrong with it; the
    /// check-by-check list stays on the operation it is about.
    var compact = false

    var body: some View {
        Eyebrow("Verification")
        if !cnc.jobCurrent {
            Text("Build the job to have it checked against the part.")
                .font(.caption).foregroundStyle(.secondary)
        } else {
            headline
            ForEach(cnc.blockers) { finding in
                findingRow(finding, tint: .red, symbol: "xmark.octagon.fill")
            }
            ForEach(cnc.warnings) { finding in
                warningRow(finding)
            }
            ForEach(compact ? [] : cnc.checkRows) { row in
                Button { select(row) } label: {
                    HStack(alignment: .firstTextBaseline, spacing: Theme.Space.s) {
                        Image(systemName: row.symbol).foregroundStyle(colour(row.verdict))
                            .font(.caption)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(row.title).font(.callout)
                            Text(row.value).font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 0)
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(row.detail)
                .accessibilityLabel("\(row.title), \(verdictWord(row.verdict)), \(row.value)")
            }
        }
    }

    @ViewBuilder private var headline: some View {
        let blocked = cnc.blockedByVerification || !cnc.blockers.isEmpty
        HStack(spacing: Theme.Space.s) {
            Image(systemName: blocked ? "xmark.octagon.fill"
                  : cnc.verified ? "checkmark.seal.fill" : "questionmark.circle")
                .foregroundStyle(blocked ? Color.red : cnc.verified ? Color.green : Color.secondary)
            Text(blocked ? "Refused — this job will not run"
                 : cnc.verified ? "Replayed against the part and clean"
                 : "Not verified")
                .font(.callout.weight(.medium))
        }
    }

    private func findingRow(_ finding: CNCFinding, tint: Color, symbol: String) -> some View {
        Button { cnc.select(finding) } label: {
            HStack(alignment: .firstTextBaseline, spacing: Theme.Space.s) {
                Image(systemName: symbol).foregroundStyle(tint).font(.caption)
                Text(finding.text).font(.caption).multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 0)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(finding.text)
    }

    /// A warning does not block, but it has to be acknowledged one at a time —
    /// a slug that drops free is not a thing to click past in a batch. The
    /// checkbox acknowledges; the text goes to the cut it is about.
    private func warningRow(_ finding: CNCFinding) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Theme.Space.s) {
            Toggle("", isOn: Binding(
                get: { cnc.acknowledgements.contains(finding.id) },
                set: { cnc.acknowledge(finding.id, on: $0) }))
                .toggleStyle(.checkbox).labelsHidden()
                .accessibilityLabel("Acknowledge: \(finding.text)")
            Button { cnc.select(finding) } label: {
                HStack(alignment: .firstTextBaseline, spacing: Theme.Space.xs) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(.orange).font(.caption)
                    Text(finding.text).font(.caption).multilineTextAlignment(.leading)
                        .fixedSize(horizontal: false, vertical: true)
                    Spacer(minLength: 0)
                }
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Show where: \(finding.text)")
        }
    }

    private func select(_ row: CNCCheckRow) {
        guard let check = cnc.verification?.check(named: row.id), !check.pass else { return }
        let finding = (cnc.blockers + cnc.warnings).first { $0.id == row.id }
        if let finding { cnc.select(finding) }
    }
    private func colour(_ verdict: CNCCheckRow.Verdict) -> Color {
        switch verdict {
        case .pass: return .green
        case .warning: return .orange
        case .blocked: return .red
        case .notRun: return .secondary
        }
    }
    private func verdictWord(_ verdict: CNCCheckRow.Verdict) -> String {
        switch verdict {
        case .pass: return "pass"
        case .warning: return "warning"
        case .blocked: return "blocked"
        case .notRun: return "not run"
        }
    }
}

// MARK: - The setup summary

/// What was generated, on the stage where the user is still setting up. Item
/// 23: the preview was reachable only after Generate, in another stage, and
/// Setup said nothing at all about what had been made.
struct CNCSetupSummary: View {
    @Bindable var cnc: CNCWorkspace
    /// On a setup panel the verdict is enough; the check-by-check list lives
    /// on the operation it is about, where there is room for it.
    var compact = true

    var body: some View {
        Eyebrow("This job")
        if !cnc.jobCurrent {
            Text(cnc.generating ? "Building and checking…"
                 : "\(counted(cnc.operations.count, "operation")) · not built yet")
                .font(.caption).foregroundStyle(.secondary)
        } else {
            KeyValueRow("Operations", "\(cnc.operations.count)")
            KeyValueRow("Moves", "\(cnc.jobMoves.count)")
            KeyValueRow("Time with acceleration", CNCWorkspace.durationLabel(cnc.jobDuration))
            if let v = cnc.verification {
                KeyValueRow("Deepest Z", "\(CNCVerdictText.mm(v.depth.deepestZ, 3)) mm")
            }
            // Tool checks are not listed twice: they arrive below as findings,
            // where they can also be acknowledged.
            CNCVerificationSection(cnc: cnc, compact: compact)
        }
    }
}
