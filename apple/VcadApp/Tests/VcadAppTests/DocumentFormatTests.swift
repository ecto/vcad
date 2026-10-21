import XCTest
@testable import VcadApp

/// Open accepts every `.vcad` format, not just JSON: the compact VCode
/// example (`examples/sensor-mast.vcad`) used to come back unparseable.
@MainActor
final class DocumentFormatTests: XCTestCase {
    private var examples: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("examples")
    }

    func testVCodeDocumentOpensAsIRJSON() throws {
        let path = examples.appendingPathComponent("sensor-mast.vcad").path
        let data = try XCTUnwrap(EditorModel.documentBytes(path: path))
        let dict = try XCTUnwrap((try JSONSerialization.jsonObject(with: data)) as? [String: Any])
        XCTAssertFalse((dict["nodes"] as? [String: Any] ?? [:]).isEmpty)
        XCTAssertNotNil(EditorModel.openInEngine(dict), "opens editable in the engine")
    }

    func testJSONDocumentPassesThroughUnchanged() throws {
        let url = examples.appendingPathComponent("plate.vcad")
        XCTAssertEqual(EditorModel.documentBytes(path: url.path), try Data(contentsOf: url))
    }
}
