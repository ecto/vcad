import AppKit
import SwiftUI
import XCTest
@testable import VcadApp

/// The Manufacture workspace, rendered offscreen with a real job in it.
///
/// The editor window is borderless, so there is no screenshot to take and no
/// accessibility window to query (friction-log item 31). These render the
/// panels to PNGs under `/tmp/vcad-manufacture` so the layout can be looked at:
///
///     VCAD_CNC_SNAPSHOTS=1 swift test --filter CNCJobSnapshotTests
@MainActor
final class CNCJobSnapshotTests: XCTestCase {

    private func job(blocked: Bool) async throws -> EditorModel {
        let model = EditorModel()
        model.workspace = .manufacture
        let cnc = model.cnc
        cnc.shown = true
        cnc.toolDiameter = 2
        cnc.stockThickness = blocked ? 0.8 : 1.0
        cnc.toolStickout = 18
        cnc.toolFluteLength = 6
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1.0
            cnc.setup.bottomAllowance = blocked ? 0 : 0.15
            cnc.setup.feed = 250; cnc.setup.plunge = 40; cnc.setup.rpm = 13500
            cnc.setup.stepdown = 0.17
            if cnc.setup.tabs > 0 { cnc.setup.tabHeight = 0.42 }
        }
        cnc.build()
        let deadline = Date().addingTimeInterval(120)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
        cnc.select(.operation(cnc.operations[0].id))
        return model
    }

    func testManufactureWorkspaceSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else {
            throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render Manufacture snapshots.")
        }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        for (name, isBlocked) in [("job-passing", false), ("job-blocked", true)] {
            let model = try await job(blocked: isBlocked)
            let cnc = model.cnc
            cnc.machine.connect(simulated: true)
            defer { cnc.machine.disconnect() }
            XCTAssertEqual(!cnc.blockers.isEmpty, isBlocked,
                           "\(name): blockers \(cnc.blockers.map(\.text))")

            for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
                let content = VStack(spacing: 12) {
                    WorkspaceHeader(model: model)
                    HStack(alignment: .top, spacing: 12) {
                        CNCStudioOutline(cnc: cnc).frame(height: 620).panelSurface()
                        Spacer(minLength: 0)
                        CNCStudioInspector(model: model).frame(height: 620).panelSurface()
                    }
                    CNCStudioTransport(cnc: cnc).panelSurface()
                }
                .padding(12)
                .frame(width: 1320, height: 860)
                .background(Color(nsColor: .windowBackgroundColor))

                let hosting = NSHostingView(rootView: content)
                let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1320, height: 860),
                                      styleMask: [.borderless], backing: .buffered, defer: false)
                window.appearance = NSAppearance(named: appearance)
                window.contentView = hosting; window.orderFront(nil)
                try? await Task.sleep(for: .milliseconds(350))
                hosting.layoutSubtreeIfNeeded()
                XCTAssertLessThanOrEqual(hosting.fittingSize.width, 1321,
                                         "\(name)-\(suffix): the panels must fit the window")
                let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
                hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
                let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
                try data.write(to: directory.appendingPathComponent("\(name)-\(suffix).png"))
                XCTAssertGreaterThan(data.count, 10_000)
                window.orderOut(nil)
            }
        }
    }
}
