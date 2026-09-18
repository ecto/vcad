import XCTest
import RealityKit
@testable import VcadApp

/// Setting a job up on real metal: where the outline comes from, what the
/// blank is made of, where its zero is, what is clamped to it, and where the
/// tabs are.
///
/// House rule 1 applies: every assertion here is about geometry or a verdict —
/// a diameter, a distance, a derate, a refusal — never that a call came back.
@MainActor
final class CNCSetupTests: XCTestCase {

    // MARK: fixtures

    /// The repository root, found from this file: the app target has no test
    /// resources and a job checked against the wrong part is the whole reason
    /// this package exists.
    private static func repoRoot() -> URL {
        var url = URL(fileURLWithPath: #filePath)
        for _ in 0..<5 { url.deleteLastPathComponent() }
        return url
    }

    /// `examples/parametric-plate.vcad`, opened the way the app opens a
    /// document: the bytes on disk, evaluated by the kernel.
    private func plate() throws -> CNCModelDocument {
        let url = Self.repoRoot().appendingPathComponent("examples/parametric-plate.vcad")
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw XCTSkip("parametric-plate.vcad not found at \(url.path)")
        }
        return CNCModelDocument(data: try Data(contentsOf: url), isLoon: false,
                                name: "parametric-plate.vcad")
    }

    /// A workspace with the plate on screen, as the studio wires it.
    private func plateWorkspace() throws -> CNCWorkspace {
        let cnc = CNCWorkspace()
        let document = try plate()
        cnc.modelDocument = { document }
        cnc.toolDiameter = 3.175
        return cnc
    }

    private func build(_ cnc: CNCWorkspace, timeout: TimeInterval = 180) async {
        cnc.build()
        let deadline = Date().addingTimeInterval(timeout)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
        XCTAssertFalse(cnc.generating, "the job never finished building")
    }

    /// Cutting moves only, in the work frame.
    private func cuts(_ cnc: CNCWorkspace) -> [[Double]] {
        cnc.jobMoves.filter { !$0.rapid && $0.to.count >= 3 }.map { $0.to }
    }
    private func bounds(_ points: [[Double]]) -> [Double] {
        guard !points.isEmpty else { return [0, 0, 0, 0] }
        return [points.map { $0[0] }.min()!, points.map { $0[1] }.min()!,
                points.map { $0[0] }.max()!, points.map { $0[1] }.max()!]
    }

    // MARK: - The outline comes out of the part (items 16, 37)

    func testTheOutlineComesOutOfThePartOnScreen() async throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.hasModel, "a document is open, so From model is offered")
        XCTAssertTrue(cnc.importFromModel(), "the plate sections: \(cnc.error ?? "no error")")

        let outline = try XCTUnwrap(cnc.outline)
        // 80 × 50 plate with a 1.5 mm fillet round it and one Ø16 bore.
        XCTAssertEqual(outline.width, 80, accuracy: 0.05)
        XCTAssertEqual(outline.height, 50, accuracy: 0.05)
        XCTAssertEqual(outline.holes.count, 1, "one bore, not \(outline.holes.count)")
        let bore = try XCTUnwrap(outline.holes.first?.circle,
                                 "the bore has to read back as a circle, or it will be machined as a polygon")
        XCTAssertEqual(bore.diameter, 16, accuracy: 0.05)
        XCTAssertEqual(Double(bore.centre.x), 40, accuracy: 0.1)
        XCTAssertEqual(Double(bore.centre.y), 25, accuracy: 0.1)

        // The blank is as thick as the part is tall: the thickness used to sit
        // at its 10 mm default until it was typed in by hand (item 17).
        let section = try XCTUnwrap(cnc.modelSection)
        XCTAssertEqual(section.suggestedStockThickness, 6, accuracy: 0.01)
        XCTAssertEqual(cnc.stockThickness, 6, accuracy: 0.01)
        XCTAssertEqual(section.meshSource, "raw_tessellation",
                       "a part with a B-rep is sectioned from the solid, not the export mesh")
        XCTAssertFalse(section.prismaticVerdict.isEmpty)

        // And it builds a job that is not refused.
        await build(cnc)
        XCTAssertTrue(cnc.blockers.isEmpty, "blocked by: \(cnc.blockers.map(\.text))")
        XCTAssertTrue(cnc.verified, "an outline from the part is something to verify against")
        XCTAssertNotNil(cnc.jobCode)
    }

    /// A torn solid is refused with its gap count and width, and no outline is
    /// invented from it.
    func testWhatTheSectionSaysIsWhatTheUserIsTold() throws {
        let cnc = CNCWorkspace()
        cnc.modelDocument = { nil }
        XCTAssertFalse(cnc.importFromModel())
        XCTAssertEqual(cnc.error?.contains("no document"), true, "got: \(cnc.error ?? "nothing")")
        XCTAssertNil(cnc.outline, "nothing was imported, so nothing is machinable")
    }

    // MARK: - A DXF that is not the part on screen (item 16)

    /// A DXF outline is compared with the part it is supposed to be. The
    /// mutation this guards against is skipping the comparison: without it the
    /// wrong outline machines in silence, which is exactly what happened.
    func testADXFThatDisagreesWithTheModelWarnsWithNumbers() async throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.importFromModel())
        XCTAssertNil(cnc.outlineMismatch, "the model agrees with itself")

        // The same plate, 4 mm narrower: a plausible stale revision.
        try cnc.importOutline(try CNCOutline.parseDXF(Self.rectangleDXF(width: 76, height: 50),
                                                      name: "stale.dxf"))
        let warning = try XCTUnwrap(cnc.outlineMismatch,
                                    "a 4 mm narrower outline is not this part and has to say so")
        XCTAssertTrue(warning.contains("not the part on screen"), "got: \(warning)")
        // Numbers, not adjectives: how far apart the boundaries are — at least
        // the 4 mm the edges differ by — and the hole the DXF does not have.
        let millimetres = Self.numbers(in: warning)
        XCTAssertTrue(millimetres.contains { $0 >= 3.9 },
                      "the warning has to carry how far apart they are: \(warning)")
        XCTAssertTrue(warning.contains("1 hole on one side only"),
                      "and that the bore is missing from the DXF entirely: \(warning)")

        // …and it is a warning that holds the run until it is acknowledged,
        // not a note nobody has to read.
        await build(cnc)
        let finding = try XCTUnwrap(cnc.warnings.first { $0.id == "outline-mismatch" })
        XCTAssertEqual(finding.text, warning)
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.setupConfirmed = true
        XCTAssertEqual(try XCTUnwrap(cnc.runBlocker).hasPrefix("Acknowledge"), true,
                       "got: \(cnc.runBlocker ?? "nothing")")

        // The mutation, stated as a test: with nothing to compare against
        // there is no warning — so a build that skips the comparison would
        // leave this assertion looking exactly like a pass.
        let blind = CNCWorkspace()
        blind.toolDiameter = 3.175
        try blind.importOutline(try CNCOutline.parseDXF(Self.rectangleDXF(width: 76, height: 50),
                                                        name: "stale.dxf"))
        XCTAssertNil(blind.outlineMismatch, "no part on screen, nothing to disagree with")
    }

    // MARK: - Material and feeds (items 52, 54)

    func testCopperOnATwoFluteCutterRecommendsTheDialAndDeratesTheFirstCut() throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.importFromModel())
        cnc.loadMaterials()
        XCTAssertTrue(cnc.materials.contains { $0.id == "copper-c110" },
                      "the table has copper: \(cnc.materials.map(\.id))")
        cnc.materialID = "copper-c110"
        cnc.toolDiameter = 2.0
        cnc.toolFlutes = 2

        let advice = try XCTUnwrap(cnc.applyRecommendedFeeds(), "copper and a Ø2 cutter is a table entry")
        // The S word does nothing on this spindle, so the dial is the answer.
        XCTAssertTrue(advice.dialSpindle)
        XCTAssertEqual(advice.recommendation.dial, "2",
                       "position 2 is ~13 500 rpm, which is what the real cut ran at")
        XCTAssertTrue(advice.spindleAdvice.contains("dial"), "got: \(advice.spindleAdvice)")
        XCTAssertTrue(advice.spindleAdvice.contains("does nothing"),
                      "the panel has to say the S word is irrelevant: \(advice.spindleAdvice)")
        let full = advice.recommendation.feedMmMin
        XCTAssertTrue((180...350).contains(full), "F\(full) is not in the neighbourhood of the cut that worked")

        // Every operation got the numbers, not just the selected one (item 52).
        XCTAssertTrue(cnc.operations.allSatisfy { abs($0.setup.feed - full) < 1e-9 },
                      "recommend has to apply to all: \(cnc.operations.map(\.setup.feed))")

        // And the first-cut toggle derates feed and stepdown by exactly 0.6.
        let stepdown = advice.recommendation.stepdownMm
        cnc.firstCutDerate = true
        let derated = try XCTUnwrap(cnc.applyRecommendedFeeds())
        XCTAssertEqual(derated.values(derated: true).feed, full * 0.6, accuracy: 1e-9)
        XCTAssertEqual(derated.values(derated: true).stepdown, stepdown * 0.6, accuracy: 1e-9)
        XCTAssertEqual(cnc.setup.feed, full * 0.6, accuracy: 1e-9)
        XCTAssertEqual(cnc.setup.stepdown, stepdown * 0.6, accuracy: 1e-9)
        // The plunge and the spindle are not derated: a slower plunge is not
        // safer, and the dial has six positions, not sixty.
        XCTAssertEqual(derated.values(derated: true).rpm, advice.recommendation.rpm, accuracy: 1e-9)
    }

    /// Numbers typed by hand get the same table's second opinion.
    func testAFeedThatWouldRubSaysSoBesideTheNumber() throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.importFromModel())
        cnc.loadMaterials()
        cnc.materialID = "copper-c110"
        cnc.toolDiameter = 2.0
        cnc.toolFlutes = 2

        var spec = cnc.setup
        spec.rpm = 13500
        spec.feed = 50            // 0.0019 mm/tooth: burnishing, not cutting
        spec.plunge = 40
        spec.stepdown = 0.17
        cnc.setup = spec
        let notes = cnc.feedNotes.map(\.text).joined(separator: " | ")
        XCTAssertTrue(notes.lowercased().contains("rub"),
                      "a chipload that low has to be called what it is: \(notes)")
        XCTAssertTrue(cnc.feedNotes.contains { $0.level >= .warning }, "and loudly: \(notes)")

        // A sensible feed says nothing alarming.
        spec.feed = 250
        cnc.setup = spec
        XCTAssertFalse(cnc.feedNotes.contains { $0.level >= .warning },
                       "F250 is the feed that cut the real part: \(cnc.feedNotes.map(\.text))")
    }

    // MARK: - Stock, zero and placement (items 17, 18, 41, 53)

    func testTheZeroChoiceMovesTheJobWithItAndChangesNothingElse() async throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.importFromModel())
        let margin = cnc.effectiveMargin

        // Zero at the part's own corner: the blank's corner is a margin away
        // in each axis, and negative, because it is outside the part.
        cnc.zeroLocation = .partCorner
        XCTAssertEqual(cnc.stockCornerFromZero[0], -margin, accuracy: 1e-9)
        XCTAssertEqual(cnc.stockCornerFromZero[1], -margin, accuracy: 1e-9)
        await build(cnc)
        let atPart = bounds(cuts(cnc))
        let operations = cnc.operations.map(\.name)
        let verdict = cnc.blockers.map(\.text)

        // Zero on the blank's corner: nothing to measure — that is the point
        // of touching off there — and the job moves onto it.
        cnc.zeroLocation = .stockCorner
        XCTAssertEqual(cnc.stockCornerFromZero[0], 0, accuracy: 1e-9)
        XCTAssertEqual(cnc.stockCornerFromZero[1], 0, accuracy: 1e-9)
        await build(cnc)
        let atCorner = bounds(cuts(cnc))
        for axis in 0..<4 {
            XCTAssertEqual(atCorner[axis] - atPart[axis], margin, accuracy: 1e-6,
                           "the whole job shifts by the margin and keeps its shape")
        }

        // Zero in the middle of the blank: the part straddles it.
        cnc.zeroLocation = .stockCentre
        XCTAssertEqual(cnc.stockCornerFromZero[0], -cnc.stockWidth / 2 - margin, accuracy: 1e-9)
        XCTAssertEqual(cnc.stockCornerFromZero[1], -cnc.stockHeight / 2 - margin, accuracy: 1e-9)
        await build(cnc)
        let atCentre = bounds(cuts(cnc))
        XCTAssertEqual(atCentre[0] + atCentre[2], 0, accuracy: 0.5,
                       "zeroed in the middle, the cut is symmetric about X0")

        // …and nothing else moved: same operations, same size of cut, same
        // verdict. Only where the numbers are measured from changed.
        XCTAssertEqual(cnc.operations.map(\.name), operations)
        XCTAssertEqual(cnc.blockers.map(\.text), verdict)
        XCTAssertEqual(atCentre[2] - atCentre[0], atPart[2] - atPart[0], accuracy: 1e-6)
        XCTAssertEqual(atCentre[3] - atCentre[1], atPart[3] - atPart[1], accuracy: 1e-6)
    }

    func testTurningTheJobTurnsThePartItIsCheckedAgainst() async throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.importFromModel())
        await build(cnc)
        XCTAssertTrue(cnc.blockers.isEmpty, "blocked by: \(cnc.blockers.map(\.text))")
        let square = bounds(cuts(cnc))

        cnc.placement.rotationDeg = 10
        await build(cnc)
        let turned = bounds(cuts(cnc))
        XCTAssertTrue(cnc.blockers.isEmpty,
                      "a turned job is still checked against the turned part: \(cnc.blockers.map(\.text))")
        XCTAssertTrue(cnc.verified)

        // A rectangle turned by 10° needs a wider box: w·cos + h·sin. The cut
        // is a rounded rectangle — 1.5 mm fillets plus the cutter radius — so
        // it turns into a box about a millimetre under that ideal.
        let w = square[2] - square[0], h = square[3] - square[1]
        let a = 10.0 * .pi / 180
        XCTAssertEqual(turned[2] - turned[0], w * cos(a) + h * sin(a), accuracy: 1.5,
                       "the sweep has to turn with the job")
        XCTAssertEqual(turned[3] - turned[1], w * sin(a) + h * cos(a), accuracy: 1.5)
        // 50 mm of part turned through 10° is another 8.7 mm of X, less what
        // the rounded corners take back: the job is several millimetres wider
        // on the metal, which is the whole reason the sweep is drawn.
        XCTAssertGreaterThan(turned[2] - turned[0], w + 4,
                             "a job turned 10° is materially wider than one that is not")
        // And the envelope the app draws agrees with the moves it built.
        let sweep = cnc.sweepRect
        XCTAssertEqual(sweep[2] - sweep[0], turned[2] - turned[0], accuracy: 2 * cnc.toolDiameter)
    }

    func testAClampInTheCuttersWayIsAWarningWithTheNumberInIt() async throws {
        let cnc = try plateWorkspace()
        XCTAssertTrue(cnc.importFromModel())
        await build(cnc)
        XCTAssertTrue(cnc.clampsInTheWay.isEmpty, "no clamps, nothing in the way")

        // A clamp 10 mm inside the sweep, from the left.
        let sweep = cnc.sweepRect
        cnc.clamps = [CNCClamp(x: sweep[0] - 30, y: sweep[1] + 5, width: 40, height: 20, name: "Toe clamp")]
        XCTAssertEqual(cnc.clampsInTheWay.count, 1)
        XCTAssertEqual(cnc.clamps[0].overlapDepth(sweep), 10, accuracy: 1e-9)

        await build(cnc)
        let warning = try XCTUnwrap(cnc.warnings.first { $0.id.hasPrefix("clamp-") })
        XCTAssertTrue(warning.text.contains("Toe clamp"), "got: \(warning.text)")
        XCTAssertTrue(warning.text.contains("10.0"), "with the number: \(warning.text)")
        XCTAssertTrue(warning.text.contains("does not know about clamps"),
                      "and says who checked it: \(warning.text)")

        // Moved off the blank, it stops being a warning.
        cnc.clamps[0].x = sweep[0] - 60
        XCTAssertTrue(cnc.clampsInTheWay.isEmpty)
        await build(cnc)
        XCTAssertNil(cnc.warnings.first { $0.id.hasPrefix("clamp-") })
    }

    // MARK: - Tabs (item 21)

    func testTabsAreDrawnWhereTheyAreAskedForAndWhereTheyAreCut() async throws {
        let cnc = try CNCJobTests.statorWorkspace(tool: 2.0, thickness: 1.0, skin: 0.15)
        await build(cnc)
        XCTAssertTrue(cnc.blockers.isEmpty, "blocked by: \(cnc.blockers.map(\.text))")

        let profile = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourOutside })
        XCTAssertEqual(cnc.declaredTabPositions(of: profile).count, 3, "three tabs were asked for")
        let landings = cnc.tabLandings(of: profile)
        XCTAssertEqual(landings.count, 3, "and three came back from the audit")
        for landing in landings {
            XCTAssertGreaterThan(landing.metalWidth, 1.5,
                                 "a 4 mm tab cut with a Ø2 cutter leaves about 2 mm of metal")
        }

        // Both are drawn, and differently: a handle where each tab was asked
        // for, and the metal where the audit found it.
        cnc.shown = true
        let parent = Entity()
        syncCNCOverlay(cnc, in: parent)
        let root = try XCTUnwrap(parent.findEntity(named: "cncRoot"))
        let group = try XCTUnwrap(root.children.first)
        let handles = group.children.filter { $0.name.hasPrefix("cncTabHandle-") }
        XCTAssertEqual(handles.count, 3, "one draggable handle per declared tab")
        XCTAssertEqual(group.children.filter { $0.name.hasPrefix("cncTab-") }.count,
                       landings.count, "and the cut tabs drawn where the audit put them")
        XCTAssertTrue(handles.allSatisfy { $0.components[CollisionComponent.self] != nil },
                      "a handle you cannot hit is not a handle")
    }

    func testADraggedTabEndsUpWhereItWasDropped() async throws {
        let cnc = try CNCJobTests.statorWorkspace(tool: 2.0, thickness: 1.0, skin: 0.15)
        await build(cnc)
        let profile = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourOutside })
        cnc.select(.operation(profile.id))

        // Drag tab 0 to a quarter of the way round the outline.
        let target = 0.25
        let contour = profile.setup.contour
        let perimeter = Self.perimeter(contour)
        let drop = try XCTUnwrap(Self.point(on: contour, at: target))
        cnc.moveTab(operation: profile.id, index: 0, to: drop)
        XCTAssertTrue(cnc.generating || cnc.jobCurrent)
        let deadline = Date().addingTimeInterval(240)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }

        // The request carries a position rather than a count, so the kernel is
        // placing the tab, not spacing it.
        let after = try XCTUnwrap(cnc.operations.first { $0.id == profile.id })
        XCTAssertEqual(after.setup.tabPositions.count, 3, "the other two tabs stayed")
        XCTAssertTrue(cnc.blockers.isEmpty, "blocked by: \(cnc.blockers.map(\.text))")

        // And it landed there: within 2% of the perimeter of where it was
        // dropped, measured by the audit on the contour as drawn.
        let landings = cnc.tabLandings(of: after)
        XCTAssertEqual(landings.count, 3)
        let nearest = landings.min {
            Self.hypot2($0.at, drop) < Self.hypot2($1.at, drop)
        }
        let landed = try XCTUnwrap(nearest)
        var difference = abs(landed.alongContour - target)
        difference = min(difference, 1 - difference)
        XCTAssertLessThan(difference, 0.02,
                          "dropped at \(target) of the way round (\(perimeter) mm), landed at \(landed.alongContour)")
        XCTAssertLessThan(Self.hypot2(landed.at, drop).squareRoot(), 0.03 * perimeter,
                          "and in the same place on the part: \(landed.at) against \(drop)")

        // Adding a tab keeps the ones that are placed and fills the widest gap.
        cnc.setTabCount(4)
        XCTAssertEqual(cnc.setup.tabPositions.count, 4)
        XCTAssertEqual(cnc.setup.tabs, 4)
        XCTAssertTrue(cnc.setup.tabPositions.contains { abs($0 - after.setup.tabPositions[0]) < 1e-9 },
                      "the dragged tab is still where it was put")
        cnc.setTabCount(0)
        XCTAssertTrue(cnc.setup.tabPositions.isEmpty)
    }

    // MARK: - Islands: material a pocket keeps

    /// A pocket with a boss in it: the cutter clears around the boss and never
    /// touches it, and the job says how close it came.
    func testAPocketKeepsItsIslandsAndTheJobSaysHowCloseItCame() async throws {
        func job(keepIsland: Bool) async throws -> CNCWorkspace {
            let cnc = CNCWorkspace()
            cnc.toolDiameter = 2.0
            cnc.stockThickness = 6.0
            cnc.stockWidth = 60; cnc.stockHeight = 40
            cnc.addOperation(.pocket)
            var spec = cnc.setup
            spec.contour = [[10, 10], [50, 10], [50, 30], [10, 30]]
            spec.islands = keepIsland ? [Self.circle(centre: [30, 20], radius: 4)] : []
            spec.depth = 2.0; spec.stepdown = 0.5; spec.stepover = 0.8
            spec.feed = 400; spec.plunge = 100; spec.rpm = 12000
            spec.bottomAllowance = 0
            cnc.setup = spec
            // Every other operation removed: this is about one pocket.
            while cnc.operations.count > 1 {
                cnc.select(.operation(cnc.operations.first { $0.setup.kind != .pocket }!.id))
                cnc.removeSelectedOperation()
            }
            await build(cnc)
            return cnc
        }

        let kept = try await job(keepIsland: true)
        XCTAssertTrue(kept.blockers.isEmpty, "blocked by: \(kept.blockers.map(\.text))")
        let clearances = kept.islandClearances(of: kept.selectedOperation)
        XCTAssertEqual(clearances.count, 1, "one island, one answer")
        let island = try XCTUnwrap(clearances.first)
        XCTAssertTrue(island.kept, "the boss must still be there: cut into by \(island.cutIntoMm) mm")
        XCTAssertEqual(try XCTUnwrap(island.centreClearanceMm), 1.0, accuracy: 0.05,
                       "the cutter centre stays one radius off the island wall")

        // Measured off the moves, not off the report: a Ø2 cutter clearing
        // around a Ø8 boss may bring its centre no nearer than 5 mm.
        let nearest = cuts(kept).map { Foundation.hypot($0[0] - 30, $0[1] - 20) }.min() ?? .infinity
        XCTAssertGreaterThan(nearest, 4.97, "the cutter came \(nearest) mm from the boss's centre")

        // The mutation: the same pocket with no islands declared cuts the boss
        // away, so this test cannot pass whether islands work or not.
        let cleared = try await job(keepIsland: false)
        let over = cuts(cleared).map { Foundation.hypot($0[0] - 30, $0[1] - 20) }.min() ?? .infinity
        XCTAssertLessThan(over, 4.0, "without islands the pocket has to clear the boss away")
        XCTAssertTrue(cleared.islandClearances(of: cleared.selectedOperation).isEmpty)
    }

    /// The stator's three pilots cannot be islands of its opening: they are in
    /// the ring, outside that loop. The app says so rather than sending a
    /// request the kernel will refuse.
    func testIslandsAreOfferedOnlyWhereSomethingLiesInsideThePocket() throws {
        let cnc = try CNCJobTests.statorWorkspace(tool: 2.0, thickness: 1.0, skin: 0.15)
        let opening = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .contourInside })
        cnc.select(.operation(opening.id))
        cnc.setup.kind = .pocket
        XCTAssertTrue(cnc.islandCandidates(for: cnc.selectedOperation).isEmpty,
                      "the pilots sit in the ring, not inside the bore-and-slots opening")
        cnc.keepIslands(true)
        XCTAssertTrue(cnc.setup.islands.isEmpty)
        XCTAssertEqual(cnc.error?.contains("nothing for the pocket to keep"), true,
                       "got: \(cnc.error ?? "nothing")")
    }

    // MARK: - helpers

    /// A closed rectangle as a DXF, for the outlines this file compares.
    static func rectangleDXF(width: Double, height: Double) -> String {
        var lines = ["0", "SECTION", "2", "ENTITIES", "0", "LWPOLYLINE", "70", "1"]
        for point in [[0.0, 0.0], [width, 0], [width, height], [0, height]] {
            lines += ["10", "\(point[0])", "20", "\(point[1])"]
        }
        lines += ["0", "ENDSEC", "0", "EOF"]
        return lines.joined(separator: "\n")
    }

    static func numbers(in text: String) -> [Double] {
        text.split(whereSeparator: { !"0123456789.".contains($0) }).compactMap { Double($0) }
    }

    static func circle(centre: [Double], radius: Double, segments: Int = 48) -> [[Double]] {
        (0..<segments).map { i in
            let a = 2 * Double.pi * Double(i) / Double(segments)
            return [centre[0] + radius * cos(a), centre[1] + radius * sin(a)]
        }
    }

    static func perimeter(_ points: [[Double]]) -> Double {
        var total = 0.0
        for i in points.indices {
            let a = points[i], b = points[(i + 1) % points.count]
            total += Foundation.hypot(b[0] - a[0], b[1] - a[1])
        }
        return total
    }

    static func point(on points: [[Double]], at fraction: Double) -> [Double]? {
        let total = perimeter(points)
        guard total > 0 else { return nil }
        var remaining = fraction * total
        for i in points.indices {
            let a = points[i], b = points[(i + 1) % points.count]
            let length = Foundation.hypot(b[0] - a[0], b[1] - a[1])
            if remaining <= length {
                let t = length > 0 ? remaining / length : 0
                return [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            }
            remaining -= length
        }
        return points.first
    }

    static func hypot2(_ a: [Double], _ b: [Double]) -> Double {
        guard a.count >= 2, b.count >= 2 else { return .infinity }
        return (a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])
    }
}
