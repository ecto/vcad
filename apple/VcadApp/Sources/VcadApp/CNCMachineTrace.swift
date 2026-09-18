import Foundation
import Observation

// "Trace bounds": walk the job's swept rectangle at a safe height, corner by
// corner, and stop at each one so the operator can look. Friction-log item 53
// — the blank was clamped about 10° off and 5 mm off centre, and the only way
// that was ever found was tracing the square by hand from the sender while
// watching a camera. This is that, in the app, with a pause the machine
// respects and a feed hold that works throughout.

/// The G-code the machine side writes. Kept out of the controller so the exact
/// lines are testable without a wire, and out of `CNCCommands` because these
/// are machine-setup moves rather than the job's own program.
enum CNCMachineCommands {
    private static let posix = Locale(identifier: "en_US_POSIX")

    /// Retract to a work Z. Upward at rapid: the only direction that is safe
    /// without looking.
    static func retract(z: Double) -> String? {
        guard z.isFinite, (-500...500).contains(z) else { return nil }
        return String(format: "G21 G90 G0 Z%.3f", locale: posix, z)
    }
    /// Travel to a work XY at a feed, so a hold stops it in a hand's width.
    static func travel(x: Double, y: Double, feed: Double) -> String? {
        guard x.isFinite, y.isFinite, feed.isFinite, feed > 0, feed <= 3000,
              abs(x) <= 2000, abs(y) <= 2000 else { return nil }
        return String(format: "G21 G90 G1 X%.3f Y%.3f F%.1f", locale: posix, x, y, feed)
    }
    /// Feed down to a work Z — the dip at a corner.
    static func plunge(z: Double, feed: Double) -> String? {
        guard z.isFinite, (-500...500).contains(z), feed.isFinite, feed > 0, feed <= 1000 else { return nil }
        return String(format: "G21 G90 G1 Z%.3f F%.1f", locale: posix, z, feed)
    }
    /// A straight-line probe along one axis to a work coordinate.
    static func probe(axis: String, to target: Double, feed: Double) -> String? {
        guard ["X", "Y", "Z"].contains(axis.uppercased()), target.isFinite, abs(target) <= 2000,
              feed.isFinite, (1...500).contains(feed) else { return nil }
        return String(format: "G21 G90 G38.2 %@%.3f F%.1f", locale: posix, axis.uppercased(), target, feed)
    }
    /// Set one or more work coordinates at the current position.
    static func setWork(_ values: [(axis: String, value: Double)], workspace: String) -> String? {
        guard let index = CNCCommands.workspaces.firstIndex(of: workspace), !values.isEmpty,
              values.allSatisfy({ ["X", "Y", "Z"].contains($0.axis.uppercased()) && $0.value.isFinite && abs($0.value) <= 1000 }),
              Set(values.map { $0.axis.uppercased() }).count == values.count else { return nil }
        let words = values.map { String(format: "%@%.3f", locale: posix, $0.axis.uppercased(), $0.value) }
        return "G21 G10 L20 P\(index + 1) " + words.joined(separator: " ")
    }
}

/// One walk around the job's bounding rectangle.
///
/// The run is deliberately not automatic: each corner is one confirmed motion,
/// and the next one does not happen until the operator says so. Feed hold and
/// the controller's own gating (`canCommand`) apply to every step, so a held
/// machine simply cannot be advanced.
@MainActor @Observable
final class CNCTrace {
    enum Phase: Equatable, Sendable {
        case idle
        /// Stopped at a corner, waiting for the operator.
        case waiting(corner: Int)
        case finished
        case cancelled
    }

    private(set) var phase: Phase = .idle
    private(set) var corners: [SIMD2<Double>] = []
    /// Corners the operator dipped at, for the UI and for the tests.
    private(set) var dipped: Set<Int> = []
    private(set) var safeZ = 5.0
    private(set) var dipZ = 1.0
    var feed = 800.0
    var plungeFeed = 200.0

    var running: Bool { if case .waiting = phase { return true }; return false }
    var currentCorner: Int? { if case .waiting(let i) = phase { return i }; return nil }
    var label: String {
        switch phase {
        case .idle: return "Trace the job's bounding rectangle at a safe height."
        case .waiting(let i): return "Corner \(i + 1) of \(corners.count) — check clearance, then Continue."
        case .finished: return "Traced \(counted(corners.count, "corner")). Nothing was cut."
        case .cancelled: return "Trace cancelled."
        }
    }

    /// Begin a trace of `box` (work coordinates) at `safeZ`, dipping to `dipZ`
    /// wherever the operator asks. Answers false when the machine will not take
    /// the first move, so the UI never shows a trace that is not happening.
    @discardableResult
    func begin(box: CNCEnvelopeBox, on machine: CNCController, safeZ: Double = 5, dipZ: Double = 1) -> Bool {
        guard !running, machine.canCommand, safeZ.isFinite, dipZ.isFinite, safeZ > dipZ else { return false }
        let points = box.corners.map { SIMD2<Double>($0.x, $0.y) }
        guard points.count == 4, points.allSatisfy({ $0.x.isFinite && $0.y.isFinite }) else { return false }
        corners = points; dipped = []; self.safeZ = safeZ; self.dipZ = dipZ
        guard move(to: 0, on: machine) else { corners = []; return false }
        phase = .waiting(corner: 0)
        return true
    }

    /// Move to the next corner, or finish by retracting.
    @discardableResult
    func advance(on machine: CNCController) -> Bool {
        guard case .waiting(let i) = phase, machine.canCommand else { return false }
        if i + 1 < corners.count {
            guard move(to: i + 1, on: machine) else { return false }
            phase = .waiting(corner: i + 1)
            return true
        }
        guard let retract = CNCMachineCommands.retract(z: safeZ) else { return false }
        machine.runSetupMoves([retract])
        phase = .finished
        return true
    }

    /// Dip to the close height at the corner the machine is standing at, then
    /// come back up. The operator asks for this at the corners they cannot see.
    @discardableResult
    func dip(on machine: CNCController) -> Bool {
        guard case .waiting(let i) = phase, machine.canCommand,
              let down = CNCMachineCommands.plunge(z: dipZ, feed: plungeFeed),
              let up = CNCMachineCommands.retract(z: safeZ) else { return false }
        machine.runSetupMoves([down, up])
        dipped.insert(i)
        return true
    }

    /// Give up. This never commands motion: a machine that is moving is
    /// stopped with feed hold, which is a button of its own.
    func cancel() {
        guard running else { return }
        phase = .cancelled
    }
    func reset() { phase = .idle; corners = []; dipped = [] }

    private func move(to index: Int, on machine: CNCController) -> Bool {
        guard corners.indices.contains(index),
              let retract = CNCMachineCommands.retract(z: safeZ),
              let travel = CNCMachineCommands.travel(x: corners[index].x, y: corners[index].y, feed: feed)
        else { return false }
        // Retract first, every time: the corner before this one may have been
        // dipped, and a diagonal from Z+1 is how a clamp gets hit.
        machine.runSetupMoves([retract, travel])
        return true
    }
}
