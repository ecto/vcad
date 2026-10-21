import XCTest
@testable import VcadApp

/// v1 ships Design + Agent; Electronics, Manufacture and simulation sit
/// behind Settings ▸ Labs (off by default).
@MainActor
final class LabsTests: XCTestCase {
    private var saved: Any?

    override func setUp() async throws { saved = UserDefaults.standard.object(forKey: Prefs.labsKey) }
    override func tearDown() async throws {
        if let saved { UserDefaults.standard.set(saved, forKey: Prefs.labsKey) }
        else { UserDefaults.standard.removeObject(forKey: Prefs.labsKey) }
    }

    func testLabsOffByDefaultOffersOnlyDesign() {
        UserDefaults.standard.removeObject(forKey: Prefs.labsKey)
        XCTAssertFalse(Prefs.labs)
        XCTAssertEqual(Workspace.available, [.design])
    }

    func testLabsOnOffersEveryWorkspace() {
        UserDefaults.standard.set(true, forKey: Prefs.labsKey)
        XCTAssertEqual(Workspace.available, Workspace.allCases)
    }

    func testDXFOpenIsRefusedWithoutLabs() throws {
        UserDefaults.standard.set(false, forKey: Prefs.labsKey)
        let url = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString + ".dxf")
        try "0\nEOF\n".write(to: url, atomically: true, encoding: .utf8)
        defer { try? FileManager.default.removeItem(at: url) }
        let m = EditorModel()
        m.openDocument(url)
        XCTAssertEqual(m.workspace, .design)
        XCTAssertTrue(m.loadError?.contains("Labs") == true, m.loadError ?? "nil")
    }

    func testSimulationUIIsGated() {
        UserDefaults.standard.set(false, forKey: Prefs.labsKey)
        let m = EditorModel()
        XCTAssertFalse(m.showsSimulation)
    }
}
