import XCTest
import RealityKit
@testable import VcadApp

/// The job pipeline, on the part that was really cut.
///
/// These assert geometry and verdicts, never existence: "three operations came
/// back" is how an inside contour that cut outward shipped. Every test here
/// says what the job does to the metal, or what stops it from running.
@MainActor
final class CNCJobTests: XCTestCase {

    // MARK: fixtures

    /// `docs/cam-fixtures/stator-outline.dxf`, found from this file rather than
    /// a bundle: the app target has no test resources, and a job that verifies
    /// against the wrong outline is exactly the failure this all exists to stop.
    static func statorDXF() throws -> String {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { url.deleteLastPathComponent() }
        let fixture = url.appendingPathComponent("docs/cam-fixtures/stator-outline.dxf")
        guard FileManager.default.fileExists(atPath: fixture.path) else {
            throw XCTSkip("stator fixture not found at \(fixture.path)")
        }
        return try String(contentsOf: fixture, encoding: .utf8)
    }

    private func statorOutline() throws -> CNCOutline {
        try CNCOutline.parseDXF(try Self.statorDXF(), name: "stator-outline.dxf")
    }

    /// Build the job and wait for it, with the same wall-clock patience the
    /// stator's 400-point loops need on a debug build.
    private func build(_ cnc: CNCWorkspace, timeout: TimeInterval = 120) async {
        cnc.build()
        let deadline = Date().addingTimeInterval(timeout)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
        XCTAssertFalse(cnc.generating, "the job never finished building")
    }

    /// The job that was really cut: Ø2 two-flute, 1 mm stock, 0.15 mm skin.
    ///
    /// Shared with the setup tests, which need the same part in the same state
    /// — a second fixture would be a second part, and then the two files would
    /// be testing different things without saying so.
    static func statorWorkspace(tool: Double = 2.0,
                                thickness: Double = 1.0,
                                skin: Double = 0.15,
                                under: CNCUnderStock = .machineBed) throws -> CNCWorkspace {
        let cnc = CNCWorkspace()
        cnc.toolDiameter = tool
        cnc.stockThickness = thickness
        cnc.underStock = under
        try cnc.importOutline(try CNCOutline.parseDXF(try statorDXF(), name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.bottomAllowance = skin
            if cnc.setup.tabs > 0 {
                cnc.setup.tabHeight = min(cnc.setup.tabHeight, max(0.1, (thickness - skin) / 2))
            }
        }
        return cnc
    }

    private func statorJob(tool: Double = 2.0,
                           thickness: Double = 1.0,
                           skin: Double = 0.15,
                           under: CNCUnderStock = .machineBed) throws -> CNCWorkspace {
        try Self.statorWorkspace(tool: tool, thickness: thickness, skin: skin, under: under)
    }

    private func runOrder(_ cnc: CNCWorkspace) -> [String] {
        (cnc.job?.opRanges ?? [])
            .filter { $0.block == "operation" }
            .sorted { $0.start < $1.start }
            .compactMap { range in
                cnc.operations.first { $0.ranges.contains { $0.start == range.start } }?.name
            }
    }

    // MARK: - The part that was cut

    /// The stator, as it went to the machine: three operations, the three Ø2.5
    /// pilots bored as one, nothing refused, and one spindle start for the lot.
    func testStatorJobIsThreeOperationsAndRunsClean() async throws {
        let cnc = try statorJob()
        XCTAssertEqual(cnc.operations.count, 3,
                       "expected pilots, opening and profile, got \(cnc.operations.map(\.name))")
        let pilots = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .helicalBore })
        XCTAssertEqual(pilots.name, "Pilot Ø2.5 × 3")
        XCTAssertEqual(pilots.setup.bores.count, 3)
        XCTAssertEqual(pilots.setup.boreDiameter, 2.5, accuracy: 0.02)
        XCTAssertEqual(cnc.operations.map(\.setup.kind),
                       [.helicalBore, .contourInside, .contourOutside],
                       "small holes first, then the opening, then the profile that frees the part")
        XCTAssertEqual(cnc.operations.last?.name, "Outside profile")

        await build(cnc)
        XCTAssertNil(cnc.error, "the job should not have been refused")
        XCTAssertTrue(cnc.jobCurrent)
        XCTAssertTrue(cnc.blockers.isEmpty, "blocked by: \(cnc.blockers.map(\.text))")
        XCTAssertTrue(cnc.verified, "a job with an outline has to be replayed against it")

        let code = try XCTUnwrap(cnc.jobCode, "a job that passes has to carry G-code")
        // One tool means one spindle start: the first job restarted the spindle
        // between every operation (friction-log item 42).
        let starts = code.components(separatedBy: .newlines)
            .map { $0.components(separatedBy: "(")[0] }
            .filter { $0.split(separator: " ").contains { $0.uppercased() == "M3" } }
        XCTAssertEqual(starts.count, 1, "exactly one M3, got \(starts)")

        // The skin is really left: a 0.15 mm skin in 1 mm stock stops at −0.85.
        let deepest = try XCTUnwrap(cnc.verification?.depth.deepestZ)
        XCTAssertEqual(deepest, -0.85, accuracy: 0.002)
        // And the bore each pilot leaves measures Ø2.5, cut with a Ø2 cutter.
        let moves = cnc.jobMoves
        for range in pilots.ranges where range.start < range.end {
            let cuts = moves[range.start..<range.end].filter { !$0.rapid }
            guard !cuts.isEmpty else { continue }
            let cx = cuts.map { $0.to[0] }.reduce(0, +) / Double(cuts.count)
            let cy = cuts.map { $0.to[1] }.reduce(0, +) / Double(cuts.count)
            let widest = cuts.map { hypot($0.to[0] - cx, $0.to[1] - cy) }.max() ?? 0
            XCTAssertEqual(2 * (widest + cnc.toolDiameter / 2), 2.5, accuracy: 0.05,
                           "a pilot measures Ø\(2 * (widest + 1)), not Ø2.5")
        }
    }

    /// The same part in stock too thin for it, over a bare bed. Nothing about
    /// this may reach the machine: no G-code, no export, no run.
    func testACutPastTheUndersideIsRefusedWithNothingToExport() async throws {
        let cnc = try statorJob(thickness: 0.8, skin: 0)
        // The depth follows the thickness, so put it back to the 1 mm cut the
        // part needs: that is the mistake being tested.
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1.0
        }
        await build(cnc)

        XCTAssertTrue(cnc.blockedByVerification, "a 1 mm cut in 0.8 mm stock over nothing has to be refused")
        XCTAssertEqual(cnc.policy?.blockedBy.contains("depth"), true,
                       "blocked by \(cnc.policy?.blockedBy ?? [])")
        XCTAssertNil(cnc.jobCode, "a refused job must not carry G-code")

        let blocker = try XCTUnwrap(cnc.runBlocker)
        XCTAssertTrue(blocker.contains("past the stock underside"),
                      "the blocker has to say what is wrong, got: \(blocker)")
        XCTAssertTrue(blocker.contains("spoilboard"),
                      "and what is missing, got: \(blocker)")
        // Numbers, not adjectives: 1.0 mm into 0.8 mm is 0.2 mm past the back.
        XCTAssertTrue(blocker.contains("0.200"), "the blocker has to carry the number, got: \(blocker)")

        if cnc.jobCode == nil { XCTAssertFalse(cnc.export(job: true), "there must be no way to export a refused job") }
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.setupConfirmed = true
        cnc.startJob()
        XCTAssertFalse(cnc.machine.active, "there must be no way to run a refused job")

        // The oracle still says what it found, so the user can fix it.
        XCTAssertNotNil(cnc.verification)
        let depth = try XCTUnwrap(cnc.checkRows.first { $0.id == "depth" })
        XCTAssertEqual(depth.verdict, .blocked)
    }

    /// Declaring what is under the blank is what makes the same cut legal.
    func testTheSameCutPassesOverADeclaredSpoilboard() async throws {
        let cnc = try statorJob(thickness: 0.8, skin: 0, under: .spoilboard(3))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1.0
        }
        await build(cnc)
        XCTAssertEqual(cnc.policy?.blockedBy.contains("depth"), false,
                       "over 3 mm of board, cutting 0.2 mm past the back is the job working: \(cnc.policy?.blockedBy ?? [])")
    }

    // MARK: - Item 49: the tool decides what is machinable, and it can change

    func testHolesTooSmallForTheCutterComeBackWhenTheToolDoes() async throws {
        let cnc = CNCWorkspace()
        cnc.toolDiameter = 3.175
        cnc.stockThickness = 1.0
        try cnc.importOutline(try statorOutline())

        XCTAssertEqual(cnc.unmachinableHoles.count, 3,
                       "three Ø2.5 pilots are smaller than a Ø3.175 cutter")
        XCTAssertTrue(cnc.unmachinableHoles.allSatisfy { abs($0.diameter - 2.5) < 0.05 })
        XCTAssertFalse(cnc.operations.contains { $0.setup.kind == .helicalBore },
                       "a hole the cutter cannot enter must not become an operation")
        XCTAssertEqual(try XCTUnwrap(cnc.error).contains("Ø 3.175"), true)

        // No re-import: changing the cutter re-decides on its own.
        cnc.toolDiameter = 2.0
        XCTAssertTrue(cnc.unmachinableHoles.isEmpty)
        let pilots = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .helicalBore })
        XCTAssertEqual(pilots.name, "Pilot Ø2.5 × 3")
        XCTAssertEqual(cnc.operations.first?.id, pilots.id, "pilots are cut before the opening")
        XCTAssertNil(cnc.error)

        // And back again, losing nothing but the holes that stopped fitting.
        cnc.toolDiameter = 3.175
        XCTAssertFalse(cnc.operations.contains { $0.setup.kind == .helicalBore })
        XCTAssertEqual(cnc.operations.count, 2)
    }

    /// Settings a user typed survive a tool change; only machinability moves.
    func testATooChangeKeepsTheSettingsOnTheOperationsThatSurvive() throws {
        let cnc = CNCWorkspace()
        cnc.toolDiameter = 2.0
        cnc.stockThickness = 1.0
        try cnc.importOutline(try statorOutline())
        let profile = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourOutside })
        cnc.select(.operation(profile.id))
        cnc.setup.feed = 137
        cnc.setup.tabs = 5

        cnc.toolDiameter = 3.175
        let after = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourOutside })
        XCTAssertEqual(after.setup.feed, 137)
        XCTAssertEqual(after.setup.tabs, 5)
    }

    // MARK: - Warnings

    /// A warning does not block, but it is not something the job can carry
    /// silently either — and the moment anything changes, it is unacknowledged
    /// again, because it was acknowledged about a different job.
    func testWarningsGateTheRunAndGoStaleOnAnyEdit() async throws {
        let cnc = CNCWorkspace()
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        await build(cnc)                          // no outline: nothing to verify against
        XCTAssertFalse(cnc.verified)
        let warning = try XCTUnwrap(cnc.warnings.first { $0.id == "unverified" })
        XCTAssertTrue(warning.text.contains("outline"))

        cnc.setupConfirmed = true
        let blocked = try XCTUnwrap(cnc.runBlocker)
        XCTAssertTrue(blocked.hasPrefix("Acknowledge"), "got: \(blocked)")
        cnc.startJob()
        XCTAssertFalse(cnc.machine.active, "an unacknowledged warning holds the run")

        // One at a time, not a single "yes to everything".
        let pending = cnc.warnings.map(\.id)
        XCTAssertTrue(pending.count >= 1)
        for id in pending.dropLast() { cnc.acknowledge(id, on: true) }
        XCTAssertNotNil(cnc.runBlocker, "one left unacknowledged still holds the run")
        cnc.acknowledge(pending.last!, on: true)
        cnc.setupConfirmed = true
        XCTAssertNil(cnc.runBlocker, "acknowledged warnings let the job run")

        // Change the job and the acknowledgement is about something else now.
        cnc.setup.feed = 321
        XCTAssertTrue(cnc.acknowledgements.isEmpty)
        XCTAssertFalse(cnc.jobCurrent)
        await build(cnc)
        cnc.setupConfirmed = true
        XCTAssertTrue(try XCTUnwrap(cnc.runBlocker).hasPrefix("Acknowledge"))
    }

    // MARK: - Pocketing the waste

    /// Item 39: the bore slug came free on the last pass next to a Ø3.175
    /// cutter. Pocketing the opening means nothing comes loose at all.
    func testPocketingTheOpeningLeavesNothingLoose() async throws {
        func loosePieces(_ kind: CNCOpKind) async throws -> Int {
            let cnc = try statorJob(thickness: 1.0, skin: 0, under: .spoilboard(3))
            let opening = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourInside })
            cnc.select(.operation(opening.id))
            cnc.setup.kind = kind
            cnc.setup.bottomAllowance = -0.2     // straight through, into the board
            await build(cnc)
            let loose = try XCTUnwrap(cnc.verification?.loose, "\(kind): \(cnc.error ?? "no verification")")
            return loose.pieces.filter { !$0.isPart }.count
        }
        let cutOut = try await loosePieces(.contourInside)
        let pocketed = try await loosePieces(.pocket)
        XCTAssertGreaterThan(cutOut, 0, "cutting the opening out frees the slug")
        XCTAssertEqual(pocketed, 0, "pocketing it turns the slug into chips: \(pocketed) piece(s) still free")
    }

    // MARK: - Order

    /// The list order is the job's order, with one rule the geometry imposes:
    /// once the profile has run the part is held by tabs at best, so it goes
    /// last unless the user says otherwise in as many words.
    func testReorderingPersistsAndTheProfileStillRunsLast() async throws {
        let cnc = try statorJob()
        let profile = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourOutside })
        cnc.select(.operation(profile.id))
        cnc.moveSelectedOperation(by: -1)
        cnc.moveSelectedOperation(by: -1)
        XCTAssertEqual(cnc.operations.first?.id, profile.id, "the list keeps what the user did")

        await build(cnc)
        XCTAssertEqual(runOrder(cnc).last, "Outside profile",
                       "the profile still runs last: \(runOrder(cnc))")

        cnc.select(.operation(profile.id))
        cnc.setup.forceOrder = true
        await build(cnc)
        XCTAssertEqual(runOrder(cnc).first, "Outside profile",
                       "forced, it runs where the list puts it: \(runOrder(cnc))")
    }

    /// Item 52: five operations, one material change.
    func testApplyingFeedsToAllOperations() throws {
        let cnc = try statorJob()
        cnc.select(.operation(cnc.operations[0].id))
        cnc.setup.feed = 250
        cnc.setup.plunge = 40
        cnc.setup.rpm = 13500
        cnc.setup.stepdown = 0.17
        XCTAssertNotEqual(cnc.operations[1].setup.feed, 250)
        cnc.applyFeedsToAllOperations()
        XCTAssertTrue(cnc.operations.allSatisfy { $0.setup.feed == 250 && $0.setup.plunge == 40
            && $0.setup.rpm == 13500 && $0.setup.stepdown == 0.17 })
        // Geometry is not a feed: the loops must not have been copied about.
        XCTAssertEqual(cnc.operations.map(\.setup.kind), [.helicalBore, .contourInside, .contourOutside])
        XCTAssertEqual(cnc.operations.last?.setup.tabs, 3)
    }

    // MARK: - Preview

    /// Every move belongs to exactly one block, so slicing the preview by an
    /// operation shows that operation and nothing else.
    func testBlockRangesPartitionTheMoves() async throws {
        let cnc = try statorJob()
        await build(cnc)
        let job = try XCTUnwrap(cnc.job)
        XCTAssertGreaterThan(job.moves.count, 100)
        let ranges = job.opRanges.sorted { $0.start < $1.start }
        var cursor = 0
        for range in ranges {
            XCTAssertEqual(range.start, cursor, "a gap or overlap before \(range.name ?? range.block)")
            XCTAssertLessThanOrEqual(range.start, range.end)
            cursor = range.end
        }
        XCTAssertEqual(cursor, job.moves.count, "the ranges have to cover every move")

        // And the operations between them hold every operation block.
        let owned = cnc.operations.flatMap(\.ranges).count
        XCTAssertEqual(owned, job.opRanges.filter { $0.block == "operation" }.count)
        for operation in cnc.operations {
            XCTAssertGreaterThan(operation.seconds, 0, "\(operation.name) has no time")
            XCTAssertGreaterThan(operation.preview.duration, 0, "\(operation.name) has no preview")
        }
        // The estimate the user reads is the one that knows about acceleration.
        XCTAssertEqual(cnc.jobDuration, job.duration.accelAwareS)
        XCTAssertGreaterThan(job.duration.accelAwareS, job.duration.naiveS,
                             "acceleration can only make a job take longer")
    }

    // MARK: - Imported programs

    func testAnImportedProgramWithNoOutlineIsMarkedUnverified() async throws {
        let cnc = CNCWorkspace()
        try cnc.importProgram("G21 G90\nG0 X1 Y2 Z3\nG1 X10 F100\nM2", name: "part.nc")
        XCTAssertTrue(cnc.usesImportedProgram)
        XCTAssertFalse(cnc.verified, "nothing checked this program")
        XCTAssertTrue(cnc.checkRows.isEmpty, "there are no check results to list, so none are listed")
        let unverified = try XCTUnwrap(cnc.warnings.first { $0.id == "unverified" })
        XCTAssertTrue(unverified.text.contains("Nothing has checked"), "got: \(unverified.text)")
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.setupConfirmed = true
        XCTAssertTrue(try XCTUnwrap(cnc.runBlocker).hasPrefix("Acknowledge"),
                      "an unverified program must not be runnable as if it had been checked")
    }

    /// With an outline loaded there is something to replay against, so an
    /// imported program is checked like any other job — and a program that
    /// ploughs a cut across the part is refused even though the app did not
    /// write it.
    func testAnImportedProgramIsReplayedAgainstTheOutline() async throws {
        let cnc = try statorJob()
        // A cut straight across the middle of the stator at full depth.
        let bad = """
        G21 G90
        G0 Z5
        G0 X5 Y31
        G1 Z-0.85 F40
        G1 X58 F250
        G0 Z5
        M2
        """
        try cnc.importProgram(bad, name: "wrong.nc")
        XCTAssertNil(cnc.importedVerifyError)
        XCTAssertTrue(cnc.verified, "an outline is present, so the program was replayed")
        XCTAssertTrue(cnc.blockedByVerification, "checks: \(cnc.policy?.blockedBy ?? [])")
        XCTAssertEqual(cnc.policy?.blockedBy.contains("gouge"), true,
                       "a cut through the ring is a gouge, got \(cnc.policy?.blockedBy ?? [])")
        // Assert the absence before asking to export: there is no panel to
        // open, because there is nothing to write.
        XCTAssertNil(cnc.jobCode, "a refused imported program has nothing to send either")
        if cnc.jobCode == nil { XCTAssertFalse(cnc.export(job: true)) }
        // And the reason reads as a place on the part, not a check name.
        let blocker = try XCTUnwrap(cnc.runBlocker)
        XCTAssertTrue(blocker.contains("into the part"), "got: \(blocker)")
    }

    // MARK: - What the viewport draws

    /// Item 47: the path used to be drawn in the stock frame while the part
    /// stayed where it was modelled, so it floated beside the solid. And item
    /// 41: the blank was drawn at the part's own extents, with nothing showing
    /// the margin it needs or where zero sits on it.
    func testTheOverlayPutsThePathOnThePartWithTheBlankItNeeds() async throws {
        let cnc = try statorJob()
        cnc.shown = true
        await build(cnc)
        cnc.select(.operation(cnc.operations[0].id))

        // The outline's own translation is on the overlay root, so the path
        // lands where the part was drawn rather than at the model origin.
        let outline = try statorOutline()
        XCTAssertEqual(cnc.origin.x, Double(outline.origin.x), accuracy: 1e-9)
        XCTAssertEqual(cnc.origin.y, Double(outline.origin.y), accuracy: 1e-9)
        XCTAssertNotEqual(cnc.origin.x, 0, "the stator is not modelled at the origin")

        let parent = Entity()
        syncCNCOverlay(cnc, in: parent)
        let root = try XCTUnwrap(parent.findEntity(named: "cncRoot"))
        XCTAssertEqual(root.position.x, Float(outline.origin.x), accuracy: 1e-4)

        // The blank is the part plus its margin, not the part's own extents.
        let margin = cnc.effectiveMargin
        XCTAssertEqual(margin, 6, accuracy: 1e-9, "max(2 × Ø2 + 2, 5) is 6 mm")
        let stock = try XCTUnwrap(root.findEntity(named: "cncStock"))
        let stockSize = stock.visualBounds(relativeTo: root).extents
        XCTAssertEqual(Double(stockSize.x), cnc.stockWidth + 2 * margin, accuracy: 0.01)
        XCTAssertEqual(Double(stockSize.y), cnc.stockHeight + 2 * margin, accuracy: 0.01)

        // The cutter's swept rectangle is the part grown by one radius.
        let sweep = try XCTUnwrap(root.findEntity(named: "cncSweep"))
        let sweepSize = sweep.visualBounds(relativeTo: root).extents
        XCTAssertEqual(Double(sweepSize.x), cnc.stockWidth + cnc.toolDiameter, accuracy: 0.01)

        // Work zero, and the blank corner it is measured from.
        XCTAssertNotNil(root.findEntity(named: "cncOrigin"))
        let corner = try XCTUnwrap(root.findEntity(named: "cncStockCorner"))
        XCTAssertEqual(corner.position(relativeTo: root), [Float(-margin), Float(-margin), 0])

        // And the tabs, drawn where the audit found them rather than where
        // they were asked for.
        let tabs = try XCTUnwrap(cnc.verification?.tabs)
        XCTAssertEqual(tabs.tabCount, 3)
        XCTAssertEqual(root.children.first?.children.filter { $0.name.hasPrefix("cncTab-") }.count,
                       tabs.observations.count)
        XCTAssertGreaterThan(tabs.observations.count, 0)
        for observation in tabs.observations {
            XCTAssertGreaterThan(observation.metalWidth, 1.5,
                                 "a 4 mm tab cut with a Ø2 cutter leaves about 2 mm of metal")
            XCTAssertGreaterThan(observation.topZ, cnc.verification!.depth.deepestZ,
                                 "a tab is above the floor or it is not a tab")
        }
    }

    // MARK: - Selecting a violation

    /// Picking a violation has to take the user to the cut that made it.
    func testSelectingAViolationSelectsItsOperationAndMarksThePlace() async throws {
        let cnc = try statorJob(thickness: 0.8, skin: 0)
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1.0
        }
        await build(cnc)
        let finding = try XCTUnwrap(cnc.blockers.first { $0.xy != nil })
        cnc.select(finding)
        XCTAssertEqual(cnc.markedXY?.count, 2)
        XCTAssertEqual(cnc.markedXY, finding.xy)
        let id = try XCTUnwrap(finding.operationID, "a violation with a place has to name a cut")
        XCTAssertEqual(cnc.selectedOperation.id, id)

        // …and it has to be the right cut: the operation it names must have a
        // move that really passes through the place the oracle reported, or
        // the red mark is pointing at the wrong row.
        let xy = try XCTUnwrap(finding.xy)
        let moves = cnc.jobMoves
        let named = try XCTUnwrap(cnc.operations.first { $0.id == id })
        let nearest = named.ranges.flatMap { range -> [Double] in
            guard range.start < range.end, range.end <= moves.count else { return [] }
            return moves[range.start..<range.end].map { hypot($0.to[0] - xy[0], $0.to[1] - xy[1]) }
        }.min() ?? .greatestFiniteMagnitude
        XCTAssertLessThan(nearest, 0.05,
                          "\(named.name) is named for a violation at \(xy) but its nearest move is \(nearest) mm away")
    }
}
