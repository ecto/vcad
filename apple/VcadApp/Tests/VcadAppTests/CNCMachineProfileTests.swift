import XCTest
import AppKit
import SwiftUI
@testable import VcadApp

/// The machine side of the run gate: `$$`, the baseline diff, travel, tracing,
/// probing and the alarms. Everything here is asserted as a number or a
/// sentence a machinist could act on — "a profile came back" is how `$132=100`
/// went unnoticed for an afternoon.
@MainActor
final class CNCMachineProfileTests: XCTestCase {

    /// The bench machine's own `$$`, comments and all, as Grbl_ESP32 1.3a
    /// prints it (2026-09-17, AnoleX 4030-Evo Ultra 2).
    private let benchDump = """
    $0=10 (step pulse, usec)
    $1=255 (step idle delay, msec)
    $2=0 (step port invert mask)
    $3=0 (dir port invert mask)
    $4=0 (step enable invert, bool)
    $5=1 (limit pins invert, bool)
    $6=0 (probe pin invert, bool)
    $10=1 (status report mask)
    $11=0.010 (junction deviation, mm)
    $12=0.002 (arc tolerance, mm)
    $13=0 (report inches, bool)
    $20=1 (soft limits, bool)
    $21=1 (hard limits, bool)
    $22=1 (homing cycle, bool)
    $23=3 (homing dir invert mask)
    $24=100.000 (homing feed, mm/min)
    $25=1000.000 (homing seek, mm/min)
    $26=250 (homing debounce, msec)
    $27=3.000 (homing pull-off, mm)
    $30=10000 (rpm max)
    $31=0 (rpm min)
    $32=0 (laser mode, bool)
    $100=80.000 (x, step/mm)
    $101=80.000 (y, step/mm)
    $102=800.000 (z, step/mm)
    $110=4000.000 (x max rate, mm/min)
    $111=4000.000 (y max rate, mm/min)
    $112=2000.000 (z max rate, mm/min)
    $120=300.000 (x accel, mm/sec^2)
    $121=300.000 (y accel, mm/sec^2)
    $122=200.000 (z accel, mm/sec^2)
    $130=400.000 (x max travel, mm)
    $131=300.000 (y max travel, mm)
    $132=130.000 (z max travel, mm)
    """

    /// The same machine after the sender's setup wizard had been through it.
    private var wizardDump: String {
        benchDump
            .replacingOccurrences(of: "$132=130.000", with: "$132=100.000")
            .replacingOccurrences(of: "$21=1", with: "$21=0")
    }

    // MARK: - $$

    func testSettingsParseTheBenchListing() {
        let settings = CNCMachineSettings.parse(benchDump)
        // Every setting is kept, including the ones nothing models.
        XCTAssertEqual(settings[1], 255)
        XCTAssertEqual(settings.numbers.count, 34)

        let profile = CNCMachineProfile(settings: settings)
        XCTAssertEqual(profile.travelX.length, 400)
        XCTAssertEqual(profile.travelY.length, 300)
        XCTAssertEqual(profile.travelZ.length, 130)
        XCTAssertTrue(profile.softLimits)
        XCTAssertTrue(profile.hardLimits)
        XCTAssertTrue(profile.homingEnabled)
        XCTAssertEqual(profile.pullOff, 3)
        XCTAssertEqual(profile.maxSpindleRPM, 10_000)
        XCTAssertEqual(profile.maxRate, CNCVector(x: 4000, y: 4000, z: 2000))
        XCTAssertEqual(profile.acceleration, CNCVector(x: 300, y: 300, z: 200))

        // Grbl's machine space is [-max, 0] whatever the homing direction; the
        // direction only says where `$H` parks. The stale G54 Z of −138 seen on
        // 2026-09-17 was outside this range, which is the whole point.
        XCTAssertEqual(profile.travelZ.min, -130)
        XCTAssertEqual(profile.travelZ.max, 0)
        XCTAssertFalse(profile.travelZ.contains(-138))
        XCTAssertTrue(profile.travelZ.contains(-129))
        XCTAssertTrue(profile.travelX.homesNegative)   // $23=3: X and Y home negative
        XCTAssertTrue(profile.travelY.homesNegative)
        XCTAssertFalse(profile.travelZ.homesNegative)
        XCTAssertEqual(profile.travelZ.homePosition, -3)
        XCTAssertEqual(profile.travelX.homePosition, -397)
        XCTAssertTrue(profile.hasTravel)

        // A listing that never arrived is not a machine with no travel.
        XCTAssertFalse(CNCMachineProfile(settings: CNCMachineSettings()).hasTravel)
    }

    func testSettingsIgnoreNoiseAndKeepReportedText() {
        var settings = CNCMachineSettings()
        XCTAssertFalse(settings.ingest("ok"))
        XCTAssertFalse(settings.ingest("[GC:G0 G54 G17]"))
        XCTAssertFalse(settings.ingest("$H"))
        XCTAssertFalse(settings.ingest("$x=nope"))
        XCTAssertTrue(settings.ingest("$130=400.000 (x max travel, mm)"))
        XCTAssertEqual(settings.text(130), "400.000")
    }

    // MARK: - the baseline diff

    func testBaselineDiffNamesTheSettingsThatMoved() {
        let baseline = CNCMachineBaseline(name: "bench", settings: CNCMachineSettings.parse(benchDump))
        let deltas = baseline.diff(against: CNCMachineSettings.parse(wizardDump))
        XCTAssertEqual(deltas.map(\.number), [21, 132])

        let z = try! XCTUnwrap(deltas.first { $0.number == 132 })
        XCTAssertEqual(z.baseline, 130)
        XCTAssertEqual(z.current, 100)
        XCTAssertTrue(z.label.contains("Z maximum travel"), z.label)
        XCTAssertEqual(z.summary, "$132 Z maximum travel, mm · 130 → 100")

        let hard = try! XCTUnwrap(deltas.first { $0.number == 21 })
        XCTAssertEqual(hard.baseline, 1)
        XCTAssertEqual(hard.current, 0)
        XCTAssertEqual(hard.summary, "$21 hard limits · 1 → 0")

        // Nothing else is reported as changed, or the list is noise.
        XCTAssertNil(deltas.first { $0.number == 130 })
        XCTAssertTrue(baseline.diff(against: CNCMachineSettings.parse(benchDump)).isEmpty)

        // A setting that only one side has is a difference too.
        var extra = CNCMachineSettings.parse(benchDump)
        extra.ingest("$140=1")
        XCTAssertEqual(baseline.diff(against: extra).map(\.number), [140])
    }

    func testShippedAnolexBaselineMatchesTheBenchListing() {
        XCTAssertTrue(CNCAnolexBaseline.baseline.diff(against: CNCMachineSettings.parse(benchDump)).isEmpty)
        XCTAssertEqual(CNCAnolexBaseline.baseline.diff(against: CNCMachineSettings.parse(wizardDump)).count, 2)
    }

    func testSimulatorReadsSettingsAndDiffsThem() {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        let profile = try! XCTUnwrap(machine.profile)
        XCTAssertTrue(profile.simulated)
        XCTAssertEqual(profile.travelZ.length, 130)
        XCTAssertTrue(profile.baselineDiff.isEmpty)

        machine.simulate(settings: wizardDump)
        XCTAssertEqual(machine.profile?.travelZ.length, 100)
        XCTAssertEqual(machine.baselineDiff.map(\.number), [21, 132])
        XCTAssertEqual(machine.profile?.baselineDiff.first?.summary, "$21 hard limits · 1 → 0")
        XCTAssertFalse(machine.profile?.hardLimits ?? true)

        // Disconnecting forgets the machine: settings belong to a session.
        machine.disconnect()
        XCTAssertNil(machine.profile)
    }

    // MARK: - the envelope pre-check

    private func profile(homed: Bool = true, offset: CNCVector, dump: String? = nil) -> CNCMachineProfile {
        var profile = CNCMachineProfile(settings: CNCMachineSettings.parse(dump ?? benchDump))
        profile.homed = homed
        profile.workOffset = offset
        profile.workspace = "G54"
        return profile
    }
    /// A job 80 × 100 mm, cutting 6 mm deep and clearing at Z+5.
    private let job = CNCEnvelopeBox(min: CNCVector(x: 0, y: 0, z: -6),
                                     max: CNCVector(x: 80, y: 100, z: 5))

    func testEnvelopeBlocksAJobThatLeavesTravelByTwelveMillimetres() {
        // Work zero at machine Y −88 puts the far edge of the job 12 mm past
        // the back of a 300 mm axis.
        let out = CNCMachineCheck.findings(job: job, profile: profile(offset: CNCVector(x: -200, y: -88, z: -20)))
        let blocking = out.filter(\.blocking)
        XCTAssertEqual(blocking.count, 1)
        let text = try! XCTUnwrap(blocking.first?.text)
        XCTAssertTrue(text.hasPrefix("Job reaches Y +12 mm past the back of travel"), text)
        XCTAssertTrue(text.contains("travel −300…0 mm"), text)
        XCTAssertTrue(text.contains("job −88…12 mm"), text)
    }

    func testEnvelopeInsideTravelPasses() {
        let out = CNCMachineCheck.findings(job: job, profile: profile(offset: CNCVector(x: -200, y: -150, z: -20)))
        XCTAssertEqual(out.filter(\.blocking), [])
        XCTAssertEqual(out, [])
    }

    func testEnvelopeChecksBothEndsOfEveryAxis() {
        // Work zero 410 mm left of machine zero hangs the near edge off the
        // left of X; a Z0 only 2 mm below machine zero puts the clearance
        // plane above the top of travel. Both ends, two axes, one job.
        let out = CNCMachineCheck.travelFindings(job: job, profile: profile(offset: CNCVector(x: -410, y: -150, z: -2)))
        let ids = out.map(\.id)
        XCTAssertTrue(ids.contains("machine-travel-X-min"), "\(ids)")
        XCTAssertTrue(ids.contains("machine-travel-Z-max"), "\(ids)")
        XCTAssertFalse(ids.contains("machine-travel-Y-max"), "\(ids)")
        XCTAssertTrue(out.allSatisfy(\.blocking))
        XCTAssertTrue(out.contains { $0.text.hasPrefix("Job reaches X −10 mm past the left of travel") }, "\(out)")
        XCTAssertTrue(out.contains { $0.text.hasPrefix("Job reaches Z +3 mm past the top of travel") }, "\(out)")
    }

    func testNotHomedWarnsAndSoftLimitsOffWarns() {
        let out = CNCMachineCheck.findings(job: job, profile: profile(homed: false, offset: CNCVector(x: -200, y: -150, z: -20)))
        let warning = try! XCTUnwrap(out.first { $0.id == "machine-not-homed" })
        XCTAssertFalse(warning.blocking)
        XCTAssertTrue(warning.text.contains("Soft limits are meaningless until homed"), warning.text)

        let off = CNCMachineCheck.findings(
            job: job,
            profile: profile(offset: CNCVector(x: -200, y: -150, z: -20),
                             dump: benchDump.replacingOccurrences(of: "$20=1", with: "$20=0")))
        let soft = try! XCTUnwrap(off.first { $0.id == "machine-soft-limits-off" })
        XCTAssertFalse(soft.blocking)
        XCTAssertTrue(soft.text.contains("$20=0"), soft.text)
    }

    func testNoProfileAndNoOffsetSayNothingWasChecked() {
        let unknown = CNCMachineCheck.findings(job: job, profile: nil)
        XCTAssertEqual(unknown.count, 1)
        XCTAssertFalse(unknown[0].blocking)
        XCTAssertTrue(unknown[0].text.contains("not been read"), unknown[0].text)

        var noOffset = CNCMachineProfile(settings: CNCMachineSettings.parse(benchDump))
        noOffset.homed = true
        let out = CNCMachineCheck.travelFindings(job: job, profile: noOffset)
        XCTAssertEqual(out.map(\.id), ["machine-offset-unknown"])
    }

    func testEnvelopeFromMovesIncludesTheCutter() {
        let moves = [[0.0, 0, 5], [40, 0, -1], [40, 30, -1]]
        let box = try! XCTUnwrap(CNCEnvelopeBox.around(moves: moves, toolRadius: 1.5))
        XCTAssertEqual(box.min.x, -1.5); XCTAssertEqual(box.max.x, 41.5)
        XCTAssertEqual(box.min.y, -1.5); XCTAssertEqual(box.max.y, 31.5)
        // Z is the tool tip: nothing hangs below it.
        XCTAssertEqual(box.min.z, -1); XCTAssertEqual(box.max.z, 5)
        XCTAssertNil(CNCEnvelopeBox.around(moves: [[0, 0]], toolRadius: 1))
        XCTAssertEqual(box.corners.count, 4)
    }

    // MARK: - alarms

    func testHardLimitBlocksUntilTheMachineIsHomedAgain() async {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.home()
        XCTAssertTrue(machine.homed)
        XCTAssertEqual(machine.status.machine?.z, -3)   // parked at the pull-off

        machine.simulate(alarm: 1)
        XCTAssertFalse(machine.homed)
        XCTAssertEqual(machine.alarm, .hardLimit)
        let blocked = CNCMachineCheck.findings(job: nil, profile: machine.profile)
        XCTAssertTrue(blocked.contains { $0.blocking && $0.text.contains("Re-home before running") }, "\(blocked)")
        XCTAssertTrue(blocked.contains { $0.text.contains("machine position is not trusted") })

        machine.home()
        XCTAssertTrue(machine.homed)
        XCTAssertNil(machine.alarm)
        XCTAssertEqual(CNCMachineCheck.findings(job: nil, profile: machine.profile).filter(\.blocking), [])
    }

    func testAlarmTextsExplainTheSoftLimitAndTheProbeFailure() {
        XCTAssertEqual(CNCAlarm.parse("ALARM:2"), .softLimit)
        XCTAssertEqual(CNCAlarm.parse("Alarm:1"), .hardLimit)
        XCTAssertNil(CNCAlarm.parse("error:20"))
        XCTAssertTrue(CNCAlarm.softLimit.text.contains("nothing moved"))
        XCTAssertFalse(CNCAlarm.softLimit.positionLost)
        XCTAssertTrue(CNCAlarm.probeFailContact.text.contains("never made contact"))
        XCTAssertTrue(CNCAlarm.probeFailInitial.text.contains("already touching"))
        XCTAssertFalse(CNCAlarm.probeFailContact.positionLost)
        XCTAssertTrue(CNCAlarm.hardLimit.positionLost)
    }

    // MARK: - trace bounds

    private func waitFor(_ what: String, timeout: Double = 4, _ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(condition(), what)
    }
    private func traceMoves(_ machine: CNCController) -> [String] {
        machine.log.filter { $0.hasPrefix("→ G21 G90 G1 X") }
    }

    func testTraceWalksFourCornersAndWaitsBetweenThem() async {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        let box = CNCEnvelopeBox(min: CNCVector(x: 0, y: 0, z: -1), max: CNCVector(x: 40, y: 30, z: 5))

        XCTAssertTrue(machine.trace.begin(box: box, on: machine, safeZ: 5, dipZ: 1))
        XCTAssertEqual(machine.trace.phase, .waiting(corner: 0))
        XCTAssertEqual(traceMoves(machine).count, 1)
        XCTAssertEqual(traceMoves(machine).first, "→ G21 G90 G1 X0.000 Y0.000 F800.0")

        // Corner 2: the machine is at the first corner until it is told to go.
        XCTAssertTrue(machine.trace.advance(on: machine))
        XCTAssertEqual(machine.trace.phase, .waiting(corner: 1))
        XCTAssertEqual(traceMoves(machine).count, 2)

        // Feed hold mid-trace: nothing advances while the machine is held.
        machine.hold()
        await waitFor("simulator held") { machine.status.state == "Hold:0" }
        XCTAssertFalse(machine.trace.advance(on: machine))
        XCTAssertEqual(traceMoves(machine).count, 2)
        XCTAssertEqual(machine.trace.phase, .waiting(corner: 1))
        machine.resume()
        await waitFor("simulator resumed") { machine.status.state == "Idle" }

        XCTAssertTrue(machine.trace.advance(on: machine))   // corner 3
        // Dip only where it is asked for.
        XCTAssertTrue(machine.trace.dip(on: machine))
        XCTAssertEqual(machine.trace.dipped, [2])
        XCTAssertTrue(machine.trace.advance(on: machine))   // corner 4
        XCTAssertEqual(machine.trace.phase, .waiting(corner: 3))

        let moves = traceMoves(machine)
        XCTAssertEqual(moves, ["→ G21 G90 G1 X0.000 Y0.000 F800.0",
                               "→ G21 G90 G1 X40.000 Y0.000 F800.0",
                               "→ G21 G90 G1 X40.000 Y30.000 F800.0",
                               "→ G21 G90 G1 X0.000 Y30.000 F800.0"])
        // Every corner is approached from the trace height, never diagonally
        // from a dip.
        for move in moves {
            let index = machine.log.firstIndex(of: move)!
            XCTAssertEqual(machine.log[index - 1], "→ G21 G90 G0 Z5.000", "before \(move)")
        }
        XCTAssertEqual(machine.log.filter { $0.hasPrefix("→ G21 G90 G1 Z1.000") }.count, 1)
        XCTAssertEqual(machine.status.work?.z, 5)

        XCTAssertTrue(machine.trace.advance(on: machine))
        XCTAssertEqual(machine.trace.phase, .finished)
        XCTAssertEqual(traceMoves(machine).count, 4)
        XCTAssertEqual(machine.log.last, "→ G21 G90 G0 Z5.000")
    }

    func testTraceRefusesADipHeightAboveTheTraceHeight() {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        let box = CNCEnvelopeBox(min: CNCVector(x: 0, y: 0, z: -1), max: CNCVector(x: 40, y: 30, z: 5))
        XCTAssertFalse(machine.trace.begin(box: box, on: machine, safeZ: 1, dipZ: 5))
        XCTAssertEqual(machine.trace.phase, .idle)
        XCTAssertEqual(traceMoves(machine).count, 0)
    }

    // MARK: - probing

    func testPlateThicknessProbeSetsZeroBelowTheTouch() {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.jog(x: 0, y: 0, z: 5, feed: 300)
        // The plate's top is at work Z 1 where the machine is standing.
        machine.simulateProbe(contact: CNCVector(x: 0, y: 0, z: 1))
        machine.probeZ(distance: 10, feed: 50)
        XCTAssertEqual(machine.probePosition?.z, 1)
        machine.applyProbe(thickness: 3)
        // Work Z0 is one plate thickness below the touch, not at it.
        XCTAssertEqual(machine.status.work?.z ?? .nan, 3, accuracy: 1e-9)
        XCTAssertEqual(machine.status.machine?.z ?? .nan, 1, accuracy: 1e-9)
    }

    func testPaperTouchOffSetsTheSlipThickness() {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.jog(x: 0, y: 0, z: 2, feed: 300)
        machine.setWork(axis: "Z", value: 0.1)
        XCTAssertEqual(machine.status.work?.z ?? .nan, 0.1, accuracy: 1e-9)
        XCTAssertEqual(CNCTouchOff.paper.standoff(plateThickness: 3, paperThickness: 0.1), 0.1)
        XCTAssertEqual(CNCTouchOff.plate.standoff(plateThickness: 3, paperThickness: 0.1), 3)
        XCTAssertEqual(CNCTouchOff.surface.standoff(plateThickness: 3, paperThickness: 0.1), 0)
    }

    func testProbeThatNeverTouchesIsAnAlarmNotAZero() {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.jog(x: 0, y: 0, z: 5, feed: 300)
        machine.simulateProbe(contact: nil, misses: true)
        machine.probeZ(distance: 4, feed: 50)
        XCTAssertNil(machine.probePosition)
        XCTAssertFalse(machine.canApplyProbe)
        XCTAssertEqual(machine.alarm, .probeFailContact)
    }

    func testSkewMathMeasuresBothSignsAndOffersTheSameRotation() {
        let run = 20.0, lean = run * tan(10 * Double.pi / 180)
        // Positive: the blank is turned counter-clockwise, so the edge it
        // presents moves toward −X as Y increases.
        let plus = try! XCTUnwrap(CNCSkewProbe.skewDegrees(axis: "X",
                                                           a: SIMD2(5, 0),
                                                           b: SIMD2(5 - lean, run)))
        XCTAssertEqual(plus, 10, accuracy: 0.01)
        let minus = try! XCTUnwrap(CNCSkewProbe.skewDegrees(axis: "X",
                                                           a: SIMD2(5, 0),
                                                           b: SIMD2(5 + lean, run)))
        XCTAssertEqual(minus, -10, accuracy: 0.01)
        // The order the two stations were probed in cannot change the answer.
        XCTAssertEqual(try! XCTUnwrap(CNCSkewProbe.skewDegrees(axis: "X", a: SIMD2(5 - lean, run), b: SIMD2(5, 0))),
                       10, accuracy: 0.01)
        // A Y probe walks an edge along X: the same blank reads the same way.
        XCTAssertEqual(try! XCTUnwrap(CNCSkewProbe.skewDegrees(axis: "Y", a: SIMD2(0, 5), b: SIMD2(run, 5 + lean))),
                       10, accuracy: 0.01)
        // Two stations on top of each other measure nothing.
        XCTAssertNil(CNCSkewProbe.skewDegrees(axis: "X", a: SIMD2(5, 0), b: SIMD2(4, 0.5)))

        // The job turns the same way as the blank, not the opposite way.
        XCTAssertEqual(CNCSkewProbe.placementRotationDegrees(forSkew: plus), plus)
        XCTAssertEqual(CNCSkewProbe.placementRotationDegrees(forSkew: minus), minus)
        XCTAssertGreaterThan(CNCSkewProbe.placementRotationDegrees(forSkew: 10), 0)
        XCTAssertLessThan(CNCSkewProbe.placementRotationDegrees(forSkew: -10), 0)
    }

    /// The whole two-point sequence against the simulator, both signs.
    func testEdgeProbeMeasuresSkewAgainstTheSimulator() {
        for sign in [1.0, -1.0] {
            let machine = CNCController()
            machine.connect(simulated: true)
            defer { machine.disconnect() }
            var settings = CNCProbeSettings()
            settings.spacing = 20; settings.backOff = 5; settings.travel = 10; settings.feed = 50
            let edge = CNCEdgeProbe()
            edge.axis = "X"; edge.direction = 1

            machine.simulateProbe(contact: CNCVector(x: 5, y: 0, z: 0))
            XCTAssertTrue(edge.probe(on: machine, settings: settings))
            XCTAssertEqual(edge.phase, .first)

            XCTAssertTrue(edge.moveToSecondStation(on: machine, settings: settings))
            XCTAssertEqual(machine.status.work?.y ?? .nan, 20, accuracy: 1e-9)
            XCTAssertEqual(machine.status.work?.x ?? .nan, 0, accuracy: 1e-9)

            let lean = 20 * tan(10 * Double.pi / 180)
            machine.simulateProbe(contact: CNCVector(x: 5 - sign * lean, y: 20, z: 0))
            XCTAssertTrue(edge.probe(on: machine, settings: settings))
            XCTAssertEqual(edge.phase, .complete)
            XCTAssertEqual(edge.skewDegrees ?? .nan, sign * 10, accuracy: 0.01)
            // The controller carries it for the job side to read.
            XCTAssertEqual(machine.profile?.skewDegrees ?? .nan, sign * 10, accuracy: 0.01)
            XCTAssertEqual(machine.skewDegrees.map { CNCSkewProbe.placementRotationDegrees(forSkew: $0) } ?? .nan,
                           sign * 10, accuracy: 0.01)
        }
    }

    func testEdgeZeroAllowsForTheCutterRadius() {
        // Probing toward +X, the tool touches with its +X flank: the edge is
        // one radius past the centre, so the contact reads −radius.
        XCTAssertEqual(CNCSkewProbe.edgeZero(target: 0, toolDiameter: 3.175, direction: 1)!, -1.5875, accuracy: 1e-9)
        XCTAssertEqual(CNCSkewProbe.edgeZero(target: 0, toolDiameter: 3.175, direction: -1)!, 1.5875, accuracy: 1e-9)
        XCTAssertNil(CNCSkewProbe.edgeZero(target: 0, toolDiameter: 0, direction: 1))

        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.jog(x: 5, y: 0, z: 0, feed: 300)
        machine.simulateProbe(contact: CNCVector(x: 8, y: 0, z: 0))
        machine.probe(axis: "X", distance: 10, feed: 50)
        XCTAssertEqual(machine.probeWorkPosition?.x, 8)
        machine.applyEdgeZero(axis: "X", target: 0, toolDiameter: 4, direction: 1)
        // The tool centre sits 2 mm inside the edge, so the edge is X0.
        XCTAssertEqual(machine.status.work?.x ?? .nan, -2, accuracy: 1e-9)
    }

    // MARK: - the spindle dial

    func testDialIsReadFromTheCommandedRPM() {
        let spindle = CNCSpindleModel()
        XCTAssertEqual(spindle.dial(forRPM: 13_500)!, 2, accuracy: 1e-9)
        XCTAssertEqual(spindle.dial(forRPM: 10_000)!, 1, accuracy: 1e-9)
        XCTAssertEqual(spindle.dial(forRPM: 15_250)!, 2.5, accuracy: 1e-9)
        XCTAssertNil(spindle.dial(forRPM: 5_000))
        XCTAssertTrue(spindle.dialAdvice(forRPM: 13_500).contains("Set the router dial to 2"))
        XCTAssertTrue(spindle.dialAdvice(forRPM: 13_500).contains("13,500 rpm"),
                      spindle.dialAdvice(forRPM: 13_500))
        XCTAssertTrue(spindle.dialAdvice(forRPM: 5_000).contains("outside the router's dial"))

        let profile = CNCMachineProfile(settings: CNCMachineSettings.parse(benchDump))
        XCTAssertTrue(CNCMachineCheck.spindleInstruction(rpm: 10_000, profile: profile)!.contains("dial to 1"))
        XCTAssertNil(CNCMachineCheck.spindleInstruction(rpm: 0, profile: profile))
        XCTAssertNil(CNCMachineCheck.spindleInstruction(rpm: 10_000, profile: nil))
        // A job that carries the answer wins: it knows the material.
        XCTAssertEqual(CNCMachineCheck.spindleInstruction(rpm: 10_000, profile: profile,
                                                          notes: ["Set the dial to 3 for brass"]),
                       "Set the dial to 3 for brass")
    }

    // MARK: - persistence

    func testBaselineRoundTripsThroughPreferences() {
        let machine = CNCController()
        machine.host = "test.baseline.local"
        machine.connect(simulated: true)
        defer { machine.disconnect(); CNCMachineBaselineStore.forget(machine: "test.baseline.local") }
        // An unknown machine has no baseline, which is not "nothing changed".
        XCTAssertNil(machine.profile?.baselineName)
        machine.saveBaseline(name: "bench")
        XCTAssertEqual(machine.profile?.baselineName, "bench")
        XCTAssertTrue(machine.baselineDiff.isEmpty)
        machine.simulate(settings: wizardDump)
        XCTAssertEqual(machine.baselineDiff.map(\.number), [21, 132])

        let stored = try! XCTUnwrap(CNCMachineBaselineStore.load(machine: "test.baseline.local"))
        XCTAssertEqual(stored.values[132], 130)
        CNCMachineBaselineStore.forget(machine: "test.baseline.local")
        XCTAssertNil(CNCMachineBaselineStore.load(machine: "test.baseline.local"))
    }

    // MARK: - snapshots

    func testMachineSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else {
            throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render machine snapshots.")
        }
        _ = NSApplication.shared
        let cnc = CNCWorkspace()
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.machine.simulate(settings: wizardDump)
        cnc.generate()
        while cnc.generating { try? await Task.sleep(for: .milliseconds(20)) }

        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
            try await render(CNCMachineConnection(cnc: cnc).padding(18).frame(width: 330),
                             to: directory.appendingPathComponent("connection-\(name).png"),
                             size: NSSize(width: 330, height: 640), appearance: appearance)
            try await render(CNCReadinessList(cnc: cnc).padding(18).frame(width: 340),
                             to: directory.appendingPathComponent("readiness-\(name).png"),
                             size: NSSize(width: 340, height: 620), appearance: appearance)
            try await render(CNCProbeSheet(cnc: cnc),
                             to: directory.appendingPathComponent("probe-\(name).png"),
                             size: NSSize(width: 600, height: 700), appearance: appearance)
            try await render(CNCTraceSheet(cnc: cnc),
                             to: directory.appendingPathComponent("trace-\(name).png"),
                             size: NSSize(width: 560, height: 420), appearance: appearance)
        }
        // The snapshot is only worth having if it shows the two things it is
        // meant to show.
        XCTAssertEqual(cnc.machine.baselineDiff.count, 2)
        XCTAssertNotNil(cncMachineFindings(cnc).first(where: \.blocking))
    }

    private func render(_ view: some View, to url: URL, size: NSSize, appearance: NSAppearance.Name) async throws {
        let hosting = NSHostingView(rootView: AnyView(view.background(Color(nsColor: .windowBackgroundColor))))
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.appearance = NSAppearance(named: appearance)
        window.contentView = hosting; window.orderFront(nil)
        try? await Task.sleep(for: .milliseconds(250))
        hosting.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: url)
        window.orderOut(nil)
    }
}
