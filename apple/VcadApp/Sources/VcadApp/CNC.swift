import Foundation
import Observation
import Network

struct CNCVector: Codable, Equatable, Sendable {
    var x: Double = 0
    var y: Double = 0
    var z: Double = 0
    static func parse(_ text: Substring, scale: Double = 1) -> Self? {
        let v = text.split(separator: ",", omittingEmptySubsequences: false).prefix(3).compactMap { Double($0) }
        guard v.count == 3, v.prefix(3).allSatisfy(\.isFinite) else { return nil }
        return .init(x: v[0] * scale, y: v[1] * scale, z: v[2] * scale)
    }
    static func - (a: Self, b: Self) -> Self { .init(x: a.x-b.x, y: a.y-b.y, z: a.z-b.z) }
    static func + (a: Self, b: Self) -> Self { .init(x: a.x+b.x, y: a.y+b.y, z: a.z+b.z) }
    var floats: SIMD3<Float> { .init(Float(x), Float(y), Float(z)) }
}

struct CNCStatus: Equatable, Sendable {
    var state = "Disconnected"
    var machine: CNCVector?
    var work: CNCVector?
    var offset: CNCVector?
    var feed: Double = 0
    var rpm: Double = 0
    var feedOverride: Int?
    var spindleOverride: Int?
    var flood = false
    var mist = false
    var probeTriggered = false
    var received: Date?
    var isFresh: Bool { received.map { Date().timeIntervalSince($0) < 2 } ?? false }

    mutating func ingest(_ line: String, scale: Double, now: Date = Date()) -> Bool {
        guard line.hasPrefix("<"), line.hasSuffix(">") else { return false }
        let fields = line.dropFirst().dropLast().split(separator: "|")
        guard let first = fields.first else { return false }
        let state = String(first)
        guard ["Idle", "Run", "Hold", "Jog", "Alarm", "Door", "Check", "Home", "Sleep"].contains(state.components(separatedBy: ":")[0]) else { return false }
        var m: CNCVector?, w: CNCVector?
        var newOffset = offset
        var newFeed = feed, newRPM = rpm
        var newFeedOverride = feedOverride, newSpindleOverride = spindleOverride
        var newFlood = false, newMist = false, newProbe = false
        for field in fields.dropFirst() {
            let p = field.split(separator: ":", maxSplits: 1)
            guard p.count == 2 else { continue }
            switch p[0] {
            case "MPos": guard let v = CNCVector.parse(p[1], scale: scale) else { return false }; m = v
            case "WPos": guard let v = CNCVector.parse(p[1], scale: scale) else { return false }; w = v
            case "WCO": guard let v = CNCVector.parse(p[1], scale: scale) else { return false }; newOffset = v
            case "FS":
                let v = p[1].split(separator: ",").compactMap { Double($0) }
                guard v.count == 2, v.allSatisfy(\.isFinite) else { return false }
                newFeed = v[0] * scale; newRPM = v[1]
            case "Ov":
                let v = p[1].split(separator: ",").compactMap { Int($0) }
                guard v.count == 3, v.allSatisfy({ (0...200).contains($0) }) else { return false }
                newFeedOverride = v[0]; newSpindleOverride = v[2]
            case "A": newFlood = p[1].contains("F"); newMist = p[1].contains("M")
            case "Pn": newProbe = p[1].contains("P")
            case "F": if let v = Double(p[1]), v.isFinite { newFeed = v * scale }
            default: break
            }
        }
        // Never retain an old position as if it came from a new report.
        offset = newOffset; feed = newFeed; rpm = newRPM
        feedOverride = newFeedOverride; spindleOverride = newSpindleOverride
        flood = newFlood; mist = newMist; probeTriggered = newProbe
        machine = m ?? w.flatMap { w in offset.map { w + $0 } }
        work = w ?? m.flatMap { m in offset.map { m - $0 } }
        self.state = state; received = now
        return true
    }
}

/// Grbl text is a byte stream: packets may contain partial or many reports.
struct CNCFramer {
    private var bytes = Data()
    mutating func append(_ data: Data) throws -> [String] {
        bytes.append(data)
        var lines: [String] = []
        while let end = bytes.firstIndex(of: 10) {
            let line = bytes[..<end]
            guard line.count <= 8192, let text = String(data: line, encoding: .utf8) else {
                throw CNCError.message("Invalid controller response")
            }
            lines.append(text.trimmingCharacters(in: .whitespacesAndNewlines))
            bytes.removeSubrange(...end)
        }
        guard bytes.count <= 8192 else { throw CNCError.message("Controller response exceeded 8 KB") }
        return lines
    }
}

enum CNCError: LocalizedError {
    case message(String)
    var errorDescription: String? { if case .message(let s) = self { return s }; return nil }
}

/// One outstanding line at a time. `ok` means accepted, never finished cutting.
struct CNCStream {
    enum Phase: String { case ready, streaming, draining, complete, failed }
    private(set) var lines: [String] = []
    private(set) var acknowledged = 0
    private(set) var awaiting = false
    private(set) var phase: Phase = .ready
    mutating func begin(_ code: String) throws {
        let lines = try CNCCommands.programLines(code)
        self.lines = lines; acknowledged = 0; awaiting = false; phase = .streaming
    }
    mutating func next() -> String? {
        guard phase == .streaming, !awaiting, acknowledged < lines.count else { return nil }
        awaiting = true; return lines[acknowledged]
    }
    mutating func ack() {
        guard phase == .streaming, awaiting else { return }
        awaiting = false; acknowledged += 1
        if acknowledged == lines.count { phase = .draining }
    }
    mutating func status(_ state: String) {
        if phase == .draining && state == "Idle" { phase = .complete }
    }
    mutating func fail() { phase = .failed; awaiting = false }
}

@MainActor @Observable
final class CNCController {
    var host = "192.168.2.226"
    var port = "23"
    private(set) var connected = false
    private(set) var connecting = false
    private(set) var status = CNCStatus()
    private(set) var stream = CNCStream()
    private(set) var error: String?
    private(set) var log: [String] = []
    private(set) var firmware = "Grbl_ESP32 1.3a · 20211103 (configured)"
    private(set) var initialized = false
    private(set) var held = false
    private(set) var faulted = false
    private(set) var demo = false
    private(set) var tick = 0
    private var connection: NWConnection?
    private var poll: Task<Void, Never>?
    private var generation = UUID()
    private var framer = CNCFramer()
    private var pending: String?
    private var startup: [String] = []
    private var reportScale: Double?
    private var parserSeen = false
    private(set) var workspace = "Unknown"
    private(set) var probePosition: CNCVector?
    private(set) var jobStarted: Date?
    private(set) var jobFinished: Date?
    private var probeWorkspace: String?
    private var probeRequested = false
    private var pendingSince = Date()
    private var manualQueue: [String] = []
    private var demoOffset = CNCVector()
    private var demoOffsets: [String: CNCVector] = [:]
    private var demoFeedOverride = 100
    private var demoSpindleOverride = 100
    private var demoFlood = false
    private var demoMist = false
    private var demoRPM = 0.0
    private var demoAbsolute = true
    private var demoScale = 1.0
    private(set) var parkPosition: CNCVector?
    private var overrideTask: Task<Void, Never>?
    private(set) var changingOverride = false
    private var versionSeen = false
    private var startedAt = Date()
    private var demoPosition = CNCVector()
    private var commandBarrier = Date.distantPast
    private var resuming = false

    var active: Bool { stream.phase == .streaming || stream.phase == .draining }
    var canCommand: Bool { connected && initialized && !faulted && !active && pending == nil && status.isFresh && status.state == "Idle" && (status.received ?? .distantPast) > commandBarrier }
    var canHome: Bool { connected && initialized && !faulted && !active && pending == nil && status.isFresh && ["Idle", "Alarm"].contains(status.state) }
    var g54Active: Bool { workspace == "G54" }
    var canOverride: Bool { connected && initialized && !faulted && status.isFresh && ["Idle", "Run", "Hold"].contains(status.state.components(separatedBy: ":")[0]) }
    var canApplyProbe: Bool { canCommand && probePosition != nil && probeWorkspace == workspace && status.machine != nil }
    var elapsed: TimeInterval { jobStarted.map { (jobFinished ?? Date()).timeIntervalSince($0) } ?? 0 }
    var canStart: Bool { canCommand && g54Active && status.work != nil }
    var summary: String {
        _ = tick // Refresh time-based freshness even when reports stop.
        if connecting { return "Connecting…" }
        if !connected { return "Disconnected" }
        if !status.isFresh { return "Telemetry stale" }
        return (demo ? "Simulator · " : "") + status.state
    }

    func connect(simulated: Bool = false) {
        disconnect()
        guard simulated || (!host.trimmingCharacters(in: .whitespaces).isEmpty && UInt16(port).map { $0 > 0 } == true) else {
            error = "Enter a host and TCP port (1–65535)"; return
        }
        error = nil; faulted = false; demo = simulated; startedAt = Date()
        connecting = true
        let token = generation
        if simulated {
            connected = true; connecting = false; initialized = true
            reportScale = 1; workspace = "G54"; demoPosition = .init(); demoOffset = .init(); demoOffsets = [:]
            demoFeedOverride = 100; demoSpindleOverride = 100; demoFlood = false; demoMist = false; demoRPM = 0; demoAbsolute = true; demoScale = 1
            appendLog("Simulator connected · no hardware commands will be sent")
            reportDemo()
        } else {
            let c = NWConnection(host: NWEndpoint.Host(host), port: NWEndpoint.Port(rawValue: UInt16(port)!)!, using: .tcp)
            connection = c
            c.stateUpdateHandler = { [weak self] state in
                Task { @MainActor [weak self] in
                    guard let self, self.generation == token else { return }
                    switch state {
                    case .ready:
                        self.connected = true; self.connecting = false
                        self.startup = ["$I", "$$", "$G"]
                        self.receive(token)
                        self.sendStartup()
                    case .failed(let e): self.failConnection(e.localizedDescription)
                    case .waiting(let e): self.error = e.localizedDescription
                    default: break
                    }
                }
            }
            c.start(queue: .main)
        }
        poll = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(200))
                guard !Task.isCancelled, let self, self.generation == token else { break }
                self.tick += 1
                if self.connecting && Date().timeIntervalSince(self.startedAt) > 8 {
                    self.failConnection("Connection timed out"); break
                }
                if self.connected {
                    if self.demo { self.reportDemo() } else { self.sendRaw("?") }
                    if self.pending != nil && !self.held && Date().timeIntervalSince(self.pendingSince) > 120 {
                        self.sendRaw("!"); self.abort("Command acknowledgement timed out; reconnect to resynchronize."); break
                    }
                    if !self.initialized && Date().timeIntervalSince(self.startedAt) > 10 {
                        self.failConnection("Controller handshake timed out; check Telnet and Grbl settings"); break
                    }
                    if self.initialized && !self.status.isFresh && Date().timeIntervalSince(self.startedAt) > 3 && !self.faulted {
                        self.sendRaw("!")
                        self.abort("Telemetry lost. Feed hold requested; reconnect to resynchronize.")
                    }
                }
            }
        }
    }

    func disconnect() {
        // Stop feeding the planner before closing the transport. A lost network
        // cannot guarantee delivery of a hold; never automatically resume.
        if active { sendRaw("!") }
        generation = UUID(); poll?.cancel(); poll = nil; overrideTask?.cancel(); overrideTask = nil; changingOverride = false
        connection?.cancel(); connection = nil
        connecting = false; connected = false; initialized = false
        status = CNCStatus(); framer = CNCFramer(); startup = []; pending = nil
        reportScale = nil; parserSeen = false; versionSeen = false; workspace = "Unknown"; manualQueue = []; probePosition = nil; probeWorkspace = nil; probeRequested = false; parkPosition = nil
        held = false; resuming = false; demo = false; commandBarrier = .distantPast
        if active { stream.fail(); jobFinished = Date() }
        tick += 1
    }

    private func failConnection(_ message: String) {
        disconnect(); faulted = true; error = message
        if stream.phase != .ready && stream.phase != .complete { stream.fail() }
    }
    private func abort(_ message: String) {
        error = message; faulted = true; stream.fail(); startup = []; pending = nil; manualQueue = []; jobFinished = Date(); probePosition = nil; probeRequested = false
        tick += 1
    }
    private func receive(_ token: UUID) {
        connection?.receive(minimumIncompleteLength: 1, maximumLength: 8192) { [weak self] data, _, complete, e in
            Task { @MainActor [weak self] in
                guard let self, self.generation == token else { return }
                if let data {
                    do { for line in try self.framer.append(data) { self.ingest(line) } }
                    catch { self.failConnection(error.localizedDescription); return }
                }
                if let e { self.failConnection(e.localizedDescription); return }
                if complete { self.failConnection("Controller disconnected; job will not resume automatically"); return }
                self.receive(token)
            }
        }
    }
    private func sendRaw(_ text: String) { sendBytes(Data(text.utf8)) }
    private func sendBytes(_ bytes: Data) {
        guard connected, !demo else { return }
        let token = generation
        connection?.send(content: bytes, completion: .contentProcessed { [weak self] e in
            guard let e else { return }
            Task { @MainActor [weak self] in
                guard let self, self.generation == token else { return }
                self.failConnection(e.localizedDescription)
            }
        })
    }
    private func sendStartup() {
        guard !faulted else { return }
        if startup.isEmpty {
            initialized = reportScale != nil && parserSeen && versionSeen
            if !initialized { abort("Handshake incomplete: expected Grbl version, $13 report units and parser state") }
            sendRaw("?"); return
        }
        pending = startup.removeFirst(); pendingSince = Date(); sendRaw(pending! + "\n")
    }
    private func ingest(_ line: String) {
        guard !line.isEmpty else { return }
        if let scale = reportScale, line.hasPrefix("<") {
            if status.ingest(line, scale: scale) {
                tick += 1
                if status.state.hasPrefix("Alarm") || status.state.hasPrefix("Door") {
                    if active { abort("Controller reported \(status.state); job aborted") }
                } else if status.state.hasPrefix("Hold") {
                    held = true
                } else if !faulted {
                    if resuming && ["Run", "Idle"].contains(status.state) {
                        held = false; resuming = false; pump()
                    }
                    if !held && pending == nil {
                        stream.status(status.state)
                        if stream.phase == .complete && jobFinished == nil { jobFinished = Date() }
                    }
                }
            }
            return
        }
        appendLog(line)
        if line.hasPrefix("Grbl") && initialized {
            abort("Controller restarted; reconnect to resynchronize")
            status = CNCStatus(); return
        }
        if line.hasPrefix("[VER:") { firmware = line; versionSeen = true }
        if line.hasPrefix("$13=") {
            let value = line.dropFirst(4).split(separator: " ").first
            if value == "0" { reportScale = 1 }
            else if value == "1" { reportScale = 25.4 }
        }
        if line.hasPrefix("[GC:") {
            let words = line.dropFirst(4).replacingOccurrences(of: "]", with: "").split(separator: " ")
            parserSeen = true
            workspace = words.first(where: { CNCCommands.workspaces.contains(String($0)) }).map(String.init) ?? "Unknown"
        }
        if probeRequested, line.hasPrefix("[PRB:"), line.hasSuffix(":1]"), let scale = reportScale {
            let coordinates = line.dropFirst(5).dropLast(3)
            probePosition = CNCVector.parse(coordinates, scale: scale); probeWorkspace = workspace; probeRequested = false
        }
        if line.hasPrefix("error:") || line.hasPrefix("ALARM:") {
            sendRaw("!"); abort("\(line). Reconnect after resolving the controller error."); return
        }
        if line == "ok" && !faulted {
            if pending != nil {
                pending = nil
                commandBarrier = Date()
                if !initialized { sendStartup() }
                else if !manualQueue.isEmpty {
                    pending = manualQueue.removeFirst(); pendingSince = Date()
                    sendRaw(pending! + "\n")
                } else { sendRaw("?") }
            } else if active {
                stream.ack()
                if stream.phase == .draining {
                    pending = "G4 P0.01"; pendingSince = Date(); sendRaw("G4 P0.01\n")
                }
                pump()
            }
        }
    }
    func start(_ code: String) {
        guard canStart else { error = "Need a fresh Idle report, G54 and a known work offset"; return }
        do { try stream.begin(code); held = false; error = nil; jobStarted = Date(); jobFinished = nil; probePosition = nil; pump() }
        catch { self.error = error.localizedDescription }
    }
    private func pump() {
        guard !held, !faulted, status.isFresh, let line = stream.next() else { return }
        appendLog("→ " + line)
        if demo {
            // Deliberately separate simulation from live controller telemetry.
            let token = generation
            Task { [weak self] in
                try? await Task.sleep(for: .milliseconds(80))
                guard let self, self.generation == token, !self.faulted else { return }
                self.simulateCommand(line)
                self.stream.ack(); self.reportDemo(); self.pump()
            }
        } else { sendRaw(line + "\n") }
    }
    private func reportDemo() {
        let state = held ? "Hold:0" : (stream.phase == .streaming ? "Run" : "Idle")
        _ = status.ingest("<\(state)|MPos:\(demoPosition.x),\(demoPosition.y),\(demoPosition.z)|WCO:\(demoOffset.x),\(demoOffset.y),\(demoOffset.z)|FS:0,\(demoRPM)|Ov:\(demoFeedOverride),100,\(demoSpindleOverride)|A:\(demoFlood ? "F" : "")\(demoMist ? "M" : "")>", scale: 1)
        if !held { stream.status(state); if stream.phase == .complete && jobFinished == nil { jobFinished = Date() } }
        tick += 1
    }
    func hold() { guard connected else { return }; held = true; resuming = false; sendRaw("!"); tick += 1 }
    func resume() {
        guard connected, initialized, status.isFresh, !faulted, status.state == "Hold:0" else { return }
        if demo { held = false; reportDemo(); pump() }
        else { resuming = true; sendRaw("~") }
    }
    func reset() {
        guard connected else { return }
        sendBytes(Data([0x18]))
        abort("Soft reset requested. Reconnect before sending more commands.")
        status = CNCStatus()
    }
    func home() { probePosition = nil; command("$H", allowAlarm: true) }
    func selectG54() { selectWorkspace("G54") }
    func selectWorkspace(_ value: String) {
        guard CNCCommands.workspaces.contains(value) else { return }
        probePosition = nil; command(value)
    }
    func stopSpindle() { command("M5") }
    func zeroWork() { zero(axes: "XYZ") }
    func zero(axes: String) {
        guard let line = CNCCommands.zero(axes: axes, workspace: workspace) else { return }
        probePosition = nil; command(line)
    }
    func jog(axis: String, distance: Double, feed: Double) {
        guard ["X", "Y", "Z"].contains(axis) else { return }
        jog(x: axis == "X" ? distance : 0, y: axis == "Y" ? distance : 0, z: axis == "Z" ? distance : 0, feed: feed)
    }
    func jog(x: Double, y: Double, z: Double = 0, feed: Double) {
        guard let line = CNCCommands.jog(x: x, y: y, z: z, feed: feed) else { return }
        command(line)
    }
    func cancelJog() { guard connected else { return }; sendBytes(Data([0x85])) }
    func setOverride(spindle: Bool, percent: Int) {
        guard canOverride, !changingOverride, (10...200).contains(percent),
              (spindle ? status.spindleOverride : status.feedOverride) != nil else { return }
        appendLog("→ \(spindle ? "Spindle" : "Feed") override \(percent)%")
        if demo {
            if spindle { demoSpindleOverride = percent } else { demoFeedOverride = percent }
            reportDemo(); return
        }
        changingOverride = true
        let token = generation
        overrideTask = Task { [weak self] in
            guard let self else { return }
            defer { if self.generation == token { self.changingOverride = false } }
            // GRBL uses realtime flags, not a queue: never burst duplicate
            // override bytes. Wait for reported feedback before the next step.
            for _ in 0..<30 {
                guard !Task.isCancelled, self.canOverride,
                      let current = spindle ? self.status.spindleOverride : self.status.feedOverride,
                      let byte = CNCCommands.overrideByte(spindle: spindle, current: current, target: percent) else { return }
                self.sendBytes(Data([byte])); self.sendRaw("?")
                let deadline = Date().addingTimeInterval(2)
                while Date() < deadline {
                    try? await Task.sleep(for: .milliseconds(50))
                    guard !Task.isCancelled, self.canOverride else { return }
                    if (spindle ? self.status.spindleOverride : self.status.feedOverride) != current { break }
                }
                if (spindle ? self.status.spindleOverride : self.status.feedOverride) == current {
                    self.error = "Controller did not report the requested override."; return
                }
            }
        }
    }
    func setCoolant(flood: Bool, mist: Bool) {
        guard canCommand else { return }
        // M9 clears both outputs; serialize the requested combination afterwards.
        commandBatch(["M9"] + (flood ? ["M8"] : []) + (mist ? ["M7"] : []))
    }
    func sendMDI(_ line: String) {
        guard CNCCommands.validManual(line) else { error = "Enter one G-code line or a read-only query; settings changes require reconnecting."; return }
        probePosition = nil; command(line.uppercased())
    }
    func savePark() { guard canCommand, let position = status.machine else { return }; parkPosition = position }
    func park() {
        guard canCommand, let park = parkPosition, let position = status.machine else { return }
        let locale = Locale(identifier: "en_US_POSIX")
        commandBatch([
            String(format: "G21 G90 G53 G0 Z%.3f", locale: locale, max(position.z, park.z)),
            String(format: "G21 G90 G53 G0 X%.3f Y%.3f", locale: locale, park.x, park.y),
            String(format: "G21 G90 G53 G0 Z%.3f", locale: locale, park.z),
        ])
    }
    func returnToZero(xy: Bool, clearance: Double) {
        guard canCommand, let work = status.work, clearance.isFinite, (0...100).contains(clearance) else { return }
        if xy {
            commandBatch([String(format: "G21 G90 G0 Z%.3f", locale: Locale(identifier: "en_US_POSIX"), max(work.z, clearance)), "G21 G90 G0 X0 Y0"])
        } else { command("G21 G90 G1 Z0 F100") }
    }
    func clearLog() { log = [] }
    private func appendLog(_ line: String) {
        log.append(line); if log.count > 500 { log.removeFirst(log.count - 500) }
    }
    func probeZ(distance: Double, feed: Double) {
        guard canCommand, !status.probeTriggered, let work = status.work,
              distance.isFinite, (0.01...50).contains(distance), feed.isFinite, (1...500).contains(feed) else { return }
        probePosition = nil; probeWorkspace = workspace; probeRequested = true
        command(String(format: "G21 G90 G38.2 Z%.3f F%.1f", locale: Locale(identifier: "en_US_POSIX"), work.z - distance, feed))
    }
    func applyProbe(thickness: Double) {
        guard canApplyProbe, let probePosition, let position = status.machine,
              thickness.isFinite, (0...100).contains(thickness), let index = CNCCommands.workspaces.firstIndex(of: workspace) else { return }
        command(String(format: "G21 G10 L20 P%d Z%.3f", locale: Locale(identifier: "en_US_POSIX"), index + 1, thickness + position.z - probePosition.z))
        self.probePosition = nil
    }
    private func commandBatch(_ lines: [String]) {
        guard canCommand, let first = lines.first else { return }
        manualQueue = Array(lines.dropFirst())
        command(first)
    }
    private func command(_ line: String, allowAlarm: Bool = false) {
        guard (allowAlarm ? canHome : canCommand) else { return }
        if !line.contains("G38.2") { probePosition = nil; probeRequested = false }
        appendLog("→ " + line)
        if demo {
            simulateCommand(line)
            for queued in manualQueue { simulateCommand(queued) }
            manualQueue = []; reportDemo(); return
        }
        manualQueue.append("$G")
        pendingSince = Date()
        pending = line
        commandBarrier = Date()
        if !line.hasPrefix("$") { status.offset = nil; status.work = nil }
        sendRaw(line + "\n")
    }
    private func simulateCommand(_ line: String) {
            let regex = try! NSRegularExpression(pattern: #"([A-Z])\s*([-+]?(?:\d+\.?\d*|\.\d+))"#)
            let words = regex.matches(in: line, range: NSRange(line.startIndex..., in: line)).map { match in
                (String(line[Range(match.range(at: 1), in: line)!]), Double(line[Range(match.range(at: 2), in: line)!])!)
            }
            for (letter, number) in words {
                if letter == "G" {
                    if number == 90 { demoAbsolute = true }
                    if number == 91 && !line.hasPrefix("$J=") { demoAbsolute = false }
                    if number == 20 { demoScale = 25.4 }
                    if number == 21 { demoScale = 1 }
                    let w = "G\(Int(number))"
                    if CNCCommands.workspaces.contains(w), workspace != w { demoOffsets[workspace] = demoOffset; workspace = w; demoOffset = demoOffsets[w] ?? .init() }
                }
                if letter == "S" { demoRPM = number }
            }
            if !line.hasPrefix("$") && !line.contains("G10") && !line.contains("G38.2") {
                for (letter, number) in words {
                    let value = number * demoScale
                    let machineMove = line.contains("G53")
                    switch letter {
                    case "X": demoPosition.x = demoAbsolute ? value + (machineMove ? 0 : demoOffset.x) : demoPosition.x + value
                    case "Y": demoPosition.y = demoAbsolute ? value + (machineMove ? 0 : demoOffset.y) : demoPosition.y + value
                    case "Z": demoPosition.z = demoAbsolute ? value + (machineMove ? 0 : demoOffset.z) : demoPosition.z + value
                    default: break
                    }
                }
            }
            if CNCCommands.workspaces.contains(line) { workspace = line }
            if line == "M5" { demoRPM = 0 }
            if line == "M9" { demoFlood = false; demoMist = false }
            if line == "M8" { demoFlood = true }
            if line == "M7" { demoMist = true }
            if line == "$H" { demoPosition = .init() }
            if line.contains("G10") {
                for word in line.split(separator: " ") {
                    guard let value = Double(word.dropFirst()) else { continue }
                    switch word.first {
                    case "X": demoOffset.x = demoPosition.x - value
                    case "Y": demoOffset.y = demoPosition.y - value
                    case "Z": demoOffset.z = demoPosition.z - value
                    default: break
                    }
                }
            }
            if line.contains("G38.2") {
                if let word = line.split(separator: " ").first(where: { $0.hasPrefix("Z") }), let z = Double(word.dropFirst()) {
                    demoPosition.z = z + demoOffset.z
                    probePosition = demoPosition; probeWorkspace = workspace; probeRequested = false
                    appendLog("[PRB:\(demoPosition.x),\(demoPosition.y),\(demoPosition.z):1] (simulated)")
                }
            }
            if line.hasPrefix("$J=") {
                for word in line.split(separator: " ") {
                    guard let value = Double(word.dropFirst()) else { continue }
                    switch word.first {
                    case "X": demoPosition.x += value
                    case "Y": demoPosition.y += value
                    case "Z": demoPosition.z += value
                    default: break
                    }
                }
            }
    }
}
