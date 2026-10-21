import XCTest
@testable import VcadApp

/// The loon-first loop: compile errors with locations, `[use]` resolution,
/// last-good-model retention, folder opening.
@MainActor
final class LoonLoopTests: XCTestCase {
    private let fixtures = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Fixtures/loon")

    /// A scratch copy of the fixture project (tests rewrite files in it).
    private func project() throws -> URL {
        let dir = FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-loon-loop-\(UUID().uuidString)")
        try FileManager.default.copyItem(at: fixtures, to: dir)
        addTeardownBlock { try? FileManager.default.removeItem(at: dir) }
        return dir
    }

    func testGoodProgramCompilesWithModuleBesideIt() throws {
        let data = try LoonCompiler.compile(path: fixtures.appendingPathComponent("main.loon").path).get()
        let json = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual((json["roots"] as? [Any])?.count, 2)
    }

    func testBrokenProgramReportsFileLineAndColumn() {
        let path = fixtures.appendingPathComponent("broken.loon").path
        guard case let .failure(d) = LoonCompiler.compile(path: path) else { return XCTFail("should not compile") }
        XCTAssertTrue(d.message.contains("no-such-offset"), d.message)
        XCTAssertEqual(d.file, path)
        XCTAssertEqual(d.line, 4)
        XCTAssertNotNil(d.column)
        XCTAssertEqual(d.location, "broken.loon:4:\(d.column!)")
    }

    func testModuleResolvesThroughLoonPath() throws {
        let dir = try project()
        let lib = dir.appendingPathComponent("lib")
        try FileManager.default.createDirectory(at: lib, withIntermediateDirectories: true)
        try FileManager.default.moveItem(at: dir.appendingPathComponent("bracket.loon"),
                                         to: lib.appendingPathComponent("bracket.loon"))
        let main = dir.appendingPathComponent("main.loon").path
        guard case .failure = LoonCompiler.compile(path: main) else { return XCTFail("module moved away") }
        setenv("VCAD_LOON_PATH", lib.path, 1)
        defer { unsetenv("VCAD_LOON_PATH") }
        XCTAssertNoThrow(try LoonCompiler.compile(path: main).get())
        XCTAssertEqual(LoonCompiler.watchedDirectories(for: main, environment: ["VCAD_LOON_PATH": lib.path]),
                       [dir.path, lib.path])
    }

    func testFolderOpensItsEntryProgram() throws {
        let dir = try project()
        XCTAssertEqual(LoonCompiler.entryFile(inFolder: dir)?.lastPathComponent, "main.loon")
        let model = EditorModel()
        model.openDocument(dir)
        guard case let .document(path, _) = model.source else { return XCTFail("folder did not open") }
        XCTAssertEqual(URL(fileURLWithPath: path).lastPathComponent, "main.loon")
        XCTAssertTrue(model.usesDocumentTree)
        XCTAssertNil(model.loonDiagnostic)
    }

    func testBrokenEditKeepsLastGoodModelThenRecovers() throws {
        let dir = try project()
        let main = dir.appendingPathComponent("main.loon")
        let good = try String(contentsOf: main, encoding: .utf8)
        let model = EditorModel()
        model.openDocument(main)
        XCTAssertTrue(model.usesDocumentTree)
        let partsBefore = model.featureNodes.count
        XCTAssertGreaterThan(partsBefore, 0)

        // Save a broken edit: the diagnostic appears, the model stays.
        try String(contentsOf: dir.appendingPathComponent("broken.loon"), encoding: .utf8)
            .write(to: main, atomically: true, encoding: .utf8)
        model.rebuildLoon()
        let d = try XCTUnwrap(model.loonDiagnostic)
        XCTAssertEqual(d.line, 4)
        XCTAssertEqual(d.file, main.path)
        XCTAssertTrue(model.usesDocumentTree, "the last good model stays on screen")
        XCTAssertEqual(model.featureNodes.count, partsBefore)

        // Fix it: the diagnostic clears and the model rebuilds.
        try (good + "[root [cube 5.0 5.0 5.0] \"steel\"]\n").write(to: main, atomically: true, encoding: .utf8)
        model.rebuildLoon()
        XCTAssertNil(model.loonDiagnostic)
        XCTAssertEqual(model.featureNodes.count, partsBefore + 1)
    }

    func testBrokenProgramStillOpensWithDiagnostic() throws {
        let dir = try project()
        let model = EditorModel()
        model.openDocument(dir.appendingPathComponent("broken.loon"))
        guard case .document = model.source else { return XCTFail("a broken program should still open") }
        XCTAssertEqual(model.loonDiagnostic?.line, 4)
        XCTAssertFalse(model.usesDocumentTree)
    }
}
