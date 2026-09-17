import Foundation

// The verdict, in a machinist's words.
//
// `policy.blocked_by` names checks — "gouge", "depth", "loose_pieces". Nobody
// standing at a machine wants to read that, and the friction log's whole
// lesson is that a refusal without a reason is as useless as no refusal at
// all. So every blocked check becomes one sentence built from the check's own
// worst violation: what happened, how far, and where.

/// One reason a job will not run, or one thing worth acknowledging first.
struct CNCFinding: Identifiable, Sendable, Equatable {
    /// The check's name, which is also the acknowledgement key.
    var id: String
    /// One sentence, in millimetres and part coordinates.
    var text: String
    /// Where in the stock frame, when the check found a place.
    var xy: [Double]?
    /// Z there.
    var z: Double?
    /// The operation whose moves come nearest that place.
    var operationID: UUID?
    var blocking: Bool
}

/// What one check came to, for the Verification list.
struct CNCCheckRow: Identifiable, Sendable {
    enum Verdict: Sendable { case pass, warning, blocked, notRun }
    var id: String
    var title: String
    var verdict: Verdict
    /// The number the check turns on, already in its own unit.
    var value: String
    var detail: String

    var symbol: String {
        switch verdict {
        case .pass: return "checkmark.circle.fill"
        case .warning: return "exclamationmark.triangle.fill"
        case .blocked: return "xmark.octagon.fill"
        case .notRun: return "questionmark.circle"
        }
    }
}

enum CNCVerdictText {
    static func mm(_ v: Double, _ places: Int = 2) -> String {
        guard v.isFinite else { return "—" }
        return String(format: "%.\(places)f", v)
    }
    static func place(_ xy: [Double]?) -> String {
        guard let xy, xy.count >= 2, xy[0].isFinite, xy[1].isFinite else { return "" }
        return " at X \(mm(xy[0], 1)), Y \(mm(xy[1], 1))"
    }
    private static func sentence(_ text: String) -> String {
        guard let first = text.first else { return text }
        return first.uppercased() + text.dropFirst()
    }

    /// The name a check is known by in the inspector.
    static func title(of name: String) -> String {
        switch name {
        case "gouge": return "Cuts into the part"
        case "material_left": return "Wall left standing"
        case "rapids": return "Rapids below the stock top"
        case "depth": return "Depth against the stock"
        case "tabs": return "Holding tabs"
        case "envelope": return "Travel and sweep"
        case "loose_pieces": return "Pieces that come free"
        case "plunges": return "Plunges into uncut metal"
        default: return name.replacingOccurrences(of: "_", with: " ")
        }
    }

    /// One sentence for a failed check, built from its own worst violation.
    ///
    /// `who` is the operation the violation lands in, when one could be found;
    /// naming it is the difference between "something gouges" and "the inside
    /// contour gouges".
    static func explain(_ check: CNCCheckReport,
                        in verification: CNCVerification,
                        under: CNCUnderStock,
                        who: String?) -> String {
        let worst = check.worstExample
        let here = place(worst?.xy)
        let subject = who ?? "This job"
        switch check.name {
        case "gouge":
            return "\(subject) cuts \(mm(check.worst)) mm into the part\(here)."
        case "depth":
            let under_ = verification.depth.remainingUnderPart
            if under_ < -1e-6 {
                switch under {
                case .machineBed:
                    return "Cut goes \(mm(-under_, 3)) mm past the stock underside and no spoilboard is declared\(here)."
                case .spoilboard(let t):
                    return "Cut goes \(mm(-under_, 3)) mm past the stock underside, deeper than the \(mm(t, 1)) mm spoilboard under it\(here)."
                }
            }
            if let worst { return "\(subject) \(worst.what)\(here)." }
            return "The cut does not reach the floor the stock and allowance imply (Z \(mm(verification.depth.floorZ, 3)))."
        case "material_left":
            let m = verification.materialLeft
            return "\(mm(m.unsweptArea, 1)) mm² of wall is left standing, up to \(mm(m.maxStandoff)) mm proud\(here)."
        case "loose_pieces":
            let piece = verification.loose.pieces.first
            let what = piece?.isPart == true ? "the part" : "waste"
            return "\(mm(piece?.area ?? check.worst, 1)) mm² of \(what) comes free\(here) — no tab and no skin holds it."
        case "rapids":
            return sentence("\(worst?.what ?? "a rapid travels through material")\(here).")
        case "plunges":
            return "\(subject) plunges straight down into uncut metal\(here)."
        case "tabs", "envelope":
            return sentence("\(worst?.what ?? check.note)\(here).")
        default:
            return sentence("\(worst?.what ?? check.note)\(here).")
        }
    }

    /// The number a check turns on, for the Verification list. Every check
    /// shows one, pass or fail: "0 mm" is a result, "clean" is a mood.
    static func value(of check: CNCCheckReport, in v: CNCVerification) -> String {
        switch check.name {
        case "gouge": return "\(mm(check.worst, 3)) mm deepest"
        case "material_left": return "\(mm(v.materialLeft.unsweptArea, 1)) mm² left, \(mm(v.materialLeft.maxStandoff)) mm proud"
        case "rapids": return check.pass ? "none below the stock top" : "\(check.violationCount)"
        case "depth":
            let skin = v.depth.remainingUnderPart
            return "deepest Z \(mm(v.depth.deepestZ, 3)), \(skin >= 0 ? "\(mm(skin, 3)) mm left under" : "\(mm(-skin, 3)) mm past the underside")"
        case "tabs": return "\(v.tabs.tabCount) cut, \(v.tabs.passesBelowTabs) pass(es) step over"
        case "envelope":
            let lo = v.envelope.workMin, hi = v.envelope.workMax
            guard lo.count >= 2, hi.count >= 2 else { return "—" }
            return "X \(mm(lo[0], 1))…\(mm(hi[0], 1)), Y \(mm(lo[1], 1))…\(mm(hi[1], 1))"
        case "loose_pieces":
            return v.loose.skinHolds ? "nothing breaks through" : "\(v.loose.pieces.count) piece(s) free"
        case "plunges": return check.pass ? "none into uncut metal" : "\(check.violationCount)"
        default: return check.pass ? "clear" : "\(check.violationCount)"
        }
    }
}
