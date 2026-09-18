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

    private func job(blocked: Bool, setup: Bool = false) async throws -> EditorModel {
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
        if setup {
            // The Setup stage as it is really used: a material, zero on the
            // blank's corner, the job placed a little crooked, and two clamps
            // on the table — one of them in the cutter's way.
            cnc.loadMaterials()
            cnc.materialID = "copper-c110"
            cnc.zeroLocation = .stockCorner
            cnc.placement = CNCPlacement(dx: 0, dy: 0, rotationDeg: 4)
            let sweep = cnc.sweepRect
            cnc.clamps = [
                CNCClamp(x: sweep[0] - 34, y: sweep[1] + 8, width: 40, height: 18, name: "Left toe"),
                CNCClamp(x: sweep[2] - 6, y: sweep[3] - 30, width: 40, height: 18, name: "Right toe"),
            ]
        }
        cnc.build()
        let deadline = Date().addingTimeInterval(120)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
        if setup {
            // The tabs belong to the profile, so that is the operation the tab
            // panel is about even while the stock is what is selected.
            if let profile = cnc.operations.first(where: { $0.setup.kind == .contourOutside }) {
                cnc.select(.operation(profile.id))
            }
            cnc.select(.stock)
            cnc.mode = .setup
        } else {
            cnc.select(.operation(cnc.operations[0].id))
        }
        return model
    }

    func testManufactureWorkspaceSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else {
            throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render Manufacture snapshots.")
        }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        for (name, isBlocked, isSetup) in [("job-passing", false, false),
                                           ("job-blocked", true, false),
                                           ("job-setup", false, true)] {
            let model = try await job(blocked: isBlocked, setup: isSetup)
            let cnc = model.cnc
            cnc.machine.connect(simulated: true)
            defer { cnc.machine.disconnect() }
            XCTAssertEqual(!cnc.blockers.isEmpty, isBlocked,
                           "\(name): blockers \(cnc.blockers.map(\.text))")
            if isSetup {
                XCTAssertFalse(cnc.clampsInTheWay.isEmpty,
                               "the setup snapshot is meant to show a clamp in the sweep")
                XCTAssertFalse(cnc.tabLandings(of: cnc.operations.last!).isEmpty,
                               "…and tabs that were really cut")
            }

            for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
                let content = VStack(spacing: 12) {
                    WorkspaceHeader(model: model)
                    HStack(alignment: .top, spacing: 12) {
                        CNCStudioOutline(cnc: cnc).frame(height: 620).panelSurface()
                        if isSetup {
                            // The panels this stage is about, side by side:
                            // where zero is, where the job sits on the metal,
                            // what is clamped to it, and where the tabs are.
                            ScrollView {
                                VStack(alignment: .leading, spacing: 10) {
                                    CNCZeroSection(cnc: cnc)
                                    Divider()
                                    CNCPlacementSection(cnc: cnc)
                                    Divider()
                                    CNCClampSection(cnc: cnc)
                                    Divider()
                                    CNCTabSection(cnc: cnc)
                                }.padding(14).controlSize(.small)
                            }.frame(width: 320, height: 620).panelSurface()
                        }
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
