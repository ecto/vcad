import SwiftUI
import simd
import CVcadFFI
import UniformTypeIdentifiers

/// One operation's settings. Everything shared by the whole job — the stock,
/// the tool, what is under the blank — lives on the workspace instead, because
/// a job runs one tool over one blank and copying those into every operation is
/// how five operations ended up disagreeing about the stock thickness.
struct CNCSetup: Codable, Equatable, Sendable {
    var kind: CNCOpKind = .face
    var depth = 1.0
    var stepdown = 0.5
    var stepover = 1.5
    var feed = 400.0
    var plunge = 100.0
    var rpm = 10000.0
    var clearance = 5.0

    /// Closed polyline in the stock frame, for a contour or a shaped pocket.
    var contour: [[Double]] = []
    /// Bore centres for a helical-bore group, and the bore each one cuts.
    var bores: [[Double]] = []
    var boreDiameter = 0.0

    // Roughing and finishing.
    /// Metal the roughing passes leave on the wall for the finish pass to take.
    var stockToLeave = 0.0
    var finishStepdowns = 1
    var springPass = false
    /// Zero means "use the cutting feed".
    var finishFeed = 0.0
    var direction: CNCCutDirection = .climb
    var entry: CNCEntry = .ramp
    var rampAngle = 3.0
    var leadIn = true

    // Holding. Tabs are offered on inside cuts too: the stator's Ø27.6 bore
    // slug came free on the last pass next to a Ø3.175 cutter because they
    // were not (friction-log item 39).
    var tabs = 0
    var tabWidth = 4.0
    var tabHeight = 1.0

    // The floor. Positive leaves a skin, negative breaks through into whatever
    // is declared under the stock.
    var bottomAllowance = 0.0
    var thinSlot: CNCThinSlot = .refuse
    var thinSlotTolerance = 0.05

    /// Run this operation where the list puts it, even when its role would
    /// otherwise hold it back. Only the outside profile has a role that does.
    var forceOrder = false

    var isContour: Bool { kind.isContour }
    /// The feeds and step sizes "apply to all" copies; geometry stays put.
    var cutting: CNCCuttingValues {
        .init(stepdown: stepdown, stepover: stepover, feed: feed, plunge: plunge,
              rpm: rpm, clearance: clearance, stockToLeave: stockToLeave,
              finishStepdowns: finishStepdowns, springPass: springPass,
              finishFeed: finishFeed, direction: direction, entry: entry,
              rampAngle: rampAngle, leadIn: leadIn)
    }
    mutating func apply(_ v: CNCCuttingValues) {
        stepdown = v.stepdown; stepover = v.stepover; feed = v.feed; plunge = v.plunge
        rpm = v.rpm; clearance = v.clearance; stockToLeave = v.stockToLeave
        finishStepdowns = v.finishStepdowns; springPass = v.springPass
        finishFeed = v.finishFeed; direction = v.direction; entry = v.entry
        rampAngle = v.rampAngle; leadIn = v.leadIn
    }
}

/// The part of a setup that "Apply these feeds to all operations" copies.
struct CNCCuttingValues: Equatable, Sendable {
    var stepdown: Double, stepover: Double, feed: Double, plunge: Double
    var rpm: Double, clearance: Double, stockToLeave: Double
    var finishStepdowns: Int, springPass: Bool, finishFeed: Double
    var direction: CNCCutDirection, entry: CNCEntry, rampAngle: Double, leadIn: Bool
}

struct CNCMove: Decodable, Sendable {
    var to: [Double]
    var rapid: Bool
    var feed: Double?
    var point: SIMD3<Float> { .init(Float(to[0]), Float(to[1]), Float(to[2])) }
}
struct CNCProgram: Decodable, Sendable {
    var gcode: String
    var moves: [CNCMove]
}

enum CNCMode: String, CaseIterable, Identifiable {
    case setup = "Setup", toolpaths = "Toolpaths", machine = "Machine"
    var id: String { rawValue }
}
enum CNCSelection: Hashable { case stock, origin, tool, operation(UUID) }

/// Where an operation came from, so that changing the tool can re-decide which
/// holes are machinable without throwing away the settings on the ones that
/// still are (friction-log item 49).
enum CNCOpSource: Hashable, Codable, Sendable {
    case outer
    case opening(hole: Int)
    /// All the circular holes of one diameter, bored in one operation.
    case pilots(key: Int)
    case manual
}

struct CNCOperation: Identifiable, Equatable {
    var id = UUID()
    var setup: CNCSetup
    var source: CNCOpSource = .manual
    /// The name this operation got from the geometry it came from.
    var label: String
    /// Move ranges into the job's `moves`, filled in by a build.
    var ranges: [CNCOpRange] = []
    var seconds = 0.0
    var preview = CNCPreview()
    var contourReport: CNCContourReport?
    var fitReport: CNCFitReport?

    var name: String { label }
    var symbol: String {
        switch setup.kind {
        case .pocket: return "square.dashed.inset.filled"
        case .contourOutside: return "circle.dashed"
        case .contourInside: return "circle.dashed.inset.filled"
        case .helicalBore: return "smallcircle.filled.circle"
        case .face: return "square.3.layers.3d"
        }
    }
    static func == (a: Self, b: Self) -> Bool {
        a.id == b.id && a.setup == b.setup && a.source == b.source && a.label == b.label
    }
}

/// Everything a job is built from. Two jobs with the same key are the same
/// job — which is how "needs rebuilding" and "these acknowledgements are
/// stale" are both answered without bookkeeping.
struct CNCJobKey: Equatable, Sendable {
    var setups: [CNCSetup]
    var sources: [CNCOpSource]
    var thickness: Double
    var width: Double
    var height: Double
    var margin: Double
    var spoilboard: Double?
    var diameter: Double
    var outer: [[Double]]
    var holes: [[[Double]]]
}

/// Timed interpolation of the generated path, independent of controller state.
struct CNCPreview {
    struct Segment { var from: SIMD3<Float>; var to: SIMD3<Float>; var start: Double; var end: Double }
    var segments: [Segment] = []
    var duration: Double = 0
    var first: SIMD3<Float>?
    init() {}
    init(moves: [CNCMove], fallbackFeed: Double) {
        first = moves.first?.point
        for pair in zip(moves, moves.dropFirst()) {
            let distance = Double(simd_distance(pair.0.point, pair.1.point))
            let feed = pair.1.rapid ? 3000 : (pair.1.feed ?? fallbackFeed)
            guard distance.isFinite, distance > 0, feed.isFinite, feed > 0 else { continue }
            let end = duration + distance / feed * 60
            segments.append(.init(from: pair.0.point, to: pair.1.point, start: duration, end: end))
            duration = end
        }
    }
    init(program: CNCProgram, fallbackFeed: Double) {
        self.init(moves: program.moves, fallbackFeed: fallbackFeed)
    }
    func position(at fraction: Double) -> SIMD3<Float>? {
        guard let last = segments.last else { return first }
        let time = max(0, min(1, fraction.isFinite ? fraction : 0)) * duration
        var lo = 0, hi = segments.count - 1
        while lo < hi {
            let mid = (lo + hi) / 2
            if segments[mid].end < time { lo = mid + 1 } else { hi = mid }
        }
        let segment = segments[lo]
        if time >= duration { return last.to }
        let t = Float(max(0, min(1, (time - segment.start) / (segment.end - segment.start))))
        return segment.from + (segment.to - segment.from) * t
    }
}

@MainActor @Observable
final class CNCWorkspace {
    let machine = CNCController()
    var inspectorTab: CNCInspectorTab = .terminal
    var followSpindle = false
    var autoFit = true
    private(set) var importedProgram: CNCProgram?
    private(set) var importedName = ""
    private(set) var usesImportedProgram = false
    private(set) var importedVerification: CNCJobResult?
    /// Why an imported program could not be replayed, when it could not be.
    private(set) var importedVerifyError: String?
    private var importedPreview = CNCPreview()
    var macros: [CNCMacro] = []
    /// Panel visibility is remembered by the editor's layout memory.
    var onLayoutChange: (() -> Void)?
    var leftPanelShown = true { didSet { onLayoutChange?() } }
    var rightPanelShown = true { didSet { onLayoutChange?() } }
    var bottomPanelShown = false { didSet { onLayoutChange?() } }
    var shown = false { didSet { if !shown { pausePreview() } } }
    var mode: CNCMode = .setup {
        didSet {
            if mode != .toolpaths { pausePreview() }
            if mode == .toolpaths {
                if case .operation = selection {} else { selection = .operation(selectedOperationID) }
            } else if mode == .setup, case .operation = selection {
                selection = .stock
            }
            previewTick += 1
        }
    }
    var selection: CNCSelection {
        didSet {
            if case .operation(let id) = selection, operations.contains(where: { $0.id == id }) {
                selectedOperationID = id
                pausePreview(); previewFraction = 0; revision += 1
            }
        }
    }
    private(set) var operations: [CNCOperation]
    private var selectedOperationID: UUID
    var overlay = true
    var showStock = true
    var showClearance = false
    var showPart = true
    var showEnvelope = true
    var origin = CNCVector() { didSet { setupConfirmed = false } }

    // MARK: stock and tool, one per job

    var stockThickness = 10.0 {
        didSet {
            setupConfirmed = false
            // A contour imported as a through cut stays one. Without this, an
            // outline imported before the thickness was entered kept cutting
            // to the old depth and the job just read as blocked.
            for i in operations.indices where operations[i].setup.depth == oldValue {
                operations[i].setup.depth = stockThickness
                operations[i].setup.tabHeight = min(operations[i].setup.tabHeight, stockThickness / 2)
            }
        }
    }
    var stockWidth = 40.0 { didSet { setupConfirmed = false; revision += 1 } }
    var stockHeight = 30.0 { didSet { setupConfirmed = false; revision += 1 } }
    /// How far the blank stands proud of the part. `nil` follows the cutter.
    var stockMargin: Double? { didSet { setupConfirmed = false; revision += 1 } }
    /// What the blank is sitting on. A bare bed refuses a break-through.
    var underStock: CNCUnderStock = .machineBed { didSet { setupConfirmed = false; revision += 1 } }
    var toolDiameter = 3.175 {
        didSet {
            guard toolDiameter != oldValue else { return }
            setupConfirmed = false; revision += 1
            // Item 49: changing the cutter re-decides which holes can be
            // machined, without a re-import and without losing settings.
            reconcileOutlineOperations()
        }
    }
    var toolFlutes = 2 { didSet { setupConfirmed = false; revision += 1 } }
    /// Usable cutting length. Zero means "not declared", and the job says so
    /// rather than assuming the flutes are long enough for the cut.
    var toolFluteLength = 0.0 { didSet { setupConfirmed = false; revision += 1 } }
    /// How far the tool stands out of the holder. Same rule: undeclared means
    /// the holder-into-stock check cannot run, and the job warns.
    var toolStickout = 0.0 { didSet { setupConfirmed = false; revision += 1 } }

    /// The margin the blank needs when the user has not set one: enough for the
    /// cutter to run right around the part and still stand on material.
    var automaticMargin: Double { max(2 * toolDiameter + 2, 5) }
    var effectiveMargin: Double { stockMargin ?? automaticMargin }

    var jogStep = 1.0
    var jogFeed = 300.0
    var setupConfirmed = false
    private(set) var generating = false
    private(set) var revision = 0
    var error: String?
    /// Where in the stock frame a selected violation sits, for the viewport.
    private(set) var markedXY: [Double]?
    private(set) var markedZ: Double?
    var previewFraction = 0.0 { didSet { previewTick += 1 } }
    var previewSpeed = 5.0
    private(set) var previewTick = 0
    private(set) var previewPlaying = false
    private var previewTask: Task<Void, Never>?

    // MARK: the built job

    private(set) var job: CNCJobResult?
    private(set) var builtKey: CNCJobKey?
    private var acknowledgedKey: CNCJobKey?
    private var acknowledgedIDs: Set<String> = []

    init() {
        let initial = CNCOperation(setup: CNCSetup(), source: .manual, label: "Face stock")
        operations = [initial]; selectedOperationID = initial.id
        selection = .stock
        if let data = UserDefaults.standard.data(forKey: "cnc.console.macros"),
           let stored = try? JSONDecoder().decode([CNCMacro].self, from: data) { macros = stored }
    }

    private var index: Int { operations.firstIndex(where: { $0.id == selectedOperationID }) ?? 0 }
    var selectedOperation: CNCOperation { operations[index] }
    var setup: CNCSetup {
        get { operations[index].setup }
        set {
            guard !machine.active, newValue != operations[index].setup else { return }
            operations[index].setup = newValue; setupConfirmed = false; revision += 1
        }
    }

    // MARK: - What the job is built from

    var currentKey: CNCJobKey {
        CNCJobKey(setups: operations.map(\.setup),
                  sources: operations.map(\.source),
                  thickness: stockThickness,
                  width: stockWidth, height: stockHeight,
                  margin: effectiveMargin,
                  spoilboard: underStock.thickness,
                  diameter: toolDiameter,
                  outer: outline?.outer.points.map { [$0.x, $0.y] } ?? [],
                  holes: outline?.holes.map { $0.points.map { [$0.x, $0.y] } } ?? [])
    }
    var jobCurrent: Bool {
        if usesImportedProgram { return importedProgram != nil }
        return job != nil && builtKey == currentKey
    }
    var current: Bool { jobCurrent }

    /// Acknowledged warnings, which go stale the instant anything changes.
    var acknowledgements: Set<String> { acknowledgedKey == currentKey ? acknowledgedIDs : [] }
    func acknowledge(_ id: String, on: Bool) {
        if acknowledgedKey != currentKey { acknowledgedKey = currentKey; acknowledgedIDs = [] }
        if on { acknowledgedIDs.insert(id) } else { acknowledgedIDs.remove(id) }
        revision += 1
    }

    // MARK: - Reading the answer

    var verification: CNCVerification? {
        usesImportedProgram ? importedVerification?.verification : job?.verification
    }
    var policy: CNCJobPolicy? {
        usesImportedProgram ? importedVerification?.policy : job?.policy
    }
    /// True when this job has been replayed against the part it should make.
    var verified: Bool { jobCurrent && (policy?.verified ?? false) }
    var blockedByVerification: Bool {
        guard jobCurrent else { return false }
        if usesImportedProgram { return importedVerification?.isBlocked ?? false }
        return job?.isBlocked ?? false
    }
    var jobMoves: [CNCMove] {
        usesImportedProgram ? (importedProgram?.moves ?? []) : (job?.moves ?? [])
    }
    var jobDuration: Double {
        if usesImportedProgram { return importedPreview.duration }
        return job?.duration.accelAwareS ?? 0
    }
    var jobNotes: [CNCJobNote] { job?.notes ?? [] }

    /// Every check with its verdict, for the inspector's Verification section.
    var checkRows: [CNCCheckRow] {
        guard jobCurrent else { return [] }
        guard let v = verification else {
            return [CNCCheckRow(id: "unverified", title: "Verification",
                                verdict: .notRun, value: "not run",
                                detail: unverifiedReason)]
        }
        let blockedBy = Set(policy?.blockedBy ?? [])
        return v.checks.map { check in
            CNCCheckRow(id: check.name,
                        title: CNCVerdictText.title(of: check.name),
                        verdict: check.pass ? .pass : blockedBy.contains(check.name) ? .blocked : .warning,
                        value: CNCVerdictText.value(of: check, in: v),
                        detail: check.pass ? check.note
                            : CNCVerdictText.explain(check, in: v, under: underStock,
                                                     who: operationName(near: check.worstExample)))
        }
    }

    private var unverifiedReason: String {
        if usesImportedProgram {
            if let importedVerifyError {
                return "This program could not be replayed against the outline: \(importedVerifyError)"
            }
            return "This program was not generated here and there is no outline to replay it against. Nothing has checked that it makes the part."
        }
        return "There is no part outline to replay this job against. Import one to have the job checked before it runs."
    }

    /// Reasons this job will not run, each one a sentence about a place.
    var blockers: [CNCFinding] { findings(blocking: true) }
    /// Things worth knowing that do not block, but must be acknowledged.
    var warnings: [CNCFinding] { findings(blocking: false) }
    var unacknowledgedWarnings: [CNCFinding] {
        let done = acknowledgements
        return warnings.filter { !done.contains($0.id) }
    }

    private func findings(blocking: Bool) -> [CNCFinding] {
        guard jobCurrent else { return [] }
        var out: [CNCFinding] = []
        let result = usesImportedProgram ? importedVerification : job
        // A refusal that never reached the oracle — an impossible request, or a
        // tool that cannot make the cut — is still a reason, and still blocks.
        if blocking, let message = result?.error {
            out.append(CNCFinding(id: "request", text: message, xy: nil, z: nil,
                                  operationID: nil, blocking: true))
        }
        if blocking {
            for check in result?.toolChecks ?? [] where check.severity == "error" {
                out.append(CNCFinding(id: "tool-\(check.opIndex)", text: check.message,
                                      xy: nil, z: nil,
                                      operationID: operation(forRequest: check.opIndex),
                                      blocking: true))
            }
        }
        if let v = verification, let p = policy {
            for name in blocking ? p.blockedBy : p.warnings {
                guard let check = v.check(named: name) else { continue }
                let worst = check.worstExample
                out.append(CNCFinding(
                    id: name,
                    text: CNCVerdictText.explain(check, in: v, under: underStock,
                                                 who: operationName(near: worst)),
                    xy: worst?.xy, z: worst?.z,
                    operationID: operation(near: worst),
                    blocking: blocking))
            }
        }
        // Not being verified at all is not a blocker — plenty of real work has
        // no outline — but it must never pass silently as if it had been.
        if !blocking, !(policy?.verified ?? false) {
            out.append(CNCFinding(id: "unverified", text: unverifiedReason,
                                  xy: nil, z: nil, operationID: nil, blocking: false))
        }
        if !blocking, let checks = result?.toolChecks {
            for check in checks where check.severity == "warning" {
                out.append(CNCFinding(id: "tool-warning-\(check.opIndex)", text: check.message,
                                      xy: nil, z: nil,
                                      operationID: operation(forRequest: check.opIndex),
                                      blocking: false))
            }
        }
        return out
    }

    /// Which app operation a request index belongs to.
    private func operation(forRequest index: Int) -> UUID? {
        operations.first { $0.ranges.contains { $0.opIndex == index } }?.id
    }

    /// The operation whose moves pass nearest a violation. The oracle reports a
    /// place, not an operation — but a place inside one operation's sweep is
    /// that operation's problem, and selecting it is how the user gets to it.
    func operation(near violation: CNCViolation?) -> UUID? {
        guard let violation, violation.xy.count >= 2 else { return nil }
        let target = SIMD2<Double>(violation.xy[0], violation.xy[1])
        let moves = jobMoves
        var best: (id: UUID, distance: Double)?
        for op in operations {
            var nearest = Double.greatestFiniteMagnitude
            for range in op.ranges {
                guard range.start < range.end, range.end <= moves.count else { continue }
                for move in moves[range.start..<range.end] where move.to.count >= 2 {
                    let d = simd_distance(target, SIMD2<Double>(move.to[0], move.to[1]))
                    if d < nearest { nearest = d }
                }
            }
            if best == nil || nearest < best!.distance { best = (op.id, nearest) }
        }
        return best?.id
    }
    private func operationName(near violation: CNCViolation?) -> String? {
        guard let id = operation(near: violation) else { return nil }
        return operations.first { $0.id == id }?.name
    }

    /// Select the operation a finding lands in and mark the place in the
    /// viewport, so "which cut is that?" is one click.
    func select(_ finding: CNCFinding) {
        if let id = finding.operationID { select(.operation(id)) }
        markedXY = finding.xy; markedZ = finding.z
        revision += 1
    }
    func clearMark() { markedXY = nil; markedZ = nil; revision += 1 }

    // MARK: - Preview and display

    var program: CNCProgram? {
        if usesImportedProgram { return importedProgram }
        guard let job else { return nil }
        return CNCProgram(gcode: job.gcode ?? "", moves: job.moves)
    }
    var preview: CNCPreview { usesImportedProgram ? importedPreview : selectedOperation.preview }
    var previewTitle: String { usesImportedProgram ? importedName : selectedOperation.name }
    var jobTitle: String { usesImportedProgram ? importedName : "Manufacturing job" }
    /// What the viewport draws: the selected operation alone while its
    /// toolpath is being looked at, the whole job everywhere else.
    var displayMoves: [[CNCMove]] {
        if usesImportedProgram { return importedProgram.map { [$0.moves] } ?? [] }
        guard let job else { return [] }
        if mode == .machine { return [job.moves] }
        let slices = selectedOperation.ranges.compactMap { range -> [CNCMove]? in
            guard range.start < range.end, range.end <= job.moves.count else { return nil }
            return Array(job.moves[range.start..<range.end])
        }
        return slices.isEmpty ? [job.moves] : slices
    }
    var previewPosition: SIMD3<Float>? { preview.position(at: previewFraction) }
    var displayKey: String {
        "\(mode.rawValue)-\(shown)-\(showStock)-\(showClearance)-\(showPart)-\(showEnvelope)-\(stockWidth)-\(stockHeight)-\(effectiveMargin)-\(setup.clearance)"
    }

    // MARK: - The gate

    var runBlocker: String? {
        if generating { return "Wait for the job to be built and checked." }
        if !jobCurrent { return "Build the job after editing the setup." }
        if let first = blockers.first {
            return blockers.count == 1 ? first.text
                : "\(first.text) (\(blockers.count - 1) more)"
        }
        if jobCode == nil { return "This job produced no G-code." }
        let pending = unacknowledgedWarnings
        if let first = pending.first {
            return pending.count == 1 ? "Acknowledge: \(first.text)"
                : "Acknowledge \(counted(pending.count, "warning")), starting with: \(first.text)"
        }
        if !machine.connected { return "Connect the Anolex or choose Simulator." }
        if machine.faulted { return machine.error ?? "Reconnect after resolving the controller fault." }
        if !machine.status.isFresh { return "Waiting for fresh controller telemetry." }
        if !machine.g54Active { return "Select G54 work coordinates on the controller." }
        if !machine.canStart { return "Waiting for Idle and a known work position." }
        if !setupConfirmed { return "Confirm the tool, workholding, clearance and G54 zero." }
        return nil
    }

    /// The program, or nothing at all.
    ///
    /// A job the oracle refused has no `gcode` key in the first place, so there
    /// is nothing here to export or send — which is the point of the whole
    /// pipeline, not a UI state that could be worked around.
    var jobCode: String? {
        guard jobCurrent else { return nil }
        if usesImportedProgram {
            if importedVerification?.isBlocked == true { return nil }
            return importedProgram?.gcode
        }
        return job?.gcode
    }
    func startJob() {
        guard runBlocker == nil, let code = jobCode else { return }
        pausePreview(); machine.start(code)
    }

    func select(_ selection: CNCSelection) {
        guard !machine.active else { return }
        useGeneratedJob()
        rightPanelShown = true           // the inspector lives in the right rail
        self.selection = selection
        switch selection { case .operation: mode = .toolpaths; default: mode = .setup }
    }

    // MARK: - Operations

    func addOperation(_ kind: CNCOpKind) {
        guard !machine.active, !generating else { return }
        var spec = setup; spec.kind = kind
        if !kind.isContour && kind != .pocket { spec.contour = [] }
        if kind != .helicalBore { spec.bores = []; spec.boreDiameter = 0 }
        if !kind.isContour { spec.tabs = 0 }
        let operation = CNCOperation(setup: spec, source: .manual, label: Self.manualLabel(kind))
        operations.append(operation); select(.operation(operation.id)); setupConfirmed = false
    }
    static func manualLabel(_ kind: CNCOpKind) -> String {
        switch kind {
        case .face: return "Face stock"
        case .pocket: return "Pocket"
        case .contourOutside: return "Outside profile"
        case .contourInside: return "Inside profile"
        case .helicalBore: return "Bore"
        }
    }
    func removeSelectedOperation() {
        guard !machine.active, !generating, operations.count > 1 else { return }
        operations.remove(at: index)
        select(.operation(operations[0].id)); setupConfirmed = false; revision += 1
    }
    func moveSelectedOperation(by offset: Int) {
        guard !machine.active, !generating, operations.indices.contains(index + offset) else { return }
        operations.swapAt(index, index + offset); setupConfirmed = false; revision += 1
    }
    func canMoveSelectedOperation(by offset: Int) -> Bool {
        !machine.active && !generating && operations.indices.contains(index + offset)
    }
    /// Item 52: with five operations there was no way to change material
    /// without editing each one, or re-importing the outline to copy them.
    func applyFeedsToAllOperations() {
        guard !machine.active, !generating else { return }
        let values = setup.cutting
        for i in operations.indices { operations[i].setup.apply(values) }
        setupConfirmed = false; revision += 1
    }

    // MARK: - The outline

    private(set) var outline: CNCOutline?

    /// Circular holes too small for the installed cutter. Derived, never
    /// stored: change the tool and this answer changes with it.
    var unmachinableHoles: [(index: Int, diameter: Double)] {
        guard let outline else { return [] }
        return outline.circularHoles
            .filter { $0.diameter <= toolDiameter + 1e-9 }
            .map { (index: $0.index, diameter: $0.diameter) }
    }

    func importOutlineFile() {
        guard !machine.active, !generating else { return }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "dxf") ?? .plainText, .plainText]
        panel.message = "Choose a DXF outline (closed polylines, millimetres)"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= 8_000_000 else { throw CNCError.message("Choose a DXF smaller than 8 MB.") }
            try importOutline(CNCOutline.parseDXF(String(contentsOf: url, encoding: .utf8), name: url.lastPathComponent))
        } catch { self.error = error.localizedDescription }
    }

    /// Replace the operations with the cuts this outline implies, in the order
    /// they have to run: small holes, then openings, then the profile that
    /// frees the part.
    func importOutline(_ outline: CNCOutline) throws {
        guard !machine.active, !generating else { return }
        guard outline.width <= 1000, outline.height <= 1000 else {
            throw CNCError.message("Outline exceeds the 1 m machining region.")
        }
        useGeneratedJob()
        self.outline = outline
        job = nil; builtKey = nil
        stockWidth = (outline.width * 10).rounded(.up) / 10
        stockHeight = (outline.height * 10).rounded(.up) / 10
        // Item 47: the path is drawn in the stock frame, so the stock frame has
        // to be told where it sits in the model.
        origin = CNCVector(x: Double(outline.origin.x), y: Double(outline.origin.y), z: origin.z)
        operations = plan(from: outline, inheriting: setup.cutting)
        select(.operation(operations[0].id))
        setupConfirmed = false; revision += 1
        error = unmachinableMessage
    }

    private var unmachinableMessage: String? {
        let small = unmachinableHoles
        guard !small.isEmpty else { return nil }
        let sizes = Set(small.map { (($0.diameter * 100).rounded() / 100).formatted() }).sorted()
        return "\(counted(small.count, "hole")) (Ø \(sizes.joined(separator: ", ")) mm) cannot be machined with the Ø \(toolDiameter.formatted()) mm cutter. Fit a smaller one, or drill them separately."
    }

    /// The operations an outline asks for with the cutter that is installed.
    private func plan(from outline: CNCOutline, inheriting values: CNCCuttingValues) -> [CNCOperation] {
        var base = CNCSetup()
        base.apply(values)
        base.depth = stockThickness
        // A pass cannot step over further than the cutter's radius without
        // moving through metal no earlier pass reached, so the default follows
        // the tool rather than a number left over from another job.
        base.stepover = min(base.stepover, 0.45 * toolDiameter)
        var ops: [CNCOperation] = []

        // Circles wider than the cutter but narrower than two of it are bored
        // helically: one operation per diameter, however many holes share it.
        let circles = outline.circularHoles
        var bored = Set<Int>()
        var groups: [Int: [(index: Int, centre: CGPoint, diameter: Double)]] = [:]
        for c in circles where c.diameter > toolDiameter + 1e-9 && c.diameter < 2 * toolDiameter {
            groups[Int((c.diameter * 100).rounded()), default: []].append(c)
        }
        for key in groups.keys.sorted() {
            let group = groups[key]!
            var spec = base
            spec.kind = .helicalBore
            spec.bores = group.map { [Double($0.centre.x), Double($0.centre.y)] }
            spec.boreDiameter = group.map(\.diameter).reduce(0, +) / Double(group.count)
            group.forEach { bored.insert($0.index) }
            ops.append(CNCOperation(setup: spec, source: .pilots(key: key),
                                    label: Self.pilotLabel(diameter: spec.boreDiameter, count: group.count)))
        }

        // Everything else inside the part is an opening, cut out by default.
        let tooSmall = Set(unmachinableHoles.map(\.index))
        for (i, hole) in outline.holes.enumerated() where !bored.contains(i) && !tooSmall.contains(i) {
            var spec = base
            spec.kind = .contourInside
            spec.contour = hole.points.map { [Double($0.x), Double($0.y)] }
            ops.append(CNCOperation(setup: spec, source: .opening(hole: i),
                                    label: Self.openingLabel(hole)))
        }

        var outer = base
        outer.kind = .contourOutside
        outer.contour = outline.outer.points.map { [Double($0.x), Double($0.y)] }
        outer.tabs = 3; outer.tabWidth = 4; outer.tabHeight = min(1, stockThickness / 2)
        ops.append(CNCOperation(setup: outer, source: .outer, label: "Outside profile"))
        return ops
    }

    /// Re-decide which holes the installed cutter can machine, keeping the
    /// settings on everything that survives and the order the user chose.
    private func reconcileOutlineOperations() {
        guard let outline, !machine.active, !generating else { return }
        let wanted = plan(from: outline, inheriting: setup.cutting)
        let wantedSources = wanted.map(\.source)
        var kept = operations.filter { $0.source == .manual || wantedSources.contains($0.source) }
        // A pilot group whose diameter is now machinable comes back where the
        // plan puts it; one that is not simply leaves.
        for (position, candidate) in wanted.enumerated() where !kept.contains(where: { $0.source == candidate.source }) {
            kept.insert(candidate, at: min(position, kept.count))
        }
        // Bore geometry follows the outline, not the stale copy.
        for i in kept.indices {
            guard let fresh = wanted.first(where: { $0.source == kept[i].source }) else { continue }
            kept[i].setup.bores = fresh.setup.bores
            kept[i].setup.boreDiameter = fresh.setup.boreDiameter
            kept[i].setup.contour = fresh.setup.contour
            kept[i].label = fresh.label
        }
        guard !kept.isEmpty else { return }
        operations = kept
        if !operations.contains(where: { $0.id == selectedOperationID }) {
            selectedOperationID = operations[0].id
            selection = .operation(operations[0].id)
        }
        error = unmachinableMessage
        job = nil; builtKey = nil; revision += 1
    }

    static func pilotLabel(diameter: Double, count: Int) -> String {
        let size = ((diameter * 100).rounded() / 100).formatted()
        return count == 1 ? "Pilot Ø\(size)" : "Pilot Ø\(size) × \(count)"
    }
    static func openingLabel(_ hole: CNCLoop) -> String {
        if let circle = hole.circle {
            return "Bore Ø\(((circle.diameter * 10).rounded() / 10).formatted())"
        }
        let b = hole.bounds
        let w = (Double(b.width) * 10).rounded() / 10, h = (Double(b.height) * 10).rounded() / 10
        return "Opening \(w.formatted()) × \(h.formatted())"
    }

    // MARK: - Building the job

    /// Kept under its old name because the machine bar's readiness popover
    /// calls it; `all` no longer means anything, since a job is one request.
    func generate(all: Bool = false) { build() }

    func build() {
        guard !generating, !machine.active else { return }
        guard let request = makeRequest() else { return }
        let key = currentKey
        generating = true; error = nil; setupConfirmed = false; pausePreview(); clearMark()
        Task {
            defer { generating = false }
            do {
                let result = try await Task.detached { try CNCJobBridge.run(request) }.value
                apply(result, key: key)
            } catch {
                self.error = error.localizedDescription
                self.job = nil; self.builtKey = nil; self.revision += 1
            }
        }
    }

    private func apply(_ result: CNCJobResult, key: CNCJobKey) {
        job = result; builtKey = key
        // The kernel's own words when it could not build the job at all.
        error = result.error
        var requestIndex = 0
        for i in operations.indices {
            let count = requestCount(of: operations[i])
            let indices = Array(requestIndex..<(requestIndex + count))
            requestIndex += count
            let ranges = result.opRanges.filter { $0.opIndex.map(indices.contains) ?? false }
            operations[i].ranges = ranges
            operations[i].seconds = ranges.reduce(0) { $0 + $1.seconds }
            let moves = ranges.flatMap { range -> [CNCMove] in
                guard range.start < range.end, range.end <= result.moves.count else { return [] }
                return Array(result.moves[range.start..<range.end])
            }
            operations[i].preview = CNCPreview(moves: moves, fallbackFeed: operations[i].setup.feed)
            operations[i].contourReport = result.report.first { indices.contains($0.opIndex) }?.report
            operations[i].fitReport = result.fit.first { indices.contains($0.opIndex) }?.report
        }
        previewFraction = 0; revision += 1
    }

    /// How many kernel operations one listed operation becomes. A pilot group
    /// is one row in the list and one helical bore per hole in the request.
    private func requestCount(of operation: CNCOperation) -> Int {
        operation.setup.kind == .helicalBore ? max(1, operation.setup.bores.count) : 1
    }

    /// The whole job as one request.
    func makeRequest() -> CNCJobRequest? {
        guard stockThickness.isFinite, stockThickness > 0, stockThickness < 1000 else {
            error = "Enter a stock thickness between 0 and 1000 mm."; return nil
        }
        guard toolDiameter.isFinite, toolDiameter > 0, toolDiameter < 100 else {
            error = "Enter a tool diameter between 0 and 100 mm."; return nil
        }
        var requests: [CNCJobOperationRequest] = []
        for (position, operation) in operations.enumerated() {
            requests.append(contentsOf: requestBodies(for: operation, at: position))
        }
        guard !requests.isEmpty else { error = "Add an operation before building the job."; return nil }

        var options = CNCJobOptionsRequest()
        // One job, one safe height: the tallest clearance any operation asks
        // for, so a travel move never crosses at another operation's lower one.
        let clearance = operations.map(\.setup.clearance).filter { $0.isFinite && $0 > 0 }.max() ?? 5
        options.safeZ = max(1, clearance)
        options.parkZ = max(1, clearance)
        if let outline {
            options.part = CNCJobPartRequest(
                outer: outline.outer.points.map { [Double($0.x), Double($0.y)] },
                holes: outline.holes.map { $0.points.map { [Double($0.x), Double($0.y)] } })
        } else {
            // Nothing says where the part ends, so there is nothing to replay
            // the job against. Saying so is the honest answer; pretending the
            // job was checked is the one that destroys a part.
            options.verify = false
        }
        return CNCJobRequest(
            name: outline?.name ?? "vcad job",
            stock: CNCJobStockRequest(thickness: stockThickness,
                                      margin: effectiveMargin,
                                      spoilboard: underStock.thickness),
            tools: [CNCJobToolRequest(diameter: toolDiameter, flutes: toolFlutes,
                                     fluteLength: toolFluteLength > 0 ? toolFluteLength : nil,
                                     stickout: toolStickout > 0 ? toolStickout : nil)],
            operations: requests,
            options: options)
    }

    private func requestBodies(for operation: CNCOperation, at position: Int) -> [CNCJobOperationRequest] {
        let s = operation.setup
        func common(_ kind: CNCOpKind, name: String) -> CNCJobOperationRequest {
            var r = CNCJobOperationRequest(name: name, kind: kind,
                                           depth: s.depth, stepdown: s.stepdown,
                                           feed: s.feed, plunge: s.plunge, rpm: s.rpm)
            r.stepover = kind == .face || kind == .pocket ? s.stepover : nil
            r.bottomAllowance = s.bottomAllowance
            r.order = position
            // The outside profile runs last whatever the list says, because
            // after it the part is held by tabs at best. Forcing it means
            // saying so out loud: the operation is asked for in the phase its
            // position implies. (An explicit phase override in the FFI would
            // say this without borrowing another role's name.)
            if s.forceOrder && kind == .contourOutside { r.role = "inside_feature" }
            return r
        }
        switch s.kind {
        case .face:
            var r = common(.face, name: operation.name)
            r.rectangle = [0, 0, stockWidth, stockHeight]
            return [r]
        case .pocket:
            var r = common(.pocket, name: operation.name)
            if s.contour.count >= 3 { r.contour = s.contour } else { r.rectangle = [0, 0, stockWidth, stockHeight] }
            r.stockToLeave = s.stockToLeave > 0 ? s.stockToLeave : nil
            return [r]
        case .contourInside, .contourOutside:
            var r = common(s.kind, name: operation.name)
            r.contour = s.contour
            r.direction = s.direction.rawValue
            r.entry = s.entry.rawValue
            r.rampAngle = s.entry == .ramp ? s.rampAngle : nil
            r.leadIn = s.leadIn
            r.stockToLeave = s.stockToLeave > 0 ? s.stockToLeave : nil
            r.finishStepdowns = s.finishStepdowns > 1 ? s.finishStepdowns : nil
            r.springPass = s.springPass ? true : nil
            r.finishFeed = s.finishFeed > 0 ? s.finishFeed : nil
            if s.tabs > 0 {
                r.tabs = s.tabs; r.tabWidth = s.tabWidth; r.tabHeight = s.tabHeight
            }
            r.thinSlot = .init(strategy: s.thinSlot.rawValue,
                               tolerance: s.thinSlot == .centreLine ? s.thinSlotTolerance : nil)
            return [r]
        case .helicalBore:
            let centres = s.bores
            guard !centres.isEmpty else { return [] }
            return centres.enumerated().map { i, centre in
                var r = common(.helicalBore,
                               name: centres.count == 1 ? operation.name : "\(operation.name) · \(i + 1)")
                r.x = centre[0]; r.y = centre[1]
                r.diameter = s.boreDiameter
                r.pitch = s.stepdown
                r.through = s.bottomAllowance < 0 ? true : nil
                return r
            }
        }
    }

    // MARK: - Preview transport

    func togglePreview() {
        if previewPlaying { pausePreview(); return }
        guard !machine.active, program != nil, preview.duration > 0 else { return }
        if previewFraction >= 1 { previewFraction = 0 }
        mode = .toolpaths; previewPlaying = true
        previewTask = Task { [weak self] in
            var last = Date()
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(33))
                guard !Task.isCancelled, let self, self.previewPlaying else { break }
                let now = Date(), elapsed = min(0.25, now.timeIntervalSince(last)); last = now
                self.previewFraction = min(1, self.previewFraction + elapsed * self.previewSpeed / max(0.001, self.preview.duration))
                if self.previewFraction >= 1 { self.pausePreview(); break }
            }
        }
    }
    func pausePreview() { previewPlaying = false; previewTask?.cancel(); previewTask = nil }

    // MARK: - Files

    /// Writes the job out, and answers whether there was anything to write.
    /// A refused job has no G-code at all, so this returns false before any
    /// panel appears — there is no path from a blocked job to a file.
    @discardableResult func export(job: Bool = false) -> Bool {
        guard let code = jobCode else { return false }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = usesImportedProgram ? importedName : "anolex-job.nc"
        panel.allowedContentTypes = [UTType(filenameExtension: "nc") ?? .plainText]
        guard panel.runModal() == .OK, let url = panel.url else { return false }
        do { try code.write(to: url, atomically: true, encoding: .utf8); return true }
        catch { self.error = error.localizedDescription; return false }
    }
    func importFile() {
        guard !machine.active, !generating else { return }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "nc") ?? .plainText, UTType(filenameExtension: "gcode") ?? .plainText, .plainText]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= 8_000_000 else { throw CNCError.message("Choose a G-code file smaller than 8 MB.") }
            try importProgram(String(contentsOf: url, encoding: .utf8), name: url.lastPathComponent)
        } catch { self.error = error.localizedDescription }
    }

    /// Import a finished program. When an outline is loaded the program is
    /// replayed against it like any other job; without one it is marked
    /// unverified rather than allowed to pass as checked.
    func importProgram(_ code: String, name: String) throws {
        guard !machine.active, !generating else { return }
        let result = try CNCImport.parse(code)
        importedProgram = result; importedName = name
        importedPreview = CNCPreview(program: result, fallbackFeed: 400)
        importedVerification = verifyImported(code: result.gcode)
        useImportedJob(); error = nil
    }

    private func verifyImported(code: String) -> CNCJobResult? {
        guard let outline else { return nil }
        let part = CNCJobPartRequest(
            outer: outline.outer.points.map { [Double($0.x), Double($0.y)] },
            holes: outline.holes.map { $0.points.map { [Double($0.x), Double($0.y)] } })
        let stock = CNCJobStockRequest(thickness: stockThickness,
                                       margin: effectiveMargin,
                                       spoilboard: underStock.thickness)
        do {
            importedVerifyError = nil
            // The floor an imported program is judged against is the one this
            // setup asks for: replaying it against a floor nobody chose reads
            // a deliberate onion skin as a cut that never reached depth.
            let allowance = operations.map(\.setup.bottomAllowance)
                .reduce(0.0) { abs($1) > abs($0) ? $1 : $0 }
            return try CNCJobBridge.verify(gcode: code, part: part, stock: stock,
                                           toolDiameter: toolDiameter, bottomAllowance: allowance)
        } catch {
            // A program that cannot be replayed is not a program that passed.
            importedVerifyError = error.localizedDescription
            return nil
        }
    }

    func useImportedJob() {
        guard !machine.active, !generating, importedProgram != nil else { return }
        pausePreview(); usesImportedProgram = true; setupConfirmed = false
        previewFraction = 0; revision += 1; mode = .toolpaths; inspectorTab = .gcode
    }
    func useGeneratedJob() {
        guard !machine.active else { return }
        if usesImportedProgram {
            pausePreview(); usesImportedProgram = false; setupConfirmed = false
            previewFraction = 0; revision += 1
        }
    }
    func saveMacro(name: String, command: String) {
        guard !name.trimmingCharacters(in: .whitespaces).isEmpty, CNCCommands.validManual(command) else { error = "Give the macro a name and one valid command."; return }
        macros.append(.init(name: name, command: command)); saveMacros()
    }
    func removeMacro(_ id: UUID) { macros.removeAll { $0.id == id }; saveMacros() }
    private func saveMacros() {
        if let data = try? JSONEncoder().encode(macros) { UserDefaults.standard.set(data, forKey: "cnc.console.macros") }
    }
    static func durationLabel(_ value: Double) -> String {
        guard value.isFinite, value >= 0, value < Double(Int.max / 2) else { return "—" }
        let seconds = Int(value.rounded())
        return String(format: "%02d:%02d", seconds / 60, seconds % 60)
    }
}
