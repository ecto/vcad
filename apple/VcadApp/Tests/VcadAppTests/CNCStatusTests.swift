import XCTest
@testable import VcadApp

/// The status dump a script reads.
///
/// Friction-log item 31: the editor window is borderless, so it has no
/// accessibility window, no Window-menu entry and no title to query, and
/// verifying the app from outside meant guessing. These assert the shape
/// anything reading the dump will index into — not the values, which change
/// with the job, but the keys, which must not.
@MainActor
final class CNCStatusTests: XCTestCase {

    func testStatusDumpCarriesTheShapeAScriptReads() async throws {
        let model = EditorModel()
        model.workspace = .manufacture
        let cnc = model.cnc
        cnc.build()
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }

        let status = VcadStatus.snapshot(model)
        for key in ["at", "pid", "document", "workspace", "solve", "job", "machine", "ncSender"] {
            XCTAssertNotNil(status[key], "missing \(key)")
        }
        XCTAssertEqual(status["workspace"] as? String, "manufacture")

        let document = try XCTUnwrap(status["document"] as? [String: Any])
        XCTAssertNotNil(document["name"] as? String)

        let solve = try XCTUnwrap(status["solve"] as? [String: Any])
        XCTAssertNotNil(solve["solving"] as? Bool)

        let job = try XCTUnwrap(status["job"] as? [String: Any])
        XCTAssertEqual(job["current"] as? Bool, true)
        XCTAssertEqual(job["blocked"] as? Bool, !cnc.blockers.isEmpty)
        XCTAssertEqual((job["operations"] as? [Any])?.count, cnc.operations.count)
        XCTAssertNotNil(job["headline"] as? String)
        XCTAssertNotNil(job["runBlocker"], "present even when it is null")
        XCTAssertEqual(job["hasGcode"] as? Bool, cnc.jobCode != nil)

        let machine = try XCTUnwrap(status["machine"] as? [String: Any])
        XCTAssertEqual(machine["connected"] as? Bool, false)
        XCTAssertNil(machine["profile"],
                     "no $$ has been read, so there is no profile — and unknown is not 'fine'")

        let sender = try XCTUnwrap(status["ncSender"] as? [String: Any])
        XCTAssertNotNil(sender["url"] as? String)

        // It has to survive the encoder a script reads it with.
        XCTAssertTrue(JSONSerialization.isValidJSONObject(status))
        let url = FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-status-test-\(UUID().uuidString).json")
        defer { try? FileManager.default.removeItem(at: url) }
        XCTAssertEqual(VcadStatus.dump(model, to: url), url)
        let reread = try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        XCTAssertEqual(reread?["workspace"] as? String, "manufacture")
    }

    /// The machine's own state reaches the dump once there is one to report.
    func testAConnectedMachineReportsItsProfileAndAlarm() async throws {
        let model = EditorModel()
        let cnc = model.cnc
        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.machine.simulate(alarm: 1)

        let machine = try XCTUnwrap(VcadStatus.snapshot(model)["machine"] as? [String: Any])
        XCTAssertEqual(machine["connected"] as? Bool, true)
        XCTAssertEqual(machine["simulated"] as? Bool, true)
        let profile = try XCTUnwrap(machine["profile"] as? [String: Any])
        XCTAssertEqual((profile["travelMm"] as? [Double])?.count, 3)
        XCTAssertNotNil(profile["homed"] as? Bool)
        let alarm = try XCTUnwrap(machine["alarm"] as? [String: Any])
        XCTAssertEqual(alarm["code"] as? Int, 1)
    }

    /// The camera URL carries `user:pass@`. It is a credential: the dump says
    /// whether one is set and never what it is.
    func testTheCameraURLNeverReachesTheDump() throws {
        let model = EditorModel()
        model.cnc.camera.setURL("rtsps://operator:hunter2@nvr.local/stream")
        defer { model.cnc.camera.setURL("") }
        let status = VcadStatus.snapshot(model)
        let text = String(decoding: try JSONSerialization.data(withJSONObject: status), as: UTF8.self)
        XCTAssertFalse(text.contains("hunter2"), "the camera's credentials are in the dump")
        XCTAssertFalse(text.contains("nvr.local"), "so is its address")
        let sender = try XCTUnwrap(status["ncSender"] as? [String: Any])
        XCTAssertEqual(sender["cameraConfigured"] as? Bool, true, "…but that there is one is worth saying")
    }

    /// `VCAD_STATUS` names the file; without it the dump carries the pid, so
    /// two instances cannot overwrite each other's.
    func testStatusDestinationFollowsTheEnvironment() {
        XCTAssertEqual(VcadStatus.destination(environment: ["VCAD_STATUS": "/tmp/here.json"], pid: 7).path,
                       "/tmp/here.json")
        XCTAssertTrue(VcadStatus.destination(environment: [:], pid: 7).lastPathComponent.contains("7"))
        XCTAssertTrue(VcadStatus.destination(environment: ["VCAD_STATUS": "  "], pid: 7)
                        .lastPathComponent.contains("7"), "a blank setting is not a path")
    }
}
