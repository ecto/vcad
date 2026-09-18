import Foundation
import CVcadFFI

// One job, one request, one answer.
//
// Until now the app asked the kernel for one operation at a time
// (`vcad_cam_generate`), stitched the programs together itself, and had no way
// to ask whether the result made the part. `vcad_cam_job` takes the whole job —
// every operation, the tools, the stock and what is under it, the machine's
// limits — replays the posted G-code against the part it is meant to make, and
// **returns no `gcode` key at all** when an error-severity check fails. That
// absence is the gate: there is nothing for the app to export or send.
//
// Everything here is millimetres, feeds are mm/min, Z is up, the stock top is
// Z0 and the stock frame's XY lower-left is the part's bounding-box corner —
// the work zero the operator sets on the machine.

// MARK: - Request

/// What an operation cuts. The raw values are the kinds `vcad_cam_job` knows.
enum CNCOpKind: String, Codable, Sendable, CaseIterable {
    case face
    case pocket
    case contourOutside = "contour_outside"
    case contourInside = "contour_inside"
    case helicalBore = "helical_bore"

    var isContour: Bool { self == .contourOutside || self == .contourInside }
    /// A cut inside the part outline, rather than the profile that frees it.
    var isInside: Bool { self == .contourInside || self == .pocket || self == .helicalBore }
}

/// Which way round the cutter walks the wall.
enum CNCCutDirection: String, Codable, Sendable, CaseIterable {
    case climb, conventional
    var label: String { self == .climb ? "Climb" : "Conventional" }
}

/// How a pass gets down to depth.
enum CNCEntry: String, Codable, Sendable, CaseIterable {
    case ramp, plunge
    var label: String { self == .ramp ? "Ramp down along the cut" : "Straight down" }
}

/// What the app does where the cutter is as wide as the opening.
enum CNCThinSlot: String, Codable, Sendable, CaseIterable {
    case refuse
    case centreLine = "centre_line"
    var label: String { self == .refuse ? "Refuse the job" : "Follow the centre line" }
}

/// What sits under the blank. A bare bed means a cut may not break through.
enum CNCUnderStock: Equatable, Sendable {
    case machineBed
    case spoilboard(Double)

    var thickness: Double? { if case .spoilboard(let t) = self { return t }; return nil }
    var label: String {
        switch self {
        case .machineBed: return "Machine bed"
        case .spoilboard(let t): return "Spoilboard \(t.formatted()) mm"
        }
    }
}

/// The request body for one operation. Written out by hand rather than derived
/// from `CNCSetup` so that a field the kernel does not accept cannot be sent by
/// accident, and so an absent key really means "the kernel's default".
struct CNCJobOperationRequest: Encodable, Sendable {
    var name: String
    var tool = 1
    var kind: CNCOpKind
    var contour: [[Double]]?
    var rectangle: [Double]?
    var x: Double?
    var y: Double?
    var diameter: Double?
    var pitch: Double?
    var depth: Double
    var stepdown: Double
    var stepover: Double?
    var feed: Double
    var plunge: Double
    var rpm: Double
    var direction: String?
    var entry: String?
    var rampAngle: Double?
    var leadIn: Bool?
    var stockToLeave: Double?
    var finishStepdowns: Int?
    var springPass: Bool?
    var finishFeed: Double?
    var tabs: Int?
    /// Where each tab goes, as a fraction of the way round the loop. When this
    /// is present it *is* the tab list and `tabs` is left out.
    var tabPositions: [Double]?
    var tabWidth: Double?
    var tabHeight: Double?
    var bottomAllowance: Double?
    var thinSlot: ThinSlot?
    var through: Bool?
    var order: Int?
    var role: String?
    /// When this operation runs, lower first. An operation without one keeps
    /// the phase its role implies, so the profile that frees the part still
    /// goes last unless something says otherwise in as many words.
    var phase: Int?
    /// Material a pocket keeps: closed loops the cutter clears around.
    var islands: [[[Double]]]?

    struct ThinSlot: Encodable, Sendable {
        var strategy: String
        var tolerance: Double?
    }

    enum CodingKeys: String, CodingKey {
        case name, tool, kind, contour, rectangle, x, y, diameter, pitch
        case depth, stepdown, stepover, feed, plunge, rpm, direction, entry
        case rampAngle = "ramp_angle"
        case leadIn = "lead_in"
        case stockToLeave = "stock_to_leave"
        case finishStepdowns = "finish_stepdowns"
        case springPass = "spring_pass"
        case finishFeed = "finish_feed"
        case tabs
        case tabPositions = "tab_positions"
        case tabWidth = "tab_width"
        case tabHeight = "tab_height"
        case bottomAllowance = "bottom_allowance"
        case thinSlot = "thin_slot"
        case through, order, role, phase, islands
    }
}

struct CNCJobToolRequest: Encodable, Sendable {
    var number = 1
    var kind = "flat_end_mill"
    var diameter: Double
    var flutes = 2
    var fluteLength: Double?
    var stickout: Double?
    var centreCutting = true
    enum CodingKeys: String, CodingKey {
        case number, kind, diameter, flutes, stickout
        case fluteLength = "flute_length"
        case centreCutting = "centre_cutting"
    }
}

struct CNCJobStockRequest: Encodable, Sendable {
    var thickness: Double
    var margin: Double?
    var spoilboard: Double?
}

/// Soft limits in machine coordinates, for the envelope check.
struct CNCJobTravelRequest: Encodable, Sendable {
    var min: [Double]
    var max: [Double]
}

/// The Anolex 4030 Ultra 2, which is the machine this app drives. A trim
/// router on a dial: its `S` word does nothing, so the feeds table must never
/// assume a commanded spindle speed.
struct CNCJobMachineRequest: Encodable, Sendable {
    var name = "Anolex 4030 Ultra 2"
    var spindle = "dial"
    var `class` = "hobby"
    var maxFeed = 3000.0
    var maxAccel = 300.0
    /// Travel limits, when the machine has reported them. Absent means the
    /// envelope is checked against the stock alone, as before.
    var travel: CNCJobTravelRequest?
    /// Where G54 sits in machine coordinates, for the same check.
    var workOffset: [Double]?
    enum CodingKeys: String, CodingKey {
        case name, spindle, `class`, travel
        case maxFeed = "max_feed"
        case maxAccel = "max_accel"
        case workOffset = "work_offset"
    }
}

/// Where the part sits on the stock: rotation first, about work zero.
struct CNCJobPlacementRequest: Encodable, Sendable {
    var dx: Double
    var dy: Double
    var rotationDeg: Double
    enum CodingKeys: String, CodingKey {
        case dx, dy
        case rotationDeg = "rotation_deg"
    }
}

struct CNCJobPartRequest: Encodable, Sendable {
    var outer: [[Double]]
    var holes: [[[Double]]]
}

struct CNCJobOptionsRequest: Encodable, Sendable {
    var arcFit: ArcFit? = ArcFit(tolerance: 0.005)
    var toolChange = ToolChange()
    var spinUpSeconds = 3.0
    var parkZ = 5.0
    var safeZ = 5.0
    var wcs = "G54"
    var verify = true
    var part: CNCJobPartRequest?
    /// Moves every operation *and* the part the job is verified against, so a
    /// placed job is still checked against the metal it really cuts.
    var placement: CNCJobPlacementRequest?

    struct ArcFit: Encodable, Sendable { var tolerance: Double }
    /// No changer on this machine: a tool change is an operator stop and a
    /// re-probe, never an `M6` the controller would silently ignore.
    struct ToolChange: Encodable, Sendable { var type = "manual_pause_reprobe" }

    enum CodingKeys: String, CodingKey {
        case arcFit = "arc_fit"
        case toolChange = "tool_change"
        case spinUpSeconds = "spin_up_seconds"
        case parkZ = "park_z"
        case safeZ = "safe_z"
        case wcs, verify, part, placement
    }
}

struct CNCJobRequest: Encodable, Sendable {
    var name = "vcad job"
    var stock: CNCJobStockRequest
    var machine = CNCJobMachineRequest()
    var tools: [CNCJobToolRequest]
    var operations: [CNCJobOperationRequest]
    var options: CNCJobOptionsRequest
}

// MARK: - Response

struct CNCJobDuration: Decodable, Sendable {
    var naiveS = 0.0
    var accelAwareS = 0.0
}

/// One block's slice of `moves`. The ranges partition `moves`, so summing their
/// seconds is the job's time and slicing by one is exactly that block's motion.
struct CNCOpRange: Decodable, Sendable, Equatable {
    var block = ""
    var name: String?
    var opIndex: Int?
    var tool: Int?
    var start = 0
    var end = 0
    var seconds = 0.0
}

enum CNCSeverity: String, Decodable, Sendable {
    case error = "Error"
    case warning = "Warning"
}

struct CNCViolation: Decodable, Sendable, Equatable {
    var index = 0
    var xy: [Double] = [0, 0]
    var z = 0.0
    var value = 0.0
    var what = ""
}

struct CNCCheckReport: Decodable, Sendable {
    var name = ""
    var pass = true
    var severity: CNCSeverity = .error
    var violationCount = 0
    var worst = 0.0
    var examples: [CNCViolation] = []
    var note = ""
    /// The violation that decided the verdict.
    var worstExample: CNCViolation? { examples.first }
}

struct CNCMaterialLeftReport: Decodable, Sendable {
    var check = CNCCheckReport()
    var unsweptArea = 0.0
    var maxStandoff = 0.0
    var untouchedWalls = 0
    var walls = 0
}

struct CNCDepthReport: Decodable, Sendable {
    var check = CNCCheckReport()
    var deepestZ = 0.0
    var floorZ = 0.0
    /// Negative means the cutter went past the stock underside.
    var remainingUnderPart = 0.0
    var features = 0
}

/// One lifted stretch of one pass: a tab, as the moves actually cut it.
struct CNCTabObservation: Decodable, Sendable {
    var xy: [Double] = [0, 0]
    var path: [[Double]] = []
    var topZ = 0.0
    var passZ = 0.0
    var metalWidth = 0.0
    var height = 0.0
    var straight = true
    var passIndex = 0
}

struct CNCTabAudit: Decodable, Sendable {
    var check = CNCCheckReport()
    var observations: [CNCTabObservation] = []
    var tabCount = 0
    var passesBelowTabs = 0
}

struct CNCEnvelopeReport: Decodable, Sendable {
    var check = CNCCheckReport()
    var workMin: [Double] = [0, 0, 0]
    var workMax: [Double] = [0, 0, 0]
    var stockMargin: [Double] = [0, 0, 0, 0]
}

struct CNCFreedPiece: Decodable, Sendable {
    var area = 0.0
    var centroid: [Double] = [0, 0]
    var isPart = false
}

struct CNCLooseReport: Decodable, Sendable {
    var check = CNCCheckReport()
    var pieces: [CNCFreedPiece] = []
    var skinHolds = true
}

/// The whole answer: one job, eight checks.
struct CNCVerification: Decodable, Sendable {
    var pass = false
    var gouge = CNCCheckReport()
    var materialLeft = CNCMaterialLeftReport()
    var rapids = CNCCheckReport()
    var depth = CNCDepthReport()
    var tabs = CNCTabAudit()
    var envelope = CNCEnvelopeReport()
    var loose = CNCLooseReport()
    var plunges = CNCCheckReport()
    var moves = 0

    /// Every check, in the order a machinist reads them.
    var checks: [CNCCheckReport] {
        [gouge, materialLeft.check, rapids, depth.check, tabs.check, envelope.check, loose.check, plunges]
    }
    func check(named name: String) -> CNCCheckReport? { checks.first { $0.name == name } }
}

/// `vcad_cam_verify_gcode` answers a policy without a `warnings` list — it has
/// no operation settings to warn about — so neither list is required here.
struct CNCJobPolicy: Decodable, Sendable {
    var verified = false
    var replayed: String?
    var blockedBy: [String] = []
    var warnings: [String] = []

    init() {}
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        verified = try c.decodeIfPresent(Bool.self, forKey: .verified) ?? false
        replayed = try c.decodeIfPresent(String.self, forKey: .replayed)
        blockedBy = try c.decodeIfPresent([String].self, forKey: .blockedBy) ?? []
        warnings = try c.decodeIfPresent([String].self, forKey: .warnings) ?? []
    }
    private enum CodingKeys: String, CodingKey { case verified, replayed, blockedBy, warnings }
}

/// The name of a tagged kernel enum, however it was tagged. A unit variant
/// arrives as `"StickoutUnknown"`, one carrying numbers as
/// `{"ShankRub": {…}}`; only the name is wanted here, because the check
/// already carries a sentence written for a machinist.
struct CNCKindName: Decodable, Sendable, Equatable {
    var name: String
    init(from decoder: Decoder) throws {
        let single = try decoder.singleValueContainer()
        if let text = try? single.decode(String.self) { name = text; return }
        let object = try decoder.container(keyedBy: AnyKey.self)
        name = object.allKeys.first?.stringValue ?? ""
    }
    private struct AnyKey: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
    }
}

struct CNCToolCheck: Decodable, Sendable {
    var op = ""
    var opIndex = 0
    var tool = 1
    var severity = ""
    var kind = CNCKindName(name: "")
    var message = ""
}

extension CNCKindName {
    init(name: String) { self.name = name }
}

struct CNCJobNote: Decodable, Sendable {
    var level = ""
    var text = ""
}

struct CNCContourReport: Decodable, Sendable {
    var finalDepth = 0.0
    var roughPasses = 0
    var finishPasses = 0
    var springPass = false
    var rampEntries = 0
    var leadEntries = 0
    var plungeEntries = 0
    var maxWallError = 0.0
}

struct CNCCornerSummary: Decodable, Sendable {
    var count = 0
    var totalArea = 0.0
    var maxStandoff = 0.0
}

struct CNCFitReport: Decodable, Sendable {
    var toolDiameter = 0.0
    var fits = true
    var largestToolDiameter: Double?
    var slotClearancePerSide: Double?
    var unreachable = CNCCornerSummary()
}

/// Where one tab really ended up, measured off the toolpath.
///
/// `alongContour` is a fraction of the way round the contour **as it was
/// drawn**, while the fraction asked for in `tab_positions` is stated on the
/// cutter's own offset loop. The two frames differ by the direction of cut and
/// by drift round every corner, so the app never subtracts one from the other:
/// it drags to a place, reads back where the tab landed, and corrects.
struct CNCTabLanding: Decodable, Sendable {
    var at: [Double] = [0, 0]
    var alongContour = 0.0
    var gapToNextMm = 0.0
    var metalWidth = 0.0
    var height = 0.0
    var straight = true
    var passes = 0
}

struct CNCTabPlacement: Decodable, Sendable {
    var op = ""
    var opIndex = 0
    var requested = 0
    var requestedPositions: [Double] = []
    var found = 0
    var perimeterMm = 0.0
    var tabs: [CNCTabLanding] = []
}

/// How close the cutter came to material a pocket was told to keep.
struct CNCIslandClearance: Decodable, Sendable {
    struct Island: Decodable, Sendable {
        var island = 0
        var centreClearanceMm: Double?
        var cutIntoMm = 0.0
        var kept = true
    }
    var op = ""
    var opIndex = 0
    var toleranceMm = 0.0
    var islands: [Island] = []
}

struct CNCReportEntry<Body: Decodable & Sendable>: Decodable, Sendable {
    var op = ""
    var opIndex = 0
    var side: String?
    var report: Body
}

/// What `vcad_cam_job` answered. Three shapes arrive through this one type:
/// a validation refusal (`error`, nothing else), a tool-geometry refusal
/// (`blocked`, `error`, `toolChecks`), and a built job (everything), whose
/// `gcode` is present only when nothing blocked it.
struct CNCJobResult: Decodable, Sendable {
    var name: String?
    var blocked: Bool?
    var error: String?
    var gcode: String?
    var moves: [CNCMove] = []
    var opRanges: [CNCOpRange] = []
    var duration = CNCJobDuration()
    var toolChecks: [CNCToolCheck] = []
    var verification: CNCVerification?
    var policy = CNCJobPolicy()
    var fit: [CNCReportEntry<CNCFitReport>] = []
    var report: [CNCReportEntry<CNCContourReport>] = []
    var tabPlacement: [CNCTabPlacement] = []
    var islandClearance: [CNCIslandClearance] = []
    var notes: [CNCJobNote] = []

    init() {}

    // Three response shapes come through this one type and they do not share a
    // key: a validation refusal carries only `error`, a tool-geometry refusal
    // has no `moves`, and a blocked job has no `gcode` — that absence being the
    // whole gate. So every key is optional here on purpose. A *decode* failure
    // would be a schema drift worth hearing about, but a missing key is not.
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        blocked = try c.decodeIfPresent(Bool.self, forKey: .blocked)
        error = try c.decodeIfPresent(String.self, forKey: .error)
        gcode = try c.decodeIfPresent(String.self, forKey: .gcode)
        moves = try c.decodeIfPresent([CNCMove].self, forKey: .moves) ?? []
        opRanges = try c.decodeIfPresent([CNCOpRange].self, forKey: .opRanges) ?? []
        duration = try c.decodeIfPresent(CNCJobDuration.self, forKey: .duration) ?? CNCJobDuration()
        toolChecks = try c.decodeIfPresent([CNCToolCheck].self, forKey: .toolChecks) ?? []
        verification = try c.decodeIfPresent(CNCVerification.self, forKey: .verification)
        policy = try c.decodeIfPresent(CNCJobPolicy.self, forKey: .policy) ?? CNCJobPolicy()
        fit = try c.decodeIfPresent([CNCReportEntry<CNCFitReport>].self, forKey: .fit) ?? []
        report = try c.decodeIfPresent([CNCReportEntry<CNCContourReport>].self, forKey: .report) ?? []
        tabPlacement = try c.decodeIfPresent([CNCTabPlacement].self, forKey: .tabPlacement) ?? []
        islandClearance = try c.decodeIfPresent([CNCIslandClearance].self, forKey: .islandClearance) ?? []
        notes = try c.decodeIfPresent([CNCJobNote].self, forKey: .notes) ?? []
    }

    private enum CodingKeys: String, CodingKey {
        case name, blocked, error, gcode, moves, opRanges, duration, toolChecks
        case verification, policy, fit, report, notes, tabPlacement, islandClearance
    }

    /// True when the oracle refused the job, or the request never got that far.
    var isBlocked: Bool { blocked ?? (error != nil) }

    /// Moves belonging to one request-operation index.
    func ranges(forOperation index: Int) -> [CNCOpRange] {
        opRanges.filter { $0.opIndex == index }
    }
}

// MARK: - The call

enum CNCJobBridge {
    /// Run a job request through `vcad_cam_job`.
    ///
    /// A refusal is a document, not a null: everything the kernel learned comes
    /// back either way, so the app can say *which* operation is wrong rather
    /// than "CAM failed".
    nonisolated static func run(_ request: CNCJobRequest) throws -> CNCJobResult {
        let encoder = JSONEncoder()
        let data = try encoder.encode(request)
        let text = String(decoding: data, as: UTF8.self)
        return try text.withCString { pointer in
            guard let raw = vcad_cam_job(pointer) else {
                throw CNCError.message("The CAM kernel returned nothing for this job.")
            }
            defer { vcad_cam_free(raw) }
            return try decode(String(cString: raw), what: "job")
        }
    }

    /// Decode an answer, and say what was in it when that fails. A schema that
    /// drifted is worth a sentence naming the field, not "the data couldn't be
    /// read" over a document that may well have carried a perfectly good
    /// refusal.
    nonisolated static func decode(_ text: String, what: String) throws -> CNCJobResult {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            return try decoder.decode(CNCJobResult.self, from: Data(text.utf8))
        } catch {
            if let object = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
               let message = object["error"] as? String {
                throw CNCError.message(message)
            }
            throw CNCError.message("The CAM \(what) answer could not be read: \(error). It began: \(text.prefix(400))")
        }
    }

    /// Replay a G-code program that the app did not generate against the part
    /// it is meant to make. Used for an imported program when an outline is
    /// loaded; without one there is nothing to verify against and the program
    /// stays marked unverified.
    nonisolated static func verify(gcode: String,
                                   part: CNCJobPartRequest,
                                   stock: CNCJobStockRequest,
                                   toolDiameter: Double,
                                   bottomAllowance: Double) throws -> CNCJobResult {
        struct Request: Encodable {
            var gcode: String
            var part: CNCJobPartRequest
            var stock: CNCJobStockRequest
            var toolDiameter: Double
            var bottomAllowance: Double
            enum CodingKeys: String, CodingKey {
                case gcode, part, stock
                case toolDiameter = "tool_diameter"
                case bottomAllowance = "bottom_allowance"
            }
        }
        let data = try JSONEncoder().encode(Request(gcode: gcode, part: part, stock: stock,
                                                    toolDiameter: toolDiameter,
                                                    bottomAllowance: bottomAllowance))
        return try String(decoding: data, as: UTF8.self).withCString { pointer in
            guard let raw = vcad_cam_verify_gcode(pointer) else {
                throw CNCError.message("The CAM kernel returned nothing for this program.")
            }
            defer { vcad_cam_free(raw) }
            return try decode(String(cString: raw), what: "verification")
        }
    }
}
