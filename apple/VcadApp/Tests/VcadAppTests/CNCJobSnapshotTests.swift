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

    private func job(blocked: Bool, setup: Bool = false, twoTool: Bool = false) async throws -> EditorModel {
        let model = EditorModel()
        model.workspace = .manufacture
        let cnc = model.cnc
        cnc.shown = true
        if twoTool {
            // The stator as it really wants to be cut: a Ø3.175 for the
            // profile and the bore, a Ø2.5 drill for the three pilots that no
            // end mill in the list fits into (item 19).
            cnc.tools = [CNCTool(number: 1, kind: .flatEndMill, diameter: 3.175,
                                 flutes: 2, fluteLength: 10, stickout: 20),
                         CNCTool(number: 2, kind: .drill, diameter: 2.5,
                                 flutes: 2, fluteLength: 20, stickout: 30)]
        } else {
            cnc.toolDiameter = 2
        }
        cnc.stockThickness = blocked ? 0.8 : 1.0
        if !twoTool { cnc.toolStickout = 18; cnc.toolFluteLength = 6 }
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

    /// The Machine stage with everything the merge mounted on it: the machine
    /// bar, the ncSender panel and the camera tile, and the connection popover
    /// that the header used to show an older version of.
    ///
    ///     VCAD_CNC_SNAPSHOTS=1 swift test --filter CNCJobSnapshotTests
    func testMachineStageSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else {
            throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render Manufacture snapshots.")
        }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let model = try await job(blocked: false)
        let cnc = model.cnc
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.mode = .machine
        cnc.senderPanelShown = true
        // A camera that has never produced a frame is what the tile shows on a
        // bench with no camera, which is the state worth looking at.
        cnc.camera.setURL("rtsps://camera.example/stream")
        defer { cnc.camera.setURL("") }

        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            let content = VStack(spacing: 12) {
                WorkspaceHeader(model: model)
                HStack(alignment: .top, spacing: 12) {
                    CNCStudioOutline(cnc: cnc).frame(height: 700).panelSurface()
                    CNCNcSenderPanel(cnc: cnc, sender: cnc.ncSender, camera: cnc.camera,
                                     documentName: "stator", now: Date(timeIntervalSince1970: 1_758_153_600))
                        .frame(width: 330, height: 700).panelSurface()
                    CNCCameraTile(camera: cnc.camera).frame(width: 300).panelSurface()
                    // The connection view, as the header's and the bar's
                    // popovers both now show it.
                    ScrollView { CNCMachineConnection(cnc: cnc).padding(18) }
                        .frame(width: 330, height: 520).panelSurface()
                    Spacer(minLength: 0)
                }
                CNCStudioTransport(cnc: cnc).panelSurface()
            }
            .padding(12)
            .frame(width: 1500, height: 900)
            .background(Color(nsColor: .windowBackgroundColor))

            let hosting = NSHostingView(rootView: content)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1500, height: 900),
                                  styleMask: [.borderless], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance)
            window.contentView = hosting; window.orderFront(nil)
            try? await Task.sleep(for: .milliseconds(400))
            hosting.layoutSubtreeIfNeeded()
            XCTAssertLessThanOrEqual(hosting.fittingSize.width, 1501,
                                     "machine-\(suffix): the panels must fit the window")
            let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: directory.appendingPathComponent("machine-\(suffix).png"))
            XCTAssertGreaterThan(data.count, 10_000)
            window.orderOut(nil)
        }
    }

    /// The Machine stage with the readiness checklist mounted as a section
    /// (item 3), and a two-tool job's readiness list beside it — where the
    /// tool sequence and the "pauses N times" sentence are read (item 19).
    ///
    ///     VCAD_CNC_SNAPSHOTS=1 swift test --filter CNCJobSnapshotTests
    func testReadinessSectionSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else {
            throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render Manufacture snapshots.")
        }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let model = try await job(blocked: false, twoTool: true)
        let cnc = model.cnc
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.mode = .machine

        XCTAssertTrue(cnc.blockers.isEmpty, "blocked: \(cnc.blockers.map(\.text))")
        XCTAssertEqual(cnc.toolSequence, [2, 1])
        XCTAssertEqual(cnc.toolChangeCount, 1, "the sentence under test has to have something to say")

        // **Why the Work Zero radios look wrong in these PNGs, and why that is
        // the harness rather than the app (item 6).**
        //
        // `cacheDisplay(in:to:)` draws AppKit's control indicators without
        // their state: in dark mode every radio and checkbox comes out a solid
        // filled shape, in light mode none of them is drawn at all. So all
        // three Work Zero radios look selected — and the integrate pass could
        // not tell whether that was real.
        //
        // These two are the falsifier, and they are asserted rather than
        // described: they hold *opposite* values in this very snapshot, and
        // the render draws them identically. No selection bug can do that.
        // The model-side proof that exactly one zero is ever chosen is in
        // `CNCFollowupTests.testWorkZeroIsExactlyOneChoice`.
        XCTAssertTrue(cnc.toolCentreCutting, "\"Cuts on its centre\" is on…")
        XCTAssertFalse(cnc.setupConfirmed, "…and \"I checked the tool…\" is off, in the same picture")
        XCTAssertEqual(CNCZeroLocation.allCases.filter { $0 == cnc.zeroLocation }.count, 1,
                       "one zero is selected however many the PNG appears to show")

        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            let content = VStack(spacing: 12) {
                WorkspaceHeader(model: model)
                HStack(alignment: .top, spacing: 12) {
                    CNCStudioOutline(cnc: cnc).frame(height: 700).panelSurface()
                    // The readiness list as the Machine stage now shows it:
                    // a section in the inspector, not a popover.
                    CNCStudioInspector(model: model).frame(height: 700).panelSurface()
                    // …and the Tool panel beside it, where the list lives.
                    ScrollView {
                        VStack(alignment: .leading, spacing: 10) {
                            CNCToolListSection(cnc: cnc)
                            Divider()
                            CNCZeroSection(cnc: cnc)
                        }.padding(14).controlSize(.small)
                    }.frame(width: 320, height: 700).panelSurface()
                    Spacer(minLength: 0)
                }
                CNCStudioTransport(cnc: cnc).panelSurface()
            }
            .padding(12)
            .frame(width: 1400, height: 900)
            .background(Color(nsColor: .windowBackgroundColor))

            let hosting = NSHostingView(rootView: content)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1400, height: 900),
                                  styleMask: [.borderless], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance)
            window.contentView = hosting; window.orderFront(nil)
            try? await Task.sleep(for: .milliseconds(400))
            hosting.layoutSubtreeIfNeeded()
            XCTAssertLessThanOrEqual(hosting.fittingSize.width, 1401,
                                     "readiness-\(suffix): the panels must fit the window")
            let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: directory.appendingPathComponent("readiness-\(suffix).png"))
            XCTAssertGreaterThan(data.count, 10_000)
            window.orderOut(nil)
        }
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
