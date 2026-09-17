import AppKit
import XCTest
@testable import VcadApp

@MainActor
final class WorkspaceSwitchTests: XCTestCase {
    /// Switching workspaces must never resize the window.
    func testWorkspaceSwitchKeepsWindowFrame() async throws {
        _ = NSApplication.shared
        let model = EditorModel()
        let controller = ReleaseWindowController.shared
        controller.show(model: model, intent: IntentEngine())
        defer { controller.hide() }
        controller.setWindowed(true)
        let window = try XCTUnwrap(NSApp.windows.compactMap { $0 as? KeyableWindow }.first)
        try? await Task.sleep(for: .milliseconds(300))
        let before = window.frame
        for ws in [Workspace.manufacture, .electronics, .design] {
            model.workspace = ws
            try? await Task.sleep(for: .milliseconds(400))
            XCTAssertEqual(window.frame.size, before.size, "window resized switching to \(ws)")
        }
    }
}
