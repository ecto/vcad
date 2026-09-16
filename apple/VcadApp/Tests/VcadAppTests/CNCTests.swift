import XCTest
import RealityKit
@testable import VcadApp

@MainActor
final class CNCTests: XCTestCase {
    func testFragmentedReportsAndMultipleLines() throws {
        var framer = CNCFramer()
        XCTAssertEqual(try framer.append(Data("<Idle|MPos:1,".utf8)), [])
        XCTAssertEqual(try framer.append(Data("2,3>\r\nok\nerr".utf8)), ["<Idle|MPos:1,2,3>", "ok"])
        XCTAssertEqual(try framer.append(Data("or:20\n".utf8)), ["error:20"])
        XCTAssertThrowsError(try framer.append(Data(repeating: 65, count: 8193)))
    }
    func testCoordinateFramesAndIntermittentOffset() {
        var status = CNCStatus()
        XCTAssertTrue(status.ingest("<Idle|MPos:11,22,33|WCO:10,20,30|FS:400,10000>", scale: 1))
        XCTAssertEqual(status.work, .init(x: 1, y: 2, z: 3))
        XCTAssertTrue(status.ingest("<Run|MPos:12,24,36>", scale: 1))
        XCTAssertEqual(status.work, .init(x: 2, y: 4, z: 6))
        XCTAssertTrue(status.ingest("<Hold:0|WPos:1,2,3>", scale: 1))
        XCTAssertEqual(status.machine, .init(x: 11, y: 22, z: 33))
        XCTAssertTrue(status.ingest("<Idle>", scale: 1))
        XCTAssertNil(status.work)
        XCTAssertNil(status.machine)
    }
    func testInchReportsUnknownOffsetAndStaleData() {
        var status = CNCStatus()
        XCTAssertTrue(status.ingest("<Idle|MPos:1,2,3|FS:10,12000>", scale: 25.4, now: Date().addingTimeInterval(-3)))
        XCTAssertEqual(status.machine!.x, 25.4, accuracy: 0.00001)
        XCTAssertEqual(status.feed, 254, accuracy: 0.00001)
        XCTAssertEqual(status.rpm, 12000)
        XCTAssertNil(status.work)
        XCTAssertFalse(status.isFresh)
        XCTAssertFalse(status.ingest("<Run|MPos:nan,2,3>", scale: 1))
        XCTAssertFalse(status.ingest("<Surprise|MPos:1,2,3>", scale: 1))
    }
    func testStreamWaitsForAcknowledgementAndFinalIdle() throws {
        var stream = CNCStream()
        try stream.begin("(test)\nG21\nG0 Z5\nM2\n")
        XCTAssertEqual(stream.next(), "G21")
        XCTAssertNil(stream.next())
        stream.status("Idle")
        XCTAssertEqual(stream.phase, .streaming)
        stream.ack()
        XCTAssertEqual(stream.next(), "G0 Z5")
        stream.ack()
        XCTAssertEqual(stream.next(), "M2")
        stream.ack()
        XCTAssertEqual(stream.phase, .draining)
        stream.status("Run")
        XCTAssertEqual(stream.phase, .draining)
        stream.status("Idle")
        XCTAssertEqual(stream.phase, .complete)
    }
    func testAbortCannotBeResumedByLateAcknowledgement() throws {
        var stream = CNCStream()
        try stream.begin("G21\nM2\n")
        _ = stream.next(); stream.fail(); stream.ack(); stream.status("Idle")
        XCTAssertNil(stream.next())
        XCTAssertEqual(stream.phase, .failed)
        XCTAssertEqual(stream.acknowledged, 0)
        XCTAssertThrowsError(try stream.begin("!\u{18}\n"))
    }
    private func waitFor(_ description: String, timeout: Double = 6, _ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(condition(), description)
    }
    func testCAMBridgeAndOverlayUseGeneratedSetup() async throws {
        let workspace = CNCWorkspace()
        workspace.generate()
        await waitFor("CAM completes") { !workspace.generating }
        XCTAssertNil(workspace.error)
        XCTAssertTrue(workspace.current)
        XCTAssertTrue(workspace.program!.gcode.contains("G94"))
        let parent = Entity()
        workspace.machine.connect(simulated: true)
        defer { workspace.machine.disconnect() }
        syncCNCOverlay(workspace, in: parent)
        let root = try XCTUnwrap(parent.findEntity(named: "cncRoot"))
        XCTAssertTrue(root.isEnabled)
        XCTAssertNotNil(root.findEntity(named: "cncTool"))
        workspace.setup.width += 10
        XCTAssertFalse(workspace.current)
        workspace.overlay = false
        syncCNCOverlay(workspace, in: parent)
        XCTAssertFalse(root.isEnabled)
    }
    func testSimulatorHoldsAndDisconnectNeverResumes() async {
        let machine = CNCController()
        machine.connect(simulated: true)
        defer { machine.disconnect() }
        XCTAssertTrue(machine.canStart)
        machine.start("G21\nG0 X1 Y2 Z3\nM2\n")
        machine.hold()
        await waitFor("simulator held") { machine.status.state == "Hold:0" }
        XCTAssertLessThan(machine.stream.acknowledged, machine.stream.lines.count)
        machine.resume()
        await waitFor("simulator completes") { machine.stream.phase == .complete }
        XCTAssertEqual(machine.status.work, .init(x: 1, y: 2, z: 3))
        machine.start("G21\nG0 X9\nM2\n")
        machine.disconnect()
        try? await Task.sleep(for: .milliseconds(200))
        XCTAssertEqual(machine.stream.phase, .failed)
        XCTAssertFalse(machine.connected)
    }

    /// A real TCP peer, intentionally splitting replies across writes. It only
    /// binds loopback; no tests ever contact the configured physical machine.
    private func mockPeer() throws -> (Process, String) {
        let server = Process()
        server.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
        server.arguments = ["-u", "-c", #"""
import socket, time
s = socket.socket()
s.bind(('127.0.0.1', 0)); s.listen(1)
print(s.getsockname()[1], flush=True)
c, _ = s.accept()
buf = b''
muted = False
def reply(v):
    c.sendall(v[:3]); time.sleep(.002); c.sendall(v[3:])
while True:
    data = c.recv(4096)
    if not data: break
    for b in data:
        if b == 63 and not muted:
            reply(b'<Idle|MPos:11,22,33|WCO:10,20,30|FS:0,0>\r\n')
        elif b == 33:
            reply(b'[MSG:hold requested]\n')
        elif b == 0x85:
            reply(b'[MSG:raw jog cancel]\r\n')
        elif b == 10:
            line, buf = buf, b''
            reply(b'[MSG:line ' + line + b']\n')
            if line == b'G0 X999':
                reply(b'error:20\n'); continue
            if line == b'G0 X888': muted = True
            if line == b'G0 X777': c.close(); raise SystemExit(0)
            if line == b'$I': reply(b'[VER:1.3a.20211103:Grbl_ESP32]\n')
            if line == b'$$': reply(b'$13=0\n')
            if line == b'$G': reply(b'[GC:G0 G54 G17 G21 G90 G94 M5 M9 T0 F0 S0]\n')
            reply(b'ok\n')
        else: buf += bytes([b])
"""#]
        let output = Pipe(); server.standardOutput = output
        try server.run()
        let portData = output.fileHandleForReading.availableData
        let port = try XCTUnwrap(String(data: portData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines))
        return (server, port)
    }
    func testTCPHandshakeStreamingAndRawJogCancel() async throws {
        let (server, port) = try mockPeer()
        defer { if server.isRunning { server.terminate() } }
        let machine = CNCController(); machine.host = "127.0.0.1"; machine.port = port
        machine.connect()
        defer { machine.disconnect() }
        await waitFor("TCP initialization: \(machine.error ?? "")") { machine.canStart }
        XCTAssertEqual(machine.status.work, .init(x: 1, y: 2, z: 3))
        machine.cancelJog()
        await waitFor("raw 0x85 byte reached controller") { machine.log.contains("[MSG:raw jog cancel]") }
        machine.start("G21\nG90\nG0 Z5\nM2\n")
        await waitFor("TCP job completes") { machine.stream.phase == .complete }
        XCTAssertEqual(machine.stream.acknowledged, 4)
        XCTAssertNil(machine.error)
    }
}

@MainActor
extension CNCTests {
    func testTCPErrorStopsSubsequentLines() async throws {
        let (server, port) = try mockPeer()
        defer { if server.isRunning { server.terminate() } }
        let machine = CNCController(); machine.host = "127.0.0.1"; machine.port = port
        machine.connect(); defer { machine.disconnect() }
        await waitFor("initialized") { machine.canStart }
        machine.start("G21\nG0 X999\nG0 X1000\nM2\n")
        await waitFor("error aborts job") { machine.faulted }
        XCTAssertEqual(machine.stream.phase, .failed)
        XCTAssertEqual(machine.stream.acknowledged, 1)
        XCTAssertFalse(machine.log.contains("[MSG:line G0 X1000]"))
        XCTAssertFalse(machine.canStart)
    }
    func testTCPStaleTelemetryRequestsHoldAndAborts() async throws {
        let (server, port) = try mockPeer()
        defer { if server.isRunning { server.terminate() } }
        let machine = CNCController(); machine.host = "127.0.0.1"; machine.port = port
        machine.connect(); defer { machine.disconnect() }
        await waitFor("initialized") { machine.canStart }
        machine.start("G21\nG0 X888\nM2\n")
        await waitFor("stale telemetry aborts") { machine.faulted }
        await waitFor("hold delivered") { machine.log.contains("[MSG:hold requested]") }
        XCTAssertEqual(machine.stream.phase, .failed)
        XCTAssertFalse(machine.status.isFresh)
    }
    func testTCPDisconnectDoesNotCompleteOrResumeJob() async throws {
        let (server, port) = try mockPeer()
        defer { if server.isRunning { server.terminate() } }
        let machine = CNCController(); machine.host = "127.0.0.1"; machine.port = port
        machine.connect(); defer { machine.disconnect() }
        await waitFor("initialized") { machine.canStart }
        machine.start("G21\nG0 X777\nM2\n")
        await waitFor("disconnect aborts") { machine.faulted }
        XCTAssertEqual(machine.stream.phase, .failed)
        XCTAssertFalse(machine.connected)
        XCTAssertNil(machine.status.work)
    }
}
