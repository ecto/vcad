import XCTest
@testable import VcadApp

/// Multi-tool jobs and drilling, in the app (friction-log item 19).
///
/// The kernel has had drill ops and multi-tool assembly since wave 1; what was
/// missing was a tool list to say a second cutter exists and a planner that
/// decides a hole is *drilled* rather than milled. These assert the geometry
/// and the program that comes out — which tool cuts what, how many times the
/// job stops, and that taking the drill away puts the hole back in the
/// unmachinable list rather than quietly cutting it with something else.
@MainActor
final class CNCToolTests: XCTestCase {

    private func build(_ cnc: CNCWorkspace, timeout: TimeInterval = 120) async {
        cnc.build()
        let deadline = Date().addingTimeInterval(timeout)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
    }

    /// The stator with the two tools it really needs: a Ø3.175 end mill for
    /// the profile and the Ø27.6 bore, and a Ø2.5 drill for the three pilots
    /// that no end mill in the list fits into.
    private func twoToolStator() throws -> CNCWorkspace {
        let cnc = CNCWorkspace()
        cnc.tools = [
            CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175, flutes: 2,
                    fluteLength: 10, stickout: 20),
            CNCTool(number: 2, kind: .drill, diameter: 2.5, flutes: 2,
                    fluteLength: 20, stickout: 30),
        ]
        cnc.stockThickness = 6
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 6
            cnc.setup.feed = 250; cnc.setup.plunge = 40; cnc.setup.rpm = 13500
            cnc.setup.stepdown = 0.5
            if cnc.setup.tabs > 0 { cnc.setup.tabHeight = 0.5 }
        }
        return cnc
    }

    // MARK: - the plan

    /// Three operations, the pilots drilled on T2 and everything else milled
    /// on T1 — and nothing left in the unmachinable list, because the drill
    /// that makes those holes is now in it.
    func testTheStatorPlansThreeOperationsAcrossTwoTools() throws {
        let cnc = try twoToolStator()

        XCTAssertEqual(cnc.operations.count, 3,
                       "expected drill, opening and profile, got \(cnc.operations.map(\.name))")
        XCTAssertTrue(cnc.unmachinableHoles.isEmpty,
                      "the Ø2.5 drill makes the pilots, so nothing is unmachinable: \(cnc.unmachinableHoles)")

        let drill = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .drill })
        XCTAssertEqual(drill.name, "Drill Ø2.5 × 3")
        XCTAssertEqual(drill.setup.toolNumber, 2, "the pilots are drilled with the drill")
        XCTAssertEqual(drill.setup.bores.count, 3, "three pilots, one operation")
        XCTAssertEqual(drill.setup.boreDiameter, 2.5, accuracy: 1e-9,
                       "a drill makes exactly its own size, not the outline's rounding of it")
        XCTAssertEqual(cnc.operations.first?.id, drill.id,
                       "the small holes run before the profile that frees the part")

        // Everything else is the end mill's.
        for operation in cnc.operations where operation.setup.kind != .drill {
            XCTAssertEqual(operation.setup.toolNumber, 1,
                           "\(operation.name) should be milled with T1")
        }
        XCTAssertEqual(cnc.operations.last?.setup.kind, .contourOutside,
                       "the profile still runs last")
    }

    /// The whole job, built: one `M0`, and the tool sequence the kernel's own
    /// ordering rule produces.
    ///
    /// **[2, 1], and why.** The kernel orders by phase first (facing, inside
    /// features, the profile that frees the part), then keeps a tool's
    /// operations together *within* a phase, with a tool group running where
    /// its earliest operation asked to run. Both the drill and the Ø27.6
    /// opening are inside features, and the drill is first in the list
    /// (item 49: small holes before the profile) — so T2's group leads the
    /// inside phase and T1's follows it. The profile is a phase later but
    /// still T1, and the assembler only writes a change when the tool number
    /// actually changes, so it joins T1's block without a second stop. One
    /// change, one `M0`.
    func testTheJobStopsOnceAndRunsT2ThenT1() async throws {
        let cnc = try twoToolStator()
        await build(cnc)

        XCTAssertTrue(cnc.blockers.isEmpty, "blocked: \(cnc.blockers.map(\.text))")
        let gcode = try XCTUnwrap(cnc.jobCode, "a job that is not blocked has G-code")

        XCTAssertEqual(cnc.toolSequence, [2, 1],
                       "the drill's group leads the inside-feature phase; the profile joins T1's block")
        XCTAssertEqual(cnc.toolChangeCount, 1, "two tools, fitted once each, is one change")

        let stops = gcode.split(separator: "\n").filter { line in
            line.trimmingCharacters(in: .whitespaces).uppercased().hasPrefix("M0")
                && !line.trimmingCharacters(in: .whitespaces).uppercased().hasPrefix("M00 ")
        }
        XCTAssertEqual(stops.count, 1,
                       "one tool change is one operator stop, got \(stops.count): \(stops)")
        // …and it is an M0, never an M6: a changer-less controller ignores M6
        // silently and carries on with whatever is in the collet (item 57).
        XCTAssertFalse(gcode.uppercased().contains("M6"),
                       "this machine has no changer, so the program must not ask for one")

        XCTAssertEqual(cnc.toolChangeWarning,
                       "This job pauses 1 time for tool changes — re-zero Z after each.")
        XCTAssertEqual(cnc.toolSequenceLabel, "T2 Ø 2.5 → T1 Ø 3.175")
    }

    /// The per-tool verification the kernel sends back for a multi-tool job,
    /// decoded. Two tools, two replays, each at its own diameter.
    func testEachToolIsVerifiedAtItsOwnDiameter() async throws {
        let cnc = try twoToolStator()
        await build(cnc)

        let byTool = try XCTUnwrap(cnc.job?.verificationByTool)
        XCTAssertEqual(byTool.map(\.tool).sorted(), [1, 2],
                       "a multi-tool job is replayed once per tool")
        let drill = try XCTUnwrap(byTool.first { $0.tool == 2 })
        XCTAssertEqual(drill.diameter, 2.5, accuracy: 1e-9,
                       "T2's passes are replayed at T2's diameter, not the profile cutter's")
        let mill = try XCTUnwrap(byTool.first { $0.tool == 1 })
        XCTAssertEqual(mill.diameter, 3.175, accuracy: 1e-9)
    }

    // MARK: - mutation checks

    /// **The drill-derivation mutation check.** A drill that is not the hole's
    /// size is not the tool for the hole: a Ø2.0 drill leaves 0.25 mm a side
    /// of a Ø2.5 hole that nothing would remove, so the hole goes back to
    /// unmachinable rather than being drilled undersize and called done.
    func testADrillOfTheWrongSizeDoesNotMakeTheHole() throws {
        let cnc = CNCWorkspace()
        cnc.tools = [
            CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175),
            CNCTool(number: 2, kind: .drill, diameter: 2.0),
        ]
        cnc.stockThickness = 6
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))

        XCTAssertTrue(cnc.operations.allSatisfy { $0.setup.kind != .drill },
                      "a Ø2 drill does not make a Ø2.5 hole, so nothing is drilled")
        XCTAssertEqual(cnc.unmachinableHoles.count, 3,
                       "the three pilots are unmachinable again")
        XCTAssertTrue(cnc.unmachinableHoles.allSatisfy { abs($0.diameter - 2.5) < 0.05 })
        XCTAssertEqual(cnc.toolSequence, [1], "nothing uses T2, so nothing stops for it")
    }

    /// …and with no drill at all the app is where it was before item 19: the
    /// pilots are named as unmachinable and the job runs on one tool with no
    /// stop in it.
    func testOneToolJobHasNoToolChangeAtAll() async throws {
        let cnc = CNCWorkspace()
        cnc.tools = [CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175,
                             flutes: 2, fluteLength: 10, stickout: 20)]
        cnc.stockThickness = 6
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 6; cnc.setup.stepdown = 0.5
            cnc.setup.feed = 250; cnc.setup.plunge = 40; cnc.setup.rpm = 13500
            if cnc.setup.tabs > 0 { cnc.setup.tabHeight = 0.5 }
        }
        XCTAssertEqual(cnc.unmachinableHoles.count, 3)
        await build(cnc)

        XCTAssertEqual(cnc.toolChangeCount, 0)
        XCTAssertNil(cnc.toolChangeWarning, "a one-tool job has nothing to warn about")
        let gcode = try XCTUnwrap(cnc.jobCode)
        let stops = gcode.split(separator: "\n").filter {
            $0.trimmingCharacters(in: .whitespaces).uppercased().hasPrefix("M0")
        }
        XCTAssertTrue(stops.isEmpty, "nothing to change, so nothing to stop for: \(stops)")
    }

    // MARK: - the list itself

    /// Fitting the drill moves the holes from unmachinable to drilled without
    /// a re-import, the way changing the cutter already re-decides the bores
    /// (item 49). The derivation is the guarantee: nothing is stored.
    func testFittingADrillRePlansWithoutAReImport() throws {
        let cnc = CNCWorkspace()
        cnc.tools = [CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175)]
        cnc.stockThickness = 6
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))
        XCTAssertEqual(cnc.unmachinableHoles.count, 3)
        let opening = try XCTUnwrap(cnc.operations.first { $0.source == .opening(hole: 0) }
                                    ?? cnc.operations.first { $0.setup.kind == .contourInside })
        cnc.select(.operation(opening.id))
        cnc.setup.feed = 321      // a setting that has to survive the re-plan

        cnc.addTool(kind: .drill, diameter: 2.5)

        XCTAssertTrue(cnc.unmachinableHoles.isEmpty)
        let drill = try XCTUnwrap(cnc.operations.first { $0.setup.kind == .drill })
        XCTAssertEqual(drill.setup.toolNumber, 2)
        let survivor = try XCTUnwrap(cnc.operations.first { $0.id == opening.id })
        XCTAssertEqual(survivor.setup.feed, 321,
                       "the settings on an operation that survives the re-plan are kept")
    }

    /// Removing a tool an operation is using moves that operation to one that
    /// exists, rather than leaving it pointing at a number the kernel will
    /// refuse by name.
    func testRemovingAToolMovesItsOperationsToOneThatExists() throws {
        let cnc = try twoToolStator()
        XCTAssertTrue(cnc.operations.contains { $0.setup.toolNumber == 2 })

        XCTAssertTrue(cnc.removeTool(number: 2))
        XCTAssertEqual(cnc.tools.map(\.number), [1])
        XCTAssertTrue(cnc.operations.allSatisfy { cnc.tool(number: $0.setup.toolNumber) != nil },
                      "every operation names a tool that is in the list")
        XCTAssertFalse(cnc.removeTool(number: 1), "the last tool cannot go")
    }

    /// The tool list is part of what the job was built from, so adding a tool
    /// stales it — the same rule the stock and the cutter already follow.
    func testTheToolListIsPartOfTheJobKey() async throws {
        let cnc = try twoToolStator()
        await build(cnc)
        cnc.setupConfirmed = true
        XCTAssertTrue(cnc.jobCurrent)

        cnc.addTool(kind: .drill, diameter: 1.0)
        XCTAssertFalse(cnc.jobCurrent, "the tool list is part of the key")
        XCTAssertNil(cnc.jobCode, "a stale job has nothing to send")
    }

    /// The probe macro that rides the tool-change pause comes from the
    /// operator's own saved macro, and is absent when there is none — a macro
    /// from another machine would drive the spindle into the work.
    func testTheToolChangeProbeMacroComesFromTheMachinesOwnMacro() throws {
        let cnc = try twoToolStator()
        // Macros live in defaults and outlive a test run, so this starts from
        // a known empty bench rather than from whatever the last run left.
        let saved = cnc.macros
        defer { for macro in cnc.macros { cnc.removeMacro(macro.id) }
                for macro in saved { cnc.saveMacro(name: macro.name, command: macro.command) } }
        for macro in cnc.macros { cnc.removeMacro(macro.id) }

        XCTAssertNil(cnc.toolChangeProbeMacro, "nothing saved, so nothing is sent")
        let request = try XCTUnwrap(cnc.makeRequest())
        XCTAssertNil(request.options.toolChange.probeMacro)
        XCTAssertEqual(request.options.toolChange.type, "manual_pause_reprobe")

        cnc.saveMacro(name: "Probe Z", command: "G38.2 Z-25 F50")
        XCTAssertEqual(cnc.toolChangeProbeMacro, "G38.2 Z-25 F50")
        let withMacro = try XCTUnwrap(cnc.makeRequest())
        XCTAssertEqual(withMacro.options.toolChange.probeMacro, "G38.2 Z-25 F50")
    }

    /// Every tool in the list reaches the request, not just the one an
    /// operation happens to name: the kernel refuses an operation whose tool
    /// it cannot find.
    func testEveryToolReachesTheRequest() throws {
        let cnc = try twoToolStator()
        let request = try XCTUnwrap(cnc.makeRequest())
        XCTAssertEqual(request.tools.map(\.number).sorted(), [1, 2])
        XCTAssertEqual(request.tools.first { $0.number == 2 }?.kind, "drill")
        XCTAssertEqual(request.tools.first { $0.number == 2 }?.diameter, 2.5)
        for operation in request.operations {
            XCTAssertTrue(request.tools.contains { $0.number == operation.tool },
                          "\(operation.name) names T\(operation.tool), which is not in the list")
        }
        let drill = try XCTUnwrap(request.operations.first { $0.kind == .drill })
        XCTAssertEqual(drill.holes?.count, 3, "one operation, every hole")
        XCTAssertEqual(drill.cycle, "chip_break")
        XCTAssertNotNil(drill.peckDepth, "the kernel refuses a chip-break cycle with no depth")
        XCTAssertEqual(drill.peckDepth, 0.5,
                       "a peck depth of zero follows the roughing stepdown already set")
    }

    /// **The full-retract peck is allowed, because the oracle now knows the
    /// hole is open.**
    ///
    /// Ordinary G83 rapids back down into the hole between pecks. The 2D
    /// oracle used to refuse any rapid that descends below the stock top
    /// (item 63); it now allows one where this very program has already cut
    /// to that depth on the hole's own centre, and still refuses it anywhere
    /// else. Pinned here so a kernel that goes back to refusing every peck,
    /// or an app that quietly stops offering it, shows up as a changed test
    /// rather than as a surprise at the machine.
    func testTheFullRetractPeckIsVerifiedNotRefused() async throws {
        let cnc = try twoToolStator()
        for operation in cnc.operations where operation.setup.kind == .drill {
            cnc.select(.operation(operation.id))
            cnc.setup.drillCycle = .peck
        }
        await build(cnc)

        XCTAssertFalse(cnc.blockers.contains { $0.text.lowercased().contains("rapid") },
                       "the rapid back down the hole is the program's own hole: \(cnc.blockers.map(\.text))")
        XCTAssertEqual(cnc.policy?.verified, true, "the peck job was replayed, not waved through")
        XCTAssertNotNil(cnc.jobCode, "and it posts")
        XCTAssertTrue(try XCTUnwrap(cnc.jobCode).contains("G0 Z"), "a peck cycle retracts between pecks")
    }
}
