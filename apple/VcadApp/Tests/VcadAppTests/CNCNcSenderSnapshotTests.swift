import AppKit
import SwiftUI
import XCTest
@testable import VcadApp

/// The ncSender panel and the camera tile, rendered offscreen with the
/// recorded 2026-09-17 state in them.
///
///     VCAD_CNC_SNAPSHOTS=1 swift test --filter CNCNcSenderSnapshotTests
///
/// writes PNGs to `/tmp/vcad-ncsender`. The gate on the send — a real job,
/// really blocked — is asserted without the gate, because it is not about how
/// anything looks.
@MainActor
final class CNCNcSenderSnapshotTests: XCTestCase {

    /// A workspace with the stator in it. `blocked` removes the onion skin so
    /// the job's own verification refuses it.
    private func workspace(blocked: Bool) async throws -> EditorModel {
        let model = EditorModel()
        model.workspace = .manufacture
        let cnc = model.cnc
        cnc.shown = true
        cnc.toolDiameter = 2
        cnc.stockThickness = blocked ? 0.8 : 1.0
        cnc.toolStickout = 18
        cnc.toolFluteLength = 6
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(), name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1.0
            cnc.setup.bottomAllowance = blocked ? 0 : 0.15
            cnc.setup.feed = 250; cnc.setup.plunge = 40; cnc.setup.rpm = 13500
            cnc.setup.stepdown = 0.17
            if cnc.setup.tabs > 0 { cnc.setup.tabHeight = 0.42 }
        }
        cnc.build()
        let deadline = Date().addingTimeInterval(180)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(25)) }
        cnc.select(.operation(cnc.operations[0].id))
        return model
    }

    private func sender() async -> NcSenderSession {
        NcSenderMockProtocol.reset()
        NcSenderFixture.routeAll()
        let session = NcSenderSession(session: NcSenderMockProtocol.session(), baseURL: "http://pika:8090")
        await session.testConnection(nativeHost: "192.168.2.226", nativePort: "23")
        await session.pollOnce()
        return session
    }

    // MARK: the gate, on a real job

    func testSendIsRefusedForARealBlockedJobAndOfferedForAGoodOne() async throws {
        let sender = await sender()

        let bad = try await workspace(blocked: true).cnc
        XCTAssertFalse(bad.blockers.isEmpty, "the fixture was meant to be blocked by its own verification")
        let refused = sender.sendBlockers(jobCurrent: bad.jobCurrent, jobBlocked: !bad.blockers.isEmpty,
                                          hasCode: bad.jobCode != nil, envelope: envelope(sender, bad))
        XCTAssertTrue(refused.contains { $0.contains("blocked by its own verification") }, "\(refused)")

        let good = try await workspace(blocked: false).cnc
        XCTAssertTrue(good.blockers.isEmpty, "\(good.blockers.map(\.text))")
        XCTAssertNotNil(good.jobCode)
        let allowed = sender.sendBlockers(jobCurrent: good.jobCurrent, jobBlocked: !good.blockers.isEmpty,
                                          hasCode: good.jobCode != nil, envelope: envelope(sender, good))
        XCTAssertEqual(allowed, [], "a verified, unblocked job must be sendable")

        // And the real job's envelope, placed at the recorded work offset,
        // fits the recorded travel — the stator did, on the day.
        let check = try XCTUnwrap(envelope(sender, good))
        XCTAssertTrue(check.fits, check.blocker ?? "")
    }

    private func envelope(_ sender: NcSenderSession, _ cnc: CNCWorkspace) -> NcSenderEnvelopeCheck? {
        guard let verification = cnc.verification else { return nil }
        return sender.envelopeCheck(workMin: verification.envelope.workMin,
                                    workMax: verification.envelope.workMax)
    }

    // MARK: pictures

    func testNcSenderPanelAndCameraTileSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else {
            throw XCTSkip("Set VCAD_CNC_SNAPSHOTS=1 to render the ncSender snapshots.")
        }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: "/tmp/vcad-ncsender")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)

        let sender = await sender()
        let today = DateFormatter()
        today.locale = Locale(identifier: "en_US_POSIX")
        today.dateFormat = "yyyy-MM-dd"
        NcSenderMockProtocol.route("/api/logs/\(today.string(from: Date())).log",
                                   NcSenderMockProtocol.Reply(body: """
                                   14:02:09 SENT  $H
                                   14:02:44 SENT  G10 L20 P1 X0 Y0
                                   14:03:11 SENT  G38.2 Z-25 F40
                                   14:03:19 RECV  ALARM:5
                                   14:06:02 SENT  G0 X0 Y0
                                   """, contentType: "text/plain"))
        await sender.refreshLog(date: Date())
        let model = try await workspace(blocked: false)
        let camera = CNCCameraModel(url: CNCSecret("rtsps://admin:hunter2@10.0.9.4:7441/AbCdEfGh"),
                                    interval: 3, bounds: 2...5)
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = .current
        let day = calendar.date(from: DateComponents(year: 2026, month: 9, day: 17))!

        for (appearance, suffix) in [(NSAppearance.Name.aqua, "light"), (.darkAqua, "dark")] {
            let content = HStack(alignment: .top, spacing: 16) {
                CNCNcSenderPanel(cnc: model.cnc, sender: sender, camera: camera,
                                 documentName: "stator-outline.dxf", now: day)
                    .frame(width: 380).panelSurface()
                VStack(spacing: 16) {
                    CNCCameraTile(camera: camera, placeholder: Self.placeholderFrame())
                        .frame(width: 300).panelSurface()
                    Spacer(minLength: 0)
                }
            }
            .padding(16)
            .frame(width: 760, height: 1340)
            .background(Color(nsColor: .windowBackgroundColor))

            let hosting = NSHostingView(rootView: content)
            let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 760, height: 1340),
                                  styleMask: [.borderless], backing: .buffered, defer: false)
            window.appearance = NSAppearance(named: appearance)
            window.contentView = hosting; window.orderFront(nil)
            try? await Task.sleep(for: .milliseconds(350))
            hosting.layoutSubtreeIfNeeded()
            XCTAssertLessThanOrEqual(hosting.fittingSize.width, 761, "\(suffix): the panel must fit its column")
            let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
            hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
            let data = try XCTUnwrap(bitmap.representation(using: .png, properties: [:]))
            try data.write(to: directory.appendingPathComponent("ncsender-\(suffix).png"))
            XCTAssertGreaterThan(data.count, 10_000)
            window.orderOut(nil)
        }
    }

    /// A stand-in for a camera frame: something recognisably a picture, with no
    /// network and no ffmpeg.
    static func placeholderFrame() -> NSImage {
        let size = NSSize(width: 320, height: 180)
        let image = NSImage(size: size)
        image.lockFocus()
        NSGradient(starting: NSColor(calibratedWhite: 0.22, alpha: 1),
                   ending: NSColor(calibratedWhite: 0.42, alpha: 1))?
            .draw(in: NSRect(origin: .zero, size: size), angle: 90)
        NSColor(calibratedRed: 0.55, green: 0.42, blue: 0.28, alpha: 1).setFill()
        NSBezierPath(rect: NSRect(x: 60, y: 40, width: 200, height: 100)).fill()
        NSColor(calibratedWhite: 0.85, alpha: 0.9).setStroke()
        let crosshair = NSBezierPath()
        crosshair.move(to: NSPoint(x: 160, y: 60)); crosshair.line(to: NSPoint(x: 160, y: 120))
        crosshair.move(to: NSPoint(x: 130, y: 90)); crosshair.line(to: NSPoint(x: 190, y: 90))
        crosshair.lineWidth = 2; crosshair.stroke()
        ("CAM 1" as NSString).draw(at: NSPoint(x: 12, y: 12),
                                   withAttributes: [.foregroundColor: NSColor.white,
                                                    .font: NSFont.monospacedSystemFont(ofSize: 13, weight: .medium)])
        image.unlockFocus()
        return image
    }
}
