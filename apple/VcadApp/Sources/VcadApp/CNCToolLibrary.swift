import Foundation

// The job's tool list, and which operation uses which tool.
//
// Friction-log item 19: *"one tool per job. There is no drill op and no tool
// change in the app, so a job needing both a Ø3.175 profile and Ø2.5 pilots is
// still two jobs. The kernel has the drill ops and multi-tool assembly; the app
// does not send them."*
//
// This is the app's half. The kernel already assembles a multi-tool program —
// it groups operations by tool inside each phase, stops the spindle, writes an
// `M0` and prompts for the next cutter. What was missing was somewhere for the
// operator to say a second tool exists, and something in the outline plan that
// decides a hole is *drilled* rather than milled.
//
// The machine this app drives has no changer, so a tool change is an operator
// stop and a re-probe — never an `M6`, which a changer-less controller ignores
// silently while carrying on with whatever is in the collet (item 57).

/// What a tool is, as far as this app decides anything from it.
///
/// Only two kinds, because only two change a decision here: an end mill mills
/// a contour, a drill sinks a hole on its centre. The kernel's library knows
/// ball, bull, V and face too, and a `kind` that is not one of these two would
/// be carried through untouched — but nothing in the app would plan with it,
/// so offering it would be a promise the planner does not keep.
enum CNCToolKind: String, Codable, Sendable, CaseIterable, Identifiable {
    case flatEndMill = "flat_end_mill"
    case drill

    var id: String { rawValue }
    var label: String { self == .drill ? "Drill" : "Flat end mill" }
    /// Whether this tool can walk a wall. A drill cuts on its point only.
    var mills: Bool { self == .flatEndMill }
}

/// One entry in the job's tool list.
struct CNCTool: Identifiable, Codable, Equatable, Sendable {
    var id = UUID()
    /// `T<number>`, as the program will say it and the operator will read it.
    var number: Int = 1
    var kind: CNCToolKind = .flatEndMill
    var diameter: Double = 3.175
    var flutes: Int = 2
    /// Usable cutting length. Zero means "not declared", and the job says so
    /// rather than assuming the flutes are long enough for the cut.
    var fluteLength: Double = 0
    /// How far the tool stands out of the collet. Zero means undeclared, and
    /// the holder-into-stock check cannot run.
    var stickout: Double = 0
    /// Whether the tool cuts across its own centre, so it can plunge. A drill
    /// always does; an end mill may not, and the job checks every entry.
    var centreCutting: Bool = true

    /// What the tool-change prompt calls it.
    var label: String {
        "T\(number) · Ø \(diameter.formatted(.number.precision(.fractionLength(0...3)))) mm \(kind.label.lowercased())"
    }
    /// The short form the operation rows use.
    var shortLabel: String { "T\(number) Ø \(diameter.formatted(.number.precision(.fractionLength(0...3))))" }

    /// Whether this drill is the one that makes a hole of `diameter`.
    ///
    /// A drill makes exactly its own size, so the match is an equality within
    /// a tolerance, not a "small enough". A Ø2.5 hole is not a job for a Ø2
    /// drill: it would leave 0.25 mm a side that nothing removes, and the app
    /// would have claimed the hole was made.
    func drills(_ holeDiameter: Double, tolerance: Double = 0.05) -> Bool {
        kind == .drill && abs(diameter - holeDiameter) <= tolerance
    }
}

extension CNCWorkspace {
    /// The tool the given number names, or nil.
    func tool(number: Int) -> CNCTool? { tools.first { $0.number == number } }

    /// The narrowest end mill in the list — the one that decides which holes
    /// can be milled at all. Item 49's derivation reads this.
    var smallestEndMill: CNCTool? {
        tools.filter { $0.kind.mills && $0.diameter.isFinite && $0.diameter > 0 }
            .min { $0.diameter < $1.diameter }
    }

    /// The drill in the list that makes a hole of this diameter, if there is
    /// one. Where a drill op comes from.
    func drill(for holeDiameter: Double) -> CNCTool? {
        tools.filter { $0.drills(holeDiameter) }
            // Two drills within tolerance of the same hole is an operator's
            // mistake, not ours to arbitrate: take the closest.
            .min { abs($0.diameter - holeDiameter) < abs($1.diameter - holeDiameter) }
    }

    /// A tool number that is not in use, for a newly added tool.
    var nextToolNumber: Int {
        let used = Set(tools.map(\.number))
        var candidate = 1
        while used.contains(candidate) { candidate += 1 }
        return candidate
    }

    /// The tools this job actually uses, in the order the kernel will run
    /// them. Built from the job's answer when there is one, because the kernel
    /// owns the ordering rule (phase, then tool groups within a phase) and a
    /// second copy of it here would be one more thing to fall out of step.
    var toolSequence: [Int] {
        if let sequence = job?.toolSequence, !sequence.isEmpty { return sequence }
        // Before the job is built, the honest answer is the set of tools the
        // operations name, in list order — not a guess at the kernel's sort.
        var seen: [Int] = []
        for operation in operations where !seen.contains(operation.setup.toolNumber) {
            seen.append(operation.setup.toolNumber)
        }
        return seen
    }

    /// How many times this job stops for the operator to change the cutter.
    /// One fewer than the tool sequence, because the first tool is fitted
    /// before the program starts.
    var toolChangeCount: Int { max(0, toolSequence.count - 1) }

    /// What the readiness list says about tool changes, or nil for a job that
    /// runs on one cutter.
    var toolChangeWarning: String? {
        guard toolChangeCount > 0 else { return nil }
        return "This job pauses \(counted(toolChangeCount, "time")) for tool changes — re-zero Z after each."
    }

    /// The tool sequence written the way the operator reads it: "T2 Ø 2.5 →
    /// T1 Ø 3.175".
    var toolSequenceLabel: String {
        toolSequence.map { number in tool(number: number)?.shortLabel ?? "T\(number)" }
            .joined(separator: " → ")
    }

    // MARK: - Persistence

    /// Where this document's tool list is kept. Per document, because a tool
    /// list belongs to the job rather than to the machine: the same bench cuts
    /// one part with a Ø1 and the next with a Ø6.
    private func toolsDefaultsKey() -> String {
        guard let key = documentKey?(), !key.isEmpty else { return "cnc.tools" }
        return "cnc.tools." + key
    }

    /// Read the tool list back for the document that is open, if one was
    /// stored. Silent when there is none — the default list is a perfectly
    /// good answer, and a decode failure is not worth an alert over a
    /// convenience.
    func loadTools() {
        guard let data = UserDefaults.standard.data(forKey: toolsDefaultsKey()),
              let stored = try? JSONDecoder().decode([CNCTool].self, from: data),
              !stored.isEmpty else { return }
        tools = stored.sorted { $0.number < $1.number }
        if tool(number: selectedToolNumber) == nil { selectedToolNumber = tools[0].number }
    }

    func saveTools() {
        guard let data = try? JSONEncoder().encode(tools) else { return }
        UserDefaults.standard.set(data, forKey: toolsDefaultsKey())
    }
}
