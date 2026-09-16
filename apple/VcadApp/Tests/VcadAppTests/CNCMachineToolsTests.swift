import XCTest
import AppKit
import SwiftUI
@testable import VcadApp

@MainActor
final class CNCMachineToolsTests: XCTestCase {
    func testOverrideAndAccessoryReportsAreNotInvented() {
        var status = CNCStatus()
        XCTAssertNil(status.feedOverride)
        XCTAssertTrue(status.ingest("<Run|MPos:1,2,3|Ov:80,50,110|A:SFM|Pn:P>", scale: 1))
        XCTAssertEqual(status.feedOverride, 80)
        XCTAssertEqual(status.spindleOverride, 110)
        XCTAssertTrue(status.flood); XCTAssertTrue(status.mist); XCTAssertTrue(status.probeTriggered)
        XCTAssertTrue(status.ingest("<Idle|MPos:1,2,3>", scale: 1))
        XCTAssertEqual(status.feedOverride, 80) // Ov is intermittent.
        XCTAssertFalse(status.flood); XCTAssertFalse(status.mist); XCTAssertFalse(status.probeTriggered)
        XCTAssertFalse(status.ingest("<Run|Ov:100,nope,100>", scale: 1))
    }
    func testRealtimeBytesNeverUseUTF8OrBurstRepeatedFlags() {
        XCTAssertEqual(CNCCommands.overrideByte(spindle: false, current: 100, target: 80), 0x92)
        XCTAssertEqual(CNCCommands.overrideByte(spindle: false, current: 80, target: 81), 0x93)
        XCTAssertEqual(CNCCommands.overrideByte(spindle: true, current: 100, target: 150), 0x9a)
        XCTAssertEqual(CNCCommands.overrideByte(spindle: true, current: 99, target: 100), 0x99)
        XCTAssertNil(CNCCommands.overrideByte(spindle: true, current: 100, target: 100))
        XCTAssertNil(CNCCommands.overrideByte(spindle: true, current: 100, target: 201))
    }
    func testProgramValidationStripsCommentsAndBlocksRealtimeInjection() throws {
        XCTAssertEqual(try CNCCommands.programLines("(comment)\nG21 ; units\nG0 X1 (move)\nM2"), ["G21", "G0 X1", "M2"])
        for source in ["G0 X1!", "G0 X1~", "$H", "G0 X1\u{18}", "M2\nG0 X10", String(repeating: "X", count: 80)] {
            XCTAssertThrowsError(try CNCCommands.programLines(source), source)
        }
        XCTAssertFalse(CNCCommands.validManual("  $13=1"))
        XCTAssertFalse(CNCCommands.validManual("G0 X1\nG0 X2"))
        XCTAssertTrue(CNCCommands.validManual("$G"))
    }
    func testJogAndZeroUseExplicitUnitsAndSelectedOffset() {
        XCTAssertEqual(CNCCommands.zero(axes: "XY", workspace: "G57"), "G10 L20 P4 X0 Y0")
        XCTAssertNil(CNCCommands.zero(axes: "XX", workspace: "G54"))
        XCTAssertEqual(CNCCommands.jog(x: -1, y: 1, z: 0, feed: 300), "$J=G91 G21 X-1.000 Y1.000 Z0.000 F300.0")
        XCTAssertNil(CNCCommands.jog(x: .nan, y: 1, z: 0, feed: 300))
        XCTAssertNil(CNCCommands.jog(x: 11, y: 1, z: 0, feed: 300))
    }
    func testImportUnitsRelativeMovesAndArcs() throws {
        let p = try CNCImport.parse("G20 G90\nG0 X1 Y1 Z0\nG91 G1 X1 F10\nG21 G90\nG3 X60.8 Y25.4 I5 J0 F100\nM2")
        XCTAssertTrue(p.gcode.hasPrefix("G21 G90 G17 G54 G94 F400"))
        XCTAssertEqual(p.moves[1].to[0], 25.4, accuracy: 0.001)
        XCTAssertEqual(p.moves[2].to[0], 50.8, accuracy: 0.001)
        XCTAssertEqual(p.moves[2].feed!, 254, accuracy: 0.001)
        XCTAssertEqual(p.moves.last!.to[0], 60.8, accuracy: 0.001)
        XCTAssertEqual(p.moves.last!.to[1], 25.4, accuracy: 0.001)
        XCTAssertGreaterThan(p.moves.count, 20)
        for source in ["G53 G0 X1", "G18 G2 X10 K5", "G0 X1\nM6", "G0X1\nT2", "G2 X2 Y0 R0.1", "G0 Xnan"] {
            XCTAssertThrowsError(try CNCImport.parse(source), source)
        }
    }
    func testSimulatorWorkspaceZeroProbeOverridesAndCoolant() {
        let machine = CNCController(); machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.jog(x: 3, y: 4, z: 5, feed: 300)
        XCTAssertEqual(machine.status.machine, CNCVector(x: 3, y: 4, z: 5))
        machine.selectWorkspace("G56")
        XCTAssertFalse(machine.canStart) // CAM and imported jobs explicitly use G54.
        machine.zero(axes: "X")
        XCTAssertEqual(machine.status.work, CNCVector(x: 0, y: 4, z: 5))
        XCTAssertEqual(machine.status.machine?.x, 3)
        machine.setOverride(spindle: false, percent: 75)
        machine.setOverride(spindle: true, percent: 110)
        XCTAssertEqual(machine.status.feedOverride, 75)
        XCTAssertEqual(machine.status.spindleOverride, 110)
        machine.setCoolant(flood: true, mist: true)
        XCTAssertTrue(machine.status.flood); XCTAssertTrue(machine.status.mist)
        machine.setCoolant(flood: false, mist: true)
        XCTAssertFalse(machine.status.flood); XCTAssertTrue(machine.status.mist)
        machine.probeZ(distance: 2, feed: 50)
        XCTAssertEqual(machine.probePosition?.z, 3)
        XCTAssertTrue(machine.canApplyProbe)
        machine.sendMDI("G20")
        XCTAssertFalse(machine.canApplyProbe) // Any MDI invalidates the probe result.
        machine.probeZ(distance: 1, feed: 50)
        machine.applyProbe(thickness: 1.25)
        XCTAssertEqual(machine.status.work?.z ?? -1, 1.25, accuracy: 0.001)
        XCTAssertTrue(machine.log.contains(where: { $0.contains("G21 G10 L20 P3") }))
        machine.savePark()
        machine.jog(x: 2, y: 2, feed: 300)
        machine.park()
        XCTAssertEqual(machine.status.machine, machine.parkPosition)
        machine.selectWorkspace("G54")
        XCTAssertEqual(machine.status.work?.x, machine.status.machine?.x)
        XCTAssertTrue(machine.canStart)
    }
    func testControlsCannotInterleaveWithJob() {
        let machine = CNCController(); machine.connect(simulated: true)
        defer { machine.disconnect() }
        machine.start("G21\nG0 X2\nM2")
        let log = machine.log
        machine.sendMDI("G0 X99")
        machine.selectWorkspace("G56")
        machine.setCoolant(flood: true, mist: true)
        machine.zero(axes: "XYZ")
        XCTAssertEqual(machine.log, log)
        XCTAssertEqual(machine.workspace, "G54")
        XCTAssertFalse(machine.status.flood)
    }
    func testMachineRailSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else { throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render machine rail snapshots.") }
        _ = NSApplication.shared
        let cnc = CNCWorkspace()
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        for (name, width, appearance) in [("compact", 996.0, NSAppearance.Name.aqua), ("wide", 1400.0, .aqua), ("dark", 996.0, .darkAqua)] {
            let content = CNCMachineRail(cnc: cnc).cncFloatingPanel().padding(12)
                .frame(width: width).background(Color(nsColor: .windowBackgroundColor))
            let hosting = NSHostingView(rootView: content)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: width, height: 220), styleMask: [.borderless], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance)
            window.contentView = hosting; window.orderFront(nil)
            try? await Task.sleep(for: .milliseconds(250))
            hosting.layoutSubtreeIfNeeded()
            XCTAssertLessThanOrEqual(hosting.fittingSize.width, width + 1, "Rail must fit the minimum Manufacture window")
            let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: directory.appendingPathComponent("rail-\(name).png"))
            window.orderOut(nil)
        }
    }

    func testManufacturePanelSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else { throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render native panel snapshots.") }
        _ = NSApplication.shared
        let model = EditorModel(), cnc = model.cnc
        cnc.shown = true
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.generate()
        while cnc.generating { try? await Task.sleep(for: .milliseconds(20)) }
        let directory = URL(fileURLWithPath: "/tmp/vcad-manufacture")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        for (name, appearance) in [("light", NSAppearance.Name.aqua), ("dark", NSAppearance.Name.darkAqua)] {
            cnc.inspectorTab = .terminal
            let content = VStack(spacing: 12) {
                WorkspaceHeader(model: model)
                HStack(alignment: .top) {
                    CNCStudioOutline(cnc: cnc).frame(height: 480).cncFloatingPanel()
                    Spacer()
                    CNCStudioMachinePanel(cnc: cnc).frame(height: 480).cncFloatingPanel()
                }
                CNCStudioInspectorDock(model: model).frame(height: 150).cncFloatingPanel()
                CNCStudioTransport(cnc: cnc)
            }.padding(12).background(Color(nsColor: .windowBackgroundColor))
            let hosting = NSHostingView(rootView: content)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1200, height: 900), styleMask: [.borderless], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance)
            window.contentView = hosting; window.orderFront(nil)
            try? await Task.sleep(for: .milliseconds(250))
            hosting.layoutSubtreeIfNeeded()
            let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: directory.appendingPathComponent("panels-\(name).png"))
            XCTAssertGreaterThan(data.count, 10_000)
            window.orderOut(nil)
        }
    }
}
