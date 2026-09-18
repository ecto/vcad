import XCTest
@testable import VcadApp

/// What the app will read back.
///
/// The one that mattered: the app could not import **its own exported job**.
/// The assembler emits `M0` between tools — this machine has no changer, so a
/// tool change is an operator stop and a re-probe — and `M0` was not on the
/// accepted M list, so a program the app had just written was rejected by the
/// app that wrote it. The round trip at the bottom is the test that could not
/// have passed before.
@MainActor
final class CNCImportTests: XCTestCase {

    private func built(_ cnc: CNCWorkspace) async {
        cnc.build()
        let deadline = Date().addingTimeInterval(120)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertFalse(cnc.generating)
    }

    // MARK: - the words the app's own post emits

    func testAcceptsTheSpinUpDwell() throws {
        // G4 takes time, not distance: it must not become a move.
        let program = try CNCImport.parse("G21 G90\nG0 X0 Y0 Z5\nM3 S13500\nG4 P3000\nG1 Z-1 F40\nM5\nM30")
        XCTAssertEqual(program.moves.count, 3, "the dwell is not a move")
        XCTAssertEqual(program.moves.last?.to, [0, 0, -1])
    }

    func testAcceptsTheModalHeaderTheJobAssemblerWrites() throws {
        // Exactly the preamble `GrblPost` emits, plus the operator pause the
        // job assembler puts between tools on a machine with no changer.
        let program = try CNCImport.parse("""
        G21
        G90
        G94
        G17
        G40
        G49
        G54
        G0 Z5
        M3 S10000
        G4 P3000
        G1 X10 Y0 Z-1 F250
        M0
        G1 X20 F250
        M5
        M9
        G0 Z5
        M30
        """)
        // The postamble retracts, so the last point is the park height — the
        // cut before it is what the depth is read from.
        XCTAssertEqual(program.moves.last?.to, [20, 0, 5])
        XCTAssertEqual(program.moves.dropLast().last?.to, [20, 0, -1])
    }

    func testAcceptsEveryWorkOffset() throws {
        for word in ["G54", "G55", "G56", "G57", "G58", "G59"] {
            // They preview identically: the program is read in whichever work
            // frame it selects, and the offset between them is on the
            // controller, not in the file.
            let program = try CNCImport.parse("G21 G90\n\(word)\nG0 X5 Y5\nM30")
            XCTAssertEqual(program.moves.last?.to, [5, 5, 0], word)
        }
    }

    func testAcceptsStopsAndPausesButNotAnEarlyEnd() throws {
        XCTAssertNoThrow(try CNCImport.parse("G0 X1\nM0\nG0 X2\nM30"))
        XCTAssertNoThrow(try CNCImport.parse("G0 X1\nM1\nG0 X2\nM2"))
        // An `M2`/`M30` before the last line means two programs in one file.
        XCTAssertThrowsError(try CNCImport.parse("G0 X1\nM30\nG0 X2\nM30"))
    }

    /// The comment that broke the round trip.
    ///
    /// `GrblPost` writes the tool as `(T1 Ø3.175 flat end mill (Ø3.17) at
    /// 10000 rpm)` — a comment whose *content* carries parentheses. The old
    /// stripper stopped at the first `)` and left ` at 10000 rpm)` on the
    /// line, which the parser then called unrecognised G-code. The app could
    /// not read its own header.
    func testCommentsWhoseContentCarriesParentheses() throws {
        XCTAssertEqual(CNCCommands.stripComments("(T1 Ø3.175 flat end mill (Ø3.17) at 10000 rpm)")
                        .trimmingCharacters(in: .whitespaces), "")
        XCTAssertEqual(CNCCommands.stripComments("G1 X10 (to the wall) Y2")
                        .trimmingCharacters(in: .whitespaces), "G1 X10  Y2")
        let program = try CNCImport.parse("""
        (vcad job)
        G21
        G90
        (T1 Ø3.175 flat end mill (Ø3.17) at 10000 rpm)
        G0 X1 Y2
        M30
        """)
        XCTAssertEqual(program.moves.last?.to, [1, 2, 0])
    }

    // MARK: - what it still refuses, and why

    /// `G53` moves in machine coordinates. Where that lands depends on the
    /// controller's work offset, which is not in the file — so a previewed
    /// position would be silently one work offset out, which is worse than no
    /// preview. The word is understood; the move is refused, in as many words.
    func testG53WithMotionIsRefusedForTheRightReason() throws {
        XCTAssertNoThrow(try CNCImport.parse("G21 G90\nG53\nG0 X1\nM30"),
                         "G53 with nothing on the line changes nothing")
        var message = ""
        XCTAssertThrowsError(try CNCImport.parse("G21 G90\nG53 G0 Z-5\nG0 X1\nM30")) {
            message = ($0 as? CNCError)?.localizedDescription ?? "\($0)"
        }
        XCTAssertTrue(message.contains("machine coordinates"), message)
        XCTAssertTrue(message.contains("work offset"), message)
    }

    func testStillRefusesWhatItCannotPlace() {
        // Tool length compensation: a Z offset held in the controller's table.
        XCTAssertThrowsError(try CNCImport.parse("G21 G90\nG43 H1\nG0 Z5\nM30"))
        // A canned drilling cycle, which is a program, not a move.
        XCTAssertThrowsError(try CNCImport.parse("G21 G90\nG81 X1 Y1 Z-5 R2 F100\nM30"))
        // A tool change the controller would silently ignore: this machine has
        // no changer, so a job that asks for one would carry on cutting with
        // whatever is in the collet.
        XCTAssertThrowsError(try CNCImport.parse("G21 G90\nT1 M6\nG0 X1\nM30"))
        // A second tool, on a machine with one holder and one manual install.
        XCTAssertThrowsError(try CNCImport.parse("G21 G90\nT2\nG0 X1\nM30"))
        // An M code nothing here models.
        XCTAssertThrowsError(try CNCImport.parse("G21 G90\nM62 P1\nG0 X1\nM30"))
    }

    // MARK: - the round trip

    /// Export a job, import it again, and have the re-import verify clean
    /// against the very outline it was cut from.
    ///
    /// This is the whole point of the fix: the app's own output is the program
    /// most likely to come back through this door — off a USB stick, out of
    /// ncSender, or from the operator who saved it yesterday.
    func testTheAppCanReadBackItsOwnJob() async throws {
        let cnc = CNCWorkspace()
        cnc.toolDiameter = 2
        cnc.toolStickout = 18
        cnc.toolFluteLength = 6
        cnc.stockThickness = 1
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1
            cnc.setup.bottomAllowance = 0.15      // an onion skin, deliberately
            cnc.setup.feed = 250; cnc.setup.plunge = 40; cnc.setup.rpm = 13500
            cnc.setup.stepdown = 0.17
            if cnc.setup.tabs > 0 { cnc.setup.tabHeight = 0.42 }
        }
        await built(cnc)

        let exported = try XCTUnwrap(cnc.jobCode, "the fixture is meant to pass its own checks")
        XCTAssertTrue(exported.contains("G4 P"), "the spin-up dwell is in there")
        XCTAssertTrue(exported.contains("G17"), "and the plane word")
        XCTAssertTrue(exported.contains("G94"), "and feed-per-minute")

        // Back in through the front door.
        try cnc.importProgram(exported, name: "anolex-job.nc")
        XCTAssertTrue(cnc.usesImportedProgram)
        let imported = try XCTUnwrap(cnc.importedProgram)
        XCTAssertFalse(imported.moves.isEmpty)

        // …and it is replayed against the outline it was cut from, not merely
        // parsed. `importedVerification` is that replay.
        XCTAssertNil(cnc.importedVerifyError, cnc.importedVerifyError ?? "")
        let verification = try XCTUnwrap(cnc.importedVerification)
        XCTAssertFalse(verification.isBlocked,
                       "the re-import of a passing job must pass: \(cnc.blockers.map(\.text))")
        XCTAssertTrue(cnc.blockers.isEmpty, cnc.blockers.map(\.text).joined(separator: " · "))
        XCTAssertNotNil(cnc.jobCode, "a passing imported job still has G-code to send")

        // The geometry survived the trip: the same extent, to a tenth.
        let extent = { (moves: [CNCMove]) -> [Double] in
            let xs = moves.map { $0.to[0] }, ys = moves.map { $0.to[1] }
            return [xs.min() ?? 0, ys.min() ?? 0, xs.max() ?? 0, ys.max() ?? 0]
        }
        let before = extent(cnc.job?.moves ?? [])
        let after = extent(imported.moves)
        for axis in 0..<4 {
            XCTAssertEqual(before[axis], after[axis], accuracy: 0.1,
                           "the re-imported path covers different ground on axis \(axis)")
        }
    }
}
