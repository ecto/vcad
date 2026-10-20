import XCTest
@testable import VcadApp

final class DocumentEngineTests: XCTestCase {
    func testCommandsRoundTripThroughRust() throws {
        let engine = DocumentEngine(replicaId: 1)
        let created = try engine.apply([
            "type": "add_feature", "name": "Plate",
            "feature": ["type": "Cube", "size_x": 40, "size_y": 20, "size_z": 5],
        ])
        let id = try XCTUnwrap(created.createdFeatureId)
        XCTAssertEqual(created.partCount, 1)
        try engine.apply(["type": "set_translation", "id": id, "offset": [10, 0, 0]])
        try engine.applyTool("create", ["type": "fillet", "parent_part_id": id, "params": ["radius": 1]])
        try engine.apply(["type": "undo"])
    }

    func testRefusalsCarryTheEngineReason() {
        let engine = DocumentEngine(replicaId: 1)
        XCTAssertThrowsError(try engine.apply(["type": "delete_feature", "id": "nope"])) { error in
            XCTAssertEqual((error as? DocumentEngine.Refusal)?.kind, "unknown_id")
        }
        XCTAssertThrowsError(try engine.applyTool("rotate", [:])) { error in
            XCTAssertEqual((error as? DocumentEngine.Refusal)?.kind, "unsupported")
        }
    }
}
