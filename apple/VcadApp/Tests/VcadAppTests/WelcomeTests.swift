import XCTest
@testable import VcadApp

/// The first-run samples: every one is listed, has a thumbnail, and opens
/// through the engine with geometry.
@MainActor
final class WelcomeTests: XCTestCase {
    private let samplesDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("Samples")

    func testManifestListsTutorialAndSamples() throws {
        let samples = SampleLibrary.load(from: samplesDir)
        XCTAssertEqual(samples.count, 5, "every listed sample's file must exist")
        XCTAssertEqual(samples.filter { $0.tutorial == true }.map(\.file), ["first-part.loon"])
        for s in samples {
            XCTAssertNotNil(NSImage(contentsOf: samplesDir.appendingPathComponent(s.thumbnail)), s.thumbnail)
        }
    }

    func testDirectoryFoundThroughEnvironment() {
        XCTAssertEqual(SampleLibrary.directory(environment: ["VCAD_SAMPLES": samplesDir.path])?.standardizedFileURL,
                       samplesDir.standardizedFileURL)
    }

    func testEverySampleOpensWithGeometry() throws {
        let workspace = FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-samples-\(UUID().uuidString)")
        addTeardownBlock { try? FileManager.default.removeItem(at: workspace) }
        for s in SampleLibrary.load(from: samplesDir) {
            let url = try SampleLibrary.materialize(s, from: samplesDir, into: workspace)
            let attrs = try FileManager.default.attributesOfItem(atPath: url.path)
            XCTAssertEqual(attrs[.type] as? FileAttributeType, .typeRegular, "\(s.file) copied as a file, not a link")
            let model = EditorModel()
            model.openDocument(url)
            XCTAssertNil(model.loonDiagnosticOrNil, s.file)
            let scene = model.buildScene()
            XCTAssertGreaterThan(scene.partCount, 0, "\(s.file) evaluates to parts")
            XCTAssertGreaterThan(model.triangleCount, 0, "\(s.file) has triangles")
        }
    }

    func testMaterializeNeverOverwritesAnEditedCopy() throws {
        let workspace = FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-samples-\(UUID().uuidString)")
        addTeardownBlock { try? FileManager.default.removeItem(at: workspace) }
        let tutorial = try XCTUnwrap(SampleLibrary.load(from: samplesDir).first { $0.tutorial == true })
        let url = try SampleLibrary.materialize(tutorial, from: samplesDir, into: workspace)
        try "; edited\n".write(to: url, atomically: true, encoding: .utf8)
        _ = try SampleLibrary.materialize(tutorial, from: samplesDir, into: workspace)
        XCTAssertEqual(try String(contentsOf: url, encoding: .utf8), "; edited\n")
    }

    func testWelcomeShowsOnceAndNotUnderDevHooks() throws {
        let defaults = try XCTUnwrap(UserDefaults(suiteName: "vcad-welcome-test-\(UUID().uuidString)"))
        let model = EditorModel()
        XCTAssertTrue(WelcomeWindowController.shouldShowAtLaunch(model: model, defaults: defaults, environment: [:]))
        XCTAssertFalse(WelcomeWindowController.shouldShowAtLaunch(model: model, defaults: defaults,
                                                                  environment: ["VCAD_OPEN": "/x.vcad"]))
        XCTAssertFalse(WelcomeWindowController.shouldShowAtLaunch(model: model, defaults: defaults,
                                                                  environment: ["VCAD_NO_WELCOME": "1"]))
        defaults.set(true, forKey: WelcomeWindowController.shownKey)
        XCTAssertFalse(WelcomeWindowController.shouldShowAtLaunch(model: model, defaults: defaults, environment: [:]))
    }
}

private extension EditorModel {
    /// `loadError` on this branch (the loon diagnostic lands separately).
    var loonDiagnosticOrNil: String? { loadError }
}
