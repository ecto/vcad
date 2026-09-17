import XCTest
import RealityKit
@testable import VcadApp

@MainActor
final class CNCStudioTests: XCTestCase {
    private func generate(_ cnc: CNCWorkspace, all: Bool = true) async {
        cnc.build()
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertFalse(cnc.generating)
        XCTAssertNil(cnc.error)
    }
    /// The job is one program with one spindle start, whatever the list holds:
    /// the old pipeline posted one program per operation and stitched them.
    func testOneJobIsOneProgramWithOneSpindleStart() async throws {
        let cnc = CNCWorkspace()
        let faceID = cnc.selectedOperation.id
        cnc.addOperation(.pocket)
        let pocketID = cnc.selectedOperation.id
        XCTAssertNil(cnc.jobCode)
        await generate(cnc)
        let code = try XCTUnwrap(cnc.jobCode)
        let lines = code.components(separatedBy: .newlines).map { $0.components(separatedBy: "(")[0] }
        XCTAssertEqual(lines.filter { $0.split(separator: " ").contains { $0.uppercased() == "M3" } }.count, 1,
                       "one tool means one spindle start")
        XCTAssertEqual(lines.filter { $0.split(separator: " ").contains { $0.uppercased() == "M30" || $0.uppercased() == "M2" } }.count, 1)
        cnc.moveSelectedOperation(by: -1)
        XCTAssertEqual(cnc.operations.first?.id, pocketID)
        XCTAssertFalse(cnc.jobCurrent, "reordering is a change to the job")
        cnc.select(.operation(faceID))
        XCTAssertEqual(cnc.setup.kind, .face)
        await generate(cnc)
        cnc.setupConfirmed = true
        cnc.toolDiameter = 4
        XCTAssertFalse(cnc.jobCurrent)
        XCTAssertFalse(cnc.setupConfirmed)
        XCTAssertNil(cnc.jobCode, "a stale job has nothing to send")
        cnc.removeSelectedOperation()
        XCTAssertEqual(cnc.operations.count, 1)
        cnc.removeSelectedOperation()
        XCTAssertEqual(cnc.operations.count, 1)
    }
    func testPreviewUsesFeedsAndInterpolatesGeneratedMotion() {
        let program = CNCProgram(gcode: "", moves: [
            CNCMove(to: [0, 0, 0], rapid: true),
            CNCMove(to: [10, 0, 0], rapid: false, feed: 60),
            CNCMove(to: [10, 10, 0], rapid: true)
        ])
        let preview = CNCPreview(program: program, fallbackFeed: 999)
        XCTAssertEqual(preview.duration, 10.2, accuracy: 0.001)
        XCTAssertEqual(preview.position(at: 5 / 10.2)!.x, 5, accuracy: 0.001)
        XCTAssertEqual(preview.position(at: 1), SIMD3<Float>(10, 10, 0))
        XCTAssertEqual(preview.position(at: -1), SIMD3<Float>.zero)
        XCTAssertEqual(preview.position(at: .nan), SIMD3<Float>.zero)
    }
    func testPreviewAndReportedPositionStayIndependent() async throws {
        let cnc = CNCWorkspace()
        await generate(cnc)
        cnc.shown = true
        cnc.select(.operation(cnc.selectedOperation.id))
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect(); cnc.pausePreview() }
        cnc.previewFraction = 0.5
        XCTAssertEqual(cnc.machine.status.work, CNCVector())
        let parent = Entity()
        syncCNCOverlay(cnc, in: parent)
        let simulated = try XCTUnwrap(parent.findEntity(named: "cncPreviewTool"))
        let reported = try XCTUnwrap(parent.findEntity(named: "cncTool"))
        XCTAssertTrue(simulated.isEnabled)
        XCTAssertTrue(reported.isEnabled)
        XCTAssertNotEqual(simulated.position, reported.position)
        cnc.togglePreview()
        XCTAssertTrue(cnc.previewPlaying)
        cnc.mode = .machine
        XCTAssertFalse(cnc.previewPlaying)
        syncCNCOverlay(cnc, in: parent)
        XCTAssertFalse(parent.findEntity(named: "cncPreviewTool")!.isEnabled)
        XCTAssertTrue(parent.findEntity(named: "cncTool")!.isEnabled)
    }
    func testRunRequiresCurrentJobAndBlocksEditsDuringStreaming() async {
        let cnc = CNCWorkspace()
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.startJob()
        XCTAssertFalse(cnc.machine.active)
        await generate(cnc)
        cnc.startJob()
        XCTAssertFalse(cnc.machine.active)
        // No outline, and a tool whose stickout nobody declared: both are
        // warnings the job will not run past unacknowledged.
        for warning in cnc.warnings { cnc.acknowledge(warning.id, on: true) }
        cnc.setupConfirmed = true
        XCTAssertNil(cnc.runBlocker)
        cnc.startJob()
        XCTAssertTrue(cnc.machine.active)
        let setup = cnc.setup
        cnc.setup.depth = 9
        cnc.addOperation(.pocket)
        XCTAssertEqual(cnc.setup, setup)
        XCTAssertEqual(cnc.operations.count, 1)
    }
    func testStockEditsInvalidateTheWholeJob() async {
        let cnc = CNCWorkspace()
        cnc.addOperation(.pocket)
        await generate(cnc)
        XCTAssertTrue(cnc.jobCurrent)
        cnc.stockWidth = 55
        XCTAssertFalse(cnc.jobCurrent, "the stock is part of what the job was built from")
        XCTAssertNil(cnc.jobCode)
    }
    func testImportedJobUsesManufacturePreviewTransportAndRunGates() async throws {
        let cnc = CNCWorkspace()
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        await generate(cnc)
        let generatedCode = cnc.jobCode
        try cnc.importProgram("G21 G90\nG0 X1 Y2 Z3\nG1 X10 F100\nM2", name: "part.nc")
        XCTAssertTrue(cnc.usesImportedProgram)
        XCTAssertEqual(cnc.mode, .toolpaths)
        XCTAssertEqual(cnc.inspectorTab, .gcode)
        XCTAssertEqual(cnc.previewTitle, "part.nc")
        XCTAssertEqual(cnc.program?.gcode, cnc.jobCode)
        XCTAssertGreaterThan(cnc.preview.duration, 0)
        XCTAssertFalse(cnc.setupConfirmed)
        XCTAssertNotNil(cnc.runBlocker)
        let parent = Entity()
        syncCNCOverlay(cnc, in: parent)
        XCTAssertNotNil(parent.findEntity(named: "cncPath-0-false"))
        for warning in cnc.warnings { cnc.acknowledge(warning.id, on: true) }
        cnc.setupConfirmed = true
        XCTAssertNil(cnc.runBlocker)
        cnc.startJob()
        XCTAssertTrue(cnc.machine.active)
        cnc.useGeneratedJob()
        XCTAssertTrue(cnc.usesImportedProgram, "Cannot switch the viewed job during streaming")
        cnc.machine.disconnect()
        cnc.useGeneratedJob()
        XCTAssertFalse(cnc.usesImportedProgram)
        XCTAssertEqual(cnc.jobCode, generatedCode)
        XCTAssertFalse(cnc.setupConfirmed)
    }
    func testInvalidImportPreservesSelectedJob() async throws {
        let cnc = CNCWorkspace()
        await generate(cnc)
        let code = cnc.jobCode
        XCTAssertThrowsError(try cnc.importProgram("G53 G0 X1", name: "bad.nc"))
        XCTAssertFalse(cnc.usesImportedProgram)
        XCTAssertEqual(cnc.jobCode, code)
        try cnc.importProgram("G0 X1\nM2", name: "part.nc")
        cnc.select(.tool)
        XCTAssertFalse(cnc.usesImportedProgram)
        XCTAssertTrue(cnc.rightPanelShown)
        XCTAssertNotNil(cnc.importedProgram)
        cnc.useImportedJob()
        XCTAssertTrue(cnc.usesImportedProgram)
    }
}
