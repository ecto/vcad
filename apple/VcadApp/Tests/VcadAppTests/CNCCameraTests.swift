import AppKit
import XCTest
@testable import VcadApp

/// A backend that answers instantly, counts its calls, and can be told to fail.
/// It never touches the network, so the interval loop is the only thing under
/// test here.
final class FakeSnapshotSource: CNCCameraSnapshotSource, @unchecked Sendable {
    private let lock = NSLock()
    private var calls = 0
    private var failure: CNCCameraError?
    let label = "fake"
    /// The bytes the fake hands back: a real 8×8 PNG, so `NSImage` accepts it.
    let bytes: Data

    init(failing: CNCCameraError? = nil) {
        failure = failing
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: 8, pixelsHigh: 8,
                                   bitsPerSample: 8, samplesPerPixel: 3, hasAlpha: false,
                                   isPlanar: false, colorSpaceName: .deviceRGB,
                                   bytesPerRow: 0, bitsPerPixel: 0)!
        bytes = rep.representation(using: .png, properties: [:])!
    }

    var callCount: Int { lock.withLock { calls } }
    func fail(with error: CNCCameraError?) { lock.withLock { failure = error } }

    func grab() async throws -> Data {
        let error: CNCCameraError? = lock.withLock { calls += 1; return failure }
        if let error { throw error }
        return bytes
    }
}

@MainActor
final class CNCCameraTests: XCTestCase {

    /// The camera URL is a credential. Nothing in this test's fixture may ever
    /// appear in anything the app prints.
    private static let secretURL = "rtsps://admin:hunter2@10.0.9.4:7441/AbCdEfGhIjKlMnOp?enableSrtp"

    private func model(source: FakeSnapshotSource, interval: Double = 0.05) -> CNCCameraModel {
        CNCCameraModel(url: CNCSecret(Self.secretURL), interval: interval,
                       bounds: 0.01...5, source: source)
    }

    // MARK: frames on the interval

    func testFramesArriveOnTheIntervalAndCarryTheirAge() async throws {
        let source = FakeSnapshotSource()
        let camera = model(source: source, interval: 0.05)
        XCTAssertNil(camera.frame)
        XCTAssertEqual(camera.ageLabel, "no frame")

        camera.start()
        try await waitUntil("three frames") { source.callCount >= 3 }
        XCTAssertEqual(camera.state, .live)
        camera.stop()

        XCTAssertNotNil(camera.frame)
        XCTAssertEqual(camera.frame?.size, NSSize(width: 8, height: 8))
        // A frame that just landed reads as fresh; one from four seconds ago
        // does not, which is the whole point of the label.
        XCTAssertEqual(camera.ageLabel, "just now")
        XCTAssertEqual(try XCTUnwrap(camera.frameAt).timeIntervalSinceNow, 0, accuracy: 1.0)
    }

    func testCancellingStopsTheLoopWithinOneInterval() async throws {
        let source = FakeSnapshotSource()
        let camera = model(source: source, interval: 0.05)
        camera.start()
        try await waitUntil("two frames") { source.callCount >= 2 }
        camera.stop()
        XCTAssertFalse(camera.running)
        let after = source.callCount
        try? await Task.sleep(for: .milliseconds(250))
        XCTAssertEqual(source.callCount, after, "the loop kept grabbing after stop()")
        XCTAssertEqual(camera.state, .off)
    }

    func testGrabNowTakesOneFrameOutsideTheLoop() async {
        let source = FakeSnapshotSource()
        let camera = model(source: source)
        await camera.grabNow()
        XCTAssertEqual(source.callCount, 1)
        XCTAssertNotNil(camera.frame)
        XCTAssertFalse(camera.running, "grabbing one frame must not start watching")
    }

    func testAFailingBackendSaysSoAndKeepsTheLastFrame() async {
        let source = FakeSnapshotSource()
        let camera = model(source: source)
        await camera.grabNow()
        source.fail(with: .failed("the camera refused the connection"))
        await camera.grabNow()
        XCTAssertEqual(camera.state, .failed("the camera refused the connection"))
        XCTAssertNotNil(camera.frame, "a failed grab must not blank a frame whose age is on screen")
    }

    func testAgeLabelSpansSecondsAndMinutes() async {
        let source = FakeSnapshotSource()
        let camera = model(source: source)
        await camera.grabNow()
        XCTAssertEqual(camera.ageLabel, "just now")
        // The label's arithmetic, without waiting for real time to pass.
        for (age, expected) in [(3.0, "3 s ago"), (45.0, "45 s ago"), (200.0, "3 min ago")] {
            let stamp = Date().addingTimeInterval(-age)
            let seconds = max(0, Date().timeIntervalSince(stamp))
            let label = seconds < 1 ? "just now"
                : seconds < 90 ? "\(Int(seconds.rounded())) s ago"
                : "\(Int((seconds / 60).rounded())) min ago"
            XCTAssertEqual(label, expected)
        }
    }

    // MARK: the URL is never said out loud

    func testTheCameraURLNeverAppearsInAnythingTheAppPrints() async {
        let source = FakeSnapshotSource()
        let camera = model(source: source)
        await camera.grabNow()
        source.fail(with: .failed("connection to \(Self.secretURL) refused"))
        await camera.grabNow()
        camera.stop()

        var reflected = ""
        dump(camera, to: &reflected)
        // The secret defends itself too, not only through the model's own
        // mirror: anything else that ends up holding one must be safe to dump.
        var reflectedSecret = ""
        dump(CNCSecret(Self.secretURL), to: &reflectedSecret)
        struct Holder { var camera: CNCSecret; var note = "holder" }
        var reflectedHolder = ""
        dump(Holder(camera: CNCSecret(Self.secretURL)), to: &reflectedHolder)

        let surfaces: [(String, String)] = [
            ("dump(CNCSecret)", reflectedSecret),
            ("dump(struct holding a CNCSecret)", reflectedHolder),
            ("String(describing:)", String(describing: camera)),
            ("String(reflecting:)", String(reflecting: camera)),
            ("dump", reflected),
            ("maskedURL", camera.maskedURL),
            ("state", "\(camera.state)"),
            ("log", camera.log.joined(separator: "\n")),
            ("secret description", String(describing: CNCSecret(Self.secretURL))),
        ]
        for (name, text) in surfaces {
            XCTAssertFalse(text.contains(Self.secretURL), "\(name) leaked the camera URL: \(text)")
            XCTAssertFalse(text.contains("hunter2"), "\(name) leaked the camera password: \(text)")
            XCTAssertFalse(text.contains("AbCdEfGhIjKlMnOp"), "\(name) leaked the stream path: \(text)")
        }
        // A backend that puts the URL in its own error message is scrubbed,
        // and the mask still says enough to confirm something is set.
        XCTAssertTrue(camera.log.contains { $0.contains("rtsps://••••••••") },
                      camera.log.joined(separator: "\n"))
        XCTAssertTrue(camera.maskedURL.hasPrefix("rtsps://••••••••"))
        XCTAssertTrue(camera.maskedURL.contains("\(Self.secretURL.count) characters"))
        XCTAssertEqual(CNCSecret("").masked, "not set")
    }

    // MARK: backend choice

    func testBackendFollowsTheSchemeAndSaysSoWhenNothingCanServeIt() throws {
        // HTTP snapshot: no external binary needed.
        switch CNCCameraBackend.source(for: CNCSecret("https://nvr.local/snap.jpg"), ffmpegPath: nil) {
        case .success(let source): XCTAssertEqual(source.label, "HTTP snapshot")
        case .failure(let error): XCTFail("HTTP should always work: \(error)")
        }
        // RTSP with ffmpeg present.
        switch CNCCameraBackend.source(for: CNCSecret(Self.secretURL), ffmpegPath: "/opt/homebrew/bin/ffmpeg") {
        case .success(let source): XCTAssertEqual(source.label, "ffmpeg")
        case .failure(let error): XCTFail("ffmpeg backend should have been chosen: \(error)")
        }
        // RTSP with no ffmpeg: the tile says what to do, it does not sit blank.
        switch CNCCameraBackend.source(for: CNCSecret(Self.secretURL), ffmpegPath: nil) {
        case .success: XCTFail("RTSP cannot be served without ffmpeg on macOS")
        case .failure(let error):
            let detail = try XCTUnwrap(error.errorDescription)
            XCTAssertTrue(detail.contains("ffmpeg"), detail)
            XCTAssertTrue(detail.contains("HTTP snapshot"), detail)
            XCTAssertFalse(detail.contains("hunter2"), detail)
        }
        // A scheme nobody can read, and an empty URL.
        if case .success = CNCCameraBackend.source(for: CNCSecret("file:///tmp/x.jpg"), ffmpegPath: "/usr/bin/ffmpeg") {
            XCTFail("file:// is not a camera")
        }
        XCTAssertEqual(CNCCameraBackend.source(for: CNCSecret(""), ffmpegPath: nil).failureError, .noURL)
    }

    func testFFmpegIsLookedForOnPathAndInHomebrewsPlaces() throws {
        // A machine with no ffmpeg anywhere reports none, rather than a path
        // that would fail to launch later.
        XCTAssertNil(FFmpegSnapshotSource.locate(environment: ["PATH": "/definitely/not/here"], wellKnown: []))
        // PATH is searched, and the well-known installs are searched even when
        // PATH does not mention them — a GUI app's PATH is not the shell's.
        let sandbox = try XCTUnwrap(FileManager.default
            .urls(for: .cachesDirectory, in: .userDomainMask).first?
            .appendingPathComponent("vcad-ffmpeg-locate-\(UUID().uuidString)"))
        try FileManager.default.createDirectory(at: sandbox, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: sandbox) }
        let planted = sandbox.appendingPathComponent("ffmpeg")
        try Data("#!/bin/sh\n".utf8).write(to: planted)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: planted.path)

        XCTAssertEqual(FFmpegSnapshotSource.locate(environment: ["PATH": sandbox.path], wellKnown: []), planted.path)
        XCTAssertEqual(FFmpegSnapshotSource.locate(environment: [:], wellKnown: [planted.path]), planted.path)
    }

    // MARK: helper

    private func waitUntil(_ what: String, timeout: TimeInterval = 5,
                           _ condition: @escaping () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return }
            try? await Task.sleep(for: .milliseconds(10))
        }
        XCTFail("timed out waiting for \(what)")
    }
}

private extension Result where Failure == CNCCameraError {
    var failureError: CNCCameraError? {
        if case .failure(let error) = self { return error }
        return nil
    }
}
