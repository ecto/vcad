import AppKit
import XCTest
@testable import VcadApp

@MainActor
final class WindowPresentationTests: XCTestCase {
    func testPresentationSwitchKeepsWindowContentCameraAndCNCSession() throws {
        _ = NSApplication.shared
        let model = EditorModel()
        let intent = IntentEngine()
        model.azimuth = 0.7
        model.elevation = 0.4
        model.distance = 1.7
        model.cnc.machine.connect(simulated: true)
        let controller = ReleaseWindowController.shared
        controller.show(model: model, intent: intent)
        defer {
            model.cnc.machine.disconnect()
            controller.hide()
        }
        let window = try XCTUnwrap(NSApp.windows.compactMap { $0 as? KeyableWindow }.first)
        let content = try XCTUnwrap(window.contentView)
        controller.setWindowed(true)
        XCTAssertTrue(model.isWindowed)
        XCTAssertEqual(window.title, model.source.label)
        XCTAssertNil(window.representedURL)
        model.documentDirty = true
        controller.updateDocumentWindow()
        XCTAssertTrue(window.isDocumentEdited)
        model.documentDirty = false
        controller.updateDocumentWindow()
        XCTAssertFalse(window.isDocumentEdited)
        XCTAssertTrue(window.styleMask.contains(.titled))
        XCTAssertTrue(window.styleMask.contains(.resizable))
        window.alignTitlebarButtons()
        for type in [NSWindow.ButtonType.closeButton, .miniaturizeButton, .zoomButton] {
            let button = try XCTUnwrap(window.standardWindowButton(type))
            let center = button.convert(NSPoint(x: button.bounds.midX, y: button.bounds.midY), to: nil)
            XCTAssertEqual(center.y, window.frame.height - 26, accuracy: 0.5)
        }
        XCTAssertTrue(window.isOpaque)
        XCTAssertFalse(window.ignoresMouseEvents)
        XCTAssertTrue(window.contentView === content)
        let frame = window.frame
        controller.setWindowed(false)
        XCTAssertFalse(model.isWindowed)
        XCTAssertFalse(window.styleMask.contains(.titled))
        XCTAssertFalse(window.isOpaque)
        XCTAssertTrue(window.contentView === content)
        controller.setWindowed(true)
        XCTAssertEqual(window.frame, frame)
        XCTAssertEqual(model.azimuth, 0.7)
        XCTAssertEqual(model.elevation, 0.4)
        XCTAssertEqual(model.distance, 1.7)
        XCTAssertTrue(model.cnc.machine.connected)
        XCTAssertTrue(model.cnc.machine.demo)
    }
}
