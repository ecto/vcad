import Foundation
import Observation

// Probing that knows what it is touching. Three things went wrong on
// 2026-09-17 and all three live here: the sender's "3d-probe" type zeroed Z on
// the top of the puck with no thickness at all, the paper touch-off that
// replaced it was 0.81 mm high so the first start cut air, and the blank was
// about 10° off the machine axes with no way to find that out but a camera.

/// What the tool is touching down on, and how thick it is.
enum CNCTouchOff: String, CaseIterable, Identifiable, Sendable {
    /// A plate or puck of a declared thickness sitting on the stock.
    case plate
    /// The stock itself, through a slip of paper.
    case paper
    /// Straight onto the stock, tool touching metal.
    case surface

    var id: String { rawValue }
    var title: String {
        switch self {
        case .plate: return "Touch plate or puck"
        case .paper: return "Paper on the stock"
        case .surface: return "Straight to the stock"
        }
    }
    /// How far the tool tip is above work Z0 at the moment of contact.
    func standoff(plateThickness: Double, paperThickness: Double) -> Double {
        switch self {
        case .plate: return plateThickness
        case .paper: return paperThickness
        case .surface: return 0
        }
    }
    var explanation: String {
        switch self {
        case .plate: return "Z0 lands one plate thickness below the touch. A plate whose thickness is not declared zeroes on the plate's top, which is where the first cut goes into air."
        case .paper: return "Lower until the paper just drags, then set Z to the paper's thickness. A 0.1 mm slip left at 0 is a 0.1 mm air cut."
        case .surface: return "Contact is work Z0. Only safe on a conductive stock with a continuity probe."
        }
    }
}

/// The numbers the operator types once and should never type again.
struct CNCProbeSettings: Equatable, Sendable, Codable {
    var touchOff: String = CNCTouchOff.plate.rawValue
    /// Declared thickness of the plate or puck, mm.
    var plateThickness = 3.0
    /// Slip of paper, mm. 0.1 is ordinary printer paper.
    var paperThickness = 0.1
    var feed = 50.0
    var travel = 10.0
    /// How far apart the two touches of an edge probe are.
    var spacing = 20.0
    /// How far to back off an edge before travelling to the second station.
    /// It has to clear the skew itself: 20 mm of station spacing on a 10° skew
    /// moves the edge 3.5 mm, and a 2 mm back-off would simply miss it.
    var backOff = 5.0

    var kind: CNCTouchOff { CNCTouchOff(rawValue: touchOff) ?? .plate }
    var standoff: Double { kind.standoff(plateThickness: plateThickness, paperThickness: paperThickness) }

    static let key = "vcad.cnc.probe"
    static func load() -> CNCProbeSettings {
        guard let data = UserDefaults.standard.data(forKey: key),
              let stored = try? JSONDecoder().decode(CNCProbeSettings.self, from: data) else { return CNCProbeSettings() }
        return stored
    }
    func save() {
        if let data = try? JSONEncoder().encode(self) { UserDefaults.standard.set(data, forKey: Self.key) }
    }
}

/// The geometry of a two-point edge probe.
///
/// **Convention.** `skewDegrees` is the stock's rotation about +Z, measured
/// counter-clockwise from the machine axes, in degrees, in the app's Z-up
/// right-handed frame. A blank whose left-hand edge leans so that its top is
/// further left than its bottom has been rotated counter-clockwise and reads
/// **positive**. The rotation offered for the job's placement is the *same*
/// number, not its negative: the job is turned to lie on the stock as it
/// actually sits, so a +10° blank takes a +10° job.
enum CNCSkewProbe {
    /// Two contacts on one edge → the stock's rotation.
    ///
    /// - Parameters:
    ///   - axis: the axis that was probed, `"X"` or `"Y"`. An X probe walks an
    ///     edge that nominally runs along Y, and vice versa.
    ///   - a, b: the two contact points in work XY.
    /// - Returns: degrees, or `nil` when the two stations are too close along
    ///   the edge for the angle to mean anything.
    static func skewDegrees(axis: String, a: SIMD2<Double>, b: SIMD2<Double>) -> Double? {
        guard a.x.isFinite, a.y.isFinite, b.x.isFinite, b.y.isFinite else { return nil }
        let axis = axis.uppercased()
        // Order the pair along the edge so the answer does not depend on which
        // station was probed first.
        let (first, second): (SIMD2<Double>, SIMD2<Double>) = {
            if axis == "X" { return a.y <= b.y ? (a, b) : (b, a) }
            return a.x <= b.x ? (a, b) : (b, a)
        }()
        let dx = second.x - first.x, dy = second.y - first.y
        if axis == "X" {
            // The edge runs along Y. Rotating the stock CCW by θ takes the edge
            // direction (0,1) to (−sinθ, cosθ), so tanθ = −Δx / Δy.
            guard abs(dy) >= 1 else { return nil }
            return atan2(-dx, dy) * 180 / .pi
        }
        guard axis == "Y", abs(dx) >= 1 else { return nil }
        // The edge runs along X: (1,0) becomes (cosθ, sinθ).
        return atan2(dy, dx) * 180 / .pi
    }

    /// The rotation to give the job's placement for a measured skew.
    ///
    /// It is the skew itself, not its negative: the blank is not being
    /// straightened, the job is being laid down on the blank as it sits. A
    /// +10° blank takes a +10° job. Kept as a named function so the sign is
    /// stated once, in one place, and can be tested.
    static func placementRotationDegrees(forSkew skew: Double) -> Double { skew }

    /// Where the work coordinate has to be set at the contact so that the edge
    /// itself lands on `target`.
    ///
    /// Probing in +X, the cutter touches with its +X flank, so the edge is one
    /// radius beyond the tool centre; the contact therefore reads
    /// `target − radius`. Probing in −X it reads `target + radius`.
    static func edgeZero(target: Double, toolDiameter: Double, direction: Double) -> Double? {
        guard target.isFinite, toolDiameter.isFinite, toolDiameter > 0,
              direction.isFinite, direction != 0 else { return nil }
        return target - (direction > 0 ? 1 : -1) * toolDiameter / 2
    }
}

/// A two-point edge probe, one confirmed motion at a time.
@MainActor @Observable
final class CNCEdgeProbe {
    enum Phase: Equatable, Sendable {
        case idle
        case first
        case complete
        case failed(String)
    }
    private(set) var phase: Phase = .idle
    /// The axis being probed: "X" walks an edge that runs along Y.
    var axis = "X"
    /// +1 probes toward increasing axis, −1 toward decreasing.
    var direction = 1.0
    private(set) var contacts: [SIMD2<Double>] = []
    private(set) var skewDegrees: Double?

    var stationAxis: String { axis == "X" ? "Y" : "X" }
    var label: String {
        switch phase {
        case .idle: return "Touch the \(axis) edge twice, \(stationAxis) apart, to measure how the blank sits."
        case .first: return "First touch recorded. Move along \(stationAxis) and probe again."
        case .complete:
            guard let skew = skewDegrees else { return "Two touches recorded." }
            return String(format: "Blank is %.2f° off the machine axes.", skew)
        case .failed(let reason): return reason
        }
    }

    func reset() { phase = .idle; contacts = []; skewDegrees = nil }

    /// Probe the edge where the tool is standing now.
    @discardableResult
    func probe(on machine: CNCController, settings: CNCProbeSettings) -> Bool {
        guard machine.canCommand, let work = machine.status.work else { return false }
        let sign = direction > 0 ? 1.0 : -1.0
        let start = axis == "X" ? work.x : work.y
        guard let line = CNCMachineCommands.probe(axis: axis, to: start + sign * settings.travel, feed: settings.feed)
        else { return false }
        machine.runProbeSequence([line])
        guard let contact = machine.probeWorkPosition else {
            phase = .failed(machine.alarm?.text ?? "No contact within \(settings.travel.formatted()) mm. Nothing was probed.")
            return false
        }
        contacts.append(SIMD2<Double>(contact.x, contact.y))
        if contacts.count >= 2 {
            let measured = CNCSkewProbe.skewDegrees(axis: axis, a: contacts[contacts.count - 2], b: contacts[contacts.count - 1])
            guard let measured else {
                phase = .failed("The two touches are less than 1 mm apart along \(stationAxis); move further between them.")
                contacts.removeLast()
                return false
            }
            skewDegrees = measured
            machine.setSkew(measured)
            phase = .complete
        } else {
            phase = .first
        }
        return true
    }

    /// Back off the edge and travel to the second station.
    @discardableResult
    func moveToSecondStation(on machine: CNCController, settings: CNCProbeSettings) -> Bool {
        guard phase == .first, machine.canCommand, let work = machine.status.work else { return false }
        let sign = direction > 0 ? 1.0 : -1.0
        let back: (x: Double, y: Double) = axis == "X"
            ? (work.x - sign * settings.backOff, work.y + settings.spacing)
            : (work.x + settings.spacing, work.y - sign * settings.backOff)
        guard let line = CNCMachineCommands.travel(x: back.x, y: back.y, feed: 600) else { return false }
        machine.runSetupMoves([line])
        return true
    }
}
