import XCTest
@testable import VcadApp

@MainActor
final class AsyncEvaluationTests: XCTestCase {
    /// With background evaluation on, the first build reports "solving" and
    /// keeps the main thread free; the real scene arrives on the next build.
    func testDocumentEvaluatesOffMainThreadThenLands() async throws {
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("pattern-test.vcad").path
        guard FileManager.default.fileExists(atPath: path) else { throw XCTSkip("no sample document") }
        let model = EditorModel()
        model.evaluatesOffMainThread = true
        model.openDocument(URL(fileURLWithPath: path))
        model.geometryDirty = false
        let first = model.buildScene()
        XCTAssertTrue(first.solving)
        XCTAssertTrue(model.solving)
        let deadline = Date().addingTimeInterval(30)
        while model.solving && Date() < deadline { try? await Task.sleep(for: .milliseconds(10)) }
        XCTAssertFalse(model.solving)
        XCTAssertTrue(model.geometryDirty, "the viewport must be told to rebuild")
        let second = model.buildScene()
        XCTAssertFalse(second.solving)
        XCTAssertGreaterThan(second.partCount, 0)
        XCTAssertGreaterThan(model.triangleCount, 0)
        // Nothing left waiting: a third build evaluates again (synchronously
        // prepared scenes are consumed once).
        XCTAssertTrue(model.buildScene().solving)
    }

    /// The headless path is unchanged: synchronous, complete on the first call.
    func testSynchronousPathStillBuildsInOneCall() throws {
        let path = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("pattern-test.vcad").path
        guard FileManager.default.fileExists(atPath: path) else { throw XCTSkip("no sample document") }
        let model = EditorModel()
        model.openDocument(URL(fileURLWithPath: path))
        let scene = model.buildScene()
        XCTAssertFalse(scene.solving)
        XCTAssertGreaterThan(scene.partCount, 0)
    }
}
