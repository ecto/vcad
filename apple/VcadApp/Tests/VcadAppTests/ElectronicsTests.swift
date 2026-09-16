import XCTest
import CVcadFFI
@testable import VcadApp

@MainActor
final class ElectronicsTests: XCTestCase {
    func testPlacementAndConnectivityUndoAreAtomic() throws {
        let model = EditorModel(), ec = model.electronics
        ec.createBoard(model)
        XCTAssertEqual(ec.boards(model).count, 1)
        ec.place(model, at: CGPoint(x: 10, y: 10))
        XCTAssertEqual(ecRows(ec.board(model)["footprints"]).count, 1)
        XCTAssertEqual(ecRows(ec.schematic(model)["components"]).count, 1)
        ec.editComponent(model, key: "value", value: "22k")
        XCTAssertEqual(ecRows(ec.board(model)["footprints"])[0]["value"] as? String, "22k")
        XCTAssertEqual(ecRows(ec.schematic(model)["components"])[0]["value"] as? String, "22k")
        model.undo()
        XCTAssertEqual(ecRows(ec.board(model)["footprints"])[0]["value"] as? String, "10k")
        model.undo()
        XCTAssertEqual(ecRows(ec.board(model)["footprints"]).count, 0)
        XCTAssertEqual(ecRows(ec.schematic(model)["components"]).count, 0)
        model.redo()
        ec.place(model, at: CGPoint(x: 30, y: 10))
        ec.connect(model, pin: "R1.2"); ec.connect(model, pin: "R2.1")
        let nets = try XCTUnwrap(ec.schematic(model)["nets"] as? [String: [String]])
        XCTAssertEqual(nets["N1"], ["R1.2", "R2.1"])
        let fp = ecRows(ec.board(model)["footprints"])
        XCTAssertEqual(ecRows(fp[0]["pads"])[1]["net"] as? String, "N1")
        XCTAssertEqual(ecRows(fp[1]["pads"])[0]["net"] as? String, "N1")
        model.undo()
        XCTAssertTrue((ec.schematic(model)["nets"] as? [String: [String]] ?? [:]).isEmpty)
        XCTAssertNil(ecRows(ecRows(ec.board(model)["footprints"])[0]["pads"])[1]["net"])
    }
    func testBoardDocumentRoundTripAndKernelSchema() throws {
        let model = EditorModel(), ec = model.electronics
        ec.createBoard(model); ec.place(model, at: CGPoint(x: 20, y: 20))
        let data = try JSONSerialization.data(withJSONObject: XCTUnwrap(model.documentJSON), options: [.sortedKeys])
        let scene = data.withUnsafeBytes { vcad_scene_from_json($0.bindMemory(to: UInt8.self).baseAddress, data.count) }
        var count = 0
        let detail = vcad_last_error(&count).map { String(decoding: UnsafeBufferPointer(start: $0, count: count), as: UTF8.self) } ?? "No kernel detail"
        XCTAssertNotNil(scene, "Native board must use the actual vcad IR schema: \(detail)")
        if let scene { vcad_scene_free(scene) }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".vcad")
        try data.write(to: url); defer { try? FileManager.default.removeItem(at: url) }
        let reopened = EditorModel(); reopened.openDocument(url)
        XCTAssertEqual(ecRows(reopened.electronics.board(reopened)["footprints"]).count, 1)
        XCTAssertEqual(ecRows(reopened.electronics.schematic(reopened)["components"]).count, 1)
    }
    func testPadRoutingKeepsExactOffGridPosition() {
        let model = EditorModel(), ec = model.electronics
        ec.createBoard(model)
        ec.mutateBoard(model) { $0["nets"] = [["id": "N1", "name": "N1"]] }
        ec.activeNet = "N1"; ec.grid = 1
        let start = CGPoint(x: 1.27, y: 2.54), end = CGPoint(x: 5.08, y: 2.54)
        ec.addTrace(model, to: start, snapToGrid: false)
        ec.addTrace(model, to: end, snapToGrid: false)
        let trace = ecRows(ec.board(model)["traces"])[0]
        XCTAssertEqual(ecPoint(trace["start"]), start)
        XCTAssertEqual(ecPoint(trace["end"]), end)
    }
    func testRoutingRequiresKnownNetAndPreservesUnknownFields() {
        let model = EditorModel(), ec = model.electronics
        ec.createBoard(model)
        ec.mutateBoard(model) { $0["futureExtension"] = ["keep": true] }
        ec.addTrace(model, to: CGPoint(x: 1, y: 1))
        XCTAssertNil(ec.routeStart)
        ec.place(model, at: CGPoint(x: 10, y: 10)); ec.place(model, at: CGPoint(x: 20, y: 10))
        ec.connect(model, pin: "R1.2"); ec.connect(model, pin: "R2.1")
        ec.addTrace(model, to: CGPoint(x: 12, y: 10)); ec.addTrace(model, to: CGPoint(x: 18, y: 10))
        XCTAssertEqual(ecRows(ec.board(model)["traces"]).count, 1)
        ec.addVia(model, at: CGPoint(x: 18, y: 10))
        XCTAssertEqual(ecRows(ec.board(model)["vias"]).count, 1)
        XCTAssertEqual((ec.board(model)["futureExtension"] as? [String: Bool])?["keep"], true)
        ec.selectedTrace = 0; ec.removeSelection(model)
        XCTAssertTrue(ecRows(ec.board(model)["traces"]).isEmpty)
        model.undo(); XCTAssertEqual(ecRows(ec.board(model)["traces"]).count, 1)
    }
}
