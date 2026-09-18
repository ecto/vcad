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
    /// Which tool in the job's list cuts this operation (item 19). The kernel
    /// groups operations by tool inside each phase, so this decides where the
    /// `M0` pauses fall as much as it decides the cutter.
    var toolNumber = 1
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
    /// A `drill` op reuses both: the centres are the holes it sinks and the
    /// diameter is the drill's own, which is the hole it makes.
    var bores: [[Double]] = []
    var boreDiameter = 0.0

    // Drilling (item 19). A hole that matches a drill in the tool list is
    // sunk on its centre rather than milled round its wall.
    //
    // Chip break is the default rather than the full-retract peck, and not
    // only because it is the better cycle for a 2.5 mm hole in 6 mm of metal
    // on a router. A full-retract peck rapids back *down* into the hole
    // between pecks, which is ordinary G83 — but the 2D oracle refuses any
    // rapid that descends below the stock top, because it has no way to know
    // the hole is already open. Peck is still offered; it is simply refused,
    // in the oracle's own words, rather than shipped unverified (item 63).
    var drillCycle: CNCDrillCycle = .chipBreak
    /// How deep each peck goes. Zero follows the roughing stepdown, which is
    /// the number the operator already set for this material and cutter.
    var peckDepth = 0.0
    /// Seconds to pause at the bottom of the hole, for a flat-bottomed spot.
    var drillDwell = 0.0

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
    /// Where each tab goes, as a fraction of the way round the loop. Empty
    /// means "space `tabs` of them evenly", which is what the kernel does on
    /// its own; a dragged tab writes its fraction here (item 21).
    var tabPositions: [Double] = []

    /// Material a pocket keeps: closed loops the cutter clears around and
    /// never enters. Only a pocket has them.
    var islands: [[[Double]]] = []

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
    /// All the circular holes of one diameter that a drill in the list makes,
    /// sunk in one operation (item 19). A separate case from `pilots` so that
    /// fitting the drill moves a hole from one to the other instead of
    /// silently reusing a helical bore's settings for a plunge.
    case drilled(key: Int)
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
        case .drill: return "circle.bottomhalf.filled"
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
    /// The whole tool list, not just the installed cutter: a job built with a
    /// Ø2.5 drill in the list is not the same job once it is taken out, even
    /// when the end mill has not moved.
    var tools: [CNCTool]
    var outer: [[Double]]
    var holes: [[[Double]]]
    /// Where the part sits on the blank, and where the operator zeroes.
    var placement: CNCPlacement
    var zero: CNCZeroLocation
    /// Clamps are not in the request — they are checked here — but a job
    /// acknowledged with one clamp on the blank is not the same job with
    /// another one added.
    var clamps: [[Double]]
    /// Whether the outline still agrees with the part on screen, and what the
    /// disagreement says.
    ///
    /// In the key because the warning has to be acknowledged again when it
    /// changes. Without it, a part edited after its outline was imported kept
    /// the acknowledgement the *old* comparison earned — the outline and the
    /// clamps are identical, so the key was identical — and a freshly-staled
    /// outline would have run with a tick beside it (item 16's caveat).
    var mismatch: String?
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
    /// ncSender and the camera tile.
    ///
    /// They live on the workspace, not inside the view that shows them: a
    /// popover body is rebuilt every time it opens, so a session held there
    /// would drop its poller, its probe and its send state on every close —
    /// and the camera would forget the frame it had just taken.
    let ncSender = NcSenderSession()
    let camera = CNCCameraModel()
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
    /// The ncSender column — the send panel and the camera tile.
    var senderPanelShown = false { didSet { onLayoutChange?() } }

    // Sheets and popovers the machine bar shows. They are workspace state
    // rather than `@State` in the bar so the menu bar can open the same thing
    // the bar's button opens; a popover only a button can reach is a popover
    // a keyboard user cannot reach at all (friction-log item 45).
    var connectionShown = false
    var probeShown = false
    var traceShown = false
    var readinessShown = false
    /// The "start machining" confirmation. Workspace state for the same reason:
    /// Manufacture ▸ Run Job raises the very same dialog the bar's button does,
    /// so there is one confirmation and not two.
    var runShown = false
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
    /// The job's tool list (item 19). One entry per cutter the operator will
    /// fit; the kernel groups operations by tool and writes an `M0` between
    /// the groups, because this machine has no changer.
    ///
    /// The list is the single source of truth: `toolDiameter` and its
    /// neighbours below are views onto the primary end mill, so the panels,
    /// the named-field table and the overlay all read the same numbers the
    /// request is built from.
    var tools: [CNCTool] = [CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175)] {
        didSet {
            guard tools != oldValue else { return }
            setupConfirmed = false; revision += 1
            saveTools()
            // Item 49, now with a list: changing *any* cutter re-decides which
            // holes can be machined and which are drilled, without a re-import
            // and without losing the settings on what survives.
            reconcileOutlineOperations()
            refreshFeedNotes()
        }
    }
    /// Which tool the Tool panel is editing.
    var selectedToolNumber = 1

    /// How this document's tool list is keyed in defaults. Set by the studio
    /// from the editor's own document, so a workspace can be driven without
    /// one in a test.
    var documentKey: (() -> String?)?

    /// The primary cutter: the first end mill in the list, or the first tool
    /// if somehow there is no end mill. Everything that used to mean "the one
    /// installed tool" means this.
    var primaryToolIndex: Int {
        tools.firstIndex { $0.kind.mills } ?? 0
    }

    /// Edit a tool by number, through one path so the `didSet` above fires
    /// once per edit rather than per field.
    func updateTool(number: Int, _ change: (inout CNCTool) -> Void) {
        guard let index = tools.firstIndex(where: { $0.number == number }) else { return }
        var copy = tools
        change(&copy[index])
        tools = copy
    }

    /// Add a tool and select it. Returns the number it was given.
    @discardableResult
    func addTool(kind: CNCToolKind = .drill, diameter: Double = 2.5) -> Int {
        let number = nextToolNumber
        var copy = tools
        copy.append(CNCTool(number: number, kind: kind, diameter: diameter))
        tools = copy.sorted { $0.number < $1.number }
        selectedToolNumber = number
        return number
    }

    /// Remove a tool. The last one cannot go — an operation with no tool is
    /// not a thing the job can be asked to build.
    @discardableResult
    func removeTool(number: Int) -> Bool {
        guard tools.count > 1, tools.contains(where: { $0.number == number }) else { return false }
        // Operations move to a tool that exists rather than being left pointing
        // at one that does not: the kernel refuses an unknown tool by name, and
        // an operation silently keeping a dead number would refuse the whole
        // job with a message about a tool nobody can see.
        let fallback = tools.first { $0.number != number }?.number ?? 1
        for i in operations.indices where operations[i].setup.toolNumber == number {
            operations[i].setup.toolNumber = fallback
        }
        tools.removeAll { $0.number == number }
        if selectedToolNumber == number { selectedToolNumber = tools[0].number }
        job = nil; builtKey = nil
        return true
    }

    /// The primary end mill's diameter. Kept as a property rather than a
    /// lookup at every call site because it is read from the overlay, the
    /// field table, the envelope and the status dump.
    var toolDiameter: Double {
        get { tools.indices.contains(primaryToolIndex) ? tools[primaryToolIndex].diameter : 3.175 }
        set {
            guard tools.indices.contains(primaryToolIndex),
                  tools[primaryToolIndex].diameter != newValue else { return }
            tools[primaryToolIndex].diameter = newValue
        }
    }

    // MARK: material, feeds and what to set on the router

    /// The kernel's material table, loaded once.
    private(set) var materials: [CNCMaterial] = []
    /// What the blank is made of. Nothing is assumed: with no material chosen
    /// the app offers no numbers, because the first real cut was made with
    /// feeds typed for a material the plate turned out not to be (item 54).
    var materialID: String? {
        didSet {
            guard materialID != oldValue else { return }
            setupConfirmed = false; revision += 1; refreshFeedNotes()
        }
    }
    var material: CNCMaterial? { materials.first { $0.id == materialID } }
    /// A first cut on an unknown machine runs at 60% of the numbers: feed and
    /// stepdown are the two that decide whether the cutter survives.
    var firstCutDerate = false { didSet { revision += 1 } }
    /// What "Recommend feeds" last worked out, kept so the panel can show the
    /// dial and the working.
    private(set) var recommendation: CNCFeedAdvice?
    /// What the kernel makes of the numbers in the selected operation right
    /// now — the second opinion beside hand-typed feeds.
    private(set) var feedNotes: [CNCFeedNote] = []
    /// The primary end mill's flutes, cutting length, stickout and whether it
    /// cuts on its centre — views onto the tool list, as `toolDiameter` is.
    /// Zero means "not declared" for the two lengths, and the job says so
    /// rather than assuming the flutes are long enough for the cut.
    var toolFlutes: Int {
        get { tools.indices.contains(primaryToolIndex) ? tools[primaryToolIndex].flutes : 2 }
        set { guard tools.indices.contains(primaryToolIndex) else { return }
              tools[primaryToolIndex].flutes = newValue }
    }
    var toolCentreCutting: Bool {
        get { tools.indices.contains(primaryToolIndex) ? tools[primaryToolIndex].centreCutting : true }
        set { guard tools.indices.contains(primaryToolIndex) else { return }
              tools[primaryToolIndex].centreCutting = newValue }
    }
    var toolFluteLength: Double {
        get { tools.indices.contains(primaryToolIndex) ? tools[primaryToolIndex].fluteLength : 0 }
        set { guard tools.indices.contains(primaryToolIndex) else { return }
              tools[primaryToolIndex].fluteLength = newValue }
    }
    var toolStickout: Double {
        get { tools.indices.contains(primaryToolIndex) ? tools[primaryToolIndex].stickout : 0 }
        set { guard tools.indices.contains(primaryToolIndex) else { return }
              tools[primaryToolIndex].stickout = newValue }
    }

    /// The margin the blank needs when the user has not set one: enough for the
    /// cutter to run right around the part and still stand on material.
    var automaticMargin: Double { max(2 * toolDiameter + 2, 5) }
    var effectiveMargin: Double { stockMargin ?? automaticMargin }

    // MARK: where the job sits on the metal

    /// Where the operator will set G54 (item 41).
    var zeroLocation: CNCZeroLocation = .partCorner {
        didSet { setupConfirmed = false; revision += 1 }
    }
    /// Where the part sits on the blank, and how far round it is turned.
    var placement = CNCPlacement() { didSet { setupConfirmed = false; revision += 1 } }
    /// Clamps, toes and screw heads on the blank, in the work frame.
    var clamps: [CNCClamp] = [] { didSet { setupConfirmed = false; revision += 1 } }

    /// Where the part's own origin sits in the work frame, before the job is
    /// turned: the choice of zero, as a number.
    var zeroOffset: [Double] {
        switch zeroLocation {
        case .partCorner: return [0, 0]
        case .stockCorner: return [effectiveMargin, effectiveMargin]
        case .stockCentre: return [-stockWidth / 2, -stockHeight / 2]
        }
    }
    /// The placement the request carries: the zero the operator will set, plus
    /// whatever the part is shifted and turned by on the table.
    var effectivePlacement: CNCPlacement { placement.moved(by: zeroOffset) }

    /// How far the blank's lower-left corner is from work zero, X and Y. This
    /// is the number to walk the machine to before touching off.
    var stockCornerFromZero: [Double] {
        effectivePlacement.apply([-effectiveMargin, -effectiveMargin])
    }
    /// The rectangle the cutter sweeps, in the work frame: the part outline
    /// plus a radius, turned and placed with the job.
    var sweepRect: [Double] {
        let r = toolDiameter / 2
        let corners = [[-r, -r], [stockWidth + r, -r],
                       [stockWidth + r, stockHeight + r], [-r, stockHeight + r]]
            .map { effectivePlacement.apply($0) }
        let xs = corners.map { $0[0] }, ys = corners.map { $0[1] }
        return [xs.min() ?? 0, ys.min() ?? 0, xs.max() ?? 0, ys.max() ?? 0]
    }
    /// Clamps the cutter would sweep through. The job request has no clamp
    /// field, so this is the app's own check and is reported as one.
    var clampsInTheWay: [CNCClamp] { clamps.filter { $0.overlaps(sweepRect) } }

    /// The machine's own limits and measured skew, when the machine bar has
    /// them.
    var machineProfile: CNCJobMachineLimits { CNCJobMachineLimits(machine.profile) }

    // MARK: the skew the machine measured

    /// How far off the machine axes the two-point edge probe found the blank,
    /// or nothing when it has not run. Derived from the profile, never stored:
    /// a stale skew is worse than none.
    var measuredSkewDegrees: Double? {
        guard let skew = machineProfile.skewDegrees, skew.isFinite else { return nil }
        return skew
    }
    /// Whether "Use the measured skew" has anything left to do.
    var canApplyMeasuredSkew: Bool {
        guard !machine.active, !generating, let skew = measuredSkewDegrees else { return false }
        return CNCSkewProbe.placementRotationDegrees(forSkew: skew) != placement.rotationDeg
    }
    /// Lay the job down on the blank as it actually sits.
    ///
    /// The sign is `CNCSkewProbe`'s to state and it states it once: the blank
    /// is not being straightened, the job is being turned to match it, so a
    /// +10° blank takes a +10° job. Going through
    /// `placementRotationDegrees(forSkew:)` rather than assigning the skew
    /// directly is what keeps that convention in one place.
    @discardableResult
    func applyMeasuredSkew() -> Bool {
        guard canApplyMeasuredSkew, let skew = measuredSkewDegrees else { return false }
        placement.rotationDeg = CNCSkewProbe.placementRotationDegrees(forSkew: skew)
        return true
    }

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
            let feedsChanged = newValue.cutting != operations[index].setup.cutting
                || newValue.kind != operations[index].setup.kind
            operations[index].setup = newValue; setupConfirmed = false; revision += 1
            if feedsChanged { refreshFeedNotes() }
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
                  tools: tools,
                  outer: outline?.outer.points.map { [$0.x, $0.y] } ?? [],
                  holes: outline?.holes.map { $0.points.map { [$0.x, $0.y] } } ?? [],
                  placement: effectivePlacement,
                  zero: zeroLocation,
                  clamps: clamps.map { [$0.x, $0.y, $0.width, $0.height] },
                  mismatch: outlineMismatch)
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
        // No verification is said once, by the headline and the warning that
        // has to be acknowledged — not a third time as an empty check row.
        guard let v = verification else { return [] }
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
        // A job the tool gate refuses is turned back before the replay runs,
        // and that is not the same as having nothing to replay it against:
        // telling someone with an outline on screen to import one is wrong.
        if outline != nil {
            return "This job was refused before it could be replayed against the part. Fix the reasons above and rebuild to have it checked."
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
                out.append(CNCFinding(id: "tool-\(check.opIndex)", text: Self.toolCheckText(check),
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
        // The outline is not the part on screen. Not a refusal — a fixture is
        // not the part either — but not something to machine past in silence.
        if !blocking, let mismatch = outlineMismatch {
            out.append(CNCFinding(id: "outline-mismatch", text: mismatch, xy: nil, z: nil,
                                  operationID: nil, blocking: false))
        }
        // Clamps are the app's own check: the request has no clamp field, so
        // this says what was compared rather than implying the kernel knows.
        for clamp in clampsInTheWay where !blocking {
            let depth = clamp.overlapDepth(sweepRect)
            out.append(CNCFinding(
                id: "clamp-\(clamp.id)",
                text: "\(clamp.name) stands \(CNCVerdictText.mm(depth, 1)) mm inside the rectangle the cutter sweeps. The kernel does not know about clamps — this is the app comparing the sweep with what you typed. Move it, or move the job.",
                xy: [clamp.x + clamp.width / 2, clamp.y + clamp.height / 2], z: 0,
                operationID: nil, blocking: false))
        }
        if !blocking, let checks = result?.toolChecks {
            for check in checks where check.severity == "warning" {
                out.append(CNCFinding(id: "tool-warning-\(check.opIndex)-\(check.kind.name)",
                                      text: Self.toolCheckText(check),
                                      xy: nil, z: nil,
                                      operationID: operation(forRequest: check.opIndex),
                                      blocking: false))
            }
        }
        return out
    }

    /// A tool check names the cut it is about and reads as a sentence. The
    /// kernel's own message is a clause, because it is written to be embedded.
    private static func toolCheckText(_ check: CNCToolCheck) -> String {
        let message = check.message.prefix(1).uppercased() + check.message.dropFirst()
        return check.op.isEmpty ? message : "\(check.op): \(check.message)"
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
        // Two senders, one controller. ncSender holds the Anolex's telnet
        // session; whichever sender connects second either fails outright or —
        // worse — interleaves its commands into the same stream. So while
        // ncSender says it is holding this controller, the built-in sender is
        // not allowed to start, and says why rather than failing on the wire.
        if let contention = senderContention { return contention }
        if !machine.connected { return "Connect the Anolex or choose Simulator." }
        if machine.faulted { return machine.error ?? "Reconnect after resolving the controller fault." }
        if !machine.status.isFresh { return "Waiting for fresh controller telemetry." }
        if !machine.g54Active { return "Select G54 work coordinates on the controller." }
        if !machine.canStart {
            // An alarm is why the controller is not Idle, and it says what
            // happened and what to do about it. "Waiting for Idle and a known
            // work position" is true and useless beside it.
            if let alarm = machine.alarm { return alarm.text }
            return "Waiting for Idle and a known work position."
        }
        if !setupConfirmed { return "Confirm the tool, workholding, clearance and G54 zero." }
        // The machine's own half of the gate — travel against this job's
        // envelope, homing, alarms, limit switches — asked last, because it is
        // the only half that needs a connected controller to mean anything.
        if let machineBlocker = cncMachineBlocker(self) { return machineBlocker }
        return nil
    }

    /// What ncSender says about holding this controller, or nothing.
    ///
    /// Only a probe that found ncSender pointed at the *same* address the
    /// built-in sender is configured for produces one — see
    /// `NcSenderSession.contention`.
    var senderContention: String? { ncSender.probe?.contention }

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
        case .drill: return "Drill"
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

    /// Circular holes no cutter in the list can mill *and* no drill in it can
    /// make. Derived, never stored: change a tool and this answer changes with
    /// it (item 49), and adding the right drill empties it (item 19).
    ///
    /// The rule in one sentence: a hole narrower than the smallest end mill
    /// has to be drilled, so it is unmachinable exactly when the list has no
    /// drill that size.
    var unmachinableHoles: [(index: Int, diameter: Double)] {
        guard let outline else { return [] }
        let millFloor = smallestEndMill?.diameter ?? toolDiameter
        return outline.circularHoles
            .filter { $0.diameter <= millFloor + 1e-9 && drill(for: $0.diameter) == nil }
            .map { (index: $0.index, diameter: $0.diameter) }
    }

    /// Circular holes that are drilled: too small to mill, but matching a
    /// drill in the tool list. Same derivation, opposite answer.
    var drilledHoles: [(index: Int, diameter: Double, tool: Int)] {
        guard let outline else { return [] }
        let millFloor = smallestEndMill?.diameter ?? toolDiameter
        return outline.circularHoles.compactMap { hole in
            guard hole.diameter <= millFloor + 1e-9,
                  let bit = drill(for: hole.diameter) else { return nil }
            return (index: hole.index, diameter: hole.diameter, tool: bit.number)
        }
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
        // Item 16: nothing checked the DXF against the loaded solid, so a
        // stale outline machined silently.
        compareWithModel(outline)
        refreshFeedNotes()
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
        let mill = smallestEndMill
        let millDiameter = mill?.diameter ?? toolDiameter
        base.toolNumber = mill?.number ?? tools.first?.number ?? 1
        // A pass cannot step over further than the cutter's radius without
        // moving through metal no earlier pass reached, so the default follows
        // the tool rather than a number left over from another job.
        base.stepover = min(base.stepover, 0.45 * millDiameter)
        var ops: [CNCOperation] = []

        // A hole no end mill fits into, that a drill in the list makes, is
        // drilled: one operation per drill, however many holes share it
        // (item 19). This runs before the bores because it claims the holes
        // that are too small to mill, which is exactly the set the helical
        // bore could never reach.
        let circles = outline.circularHoles
        var claimed = Set<Int>()
        var drillGroups: [Int: [(index: Int, centre: CGPoint, diameter: Double)]] = [:]
        let drilled = drilledHoles
        for c in circles where drilled.contains(where: { $0.index == c.index }) {
            drillGroups[Int((c.diameter * 100).rounded()), default: []].append(c)
        }
        for key in drillGroups.keys.sorted() {
            let group = drillGroups[key]!
            let size = group.map(\.diameter).reduce(0, +) / Double(group.count)
            guard let bit = drill(for: size) else { continue }
            var spec = base
            spec.kind = .drill
            spec.toolNumber = bit.number
            spec.bores = group.map { [Double($0.centre.x), Double($0.centre.y)] }
            // The hole is the drill's own size, not the outline's rounding of
            // it: the drill is what makes the hole, and the request has to say
            // the same number the tool list does or the kernel's fit check
            // compares a tool against a hole it was never going to cut.
            spec.boreDiameter = bit.diameter
            // Through the plate and into the spoilboard only if one is
            // declared — the same rule every other through cut follows
            // (item 50). On a bare bed the drill stops at the underside.
            spec.bottomAllowance = 0
            group.forEach { claimed.insert($0.index) }
            ops.append(CNCOperation(setup: spec, source: .drilled(key: key),
                                    label: Self.drillLabel(diameter: bit.diameter, count: group.count)))
        }

        // Circles wider than the cutter but narrower than two of it are bored
        // helically: one operation per diameter, however many holes share it.
        var groups: [Int: [(index: Int, centre: CGPoint, diameter: Double)]] = [:]
        for c in circles where c.diameter > millDiameter + 1e-9 && c.diameter < 2 * millDiameter {
            groups[Int((c.diameter * 100).rounded()), default: []].append(c)
        }
        for key in groups.keys.sorted() {
            let group = groups[key]!
            var spec = base
            spec.kind = .helicalBore
            spec.bores = group.map { [Double($0.centre.x), Double($0.centre.y)] }
            spec.boreDiameter = group.map(\.diameter).reduce(0, +) / Double(group.count)
            group.forEach { claimed.insert($0.index) }
            ops.append(CNCOperation(setup: spec, source: .pilots(key: key),
                                    label: Self.pilotLabel(diameter: spec.boreDiameter, count: group.count)))
        }

        // Everything else inside the part is an opening, cut out by default.
        let tooSmall = Set(unmachinableHoles.map(\.index))
        for (i, hole) in outline.holes.enumerated() where !claimed.contains(i) && !tooSmall.contains(i) {
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
    static func drillLabel(diameter: Double, count: Int) -> String {
        let size = ((diameter * 100).rounded() / 100).formatted()
        return count == 1 ? "Drill Ø\(size)" : "Drill Ø\(size) × \(count)"
    }
    static func openingLabel(_ hole: CNCLoop) -> String {
        if let circle = hole.circle {
            return "Bore Ø\(((circle.diameter * 10).rounded() / 10).formatted())"
        }
        let b = hole.bounds
        let w = (Double(b.width) * 10).rounded() / 10, h = (Double(b.height) * 10).rounded() / 10
        return "Opening \(w.formatted()) × \(h.formatted())"
    }

    // MARK: - The part on screen

    /// The document the Manufacture workspace should take an outline from.
    /// Set by the studio from the editor's own source, so the workspace can be
    /// driven without one in a test.
    var modelDocument: (() -> CNCModelDocument?)?
    /// Which part of the document to section.
    var modelPartIndex = 0
    /// The last section, kept for the summary: the prismatic verdict, the
    /// thickness it suggests, which mesh it came from.
    private(set) var modelSection: CNCSection?
    /// Why the last "From model…" could not be used, in the kernel's words.
    private(set) var modelRefusal: String?
    /// True when there is a part on screen to take an outline from — which is
    /// what makes "From model…" the default import.
    var hasModel: Bool { modelDocument?() != nil }

    /// An outline from the part on screen, at its own mid-height.
    ///
    /// Items 16 and 37: the outline was a separate DXF nothing compared
    /// against the solid, and there was no way to get one *out* of the solid —
    /// the one that was lost had to be regenerated outside vcad.
    @discardableResult
    func importFromModel() -> Bool {
        guard !machine.active, !generating else { return false }
        guard let document = modelDocument?() else {
            error = "There is no document open to take an outline from. Open a part, or import a DXF."
            return false
        }
        do {
            let section = try CNCCam.section(document, partIndex: modelPartIndex)
            modelSection = section
            // A torn solid is the signal, not a nuisance: healing it away here
            // would hide exactly the thing worth seeing (item 30). The default
            // heal tolerance stays where the kernel put it.
            if let torn = section.tornMessage {
                modelRefusal = torn
                error = torn
                return false
            }
            guard let region = section.part, region.outer.count >= 3 else {
                modelRefusal = "Nothing closed came back from the section at Z \(CNCVerdictText.mm(section.z, 3))."
                error = modelRefusal
                return false
            }
            modelRefusal = nil
            let outline = CNCOutline.from(region: region, name: document.name)
            try importOutline(outline)
            // The blank is as thick as the part is tall: the stator was
            // modelled at z 11.1–17.1 and the thickness stayed at the 10 mm
            // default until it was typed in by hand (item 17).
            if section.suggestedStockThickness.isFinite, section.suggestedStockThickness > 0 {
                stockThickness = (section.suggestedStockThickness * 1000).rounded() / 1000
            }
            // …and the stock top is the part's top. Without this the blank was
            // the right thickness in the wrong place: `importOutline` sets X
            // and Y from the outline but kept whatever Z was there, so for a
            // part modelled at z 11.1–17.1 the toolpath was drawn 17 mm below
            // the solid and "Place at model top" had to be pressed by hand
            // (items 17, 18 and 47 were all this one line).
            if let top = section.zRange.last, top.isFinite {
                origin.z = (top * 1000).rounded() / 1000
            }
            outlineMismatch = nil
            return true
        } catch {
            modelRefusal = error.localizedDescription
            self.error = error.localizedDescription
            return false
        }
    }

    /// The part on screen as loops in the stock frame, for the comparison.
    private func modelLoops() -> [[[Double]]]? {
        guard let region = modelSection?.part, region.outer.count >= 3 else { return nil }
        let outline = CNCOutline.from(region: region, name: "model")
        return [outline.outer.points.map { [Double($0.x), Double($0.y)] }]
            + outline.holes.map { $0.points.map { [Double($0.x), Double($0.y)] } }
    }

    /// Why the imported outline is not the part on screen, when it is not.
    /// A warning, not a refusal — plenty of real work machines a fixture that
    /// is not the part — but one that has to be acknowledged (item 16).
    private(set) var outlineMismatch: String?

    /// What the part on screen was when the outline was last compared against
    /// it: the document's own bytes and which part was sectioned. The bytes
    /// are the solve — `camDocument()` hands back the edited JSON, so a
    /// re-solved part is different bytes — which is why this is the signal
    /// rather than a revision counter the workspace would have to be told
    /// about.
    private var comparedModelDigest: Int?

    /// The part on screen right now, as a number to compare with the above.
    private func modelDigest() -> Int? {
        guard let document = modelDocument?() else { return nil }
        var hasher = Hasher()
        hasher.combine(document.data)
        hasher.combine(modelPartIndex)
        return hasher.finalize()
    }

    /// Re-run the outline↔solid comparison when the part has changed under an
    /// outline that was already checked against it.
    ///
    /// Item 16 closed the case where the outline was stale at import. Its own
    /// caveat was the other direction: *"editing the solid after importing its
    /// outline never re-checks. A stale outline is caught; a freshly-staled
    /// one is not."* This is that direction. It runs on every build, so the
    /// answer beside a job is always about the part the job was built from.
    ///
    /// Cheap when nothing moved: a hash of the document's bytes decides, and
    /// the section — which is a kernel call — is only taken when they differ.
    func recheckOutlineAgainstModel() {
        guard let outline, modelDocument?() != nil else { return }
        let digest = modelDigest()
        guard digest != comparedModelDigest else { return }
        // The cached section is of the *old* part, so it goes: comparing
        // against it would be the very mistake this is here to catch.
        modelSection = nil
        compareWithModel(outline)
    }

    /// Compare an imported outline against the part on screen, if there is
    /// one. Silent when there is nothing to compare against.
    private func compareWithModel(_ outline: CNCOutline) {
        outlineMismatch = nil
        comparedModelDigest = modelDigest()
        // A section of the part on screen, taken now: comparing against a
        // stale one would be the very mistake this is here to catch.
        if modelSection == nil, let document = modelDocument?(),
           let section = try? CNCCam.section(document, partIndex: modelPartIndex) {
            if !section.isTorn { modelSection = section }
            // The blank is as thick as the part is tall, and its top is the
            // part's top — whether or not the section closed (item 69: a DXF
            // over the torn stator kept the 10 mm default and the first build
            // was refused for a number nobody typed). The DXF says where the
            // part ends in X and Y; the solid on screen says so in Z.
            if section.zRange.count == 2, let bottom = section.zRange.first, let top = section.zRange.last,
               top.isFinite, bottom.isFinite, top - bottom > 0 {
                stockThickness = ((top - bottom) * 1000).rounded() / 1000
                origin.z = (top * 1000).rounded() / 1000
            }
        }
        guard let loops = modelLoops() else { return }
        let imported = [outline.outer.points.map { [Double($0.x), Double($0.y)] }]
            + outline.holes.map { $0.points.map { [Double($0.x), Double($0.y)] } }
        do {
            let comparison = try CNCCam.compareOutline([
                "a": ["loops": imported],
                "b": ["loops": loops],
                "tolerance": 0.05,
            ])
            if !comparison.agrees { outlineMismatch = comparison.warning }
        } catch {
            // Not being able to compare is itself worth saying: silence here
            // would read as agreement.
            outlineMismatch = "This outline could not be compared with the part on screen: \(error.localizedDescription)"
        }
    }

    // MARK: - Material, feeds and the dial to set

    /// Load the kernel's material table once.
    func loadMaterials() {
        guard materials.isEmpty else { return }
        do { materials = try CNCCam.materials() }
        catch { self.error = error.localizedDescription }
    }

    private func toolRequest() -> [String: Any] {
        var tool: [String: Any] = ["diameter": toolDiameter, "flutes": toolFlutes, "kind": "flat_end_mill"]
        if toolFluteLength > 0 { tool["flute_length"] = toolFluteLength }
        return tool
    }
    private var machineRequestJSON: [String: Any] {
        ["class": "hobby", "spindle": "dial", "max_feed": 3000.0]
    }
    /// Which entry in the feeds table this operation is: a profile cut through
    /// sheet is buried on both sides, which is a slot however it is drawn.
    private func feedsOperationName(_ kind: CNCOpKind) -> String {
        switch kind {
        case .face: return "profile"
        case .pocket: return "pocket"
        // A drill sinks on its own centre, which is the deepest, most
        // buried cut in the table: a slot is the right entry for it too.
        case .helicalBore, .drill: return "slot"
        case .contourInside, .contourOutside: return "slot"
        }
    }

    /// The numbers the kernel recommends for this material, tool and cut.
    func recommendFeeds() -> CNCFeedAdvice? {
        guard let id = materialID else {
            error = "Choose what the blank is made of before asking for feeds."
            return nil
        }
        do {
            let advice = try CNCCam.recommend([
                "material": id,
                "op": feedsOperationName(setup.kind),
                "tool": toolRequest(),
                "machine": machineRequestJSON,
            ])
            recommendation = advice
            return advice
        } catch {
            self.error = error.localizedDescription
            return nil
        }
    }

    /// Recommend and apply: feed, plunge, stepdown, stepover and rpm, on every
    /// operation. Item 52 — with five operations the only way to change
    /// material was to edit each one, or re-import the outline.
    @discardableResult
    func applyRecommendedFeeds() -> CNCFeedAdvice? {
        guard !machine.active, !generating, let advice = recommendFeeds() else { return nil }
        let values = advice.values(derated: firstCutDerate)
        var spec = setup
        spec.feed = values.feed
        spec.plunge = values.plunge
        spec.stepdown = values.stepdown
        spec.rpm = values.rpm
        // A pass cannot step over further than the cutter is wide, and only a
        // face or a pocket steps over at all.
        spec.stepover = min(max(values.stepover, 0.01), toolDiameter)
        setup = spec
        applyFeedsToAllOperations()
        refreshFeedNotes()
        return advice
    }

    /// What the kernel makes of the numbers in the selected operation.
    func refreshFeedNotes() {
        guard let id = materialID else { feedNotes = []; return }
        let s = setup
        guard [s.feed, s.plunge, s.rpm, s.stepdown].allSatisfy({ $0.isFinite && $0 > 0 }) else {
            feedNotes = []; return
        }
        do {
            let check = try CNCCam.checkFeeds([
                "material": id,
                "op": feedsOperationName(s.kind),
                "tool": toolRequest(),
                "machine": machineRequestJSON,
                "settings": ["feed": s.feed, "plunge": s.plunge, "rpm": s.rpm,
                             "stepdown": s.stepdown,
                             "stepover": min(max(s.stepover, 0.01), toolDiameter)],
            ])
            feedNotes = check.notes
        } catch {
            feedNotes = []
        }
    }

    // MARK: - Clamps

    func addClamp() {
        guard !machine.active, !generating else { return }
        // Somewhere out of the way to start: beyond the blank's own corner, so
        // a new clamp never silently overlaps the cut.
        let corner = stockCornerFromZero
        clamps.append(CNCClamp(x: (corner[0] - 40).rounded(), y: (corner[1] - 40).rounded(),
                               name: "Clamp \(clamps.count + 1)"))
    }
    func removeClamp(_ id: UUID) {
        guard !machine.active, !generating else { return }
        clamps.removeAll { $0.id == id }
    }

    // MARK: - Islands

    /// Loops of the outline that lie inside an operation's own contour, and so
    /// could be kept rather than cleared away.
    func islandCandidates(for operation: CNCOperation) -> [[[Double]]] {
        let wall = operation.setup.contour
        guard operation.setup.kind == .pocket, wall.count >= 3, let outline else { return [] }
        return outline.holes.map { $0.points.map { [Double($0.x), Double($0.y)] } }
            .filter { hole in
                hole.count >= 3 && hole.allSatisfy { contains(wall, $0) }
            }
    }

    /// Keep everything inside this pocket, or clear it all away.
    func keepIslands(_ keep: Bool) {
        guard !machine.active, !generating, setup.kind == .pocket else { return }
        var spec = setup
        spec.islands = keep ? islandCandidates(for: selectedOperation) : []
        setup = spec
        if keep && spec.islands.isEmpty {
            error = "Nothing in this outline lies inside this opening, so there is nothing for the pocket to keep."
        }
    }

    /// What the last build said about the material this operation keeps.
    func islandClearances(of operation: CNCOperation) -> [CNCIslandClearance.Island] {
        guard jobCurrent, !usesImportedProgram else { return [] }
        let indices = requestIndices(of: operation)
        return (job?.islandClearance ?? [])
            .filter { indices.contains($0.opIndex) }
            .flatMap(\.islands)
    }

    /// Point in closed polygon, by ray crossing.
    private func contains(_ polygon: [[Double]], _ p: [Double]) -> Bool {
        var inside = false
        for i in polygon.indices {
            let a = polygon[i], b = polygon[(i + 1) % polygon.count]
            guard a.count >= 2, b.count >= 2 else { continue }
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let t = (p[1] - a[1]) / (b[1] - a[1])
                if p[0] < a[0] + t * (b[0] - a[0]) { inside.toggle() }
            }
        }
        return inside
    }

    // MARK: - Tabs

    /// The tabs of an operation as the audit found them, in order round the
    /// loop. These are where the metal really is, not where it was asked for.
    func tabLandings(of operation: CNCOperation) -> [CNCTabLanding] {
        guard jobCurrent, !usesImportedProgram else { return [] }
        let indices = requestIndices(of: operation)
        return (job?.tabPlacement ?? [])
            .filter { indices.contains($0.opIndex) }
            .flatMap(\.tabs)
    }

    /// The tabs an operation asks for, as fractions round its own contour.
    /// Empty positions mean "evenly spaced", which is what the kernel does —
    /// so this fills them in before the first drag rather than inventing a
    /// different rule.
    func declaredTabPositions(of operation: CNCOperation) -> [Double] {
        let s = operation.setup
        if !s.tabPositions.isEmpty { return s.tabPositions }
        guard s.tabs > 0 else { return [] }
        return (0..<s.tabs).map { (Double($0) + 0.5) / Double(s.tabs) }
    }

    /// Where a tab sits on the contour the user sees, as a fraction.
    ///
    /// The request states tab positions on the cutter's own offset loop and
    /// the audit measures the contour as drawn; the two are a rotation apart.
    /// The panel and the viewport both speak the drawn contour, so this is the
    /// number they show — the request's own value plus whatever the last build
    /// measured the difference to be.
    func tabFraction(of operation: CNCOperation, index: Int) -> Double {
        let declared = declaredTabPositions(of: operation)
        guard declared.indices.contains(index) else { return 0 }
        return wrapFraction(declared[index] + (tabFrameOffset[operation.id] ?? 0))
    }

    /// Where a tab sits on the contour, in the stock frame, for the overlay to
    /// draw a handle on.
    func declaredTabPoint(of operation: CNCOperation, index: Int) -> [Double]? {
        let points = operation.setup.contour
        guard points.count >= 3 else { return nil }
        return point(on: points, at: tabFraction(of: operation, index: index))
    }

    /// Put a tab at a fraction of the way round the contour, typed rather than
    /// dragged. Both paths mean the same thing and go the same way.
    func setTabPosition(index: Int, to fraction: Double) {
        guard fraction.isFinite else { return }
        let operation = selectedOperation
        guard let xy = point(on: operation.setup.contour, at: wrapFraction(fraction)) else { return }
        moveTab(operation: operation.id, index: index, to: xy)
    }

    /// What a drag is waiting to find out: the place the user dropped a tab,
    /// and whether the rebuild put it there.
    private struct TabDrag {
        var operationID: UUID
        var index: Int
        var target: Double
        var xy: [Double]
        var corrected = false
    }
    private var pendingTabDrag: TabDrag?
    /// How far an operation's requested fractions sit from where tabs land,
    /// learned from the last build. The request is stated on the cutter's own
    /// offset loop and the audit measures the drawn contour, so the two are a
    /// rotation apart — measured, never assumed.
    private var tabFrameOffset: [UUID: Double] = [:]

    /// Drag a tab to a point on the contour, in the work frame.
    ///
    /// Everything goes through the request: the drop point becomes a fraction
    /// of the way round the contour, the fraction becomes `tab_positions`, and
    /// the job is rebuilt. Where the tab really ended up comes back in
    /// `tab_placement`, and if the kernel settled it somewhere else — it lands
    /// tabs on straight stretches — that is what the overlay then draws.
    func moveTab(operation id: UUID, index: Int, to xy: [Double]) {
        guard !machine.active, !generating,
              let position = operations.firstIndex(where: { $0.id == id }),
              xy.count >= 2, xy.allSatisfy(\.isFinite) else { return }
        let points = operations[position].setup.contour
        guard points.count >= 3 else { return }
        var positions = declaredTabPositions(of: operations[position])
        guard positions.indices.contains(index) else { return }
        let target = fraction(on: points, nearest: xy)
        positions[index] = wrapFraction(target - (tabFrameOffset[id] ?? 0))
        operations[position].setup.tabPositions = positions
        operations[position].setup.tabs = positions.count
        pendingTabDrag = TabDrag(operationID: id, index: index, target: target, xy: xy)
        setupConfirmed = false; revision += 1
        build()
    }

    /// The tab handle being dragged in the viewport, by entity name.
    private(set) var draggingTabName: String?

    /// Start dragging the tab handle an entity name points at.
    /// `cncTabHandle-<operation uuid>-<index>`.
    @discardableResult
    func beginTabDrag(named name: String) -> Bool {
        guard !machine.active, !generating, tab(named: name) != nil else { return false }
        draggingTabName = name
        return true
    }
    func cancelTabDrag() { draggingTabName = nil }

    /// Drop the dragged tab at a point in the work frame.
    func dropTab(atWork xy: [Double]) {
        guard let name = draggingTabName, let target = tab(named: name) else { return }
        draggingTabName = nil
        guard xy.count >= 2, xy.allSatisfy(\.isFinite) else { return }
        // The viewport works in the work frame; a contour is stated in the
        // part's own. Undo the placement rather than dragging the tab to a
        // place on a part that is not there.
        moveTab(operation: target.id, index: target.index, to: unplace(xy))
    }

    private func tab(named name: String) -> (id: UUID, index: Int)? {
        let parts = name.split(separator: "-")
        guard parts.count >= 3, parts[0] == "cncTabHandle",
              let index = Int(parts[parts.count - 1]) else { return nil }
        let uuid = parts[1..<(parts.count - 1)].joined(separator: "-")
        guard let id = UUID(uuidString: uuid), operations.contains(where: { $0.id == id }) else { return nil }
        return (id, index)
    }

    /// A point in the work frame, back in the part's own frame.
    func unplace(_ xy: [Double]) -> [Double] {
        let p = effectivePlacement
        let a = -p.rotationDeg * .pi / 180
        let (x, y) = (xy[0] - p.dx, xy[1] - p.dy)
        return [x * cos(a) - y * sin(a), x * sin(a) + y * cos(a)]
    }

    /// Add or remove tabs on the selected contour, keeping the ones that are
    /// already placed where they are.
    func setTabCount(_ count: Int) {
        guard !machine.active, !generating, setup.isContour else { return }
        let wanted = max(0, min(12, count))
        var spec = setup
        var positions = declaredTabPositions(of: selectedOperation)
        if wanted == 0 {
            positions = []
        } else if wanted < positions.count {
            positions = Array(positions.prefix(wanted))
        } else if wanted > positions.count {
            // A new tab goes into the widest gap between the ones that are
            // already placed, which is where a machinist would put it.
            while positions.count < wanted {
                positions.append(widestGapMidpoint(positions))
                positions.sort()
            }
        }
        spec.tabs = wanted
        spec.tabPositions = positions.isEmpty ? [] : positions
        setup = spec
    }

    private func widestGapMidpoint(_ positions: [Double]) -> Double {
        guard let first = positions.first else { return 0.5 }
        guard positions.count > 1 else { return wrapFraction(first + 0.5) }
        let sorted = positions.sorted()
        var best = (gap: 0.0, mid: 0.5)
        for i in sorted.indices {
            let a = sorted[i], b = sorted[(i + 1) % sorted.count]
            let gap = wrapFraction(b - a)
            if gap > best.gap { best = (gap, wrapFraction(a + gap / 2)) }
        }
        return best.mid
    }

    private func wrapFraction(_ v: Double) -> Double {
        guard v.isFinite else { return 0 }
        let r = v.truncatingRemainder(dividingBy: 1)
        return r < 0 ? r + 1 : r
    }

    /// The fraction of the way round a closed polyline nearest a point.
    private func fraction(on points: [[Double]], nearest p: [Double]) -> Double {
        var run = 0.0, best = (fraction: 0.0, distance: Double.greatestFiniteMagnitude)
        let total = perimeter(points)
        guard total > 0 else { return 0 }
        for i in points.indices {
            let a = points[i], b = points[(i + 1) % points.count]
            let dx = b[0] - a[0], dy = b[1] - a[1]
            let length = hypot(dx, dy)
            if length > 0 {
                let t = max(0, min(1, ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (length * length)))
                let d = hypot(a[0] + t * dx - p[0], a[1] + t * dy - p[1])
                if d < best.distance { best = ((run + t * length) / total, d) }
            }
            run += length
        }
        return best.fraction
    }

    /// The point a fraction of the way round a closed polyline.
    private func point(on points: [[Double]], at fraction: Double) -> [Double]? {
        let total = perimeter(points)
        guard total > 0 else { return nil }
        var remaining = wrapFraction(fraction) * total
        for i in points.indices {
            let a = points[i], b = points[(i + 1) % points.count]
            let length = hypot(b[0] - a[0], b[1] - a[1])
            if remaining <= length || i == points.count - 1 {
                let t = length > 0 ? remaining / length : 0
                return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            }
            remaining -= length
        }
        return points.first
    }

    private func perimeter(_ points: [[Double]]) -> Double {
        var total = 0.0
        for i in points.indices {
            let a = points[i], b = points[(i + 1) % points.count]
            total += hypot(b[0] - a[0], b[1] - a[1])
        }
        return total
    }

    /// Learn where the request's fractions land, and correct a drag that
    /// missed. Called after every build that carries a tab audit.
    private func settleTabs() {
        guard let drag = pendingTabDrag,
              let position = operations.firstIndex(where: { $0.id == drag.operationID }) else { return }
        let landings = tabLandings(of: operations[position])
        guard !landings.isEmpty else { pendingTabDrag = nil; return }
        // The tab that landed nearest where the user dropped it is the one
        // that was dragged; no pairing of two different frames is needed.
        let landed = landings.min {
            hypot($0.at[0] - drag.xy[0], $0.at[1] - drag.xy[1])
                < hypot($1.at[0] - drag.xy[0], $1.at[1] - drag.xy[1])
        }
        guard let landed else { pendingTabDrag = nil; return }
        var residual = drag.target - landed.alongContour
        residual -= (residual).rounded()          // the short way round the loop
        tabFrameOffset[drag.operationID] = wrapFraction((tabFrameOffset[drag.operationID] ?? 0) + residual)
        // One correction, then the answer stands: the kernel settles tabs onto
        // straight stretches on purpose, and chasing that would be a loop.
        if !drag.corrected, abs(residual) > 0.01,
           operations[position].setup.tabPositions.indices.contains(drag.index) {
            operations[position].setup.tabPositions[drag.index] =
                wrapFraction(operations[position].setup.tabPositions[drag.index] + residual)
            pendingTabDrag?.corrected = true
            revision += 1
            // The build that is reporting this is still running, so the
            // correction goes after it rather than into a guard that would
            // drop it on the floor.
            Task { @MainActor [weak self] in self?.build() }
            return
        }
        pendingTabDrag = nil
    }

    // MARK: - Building the job

    /// Kept under its old name because the machine bar's readiness popover
    /// calls it; `all` no longer means anything, since a job is one request.
    func generate(all: Bool = false) { build() }

    /// Replay this job against the part it is meant to make, and say so.
    ///
    /// For a generated job that *is* the build — the kernel verifies on its way
    /// out and withholds the G-code when a check fails — so this rebuilds. For
    /// an imported program there is nothing to build, only the outline to
    /// replay it against, which may have changed since it was imported.
    func verify() {
        guard !generating, !machine.active else { return }
        guard usesImportedProgram else { build(); return }
        guard let program = importedProgram else { return }
        importedVerification = verifyImported(code: program.gcode)
        revision += 1
    }

    func build() {
        guard !generating, !machine.active else { return }
        // Item 16's caveat: the outline was compared against the solid at
        // import and never again, so a part edited afterwards machined its old
        // outline without a word. A build is the moment the answer has to be
        // about the part the job is really being built from.
        recheckOutlineAgainstModel()
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

    /// Which request-operation indices one listed operation became.
    private func requestIndices(of operation: CNCOperation) -> [Int] {
        var start = 0
        for candidate in operations {
            let count = requestCount(of: candidate)
            if candidate.id == operation.id { return Array(start..<(start + count)) }
            start += count
        }
        return []
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
        // A dragged tab is not placed until the job says where it went.
        settleTabs()
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
        // No changer on this machine, so every tool change is an operator stop
        // and a re-probe (item 19). The probe macro comes from the machine
        // profile when it has one; without it the kernel writes the touch-off
        // instruction as a comment, which is the safe default — a macro from
        // another machine would drive the spindle into the work.
        options.toolChange = CNCJobOptionsRequest.ToolChange(probeMacro: toolChangeProbeMacro)
        // One job, one safe height: the tallest clearance any operation asks
        // for, so a travel move never crosses at another operation's lower one.
        let clearance = operations.map(\.setup.clearance).filter { $0.isFinite && $0 > 0 }.max() ?? 5
        options.safeZ = max(1, clearance)
        options.parkZ = max(1, clearance)
        // Where the part sits on the blank, and where zero is: the job moves
        // and the part it is checked against moves with it, so a turned or
        // shifted job is verified against the metal it will really cut.
        let placed = effectivePlacement
        if !placed.isIdentity {
            options.placement = CNCJobPlacementRequest(dx: placed.dx, dy: placed.dy,
                                                      rotationDeg: placed.rotationDeg)
        }
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
        // The machine's own limits, when the machine bar has measured them: a
        // job that would run off the end of the table is worth hearing about
        // before the cutter is in the work, not after.
        var machineRequest = CNCJobMachineRequest()
        let profile = machineProfile
        if let lo = profile.travelMin, let hi = profile.travelMax, lo.count == 3, hi.count == 3 {
            machineRequest.travel = CNCJobTravelRequest(min: lo, max: hi)
        }
        if let offset = profile.workOffset, offset.count == 3 {
            machineRequest.workOffset = offset
        }
        return CNCJobRequest(
            name: outline?.name ?? "vcad job",
            stock: CNCJobStockRequest(thickness: stockThickness,
                                      margin: effectiveMargin,
                                      spoilboard: underStock.thickness),
            machine: machineRequest,
            // Every tool in the list, not just the one an operation happens to
            // name: the kernel refuses an operation whose tool it cannot find,
            // and sending the list is also what lets it write the change
            // prompt with the next cutter's real name.
            tools: tools.map(CNCJobToolRequest.init),
            operations: requests,
            options: options)
    }

    /// The machine profile's own Z-probe sequence, for the tool-change pause.
    ///
    /// Taken from a saved macro named for probing, because that is where this
    /// app already keeps a machine-specific command the operator wrote and
    /// trusts. Nothing is invented: with no such macro the job says "touch off
    /// and set G54 Z0 before resuming" in words instead.
    var toolChangeProbeMacro: String? {
        let wanted = macros.first {
            let name = $0.name.lowercased()
            return name.contains("probe") && name.contains("z")
        }
        guard let command = wanted?.command.trimmingCharacters(in: .whitespacesAndNewlines),
              !command.isEmpty else { return nil }
        return command
    }

    private func requestBodies(for operation: CNCOperation, at position: Int) -> [CNCJobOperationRequest] {
        let s = operation.setup
        func common(_ kind: CNCOpKind, name: String) -> CNCJobOperationRequest {
            var r = CNCJobOperationRequest(name: name, kind: kind,
                                           depth: s.depth, stepdown: s.stepdown,
                                           feed: s.feed, plunge: s.plunge, rpm: s.rpm)
            // Which cutter runs this operation. The kernel groups by it, so
            // this is also what decides where the `M0` pauses fall.
            r.tool = tool(number: s.toolNumber) != nil ? s.toolNumber : (tools.first?.number ?? 1)
            r.stepover = kind == .face || kind == .pocket ? s.stepover : nil
            r.bottomAllowance = s.bottomAllowance
            r.order = position
            // The outside profile runs last whatever the list says, because
            // after it the part is held by tabs at best. Forcing it says so
            // outright: the operation runs in the phase its place in the list
            // implies. It used to be said by calling the profile an
            // "inside_feature" — a lie about what the cut is, told to change
            // when it happens.
            if s.forceOrder { r.phase = position }
            return r
        }
        switch s.kind {
        case .face:
            var r = common(.face, name: operation.name)
            // A rectangle cannot be turned and stay a rectangle, and the
            // kernel refuses one rather than quietly facing its bounding box.
            // A turned job says the same area as four points instead.
            if effectivePlacement.rotationDeg != 0 {
                r.contour = [[0, 0], [stockWidth, 0], [stockWidth, stockHeight], [0, stockHeight]]
                r.kind = .pocket
                // It is still facing, and still runs first: clearing the same
                // area under another name must not change when it happens.
                if r.phase == nil { r.role = "facing" }
            } else {
                r.rectangle = [0, 0, stockWidth, stockHeight]
            }
            return [r]
        case .pocket:
            var r = common(.pocket, name: operation.name)
            if s.contour.count >= 3 { r.contour = s.contour } else { r.rectangle = [0, 0, stockWidth, stockHeight] }
            r.stockToLeave = s.stockToLeave > 0 ? s.stockToLeave : nil
            // Material the pocket keeps. The kernel clears around it and the
            // answer reports how close the cutter came to each one.
            let islands = s.islands.filter { $0.count >= 3 }
            if !islands.isEmpty { r.islands = islands }
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
            // Dragged tabs are sent as the positions they were dragged to;
            // untouched ones as a count the kernel spaces evenly. Sending both
            // is refused by the kernel when they disagree, which is right: two
            // answers to "where are the tabs" is one too many.
            let positions = s.tabPositions.filter { $0.isFinite && (0...1).contains($0) }
            if !positions.isEmpty {
                r.tabPositions = positions
                r.tabWidth = s.tabWidth; r.tabHeight = s.tabHeight
            } else if s.tabs > 0 {
                r.tabs = s.tabs; r.tabWidth = s.tabWidth; r.tabHeight = s.tabHeight
            }
            r.thinSlot = .init(strategy: s.thinSlot.rawValue,
                               tolerance: s.thinSlot == .centreLine ? s.thinSlotTolerance : nil)
            return [r]
        case .drill:
            // One operation drills every hole in the list, unlike a helical
            // bore, which is one request per hole — so `requestCount` says 1
            // and the moves for all of them come back under one range.
            let centres = s.bores.filter { $0.count >= 2 }
            guard !centres.isEmpty else { return [] }
            var r = common(.drill, name: operation.name)
            r.holes = centres
            r.diameter = s.boreDiameter > 0 ? s.boreDiameter : nil
            r.cycle = s.drillCycle.rawValue
            // A peck depth of zero means "follow the roughing stepdown", which
            // is the number already set for this material and cutter. The
            // kernel refuses a peck cycle with no depth, so it is filled in
            // here rather than left for it to reject.
            if s.drillCycle.needsPeckDepth {
                let peck = s.peckDepth > 0 ? s.peckDepth : s.stepdown
                r.peckDepth = peck > 0 ? peck : nil
            }
            r.dwell = s.drillDwell > 0 ? s.drillDwell : nil
            r.through = s.bottomAllowance < 0 ? true : nil
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
        // The program is in the work frame, so the part it is checked against
        // has to be placed there too — otherwise a job on skewed stock reads
        // as gouging everything it touches.
        let placement = effectivePlacement
        let part = CNCJobPartRequest(
            outer: outline.outer.points.map { placement.apply([Double($0.x), Double($0.y)]) },
            holes: outline.holes.map { $0.points.map { placement.apply([Double($0.x), Double($0.y)]) } })
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
