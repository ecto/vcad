import AppKit
import SwiftUI
import XCTest
@testable import VcadApp

@MainActor
final class DesignShellTests: XCTestCase {
    func testNativeDesignShellLayout() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else { throw XCTSkip("Opt-in native shell snapshots") }
        _ = NSApplication.shared
        let model = EditorModel()
        model.isWindowed = true
        for width in [800.0, 1240.0] {
            let view = VStack {
                WorkspaceHeader(model: model)
                HStack(alignment: .top) {
                    DesignModelNavigator(model: model)
                    Spacer(minLength: 0)
                    ObjectInspectorWindow(model: model)
                }.padding(16)
                Spacer()
                HStack { Spacer(); DesignViewportTools(model: model) }.padding(16)
            }.frame(width: width, height: 640).background(Color(nsColor: .windowBackgroundColor))
            let hosting = NSHostingView(rootView: view)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 640), styleMask: [.borderless], backing: .buffered, defer: false)
            window.contentView = hosting; window.orderFront(nil)
            try? await Task.sleep(for: .milliseconds(250))
            hosting.layoutSubtreeIfNeeded()
            XCTAssertLessThanOrEqual(hosting.fittingSize.width, width + 1)
            let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: URL(fileURLWithPath: "/tmp/vcad-manufacture/design-\(Int(width)).png"))
            window.orderOut(nil)
        }
    }
}
