import XCTest
@testable import VcadApp

// ncSender, played back.
//
// The fixtures are the responses recorded off `pika` on 2026-09-17 — the day
// the first real cut was sent through ncSender rather than this app. The
// numbers are that machine's: MPos 113.900,-106.221,-118.200, WCO
// 82.325,-137.796,-124.012, `homed:false`, `Pn Y` with `alarmCode 1`, and
// $130/131/132 = 400/300/130 with soft and hard limits on.

// MARK: - The mock transport

/// Plays recorded bodies back for paths, and records what was posted.
final class NcSenderMockProtocol: URLProtocol {
    struct Reply: Sendable {
        var status = 200
        var body: String
        var contentType = "application/json"
    }
    struct Sent: Sendable {
        var path: String
        var method: String
        var contentType: String?
        var body: Data
    }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var routes: [String: Reply] = [:]
    nonisolated(unsafe) private static var sent: [Sent] = []

    static func reset() { lock.withLock { routes = [:]; sent = [] } }
    static func route(_ path: String, _ reply: Reply) { lock.withLock { routes[path] = reply } }
    static func route(_ path: String, json: String) { route(path, Reply(body: json)) }
    static var recorded: [Sent] { lock.withLock { sent } }

    /// A session wired to this protocol and nothing else.
    static func session() -> URLSession {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [NcSenderMockProtocol.self]
        return URLSession(configuration: configuration)
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        let path = request.url?.path ?? ""
        // URLSession hands URLProtocol an httpBodyStream even when the caller
        // set httpBody, so read whichever is there.
        var body = request.httpBody ?? Data()
        if body.isEmpty, let stream = request.httpBodyStream {
            stream.open()
            var buffer = [UInt8](repeating: 0, count: 65_536)
            while stream.hasBytesAvailable {
                let read = stream.read(&buffer, maxLength: buffer.count)
                if read <= 0 { break }
                body.append(contentsOf: buffer[0..<read])
            }
            stream.close()
        }
        Self.lock.withLock {
            Self.sent.append(Sent(path: path, method: request.httpMethod ?? "GET",
                                  contentType: request.value(forHTTPHeaderField: "Content-Type"),
                                  body: body))
        }
        let reply = Self.lock.withLock { Self.routes[path] }
            ?? Reply(status: 404, body: "{\"error\":\"no route for \(path)\"}")
        let response = HTTPURLResponse(url: request.url!, statusCode: reply.status,
                                       httpVersion: "HTTP/1.1",
                                       headerFields: ["Content-Type": reply.contentType])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(reply.body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

// MARK: - The recorded day

enum NcSenderFixture {
    static let serverState = """
    {
      "version": "ncSender 1.4.2",
      "machineState": {
        "status": "Alarm",
        "MPos": [113.900, -106.221, -118.200],
        "WPos": [31.575, 31.575, 5.812],
        "WCO": [82.325, -137.796, -124.012],
        "Pn": "Y",
        "homed": false,
        "alarmCode": 1,
        "spindleActive": false
      },
      "jobLoaded": {
        "filename": "stator-d2-20260917.nc",
        "currentLine": 37,
        "totalLines": 7642,
        "progressPercent": 0.484,
        "status": "running"
      }
    }
    """
    static let firmware = """
    {"settings": {"$20": "1", "$21": "1", "$23": "1", "$27": "3", "$30": "10000",
                  "$130": "400", "$131": "300", "$132": "130"}}
    """
    static let jobStatus = """
    {"filename": "stator-d2-20260917.nc", "currentLine": 37, "totalLines": 7642,
     "progressPercent": 0.484, "status": "running", "elapsedSeconds": 42}
    """
    static let settings = """
    {"connection": {"ip": "192.168.2.226", "port": 23}, "probe": {"type": "3d-probe"}}
    """
    static let health = #"{"status": "ok", "version": "1.4.2"}"#

    static func routeAll() {
        NcSenderMockProtocol.route("/api/server-state", json: serverState)
        NcSenderMockProtocol.route("/api/firmware", json: firmware)
        NcSenderMockProtocol.route("/api/gcode-job/status", json: jobStatus)
        NcSenderMockProtocol.route("/api/settings", json: settings)
        NcSenderMockProtocol.route("/api/health", json: health)
    }

    /// The stator's own envelope: the swept extent of the job that was cut,
    /// in work coordinates, cutter included.
    static let workMin = [-2.0, -2.0, -1.05]
    static let workMax = [65.0, 65.0, 5.0]
}

@MainActor
final class CNCNcSenderTests: XCTestCase {

    override func setUp() {
        super.setUp()
        NcSenderMockProtocol.reset()
        NcSenderFixture.routeAll()
    }

    private func session() -> NcSenderSession {
        NcSenderSession(session: NcSenderMockProtocol.session(), baseURL: "http://pika:8090")
    }

    // MARK: what the fixtures say

    func testRecordedStateShowsTravelNotHomedAlarmAndProgress() async throws {
        let sender = session()
        await sender.testConnection(nativeHost: "10.0.0.5", nativePort: "23")
        await sender.pollOnce()

        let machine = try XCTUnwrap(sender.machine)
        XCTAssertEqual(machine.status, "Alarm")
        XCTAssertEqual(try XCTUnwrap(machine.mpos).x, 113.900, accuracy: 1e-9)
        XCTAssertEqual(try XCTUnwrap(machine.mpos).y, -106.221, accuracy: 1e-9)
        XCTAssertEqual(try XCTUnwrap(machine.mpos).z, -118.200, accuracy: 1e-9)
        XCTAssertEqual(try XCTUnwrap(machine.workOffset).x, 82.325, accuracy: 1e-9)
        XCTAssertFalse(machine.homed)

        // Travel: the numbers $130–132 gave, and the interval each axis lives
        // in, taken from where the machine is actually standing.
        let travel = try XCTUnwrap(sender.travel?.travel)
        XCTAssertEqual(travel.summary, "400 / 300 / 130 mm")
        XCTAssertEqual(travel.axes[0], NcSenderAxisTravel(lower: 0, upper: 400, maxTravel: 400, source: .position))
        XCTAssertEqual(travel.axes[1], NcSenderAxisTravel(lower: -300, upper: 0, maxTravel: 300, source: .position))
        XCTAssertEqual(travel.axes[2], NcSenderAxisTravel(lower: -130, upper: 0, maxTravel: 130, source: .position))
        XCTAssertEqual(sender.firmware?.softLimits, true)
        XCTAssertEqual(sender.firmware?.hardLimits, true)

        // Not homed is a warning, and it says why it matters.
        let notHomed = try XCTUnwrap(sender.notHomedWarning)
        XCTAssertTrue(notHomed.contains("has not homed"), notHomed)

        // ALARM:1 is explained, including that position is lost.
        let alarm = try XCTUnwrap(sender.alarmExplanation)
        XCTAssertTrue(alarm.hasPrefix("ALARM:1 hard limit"), alarm)
        XCTAssertTrue(alarm.contains("re-home") || alarm.contains("Re-home"), alarm)
        XCTAssertEqual(sender.assertedPins, ["Y limit"])

        // Progress comes from the job endpoint, which is the authority.
        let job = try XCTUnwrap(sender.job)
        XCTAssertEqual(job.currentLine, 37)
        XCTAssertEqual(job.totalLines, 7642)
        XCTAssertEqual(job.filename, "stator-d2-20260917.nc")
        XCTAssertTrue(job.isRunning)
        let times = try XCTUnwrap(sender.progressTimes)
        XCTAssertEqual(times.elapsed, 42, accuracy: 0.001)
        // 37 of 7642 lines in 42 s → 8632.7 s to go. Assert the arithmetic, not a mood.
        XCTAssertEqual(try XCTUnwrap(times.remaining), 42 * 7605 / 37, accuracy: 0.01)
        // Minutes and seconds under an hour, with the hour carried above it:
        // a 2h24 estimate must not read as "143:53" minutes.
        XCTAssertEqual(NcSenderJobPlan.duration(times.elapsed), "00:42")
        XCTAssertEqual(NcSenderJobPlan.duration(try XCTUnwrap(times.remaining)), "2:23:53")
        XCTAssertEqual(NcSenderJobPlan.duration(3599), "59:59")
        XCTAssertEqual(NcSenderJobPlan.duration(3600), "1:00:00")
        XCTAssertEqual(NcSenderJobPlan.duration(.infinity), "—")
    }

    func testAlarm2SaysNothingMoved() {
        XCTAssertTrue(NcSenderAlarm.explain(2).contains("nothing moved"))
        XCTAssertTrue(NcSenderAlarm.explain(5).contains("never touched"))
        XCTAssertTrue(NcSenderAlarm.explain(99).contains("ALARM:99"))
    }

    // MARK: the envelope, placed in machine coordinates

    func testJobFitsTravelAtTheRecordedWorkOffset() async throws {
        let sender = session()
        await sender.testConnection(nativeHost: "", nativePort: "")
        let check = sender.envelopeCheck(workMin: NcSenderFixture.workMin, workMax: NcSenderFixture.workMax)
        XCTAssertNil(check.blocker, check.blocker ?? "")
        XCTAssertTrue(check.fits)
        // X: −2…65 offset by 82.325 → 80.325…147.325, inside 0…400.
        XCTAssertEqual(check.machineMin[0], 80.325, accuracy: 1e-9)
        XCTAssertEqual(check.machineMax[0], 147.325, accuracy: 1e-9)
        XCTAssertEqual(check.overshoot, [0, 0, 0])
    }

    func testJobBeyondTravelIsBlockedWithTheOvershootInMillimetres() throws {
        let firmware = try JSONDecoder().decode(NcSenderFirmware.self, from: Data(NcSenderFixture.firmware.utf8))
        // Same job, a work zero 340 mm along X: the far side reaches 405 on a
        // 400 mm axis.
        let offset = NcSenderVector(x: 340, y: -137.796, z: -124.012)
        let position = NcSenderVector(x: 113.9, y: -106.221, z: -118.2)
        let check = NcSenderEnvelopeCheck.run(workMin: NcSenderFixture.workMin,
                                              workMax: NcSenderFixture.workMax,
                                              offset: offset,
                                              travel: NcSenderTravel.resolve(firmware: firmware,
                                                                             machinePosition: position))
        XCTAssertFalse(check.fits)
        XCTAssertEqual(check.worstAxis, 0)
        XCTAssertEqual(check.overshoot[0], 5.0, accuracy: 1e-9)      // 340 + 65 − 400
        XCTAssertEqual(check.overshoot[1], 0, accuracy: 1e-9)
        XCTAssertEqual(check.overshoot[2], 0, accuracy: 1e-9)
        XCTAssertEqual(check.blocker, "Job leaves X travel by 5 mm.")

        // And half a step further, so the fraction is carried through.
        let further = NcSenderEnvelopeCheck.run(workMin: NcSenderFixture.workMin,
                                                workMax: NcSenderFixture.workMax,
                                                offset: NcSenderVector(x: 342.5, y: -137.796, z: -124.012),
                                                travel: NcSenderTravel.resolve(firmware: firmware,
                                                                               machinePosition: position))
        XCTAssertEqual(further.overshoot[0], 7.5, accuracy: 1e-9)
        XCTAssertEqual(further.blocker, "Job leaves X travel by 7.5 mm.")
    }

    func testEnvelopeRefusesRatherThanGuessWhenTravelIsUnknown() {
        // No $130–132 at all.
        let blank = NcSenderEnvelopeCheck.run(workMin: NcSenderFixture.workMin, workMax: NcSenderFixture.workMax,
                                              offset: NcSenderVector(x: 0, y: 0, z: 0),
                                              travel: NcSenderTravel.resolve(firmware: NcSenderFirmware(),
                                                                             machinePosition: nil))
        XCTAssertFalse(blank.fits)
        XCTAssertTrue(try! XCTUnwrap(blank.blocker).contains("$130"))

        // Travel known, but no work offset: the job cannot be placed at all.
        let firmware = NcSenderFirmware(settings: ["$130": 400, "$131": 300, "$132": 130, "$23": 1])
        let unplaced = NcSenderEnvelopeCheck.run(workMin: NcSenderFixture.workMin, workMax: NcSenderFixture.workMax,
                                                 offset: nil,
                                                 travel: NcSenderTravel.resolve(firmware: firmware,
                                                                                machinePosition: NcSenderVector(x: 1, y: -1, z: -1)))
        XCTAssertFalse(unplaced.fits)
        XCTAssertTrue(try! XCTUnwrap(unplaced.blocker).contains("work offset"))

        // An axis sitting exactly on its origin: $23 decides, and without it
        // the check refuses rather than picking a side.
        let onOrigin = NcSenderVector(x: 0, y: -106.221, z: -118.2)
        XCTAssertEqual(NcSenderTravel.resolve(firmware: firmware, machinePosition: onOrigin).travel?.axes[0],
                       NcSenderAxisTravel(lower: 0, upper: 400, maxTravel: 400, source: .homingMask))
        let noMask = NcSenderFirmware(settings: ["$130": 400, "$131": 300, "$132": 130])
        XCTAssertTrue(try! XCTUnwrap(NcSenderTravel.resolve(firmware: noMask, machinePosition: onOrigin).reason)
            .contains("$23"))
    }

    // MARK: the upload

    func testMultipartBodyIsExactlyWhatNcSenderIsGiven() {
        let gcode = "G21 G90\nG0 Z5\nM2\n"
        let body = NcSenderClient.multipartBody(boundary: "vcad-TEST", filename: "stator-d2-20260917.nc", gcode: gcode)
        let text = String(decoding: body, as: UTF8.self)
        XCTAssertEqual(text, """
        --vcad-TEST\r
        Content-Disposition: form-data; name="file"; filename="stator-d2-20260917.nc"\r
        Content-Type: text/plain; charset=utf-8\r
        \r
        G21 G90
        G0 Z5
        M2
        \r
        --vcad-TEST--\r

        """)
        // Byte count: the two boundary lines and headers, plus the file verbatim.
        let overhead = "--vcad-TEST\r\n"
            + "Content-Disposition: form-data; name=\"file\"; filename=\"stator-d2-20260917.nc\"\r\n"
            + "Content-Type: text/plain; charset=utf-8\r\n\r\n"
            + "\r\n--vcad-TEST--\r\n"
        XCTAssertEqual(body.count, overhead.utf8.count + gcode.utf8.count)
        XCTAssertTrue(body.range(of: Data(gcode.utf8)) != nil, "the G-code must go out byte for byte")
    }

    func testUploadPostsTheMultipartToTheFilesEndpoint() async throws {
        NcSenderMockProtocol.route("/api/gcode-files", json: #"{"ok":true}"#)
        let client = NcSenderClient(baseURL: URL(string: "http://pika:8090")!, session: NcSenderMockProtocol.session())
        _ = try await client.upload(filename: "stator-d2-20260917.nc", gcode: "G21\nM2\n", boundary: "vcad-TEST")
        let post = try XCTUnwrap(NcSenderMockProtocol.recorded.first { $0.path == "/api/gcode-files" })
        XCTAssertEqual(post.method, "POST")
        XCTAssertEqual(post.contentType, "multipart/form-data; boundary=vcad-TEST")
        let body = String(decoding: post.body, as: UTF8.self)
        XCTAssertTrue(body.contains("filename=\"stator-d2-20260917.nc\""), body)
        XCTAssertTrue(body.contains("G21\nM2\n"), body)
    }

    func testFilenamesThatWouldEscapeAreRefused() async {
        let client = NcSenderClient(baseURL: URL(string: "http://pika:8090")!, session: NcSenderMockProtocol.session())
        for bad in ["../etc/passwd.nc", "a/b.nc", "he\"re.nc", "line\nbreak.nc", ""] {
            do {
                _ = try await client.upload(filename: bad, gcode: "G21\n")
                XCTFail("“\(bad)” should not be handed to ncSender")
            } catch let error as NcSenderError {
                guard case .badFilename = error else { return XCTFail("wrong refusal for \(bad): \(error)") }
            } catch { XCTFail("wrong error type for \(bad)") }
        }
    }

    func testLoadedFileCheckFailsLoudlyWhenLineCountsDiffer() {
        let gcode = "G21 G90\nG0 Z5\n\nG1 X1 F250\nM2\n"   // 5 file lines, 4 with content
        XCTAssertEqual(NcSenderJobPlan.fileLineCount(gcode), 5)
        XCTAssertEqual(NcSenderJobPlan.codeLineCount(gcode), 4)

        func check(_ reported: Int, name: String = "job.nc") -> NcSenderLoadCheck {
            NcSenderLoadCheck(expectedFilename: "job.nc",
                              fileLines: NcSenderJobPlan.fileLineCount(gcode),
                              codeLines: NcSenderJobPlan.codeLineCount(gcode),
                              reportedFilename: name, reportedLines: reported)
        }
        XCTAssertTrue(check(5).ok)                       // counted every line
        XCTAssertTrue(check(4).ok)                       // counted only the ones with content
        XCTAssertFalse(check(3).ok)                      // a truncated upload
        let failure = try! XCTUnwrap(check(3).failure)
        XCTAssertTrue(failure.contains("3 lines") && failure.contains("5"), failure)
        XCTAssertTrue(failure.contains("upload again"), failure)
        XCTAssertTrue(try! XCTUnwrap(check(5, name: "something-else.nc").failure).contains("something-else.nc"))
    }

    func testSendJobRefusesWhenTheSenderLoadedSomethingElse() async throws {
        NcSenderMockProtocol.route("/api/gcode-files", json: #"{"ok":true}"#)
        // ncSender reports 7642 lines; this job has 3.
        let sender = session()
        let sent = await sender.sendJob(gcode: "G21\nG0 Z5\nM2\n", filename: "stator-d2-20260917.nc")
        XCTAssertFalse(sent)
        guard case .failed(let detail) = sender.send else { return XCTFail("expected a refusal, got \(sender.send)") }
        XCTAssertTrue(detail.contains("7642"), detail)
    }

    func testSendJobSucceedsWhenTheSenderIsHoldingExactlyIt() async throws {
        NcSenderMockProtocol.route("/api/gcode-files", json: #"{"ok":true}"#)
        let gcode = (0..<7642).map { "G1 X\($0)" }.joined(separator: "\n") + "\n"
        XCTAssertEqual(NcSenderJobPlan.fileLineCount(gcode), 7642)
        let sender = session()
        let sent = await sender.sendJob(gcode: gcode, filename: "stator-d2-20260917.nc")
        XCTAssertTrue(sent, sender.lastError ?? "")
        guard case .loaded(let name, let lines) = sender.send else { return XCTFail("expected loaded, got \(sender.send)") }
        XCTAssertEqual(name, "stator-d2-20260917.nc")
        XCTAssertEqual(lines, 7642)
    }

    // MARK: the gate

    func testSendIsBlockedWhenTheWorkspaceReportsABlockedJob() async {
        let sender = session()
        await sender.testConnection(nativeHost: "", nativePort: "")
        let fits = sender.envelopeCheck(workMin: NcSenderFixture.workMin, workMax: NcSenderFixture.workMax)
        XCTAssertTrue(sender.sendBlockers(jobCurrent: true, jobBlocked: false, hasCode: true, envelope: fits).isEmpty)

        let blocked = sender.sendBlockers(jobCurrent: true, jobBlocked: true, hasCode: true, envelope: fits)
        XCTAssertEqual(blocked.count, 1)
        XCTAssertTrue(blocked[0].contains("blocked by its own verification"), blocked[0])

        XCTAssertFalse(sender.sendBlockers(jobCurrent: false, jobBlocked: false, hasCode: true, envelope: fits).isEmpty)
        XCTAssertFalse(sender.sendBlockers(jobCurrent: true, jobBlocked: false, hasCode: false, envelope: fits).isEmpty)

        // A job that leaves travel is blocked here too, with the same sentence
        // the panel shows.
        let offTravel = NcSenderEnvelopeCheck.run(workMin: NcSenderFixture.workMin, workMax: NcSenderFixture.workMax,
                                                  offset: NcSenderVector(x: 340, y: -137.796, z: -124.012),
                                                  travel: sender.travel ?? .unknown(""))
        XCTAssertTrue(sender.sendBlockers(jobCurrent: true, jobBlocked: false, hasCode: true, envelope: offTravel)
            .contains("Job leaves X travel by 5 mm."))

        // Not connected at all is its own reason.
        let cold = NcSenderSession(session: NcSenderMockProtocol.session(), baseURL: "http://pika:8090")
        XCTAssertTrue(cold.sendBlockers(jobCurrent: true, jobBlocked: false, hasCode: true, envelope: nil)
            .contains { $0.contains("Not connected") })
    }

    // MARK: commands cannot get out unconfirmed

    func testMotionCommandsCannotBeIssuedWithoutConfirmation() async throws {
        NcSenderMockProtocol.route("/api/send-command", json: #"{"ok":true}"#)
        let client = NcSenderClient(baseURL: URL(string: "http://pika:8090")!, session: NcSenderMockProtocol.session())

        for command in ["$H", "$J=G91 G21 X1 F300", "G0 X10", "G1 Z-1 F100", "G10 L20 P1 X0",
                        "G38.2 Z-5 F40", "$X", "M3 S13500", "WHO KNOWS"] {
            do {
                _ = try await client.sendCommand(command)
                XCTFail("“\(command)” went out without a confirmation")
            } catch let error as NcSenderError {
                guard case .confirmationRequired = error else {
                    return XCTFail("“\(command)” failed for the wrong reason: \(error)")
                }
            } catch { XCTFail("“\(command)” threw the wrong type") }
        }
        XCTAssertTrue(NcSenderMockProtocol.recorded.allSatisfy { $0.path != "/api/send-command" },
                      "nothing may reach the sender before it is confirmed")

        // Consent minted for one line does not authorise another.
        do {
            _ = try await client.sendCommand("$H", consent: .motion(confirming: "$J=G91 G21 X1 F300"))
            XCTFail("consent for a jog started a homing cycle")
        } catch let error as NcSenderError {
            guard case .confirmationRequired = error else { return XCTFail("wrong refusal: \(error)") }
        }

        // With its own consent it goes.
        _ = try await client.sendCommand("$H", consent: .motion(confirming: "$H"))
        XCTAssertEqual(NcSenderMockProtocol.recorded.filter { $0.path == "/api/send-command" }.count, 1)
        let body = try XCTUnwrap(NcSenderMockProtocol.recorded.last?.body)
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: body) as? [String: Any])
        XCTAssertEqual(json["command"] as? String, "$H")
        XCTAssertEqual((json["meta"] as? [String: Any])?["sourceId"] as? String, "vcad")
        XCTAssertNotNil(json["commandId"])
    }

    func testReadOnlyQueriesNeedNoConfirmationAndTheListIsClosed() async throws {
        NcSenderMockProtocol.route("/api/send-command", json: #"{"ok":true}"#)
        let client = NcSenderClient(baseURL: URL(string: "http://pika:8090")!, session: NcSenderMockProtocol.session())
        for query in ["$$", "$G", "$I", "$#", "?"] {
            _ = try await client.sendCommand(query)
        }
        XCTAssertEqual(NcSenderMockProtocol.recorded.filter { $0.path == "/api/send-command" }.count, 5)
        // Anything not on the list is treated as motion, not waved through.
        XCTAssertEqual(NcSenderCommand.classify("$RST=$"), .motion(why: "this line is not on the read-only list, so it is treated as one that can move the machine"))
    }

    func testSettingsWritesNeedTheSettingNameTypedBack() async throws {
        NcSenderMockProtocol.route("/api/send-command", json: #"{"ok":true}"#)
        let client = NcSenderClient(baseURL: URL(string: "http://pika:8090")!, session: NcSenderMockProtocol.session())
        XCTAssertEqual(NcSenderCommand.classify("$132=130"), .settingsWrite(setting: "$132"))

        // A plain motion confirmation is not enough for a settings write.
        do {
            _ = try await client.sendCommand("$132=130", consent: .motion(confirming: "$132=130"))
            XCTFail("a settings write went out on a motion confirmation")
        } catch let error as NcSenderError {
            guard case .confirmationRequired = error else { return XCTFail("wrong refusal: \(error)") }
        }
        // Typing the wrong setting's name is the mistake this catches: $132
        // was the one ncSender's wizard set to 100 on 2026-09-17.
        do {
            _ = try await client.sendCommand("$132=130", consent: .setting(confirming: "$132=130", typedName: "$131"))
            XCTFail("a mistyped setting name was accepted")
        } catch let error as NcSenderError {
            XCTAssertEqual(error, .settingNameMismatch(setting: "$132", typed: "$131"))
        }
        XCTAssertTrue(NcSenderMockProtocol.recorded.allSatisfy { $0.path != "/api/send-command" })

        _ = try await client.sendCommand("$132=130", consent: .setting(confirming: "$132=130", typedName: "$132"))
        XCTAssertEqual(NcSenderMockProtocol.recorded.filter { $0.path == "/api/send-command" }.count, 1)
    }

    // MARK: connection

    func testTestConnectionReportsVersionControllerAndProbeType() async throws {
        let sender = session()
        await sender.testConnection(nativeHost: "10.0.0.5", nativePort: "23")
        let probe = try XCTUnwrap(sender.probe)
        XCTAssertTrue(probe.reachable)
        XCTAssertEqual(probe.version, "1.4.2")
        XCTAssertEqual(probe.controllerAddress, "192.168.2.226:23")
        XCTAssertEqual(probe.probeType, "3d-probe")
        XCTAssertNil(probe.contention, "a different controller address must not warn")
    }

    func testTwoSendersOnOneControllerIsWarnedAbout() async throws {
        let sender = session()
        // The native sender's default host is the Anolex's telnet address —
        // the same session ncSender is holding.
        await sender.testConnection(nativeHost: "192.168.2.226", nativePort: "23")
        let warning = try XCTUnwrap(sender.probe?.contention)
        XCTAssertTrue(warning.contains("Only one sender may hold the controller"), warning)

        XCTAssertNil(NcSenderSession.contention(ncSenderHolds: "192.168.2.226:23",
                                                nativeHost: "192.168.2.99", nativePort: "23"))
        XCTAssertNil(NcSenderSession.contention(ncSenderHolds: nil, nativeHost: "192.168.2.226", nativePort: "23"))
        XCTAssertNotNil(NcSenderSession.contention(ncSenderHolds: "192.168.2.226",
                                                   nativeHost: "192.168.2.226", nativePort: "23"))
    }

    func testAddressParsing() throws {
        XCTAssertEqual(try NcSenderClient.url(from: "pika:8090").absoluteString, "http://pika:8090")
        XCTAssertEqual(try NcSenderClient.url(from: " http://pika:8090 ").absoluteString, "http://pika:8090")
        XCTAssertThrowsError(try NcSenderClient.url(from: "ftp://pika"))
        XCTAssertThrowsError(try NcSenderClient.url(from: ""))
    }

    func testHTTPFailuresCarryThePathAndTheStatus() async {
        NcSenderMockProtocol.route("/api/server-state", NcSenderMockProtocol.Reply(status: 502, body: "bad gateway"))
        let sender = session()
        await sender.pollOnce()
        let error = sender.lastError ?? ""
        XCTAssertTrue(error.contains("502") && error.contains("/api/server-state"), error)
        XCTAssertNil(sender.state)
    }

    // MARK: the filename

    func testFilenameIsDeterministicAndHasOneDot() {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC")!
        let day = calendar.date(from: DateComponents(year: 2026, month: 9, day: 17))!
        let name = NcSenderJobPlan.filename(document: "stator-outline.dxf", toolDiameter: 2, date: day, calendar: calendar)
        XCTAssertEqual(name, "stator-outline-d2-20260917.nc")
        XCTAssertEqual(name.filter { $0 == "." }.count, 1, "the extension is the only dot")
        XCTAssertEqual(NcSenderJobPlan.filename(document: "Stator Ring v2", toolDiameter: 3.175, date: day, calendar: calendar),
                       "stator-ring-v2-d3p175-20260917.nc")
        XCTAssertEqual(NcSenderJobPlan.filename(document: "", toolDiameter: 6, date: day, calendar: calendar),
                       "untitled-d6-20260917.nc")
        // Same inputs, same name: re-sending replaces rather than piles up.
        XCTAssertEqual(NcSenderJobPlan.filename(document: "stator-outline.dxf", toolDiameter: 2, date: day, calendar: calendar), name)
        XCTAssertNoThrow(try NcSenderClient.checkFilename(name))
    }

    // MARK: the log

    func testLogTailKeepsCommandsAlarmsAndProbes() async throws {
        let log = """
        2026-09-17 14:02:01 INFO  connected to 192.168.2.226:23
        2026-09-17 14:02:09 SENT  $H
        2026-09-17 14:03:11 SENT  G38.2 Z-25 F40
        2026-09-17 14:03:19 RECV  ALARM:5
        2026-09-17 14:03:19 INFO  websocket client joined
        2026-09-17 14:04:02 RECV  [PRB:0.000,0.000,-118.200:0]
        """
        NcSenderMockProtocol.route("/api/logs/2026-09-17.log",
                                   NcSenderMockProtocol.Reply(body: log, contentType: "text/plain"))
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = .current
        let sender = session()
        await sender.refreshLog(date: calendar.date(from: DateComponents(year: 2026, month: 9, day: 17))!)

        XCTAssertEqual(sender.logLines.count, 4, sender.logLines.map(\.text).joined(separator: " | "))
        XCTAssertEqual(sender.logLines.map(\.kind), [.command, .probe, .alarm, .probe])
        XCTAssertTrue(sender.logLines.contains { $0.text.contains("ALARM:5") })
        XCTAssertFalse(sender.logLines.contains { $0.text.contains("websocket") })
        // The tail keeps the last N, not the first.
        XCTAssertEqual(NcSenderLogFilter.tail(log, limit: 2).map(\.kind), [.alarm, .probe])
    }

    // MARK: tolerant decoding

    func testPositionsDecodeFromEveryShapeNcSenderMightUse() throws {
        let decoder = JSONDecoder()
        let expected = NcSenderVector(x: 113.9, y: -106.221, z: -118.2)
        for body in ["[113.900,-106.221,-118.200]",
                     #"{"x":113.900,"y":-106.221,"z":-118.200}"#,
                     #""113.900,-106.221,-118.200""#] {
            XCTAssertEqual(try decoder.decode(NcSenderVector.self, from: Data(body.utf8)), expected, body)
        }
        // A shape nobody can read is an error, never a zeroed position — a
        // silent 0,0,0 would place the job at the machine origin.
        XCTAssertThrowsError(try decoder.decode(NcSenderVector.self, from: Data("[1,2]".utf8)))
        XCTAssertThrowsError(try decoder.decode(NcSenderVector.self, from: Data(#"{"a":1}"#.utf8)))
    }

    func testFirmwareSettingsAreFoundWhateverShapeTheyArriveIn() throws {
        let decoder = JSONDecoder()
        for body in [#"{"settings":{"$130":"400","$131":300,"$132":"130"}}"#,
                     #"{"$130":400,"$131":300,"$132":130}"#,
                     #"{"settings":[{"key":"$130","value":"400"},{"key":"$131","value":"300"},{"key":"$132","value":130}]}"#] {
            let firmware = try decoder.decode(NcSenderFirmware.self, from: Data(body.utf8))
            XCTAssertEqual(firmware.maxTravel, [400, 300, 130], body)
        }
        // A shape with no settings in it yields none, and "none" blocks the
        // envelope check rather than reading as zero travel.
        let empty = try decoder.decode(NcSenderFirmware.self, from: Data(#"{"note":"nothing here"}"#.utf8))
        XCTAssertNil(empty.maxTravel)
    }
}
