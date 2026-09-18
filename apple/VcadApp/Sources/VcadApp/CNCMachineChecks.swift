import Foundation

// The machine's half of the run gate: does this job fit on this machine, at
// the zero it is about to be started from. The job side (does the toolpath
// make the part) is the verification oracle's; this side is travel, homing,
// alarms and the spindle the operator has to set by hand.
//
// Everything here is pure: a box, a profile, and sentences. The controller and
// the machine bar call it; nothing in it touches the wire.

/// An axis-aligned box in one frame — the job's swept envelope, or the same
/// box moved into machine coordinates.
struct CNCEnvelopeBox: Equatable, Sendable {
    var min: CNCVector
    var max: CNCVector

    /// The box a set of moves sweeps with a given cutter. XY grows by the
    /// tool's radius because the cutter is not a point; Z does not, because
    /// the tool tip is the lowest thing on it.
    static func around(moves: [[Double]], toolRadius: Double = 0) -> CNCEnvelopeBox? {
        var lo = CNCVector(x: .infinity, y: .infinity, z: .infinity)
        var hi = CNCVector(x: -.infinity, y: -.infinity, z: -.infinity)
        var seen = false
        for point in moves {
            guard point.count >= 3, point.allSatisfy(\.isFinite) else { continue }
            seen = true
            lo.x = Swift.min(lo.x, point[0]); hi.x = Swift.max(hi.x, point[0])
            lo.y = Swift.min(lo.y, point[1]); hi.y = Swift.max(hi.y, point[1])
            lo.z = Swift.min(lo.z, point[2]); hi.z = Swift.max(hi.z, point[2])
        }
        guard seen else { return nil }
        let r = toolRadius.isFinite && toolRadius > 0 ? toolRadius : 0
        return CNCEnvelopeBox(min: CNCVector(x: lo.x - r, y: lo.y - r, z: lo.z),
                              max: CNCVector(x: hi.x + r, y: hi.y + r, z: hi.z))
    }

    /// The oracle's own swept envelope, which already includes the cutter.
    static func verified(workMin: [Double], workMax: [Double]) -> CNCEnvelopeBox? {
        guard workMin.count >= 3, workMax.count >= 3,
              workMin.allSatisfy(\.isFinite), workMax.allSatisfy(\.isFinite) else { return nil }
        return CNCEnvelopeBox(min: CNCVector(x: workMin[0], y: workMin[1], z: workMin[2]),
                              max: CNCVector(x: workMax[0], y: workMax[1], z: workMax[2]))
    }

    /// The same box in machine coordinates: machine = work + WCO.
    func inMachineFrame(workOffset: CNCVector) -> CNCEnvelopeBox {
        CNCEnvelopeBox(min: min + workOffset, max: max + workOffset)
    }

    /// The four XY corners, anticlockwise from the near-left one, for tracing.
    var corners: [(x: Double, y: Double)] {
        [(min.x, min.y), (max.x, min.y), (max.x, max.y), (min.x, max.y)]
    }
}

/// One thing the machine has to say about this job. Blocking findings keep Run
/// disabled; the rest are read and decided on by the operator.
struct CNCMachineFinding: Identifiable, Equatable, Sendable {
    var id: String
    var text: String
    var blocking: Bool
}

enum CNCMachineCheck {
    /// Everything the machine has to say before this job runs.
    ///
    /// `job` is the swept envelope in *work* coordinates — the same frame the
    /// G-code is posted in. Passing `nil` means there is no job yet, which is
    /// not a machine problem and produces no findings about travel.
    static func findings(job: CNCEnvelopeBox?, profile: CNCMachineProfile?) -> [CNCMachineFinding] {
        var out: [CNCMachineFinding] = []
        guard let profile else {
            return [CNCMachineFinding(
                id: "machine-settings-unknown",
                text: "The controller's settings have not been read, so nothing has checked this job against the machine's travel.",
                blocking: false)]
        }

        // An alarm comes first: everything after it is measured against a
        // position the controller has already said it does not know.
        if let alarm = profile.alarm {
            out.append(CNCMachineFinding(id: "machine-alarm-\(alarm.rawValue)",
                                         text: alarm.text,
                                         blocking: alarm.positionLost))
        }
        if profile.homingEnabled && !profile.homed && profile.alarm?.positionLost != true {
            out.append(CNCMachineFinding(
                id: "machine-not-homed",
                text: "The machine has not been homed this session, so its machine coordinates are wherever it was switched on. Soft limits are meaningless until homed.",
                blocking: false))
        } else if let alarm = profile.alarm, alarm.positionLost {
            out.append(CNCMachineFinding(id: "machine-rehome",
                                         text: "Re-home before running: the machine position is not trusted.",
                                         blocking: true))
        }
        if !profile.softLimits {
            out.append(CNCMachineFinding(
                id: "machine-soft-limits-off",
                text: "Soft limits are off ($20=0): the controller will not refuse a move that leaves its travel. It will simply run into the end.",
                blocking: false))
        }
        if !profile.hardLimits {
            out.append(CNCMachineFinding(
                id: "machine-hard-limits-off",
                text: "Hard limits are off ($21=0): the limit switches will not stop motion.",
                blocking: false))
        }

        out.append(contentsOf: travelFindings(job: job, profile: profile))
        return out
    }

    /// The envelope pre-check itself, in machine coordinates.
    static func travelFindings(job: CNCEnvelopeBox?, profile: CNCMachineProfile) -> [CNCMachineFinding] {
        guard let job else { return [] }
        guard profile.hasTravel else {
            return [CNCMachineFinding(
                id: "machine-travel-unknown",
                text: "The controller did not report $130–$132, so this job's travel cannot be checked.",
                blocking: false)]
        }
        guard let offset = profile.workOffset else {
            return [CNCMachineFinding(
                id: "machine-offset-unknown",
                text: "The work offset is not known yet, so where this job lands in machine coordinates cannot be checked against travel.",
                blocking: false)]
        }
        let box = job.inMachineFrame(workOffset: offset)
        var out: [CNCMachineFinding] = []
        // The Simulator has no table. Its travel is the AnoleX's listing and
        // its work offset is whatever nobody has set, so a job "outside
        // travel" there is outside a fiction — and blocking on it meant the
        // Simulator could never run anything, which is the one thing it is
        // for. The sentence is still said, in full, with its numbers; it is
        // the *blocking* that the Simulator does not earn. A real controller
        // is unchanged: this is friction-log item 48's distinction, applied to
        // the gate rather than only to the readiness tick.
        let blocks = !profile.simulated
        for travel in profile.travels {
            let lo = component(box.min, travel.axis), hi = component(box.max, travel.axis)
            // Both ends are checked: a job can hang off the front and the back
            // of a short axis at once, and saying only one is half an answer.
            for (value, positive) in [(hi, true), (lo, false)] {
                let excess = travel.excess(value)
                guard excess != 0, (excess > 0) == positive else { continue }
                // Blocking even when the machine has not been homed: the
                // coordinates are then a guess, but a guess that lands outside
                // travel is exactly the case that must not reach Run. Homing
                // and re-checking is the cheap way out, and the sentence says
                // so rather than leaving the number looking authoritative.
                // Short, because it is repeated per axis and the not-homed
                // warning above already says it at length.
                let caveat = profile.simulated
                    ? " (Simulator: no real table, so this is not a refusal)"
                    : profile.homingEnabled && !profile.homed ? " (not homed: unverified)" : ""
                out.append(CNCMachineFinding(
                    id: "machine-travel-\(travel.axis)-\(positive ? "max" : "min")",
                    text: "Job reaches \(travel.axis) \(signed(excess)) mm past the \(travel.endName(positive: positive)) of travel "
                        + "(travel \(number(travel.min))…\(number(travel.max)) mm, job \(number(lo))…\(number(hi)) mm). "
                        + "Move the work zero or re-clamp the blank." + caveat,
                    blocking: blocks))
            }
        }
        return out
    }

    /// What the operator has to do to the spindle before pressing Run. A job
    /// may carry the answer as a note; otherwise the dial table gives it.
    static func spindleInstruction(rpm: Double, profile: CNCMachineProfile?, notes: [String] = []) -> String? {
        // The job's own word wins: it knows the material and the cutter.
        if let note = notes.first(where: { $0.lowercased().contains("dial") }) { return note }
        guard let profile, rpm.isFinite, rpm > 0 else { return nil }
        return profile.spindle.dialAdvice(forRPM: rpm)
    }

    private static func component(_ v: CNCVector, _ axis: String) -> Double {
        switch axis { case "X": return v.x; case "Y": return v.y; default: return v.z }
    }
    private static func signed(_ value: Double) -> String {
        (value > 0 ? "+" : "−") + number(abs(value))
    }
    /// Millimetres in prose. A typographic minus throughout, so "−300…0" and
    /// "−12 mm past" are the same character in the same sentence.
    private static func number(_ value: Double) -> String {
        let rounded = (value * 10).rounded() / 10
        if rounded == 0 { return "0" }
        let text = rounded == rounded.rounded() ? String(Int(abs(rounded))) : String(format: "%.1f", abs(rounded))
        return (rounded < 0 ? "−" : "") + text
    }
}
