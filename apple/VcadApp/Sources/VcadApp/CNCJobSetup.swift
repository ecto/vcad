import Foundation
import CoreGraphics
import CVcadFFI

// Setting a job up: what the stock is made of, what the cutter is, where the
// blank sits on the table and where its zero is, what is clamped on top of it,
// and how to get an outline out of the part on screen instead of a DXF that
// may be a different part altogether.
//
// Friction-log items 16, 17, 18, 37, 41, 52, 53 and 54. Item 54 in one line:
// *"The app has no material setting at all — feeds were typed by hand for a
// material that turned out to be a different one."*

// MARK: - Material and feeds

/// One material in the kernel's table. The `id` is what a document stores.
struct CNCMaterial: Decodable, Identifiable, Sendable, Hashable {
    var id: String
    var name: String
    var family: String
    /// How hard it is on a light machine — the order the picker lists them in.
    var difficultyRank: Int
    var coolant: String

    /// "Flood", "MistOrAir", "None" — the kernel's own words, in the user's.
    var coolantAdvice: String? {
        switch coolant {
        case "None": return nil
        case "Flood": return "wants flood coolant"
        case "MistOrAir", "Mist", "Air": return "wants mist or an air blast"
        case "Lubricant", "Wax": return "wants a lubricant on the cutter"
        default: return "coolant: \(coolant.lowercased())"
        }
    }
}

/// How loudly the kernel means a note. The names are its own.
enum CNCNoteLevel: String, Decodable, Sendable, Comparable {
    case info = "Info", caution = "Caution", warning = "Warning", danger = "Danger"
    private var rank: Int {
        switch self {
        case .info: return 0
        case .caution: return 1
        case .warning: return 2
        case .danger: return 3
        }
    }
    static func < (a: Self, b: Self) -> Bool { a.rank < b.rank }
    var symbol: String {
        switch self {
        case .info: return "info.circle"
        case .caution: return "exclamationmark.circle"
        case .warning: return "exclamationmark.triangle.fill"
        case .danger: return "xmark.octagon.fill"
        }
    }
}

struct CNCFeedNote: Decodable, Sendable, Identifiable, Hashable {
    var level: CNCNoteLevel
    var text: String
    var id: String { "\(level.rawValue)-\(text)" }
}

/// What `vcad_cam_recommend` worked out, in the fields the operation uses.
struct CNCRecommendation: Decodable, Sendable {
    var materialId = ""
    var rpm = 0.0
    /// The dial position to set by hand. On this spindle the `S` word does
    /// nothing at all, so a recommendation without this is unusable.
    var dial: String?
    var chiploadMm = 0.0
    var feedMmMin = 0.0
    var plungeMmMin = 0.0
    var rampAngleDeg = 3.0
    var stepdownMm = 0.0
    var stepoverMm = 0.0
    var finishAllowanceMm = 0.0
    var notes: [CNCFeedNote] = []
}

/// The whole answer, with the material and machine it was worked out for.
struct CNCFeedAdvice: Decodable, Sendable {
    struct Named: Decodable, Sendable { var id = ""; var name = "" }
    var material = Named()
    /// True when the spindle has no `S` word worth sending.
    var dialSpindle = false
    var recommendation = CNCRecommendation()

    /// The numbers as the operation will hold them, with the first-cut derate
    /// applied. Feed and stepdown are the two that decide whether a first cut
    /// on an unknown machine survives, so they are the two that are derated.
    func values(derated: Bool) -> (feed: Double, plunge: Double, stepdown: Double,
                                   stepover: Double, rpm: Double) {
        let scale = derated ? 0.6 : 1.0
        return (feed: recommendation.feedMmMin * scale,
                plunge: recommendation.plungeMmMin,
                stepdown: recommendation.stepdownMm * scale,
                stepover: recommendation.stepoverMm,
                rpm: recommendation.rpm)
    }

    /// What to set on the router, said the way the machine works: the dial,
    /// because the number in the G-code does nothing.
    var spindleAdvice: String {
        guard dialSpindle else {
            return "\(Int(recommendation.rpm.rounded())) rpm, commanded by the S word."
        }
        guard let dial = recommendation.dial else {
            return "This router's speed is a dial, not an S word: the spindle number in the G-code does nothing."
        }
        return "Set the router dial to \(dial) (about \(Int(recommendation.rpm.rounded())) rpm). The S word in the G-code does nothing on this spindle."
    }
}

/// A second opinion on numbers that were typed rather than recommended.
struct CNCFeedCheck: Decodable, Sendable {
    var ok = true
    var worstLevel: CNCNoteLevel?
    var notes: [CNCFeedNote] = []
}

// MARK: - Where the job sits on the metal

/// Where the operator will set G54. Everything in a job is measured from it,
/// so choosing it is choosing what the numbers in the program mean.
enum CNCZeroLocation: String, CaseIterable, Identifiable, Sendable {
    /// The part's own bounding-box corner — inside the blank, and hard to
    /// touch off (friction-log item 41).
    case partCorner = "part"
    /// The blank's lower-left corner: an edge you can actually find.
    case stockCorner = "stock"
    /// The middle of the blank, for a part centred on round stock.
    case stockCentre = "centre"

    var id: String { rawValue }
    var label: String {
        switch self {
        case .partCorner: return "Part corner"
        case .stockCorner: return "Stock corner"
        case .stockCentre: return "Stock centre"
        }
    }
    var detail: String {
        switch self {
        case .partCorner: return "Lower-left of the part's bounding box — inside the blank, so it needs an edge finder or a measurement."
        case .stockCorner: return "Lower-left corner of the blank. The easiest thing on the table to touch off."
        case .stockCentre: return "The middle of the blank."
        }
    }
}

/// Where the part sits on the stock, as the job request states it.
struct CNCPlacement: Equatable, Sendable {
    var dx = 0.0
    var dy = 0.0
    var rotationDeg = 0.0
    var isIdentity: Bool { dx == 0 && dy == 0 && rotationDeg == 0 }

    /// This placement with `offset` applied before it — the offset turns with
    /// the part, because a blank clamped crooked turns about the point the
    /// operator zeroed on, carrying everything on it round with it.
    func moved(by offset: [Double]) -> CNCPlacement {
        let a = rotationDeg * .pi / 180
        return CNCPlacement(dx: dx + offset[0] * cos(a) - offset[1] * sin(a),
                            dy: dy + offset[0] * sin(a) + offset[1] * cos(a),
                            rotationDeg: rotationDeg)
    }
    func apply(_ p: [Double]) -> [Double] {
        let a = rotationDeg * .pi / 180
        return [p[0] * cos(a) - p[1] * sin(a) + dx,
                p[0] * sin(a) + p[1] * cos(a) + dy]
    }
}

/// A clamp, toe or screw head on the blank: a rectangle the cutter may not
/// sweep through. The job request has no clamp field, so this is checked here
/// and reported as a warning — an honest "the app checked this", not a claim
/// that the kernel did.
struct CNCClamp: Identifiable, Equatable, Sendable {
    var id = UUID()
    /// Lower-left corner in the work frame (millimetres from zero).
    var x = 0.0
    var y = 0.0
    var width = 30.0
    var height = 20.0
    var name = "Clamp"

    var maxX: Double { x + width }
    var maxY: Double { y + height }
    /// Whether this clamp stands in a rectangle the cutter sweeps.
    func overlaps(_ rect: [Double]) -> Bool {
        guard rect.count >= 4 else { return false }
        return x < rect[2] && maxX > rect[0] && y < rect[3] && maxY > rect[1]
    }
    /// How far into the sweep it stands, in millimetres, on the shallower axis.
    func overlapDepth(_ rect: [Double]) -> Double {
        guard overlaps(rect) else { return 0 }
        return min(min(maxX, rect[2]) - max(x, rect[0]), min(maxY, rect[3]) - max(y, rect[1]))
    }
}

// MARK: - The part on screen

/// The document the Manufacture workspace should section: the bytes the kernel
/// evaluates, exactly as the editor would hand them over.
struct CNCModelDocument: Sendable {
    var data: Data
    /// `.loon` source rather than a `.vcad` document.
    var isLoon: Bool
    var name: String
    var partCount: Int?
}

/// What a section at Z came back with.
struct CNCSection: Decodable, Sendable {
    struct Region: Decodable, Sendable {
        var outer: [[Double]] = []
        var holes: [[[Double]]] = []
        var area = 0.0
    }
    struct Circle: Decodable, Sendable {
        var region = 0
        var hole = 0
        var center: [Double] = [0, 0]
        var diameter = 0.0
    }
    struct Gap: Decodable, Sendable {
        var distance = 0.0
    }
    /// `raw_tessellation`, `export_mesh` or `cached_root_mesh`.
    var meshSource = ""
    var z = 0.0
    var zRange: [Double] = [0, 0]
    var suggestedStockThickness = 0.0
    /// Gaps that were closed because they were inside the heal tolerance.
    /// Non-empty means the mesh was not watertight at this height — worth
    /// knowing even when the section closed.
    var healed: [Gap] = []
    var regions: [Region] = []
    var circles: [Circle] = []
    var prismatic: Prismatic?
    /// Present only when the solid is torn at this plane.
    var error: String?
    var gaps: [Gap]?

    /// Is this part the same shape all the way up, and by how much is it not?
    struct Prismatic: Decodable, Sendable {
        var prismatic: Bool?
        /// The worst disagreement between two heights (mm).
        var maxBoundaryDistance: Double?
        var worstZ: Double?
        /// Why the question could not be answered at all.
        var refused: String?
    }

    var isTorn: Bool { error != nil }
    /// The largest region: the part, rather than a stray sliver.
    var part: Region? { regions.max { $0.area < $1.area } }

    /// The verdict in one sentence, for the import summary.
    var prismaticVerdict: String {
        guard let p = prismatic else { return "Constant section was not checked." }
        if let refused = p.refused { return "Not a constant section: \(refused)" }
        if p.prismatic == true {
            return "Constant section all the way through — one contour machines the whole part."
        }
        let by = p.maxBoundaryDistance.map { " (out by \(CNCVerdictText.mm($0, 3)) mm" } ?? ""
        let at = p.worstZ.map { " at Z \(CNCVerdictText.mm($0, 2)))" } ?? (by.isEmpty ? "" : ")")
        return "This part is not the same shape at every height\(by)\(at): a contour at one plane will not make all of it."
    }
    var meshSourceNote: String {
        switch meshSource {
        case "raw_tessellation": return "sectioned from the solid itself"
        case "cached_root_mesh": return "sectioned from the cached mesh — no topology, so the wall is as exact as that mesh"
        case "export_mesh": return "sectioned from the export mesh, which is repaired for printing and can differ from the solid by a few tenths"
        default: return meshSource
        }
    }
    /// The refusal, with the numbers that make it actionable.
    var tornMessage: String? {
        guard let error else { return nil }
        let widest = (gaps ?? []).map(\.distance).max() ?? 0
        return "\(error) \(counted((gaps ?? []).count, "gap")), the widest \(CNCVerdictText.mm(widest, 3)) mm."
    }
}

/// What `vcad_cam_compare_outline` made of a DXF against the part on screen.
struct CNCOutlineComparison: Decodable, Sendable {
    struct Diff: Decodable, Sendable {
        var maxBoundaryDistance = 0.0
        var meanBoundaryDistance: Double?
        var areaDifference: Double?
        var unmatchedA: [Int] = []
        var unmatchedB: [Int] = []
    }
    var agrees = false
    var tolerance = 0.0
    var diff = Diff()
    var note = ""

    /// The warning the user has to acknowledge, with its numbers.
    var warning: String {
        let holes = diff.unmatchedA.count + diff.unmatchedB.count
        return "This outline is not the part on screen: boundaries up to \(CNCVerdictText.mm(diff.maxBoundaryDistance, 3)) mm apart (allowed \(CNCVerdictText.mm(tolerance, 3)) mm)"
            + (holes > 0 ? ", \(counted(holes, "hole")) on one side only." : ".")
            + " Machining one as if it were the other cuts the wrong part."
    }
}

// MARK: - The calls

/// The CAM entry points this package uses, each one JSON in and JSON out.
enum CNCCam {
    nonisolated static func decode<T: Decodable>(_ text: String, as type: T.Type, what: String) throws -> T {
        // A refusal is a document, so it is thrown as the sentence it carries
        // rather than as a decoding failure. A *section* is the exception: its
        // refusal comes with the gaps that caused it, and those are the whole
        // point — so it is decoded and the caller reads `error` and `gaps`.
        if let object = try? JSONSerialization.jsonObject(with: Data(text.utf8)) as? [String: Any],
           let message = object["error"] as? String, T.self != CNCSection.self {
            throw CNCError.message(message)
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do { return try decoder.decode(T.self, from: Data(text.utf8)) }
        catch {
            throw CNCError.message("The CAM \(what) answer could not be read: \(error). It began: \(text.prefix(300))")
        }
    }

    private nonisolated static func call(_ entry: (UnsafePointer<CChar>?) -> UnsafeMutablePointer<CChar>?,
                                         _ request: String, what: String) throws -> String {
        try request.withCString { pointer in
            guard let raw = entry(pointer) else {
                throw CNCError.message("The CAM kernel returned nothing for this \(what).")
            }
            defer { vcad_cam_free(raw) }
            return String(cString: raw)
        }
    }

    /// The material table.
    nonisolated static func materials() throws -> [CNCMaterial] {
        guard let raw = vcad_cam_materials() else {
            throw CNCError.message("The CAM kernel returned no material table.")
        }
        defer { vcad_cam_free(raw) }
        struct Table: Decodable { var materials: [CNCMaterial] }
        return try decode(String(cString: raw), as: Table.self, what: "material table").materials
            .sorted { $0.difficultyRank < $1.difficultyRank }
    }

    nonisolated static func recommend(_ request: [String: Any]) throws -> CNCFeedAdvice {
        let data = try JSONSerialization.data(withJSONObject: request)
        let text = try call(vcad_cam_recommend, String(decoding: data, as: UTF8.self), what: "feeds request")
        return try decode(text, as: CNCFeedAdvice.self, what: "feeds")
    }

    nonisolated static func checkFeeds(_ request: [String: Any]) throws -> CNCFeedCheck {
        let data = try JSONSerialization.data(withJSONObject: request)
        let text = try call(vcad_cam_check_feeds, String(decoding: data, as: UTF8.self), what: "feed check")
        return try decode(text, as: CNCFeedCheck.self, what: "feed check")
    }

    nonisolated static func compareOutline(_ request: [String: Any]) throws -> CNCOutlineComparison {
        let data = try JSONSerialization.data(withJSONObject: request)
        let text = try call(vcad_cam_compare_outline, String(decoding: data, as: UTF8.self), what: "outline comparison")
        return try decode(text, as: CNCOutlineComparison.self, what: "outline comparison")
    }

    /// Section a part of the document on screen at its own mid-height.
    ///
    /// The scene is built and freed here: the editor keeps no resident scene,
    /// and sectioning one is a few tenths of a second on a solved document.
    nonisolated static func section(_ document: CNCModelDocument,
                                    partIndex: Int,
                                    z: Double = 0,
                                    autoZ: Bool = true,
                                    options: String = "{}") throws -> CNCSection {
        let scene: OpaquePointer? = document.data.withUnsafeBytes { raw -> OpaquePointer? in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else { return nil }
            return document.isLoon
                ? vcad_scene_from_loon(base, document.data.count)
                : vcad_scene_from_json(base, document.data.count)
        }
        guard let scene else {
            throw CNCError.message("\(document.name) could not be evaluated, so there is no solid to take an outline from.")
        }
        defer { vcad_scene_free(scene) }
        let count = vcad_scene_part_count(scene)
        guard count > 0 else {
            throw CNCError.message("\(document.name) has no parts to take an outline from.")
        }
        guard partIndex < count else {
            throw CNCError.message("This document has \(counted(count, "part")), so there is no part \(partIndex + 1).")
        }
        guard let raw = vcad_cam_outline_from_scene(scene, partIndex, z, autoZ ? 1 : 0, options) else {
            throw CNCError.message("The CAM kernel returned nothing for this section.")
        }
        defer { vcad_cam_free(raw) }
        return try decode(String(cString: raw), as: CNCSection.self, what: "section")
    }
}

extension EditorModel {
    /// The document the CAM workspace should section, as the kernel would
    /// evaluate it: the edited JSON when there is one, the file on disk
    /// otherwise, or the loon the intent bar generated.
    func camDocument() -> CNCModelDocument? {
        switch source {
        case .sandbox, .gripper:
            return nil
        case .document(let path, let label):
            let data = documentJSON.flatMap(DocEdit.serialize)
                ?? (try? Data(contentsOf: URL(fileURLWithPath: path)))
            return data.map { CNCModelDocument(data: $0, isLoon: false, name: label) }
        case .generated(let loon, let label):
            return CNCModelDocument(data: Data(loon.utf8), isLoon: true, name: label)
        }
    }
}

// MARK: - The machine's own numbers, if it has any yet

/// Travel limits, work offset and measured skew, read off `cnc.machine` if it
/// carries them.
///
/// The machine profile is another package's work and may not have landed. This
/// reads it reflectively rather than naming a type that might not exist, so
/// this package compiles either way and starts using the numbers the moment
/// they appear. Everything is optional: a missing profile means the job asks
/// for no travel check, which is what it did before.
struct CNCMachineProfile: Sendable {
    var travelMin: [Double]?
    var travelMax: [Double]?
    var workOffset: [Double]?
    var skewDegrees: Double?

    var hasTravel: Bool { travelMin?.count == 3 && travelMax?.count == 3 }

    /// Read whatever of a profile the machine happens to expose.
    static func read(_ machine: Any) -> CNCMachineProfile {
        var out = CNCMachineProfile()
        guard let profile = child(of: machine, named: "profile") else { return out }
        if let travel = child(of: profile, named: "travel") {
            let (lo, hi) = axisLimits(travel)
            out.travelMin = lo
            out.travelMax = hi
        }
        if let offset = child(of: profile, named: "workOffset") ?? child(of: profile, named: "work_offset") {
            out.workOffset = numbers(offset)
        }
        if let skew = child(of: profile, named: "skewDegrees") ?? child(of: profile, named: "skew_degrees") {
            out.skewDegrees = (skew as? Double) ?? (skew as? Float).map(Double.init)
        }
        return out
    }

    /// One stored property by name, unwrapped if it is an optional. Observation
    /// renames stored properties to `_name`, so both spellings are tried.
    private static func child(of value: Any, named name: String) -> Any? {
        for candidate in [name, "_\(name)"] {
            for child in Mirror(reflecting: value).children where child.label == candidate {
                return unwrap(child.value)
            }
        }
        return nil
    }
    private static func unwrap(_ value: Any) -> Any? {
        let mirror = Mirror(reflecting: value)
        guard mirror.displayStyle == .optional else { return value }
        return mirror.children.first.map { unwrap($0.value) } ?? nil
    }

    /// `[Double]`, or anything with x/y/z, as three numbers.
    private static func numbers(_ value: Any) -> [Double]? {
        if let v = value as? [Double] { return v }
        if let v = value as? [Float] { return v.map(Double.init) }
        let mirror = Mirror(reflecting: value)
        let axes = ["x", "y", "z"].compactMap { name -> Double? in
            mirror.children.first { $0.label == name }.flatMap { $0.value as? Double }
        }
        return axes.count == 3 ? axes : nil
    }

    /// Travel as `[[min, max]]` per axis, or as a pair of triples.
    private static func axisLimits(_ value: Any) -> ([Double]?, [Double]?) {
        if let pairs = value as? [[Double]], pairs.count == 3, pairs.allSatisfy({ $0.count == 2 }) {
            return (pairs.map { $0[0] }, pairs.map { $0[1] })
        }
        if let lo = child(of: value, named: "min").flatMap(numbers),
           let hi = child(of: value, named: "max").flatMap(numbers) {
            return (lo, hi)
        }
        return (nil, nil)
    }
}
