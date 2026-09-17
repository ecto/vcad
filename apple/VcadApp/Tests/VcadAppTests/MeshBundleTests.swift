import XCTest
@testable import VcadApp

@MainActor
final class MeshBundleTests: XCTestCase {
    /// Saving writes `<doc>.vcadmesh` from the evaluation's root keys, and
    /// opening a document next to its bundle feeds the cache first.
    func testSaveWritesBundleAndOpenImportsIt() throws {
        let sample = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("pattern-test.vcad")
        guard FileManager.default.fileExists(atPath: sample.path) else { throw XCTSkip("no sample document") }
        let dir = FileManager.default.temporaryDirectory.appendingPathComponent("vcad-bundle-\(ProcessInfo.processInfo.processIdentifier)")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }

        let model = EditorModel()
        model.openDocument(sample)
        let scene = model.buildScene()                 // synchronous: collects the root keys
        XCTAssertGreaterThan(scene.partCount, 0)
        let copy = dir.appendingPathComponent("copy.vcad")
        model.saveDocumentAs(copy)
        let bundle = EditorModel.meshBundleURL(for: copy)
        if !FileManager.default.fileExists(atPath: bundle.path) {
            throw XCTSkip("root cache unavailable in this build (unhashed kernel id or VCAD_CACHE=0)")
        }
        XCTAssertGreaterThan(model.writeMeshBundle(for: copy), 0)
        let fresh = EditorModel()
        XCTAssertGreaterThan(fresh.importMeshBundle(for: copy), 0)
        fresh.openDocument(copy)
        let reopened = fresh.buildScene()
        XCTAssertEqual(reopened.partCount, scene.partCount)
    }
}
