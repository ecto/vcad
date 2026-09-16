import XCTest
import RealityKit
@testable import VcadApp

@MainActor
final class CNCStudioTests: XCTestCase {
    private func generate(_ cnc: CNCWorkspace, all: Bool = true) async {
        cnc.generate(all: all)
        let deadline = Date().addingTimeInterval(8)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertFalse(cnc.generating)
        XCTAssertNil(cnc.error)
    }
    func testJobGenerationOrderAndSingleProgramEnd() async throws {
        let cnc = CNCWorkspace()
        let faceID = cnc.selectedOperation.id
        cnc.addOperation("pocket")
        let pocketID = cnc.selectedOperation.id
        XCTAssertNil(cnc.jobCode)
        await generate(cnc)
        let code = try XCTUnwrap(cnc.jobCode)
        XCTAssertEqual(code.components(separatedBy: .newlines).filter { $0 == "M2" }.count, 1)
        XCTAssertEqual(code.components(separatedBy: .newlines).filter { $0.hasPrefix("M3 ") }.count, 2)
        XCTAssertTrue(code.hasPrefix("(vcad single-tool face"))
        cnc.moveSelectedOperation(by: -1)
        XCTAssertEqual(cnc.operations.first?.id, pocketID)
        XCTAssertTrue(cnc.jobCode!.hasPrefix("(vcad single-tool pocket"))
        cnc.select(.operation(faceID))
        XCTAssertEqual(cnc.setup.operation, "face")
        cnc.setupConfirmed = true
        cnc.toolDiameter = 4
        XCTAssertTrue(cnc.operations.allSatisfy { $0.setup.diameter == 4 })
        XCTAssertFalse(cnc.jobCurrent)
        XCTAssertFalse(cnc.setupConfirmed)
        XCTAssertNil(cnc.jobCode)
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
        cnc.setupConfirmed = true
        XCTAssertNil(cnc.runBlocker)
        cnc.startJob()
        XCTAssertTrue(cnc.machine.active)
        let setup = cnc.setup
        cnc.setup.depth = 9
        cnc.addOperation("pocket")
        XCTAssertEqual(cnc.setup, setup)
        XCTAssertEqual(cnc.operations.count, 1)
    }
    func testStockEditsInvalidateEveryOperation() async {
        let cnc = CNCWorkspace()
        cnc.addOperation("profile")
        await generate(cnc)
        XCTAssertTrue(cnc.jobCurrent)
        cnc.stockWidth = 55
        XCTAssertTrue(cnc.operations.allSatisfy { $0.setup.width == 55 })
        XCTAssertFalse(cnc.operations.contains(where: \.current))
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
        XCTAssertEqual(cnc.inspectorTab, .inspector)
        XCTAssertNotNil(cnc.importedProgram)
        cnc.useImportedJob()
        XCTAssertTrue(cnc.usesImportedProgram)
    }
}
