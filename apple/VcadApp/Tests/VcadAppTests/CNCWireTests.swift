import XCTest
@testable import VcadApp

@MainActor
final class CNCWireTests: XCTestCase {
    func testManufactureCommandsUseSerializedWireAndReportedOverrides() async throws {
        let server = Process()
        server.executableURL = URL(fileURLWithPath: "/usr/bin/python3")
        server.arguments = ["-u", "-c", #"""
import socket
s=socket.socket(); s.bind(('127.0.0.1',0)); s.listen(1)
print(s.getsockname()[1],flush=True)
c,_=s.accept(); buf=b''; workspace=b'G54'; feed=100; spindle=100
def reply(v): c.sendall(v+b'\n')
def status(): reply(b'<Idle|MPos:1,2,3|WCO:0,0,0|FS:0,0|Ov:'+str(feed).encode()+b',100,'+str(spindle).encode()+b'>')
while True:
 data=c.recv(4096)
 if not data: break
 for b in data:
  if b==63: status()
  elif 0x90<=b<=0x94:
   feed=100 if b==0x90 else feed+{0x91:10,0x92:-10,0x93:1,0x94:-1}[b]
   reply(b'[MSG:feed-byte '+str(b).encode()+b']'); status()
  elif 0x99<=b<=0x9d:
   spindle=100 if b==0x99 else spindle+{0x9a:10,0x9b:-10,0x9c:1,0x9d:-1}[b]
   status()
  elif b==10:
   line,buf=buf,b''
   reply(b'[MSG:line '+line+b']')
   if line in [b'G54',b'G55',b'G56']: workspace=line
   if line==b'$I': reply(b'[VER:1.3a:Grbl_ESP32]')
   if line==b'$$': reply(b'$13=0')
   if line==b'$G': reply(b'[GC:G0 '+workspace+b' G17 G21 G90 G94 M5 M9 T0 F0 S0]')
   if line==b'$#': reply(b'[PRB:1,2,3:1]')
   if b'G38.2' in line: reply(b'[PRB:1,2,1:1]')
   reply(b'ok')
  elif b<128: buf+=bytes([b])
"""#]
        let output = Pipe(); server.standardOutput = output
        try server.run()
        defer { if server.isRunning { server.terminate() } }
        let port = try XCTUnwrap(String(data: output.fileHandleForReading.availableData, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines))
        let machine = CNCController(); machine.host = "127.0.0.1"; machine.port = port
        machine.connect(); defer { machine.disconnect() }
        await wait { machine.canStart }
        machine.setOverride(spindle: false, percent: 132)
        await wait { !machine.changingOverride }
        XCTAssertEqual(machine.status.feedOverride, 132)
        XCTAssertEqual(machine.log.filter { $0 == "[MSG:feed-byte 145]" }.count, 3)
        XCTAssertEqual(machine.log.filter { $0 == "[MSG:feed-byte 147]" }.count, 2)
        machine.setOverride(spindle: true, percent: 90)
        await wait { !machine.changingOverride }
        XCTAssertEqual(machine.status.spindleOverride, 90)
        machine.selectWorkspace("G55")
        await wait { machine.canCommand && machine.workspace == "G55" }
        XCTAssertFalse(machine.canStart)
        machine.zero(axes: "XY")
        await wait { machine.canCommand }
        XCTAssertTrue(machine.log.contains("[MSG:line G10 L20 P2 X0 Y0]"))
        machine.sendMDI("$#")
        await wait { machine.canCommand }
        XCTAssertNil(machine.probePosition, "Historical PRB query must not authorize zeroing")
        machine.probeZ(distance: 2, feed: 50)
        await wait { machine.canApplyProbe }
        XCTAssertEqual(machine.probePosition?.z, 1)
        machine.applyProbe(thickness: 1)
        await wait { machine.canCommand }
        XCTAssertTrue(machine.log.contains("[MSG:line G21 G10 L20 P2 Z3.000]"))
    }
    private func wait(_ condition: () -> Bool) async {
        let deadline = Date().addingTimeInterval(5)
        while !condition() && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(condition())
    }
}
