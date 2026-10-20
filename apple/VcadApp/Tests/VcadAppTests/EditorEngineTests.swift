import XCTest
@testable import VcadApp

/// The editor's document mutations run through the Rust engine: these drive
/// `EditorModel` the way the UI does and read back the engine's materialized
/// document.
@MainActor
final class EditorEngineTests: XCTestCase {
    private var url: URL!

    override func setUp() async throws {
        let doc: [String: Any] = [
            "version": "0.1",
            "nodes": [
                "1": ["id": 1, "name": "Block", "op": ["type": "Cube", "size": ["x": 20, "y": 10, "z": 5]]],
                "2": ["id": 2, "name": "Pin", "op": ["type": "Cylinder", "radius": 3, "height": 12, "segments": 32]],
                "3": ["id": 3, "op": ["type": "Translate", "child": 2, "offset": ["x": 40, "y": 0, "z": 0]]],
            ],
            "materials": [:], "part_materials": [:],
            "roots": [["root": 1, "material": "aluminum"], ["root": 3, "material": "steel"]],
        ]
        url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".vcad")
        try JSONSerialization.data(withJSONObject: doc).write(to: url)
    }

    override func tearDown() async throws { try? FileManager.default.removeItem(at: url) }

    private func open() throws -> EditorModel {
        let m = EditorModel()
        m.openDocument(url)
        XCTAssertTrue(m.canEditDocument, m.engineError ?? "no engine")
        XCTAssertEqual(roots(m).count, 2)
        return m
    }

    private func roots(_ m: EditorModel) -> [[String: Any]] {
        m.documentJSON?["roots"] as? [[String: Any]] ?? []
    }

    /// Op of the leaf node under part `i`'s transform chain.
    private func leafOp(_ m: EditorModel, part i: Int) -> [String: Any]? {
        guard let id = m.partId(forPart: i),
              let part = m.engineState?.parts.first(where: { $0["id"] as? String == id }) else { return nil }
        let leaf = part.first { $0.key.hasSuffix("NodeId") && !["translateNodeId", "rotateNodeId", "scaleNodeId"].contains($0.key) }
        return (leaf?.value as? NSNumber).flatMap { DocEdit.op(m.documentJSON ?? [:], nodeId: $0.intValue) }
    }

    func testMaterialUndoRedoAndDirtyTrackTheEngine() throws {
        let m = try open()
        XCTAssertFalse(m.documentDirty)
        let id = try XCTUnwrap(m.partId(forPart: 0))
        m.setPartMaterial(0, "brass")
        XCTAssertEqual(roots(m)[0]["material"] as? String, "brass")
        XCTAssertEqual(m.partId(forPart: 0), id, "part ids are stable across materializes")
        XCTAssertTrue(m.documentDirty)
        XCTAssertEqual(m.undoActionName, "Change Material")
        m.undo()
        XCTAssertEqual(roots(m)[0]["material"] as? String, "aluminum")
        XCTAssertFalse(m.documentDirty, "undoing back to the saved state clears the edited dot")
        m.redo()
        XCTAssertEqual(roots(m)[0]["material"] as? String, "brass")
    }

    func testAddDeleteAndCombineAreEngineCommands() throws {
        let m = try open()
        m.addPrimitive(.sphere)
        XCTAssertEqual(roots(m).count, 3)
        XCTAssertEqual(leafOp(m, part: 2)?["type"] as? String, "Sphere")
        m.deletePart(0)
        XCTAssertEqual(roots(m).count, 2)
        m.undo(); m.undo()
        XCTAssertEqual(roots(m).count, 2)
        XCTAssertEqual(leafOp(m, part: 0)?["type"] as? String, "Cube")

        m.multiSelectedParts = [0, 1]
        m.combineSelected(.difference)
        XCTAssertEqual(roots(m).count, 1)
        XCTAssertEqual(leafOp(m, part: 0)?["type"] as? String, "Difference")
        m.undo()
        XCTAssertEqual(roots(m).count, 2)

        m.multiSelectedParts = [0, 1]
        m.deleteSelectedParts()
        XCTAssertEqual(roots(m).count, 0)
        XCTAssertEqual(m.undoActionName, "Delete 2 Parts")
        m.undo()
        XCTAssertEqual(roots(m).count, 2, "a multi-delete is one undo step")
    }

    func testFieldScrubsMapOntoFeatureParams() throws {
        let m = try open()
        let cubeLeaf = try XCTUnwrap(m.engineState?.parts.first { $0["kind"] as? String == "cube" })
        let cubeNode = try XCTUnwrap((cubeLeaf["primitiveNodeId"] as? NSNumber)?.intValue)
        // One gesture: first event snapshots, the rest continue it.
        m.editVec(nodeId: cubeNode, key: "size", axis: "x", value: 25, snapshot: true)
        let after = try XCTUnwrap(m.engineState?.parts.first { $0["kind"] as? String == "cube" })
        let node2 = try XCTUnwrap((after["primitiveNodeId"] as? NSNumber)?.intValue)
        m.editVec(nodeId: node2, key: "size", axis: "x", value: 30, snapshot: false)
        let size = leafOp(m, part: 0)?["size"] as? [String: Any]
        XCTAssertEqual((size?["x"] as? NSNumber)?.doubleValue, 30)
        XCTAssertEqual((size?["y"] as? NSNumber)?.doubleValue, 10)
        XCTAssertEqual(m.undoStack.count, 1)
        m.undo()
        XCTAssertEqual(((leafOp(m, part: 0)?["size"] as? [String: Any])?["x"] as? NSNumber)?.doubleValue, 20)

        let pin = try XCTUnwrap(m.engineState?.parts.first { $0["kind"] as? String == "cylinder" })
        let pinNode = try XCTUnwrap((pin["primitiveNodeId"] as? NSNumber)?.intValue)
        m.editScalar(nodeId: pinNode, key: "radius", value: 4.5, snapshot: true)
        XCTAssertEqual((leafOp(m, part: 1)?["radius"] as? NSNumber)?.doubleValue, 4.5)
        // The pin keeps its placement through the edit.
        let pin2 = try XCTUnwrap(m.engineState?.parts.first { $0["kind"] as? String == "cylinder" })
        let tn = try XCTUnwrap((pin2["translateNodeId"] as? NSNumber)?.intValue)
        let offset = DocEdit.op(m.documentJSON ?? [:], nodeId: tn)?["offset"] as? [String: Any]
        XCTAssertEqual((offset?["x"] as? NSNumber)?.doubleValue, 40)
    }

    func testRenameAndUnsupportedFieldFallBackThroughTheEngine() throws {
        let m = try open()
        let cube = try XCTUnwrap(m.engineState?.parts.first { $0["kind"] as? String == "cube" })
        let node = try XCTUnwrap((cube["primitiveNodeId"] as? NSNumber)?.intValue)
        m.renameFeature(node, to: "Base Plate")
        let renamed = try XCTUnwrap(m.engineState?.parts.first { $0["kind"] as? String == "cube" })
        XCTAssertEqual(renamed["name"] as? String, "Base Plate")
        m.undo()
        XCTAssertNotEqual(m.engineState?.parts.first { $0["kind"] as? String == "cube" }?["name"] as? String, "Base Plate")
    }

    func testSaveWritesTheEngineDocumentAndClearsDirty() throws {
        let m = try open()
        m.addPrimitive(.cube)
        XCTAssertTrue(m.documentDirty)
        m.saveDocument()
        XCTAssertFalse(m.documentDirty)
        let reopened = EditorModel()
        reopened.openDocument(url)
        XCTAssertTrue(reopened.canEditDocument)
        XCTAssertEqual(roots(reopened).count, 3)
    }

    func testRefusedCommandLeavesDocumentAndHistoryAlone() throws {
        let m = try open()
        let before = m.engine?.stateHash
        m.multiSelectedParts = [0, 0]
        m.combineSelected(.union)
        m.setPartMaterial(9, "steel")
        XCTAssertEqual(m.engine?.stateHash, before)
        XCTAssertTrue(m.undoStack.isEmpty)
    }

    func testElectronicsDocumentsOpenViewOnly() throws {
        var doc = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
        doc["schematic"] = ["components": [], "wires": []]
        try JSONSerialization.data(withJSONObject: doc).write(to: url)
        let m = EditorModel()
        m.openDocument(url)
        XCTAssertFalse(m.canEditDocument)
        XCTAssertNotNil(m.engineError)
        XCTAssertEqual(roots(m).count, 2, "the file is shown as authored")
        m.addPrimitive(.cube)
        XCTAssertEqual(roots(m).count, 2)
    }
}
