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

    // MARK: the machine itself

    /// Every `$$` setting the controller has reported this session.
    private(set) var settings = CNCMachineSettings()
    /// Whether a homing cycle has completed since this connection opened.
    /// Grbl never reports it, and anything that loses the position drops it.
    private(set) var homed = false
    /// The alarm the controller is sitting in, if any.
    private(set) var alarm: CNCAlarm?
    /// The stock rotation measured by the two-point edge probe, degrees CCW.
    private(set) var skewDegrees: Double?
    /// The saved `$$` snapshot this machine is compared against.
    private(set) var baseline: CNCMachineBaseline?
    /// Settings that differ from that baseline, recomputed as `$$` arrives.
    private(set) var baselineDiff: [CNCSettingDelta] = []
    /// Walking the job's bounding rectangle before the cut.
    let trace = CNCTrace()
    private var simulatedProbeContact: CNCVector?
    private var simulatedProbeMisses = false

    /// Which machine the baseline belongs to. The simulator borrows the
    /// configured host's baseline, so a diff can be exercised without hardware.
    var baselineMachine: String { host.trimmingCharacters(in: .whitespaces) }

    /// What the app knows about the machine under the job, or nothing at all
    /// before `$$` has been read. Consumers must treat `nil` as unknown rather
    /// than as "fine": no profile means nothing has checked the travel.
    var profile: CNCMachineProfile? {
        guard connected, !settings.isEmpty else { return nil }
        var profile = CNCMachineProfile(settings: settings)
        profile.homed = homed
        profile.alarm = alarm
        profile.skewDegrees = skewDegrees
        profile.workspace = workspace
        profile.workOffset = status.isFresh ? status.offset : nil
        profile.baselineName = baseline?.name
        profile.baselineDiff = baselineDiff
        profile.simulated = demo
        return profile
    }

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
        baseline = CNCMachineBaselineStore.load(machine: baselineMachine)
        let token = generation
        if simulated {
            connected = true; connecting = false; initialized = true
            reportScale = 1; workspace = "G54"; demoPosition = .init(); demoOffset = .init(); demoOffsets = [:]
            demoFeedOverride = 100; demoSpindleOverride = 100; demoFlood = false; demoMist = false; demoRPM = 0; demoAbsolute = true; demoScale = 1
            // The simulator answers `$$` like the machine does, so everything
            // downstream of the profile can be run without hardware.
            ingestSettings(CNCAnolexBaseline.dump)
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
        // A new connection knows nothing about where the machine is: homing,
        // the settings and a measured skew all belong to the session that saw
        // them. Keeping any of them would be inventing a machine state.
        settings = CNCMachineSettings(); baselineDiff = []; homed = false; alarm = nil; skewDegrees = nil
        simulatedProbeContact = nil; simulatedProbeMisses = false; trace.reset()
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
                    noteAlarm(CNCAlarm.parse(status.state))
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
        // Every `$$` setting is kept, not just the ones this app models: the
        // setting that ruins the day is the one nobody thought to parse.
        if settings.ingest(line) { refreshBaselineDiff() }
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
            let alarm = CNCAlarm.parse(line)
            noteAlarm(alarm)
            sendRaw("!")
            // An alarm already says what happened and what to do about it;
            // repeating the raw code and nothing else is what made "$132=100"
            // read as a mystery for an afternoon.
            abort((alarm?.text ?? "\(line).") + " Reconnect after resolving the controller error.")
            return
        }
        if line == "ok" && !faulted {
            if pending != nil {
                // Grbl answers `$H` with `ok` only once the cycle has finished,
                // so this is the one moment the machine position is known.
                if pending == "$H" { homed = true; alarm = nil }
                pending = nil
                commandBarrier = Date()
                if !initialized { sendStartup() }
                else if !manualQueue.isEmpty {
                    pending = manualQueue.removeFirst(); pendingSince = Date()
                    // Every line that reaches the wire is in the terminal: a
                    // batch whose later moves were invisible is a batch nobody
                    // can check afterwards.
                    appendLog("→ " + pending!)
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
        // An alarm outranks everything: the simulator has to be able to put the
        // app in the state a hard limit puts it in, or nothing downstream of an
        // alarm can be tested without hitting a real switch.
        // Grbl's status line says "Alarm" with no subcode — the number only
        // ever arrives in the pushed `ALARM:n`. Reporting "Alarm:1" here would
        // be a report no controller sends, and `canHome` would not match it.
        let state = alarm != nil ? "Alarm" : (held ? "Hold:0" : (stream.phase == .streaming ? "Run" : "Idle"))
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
    // MARK: - The machine's own settings, homing and alarms

    /// Take a whole `$$` listing at once (the simulator, and tests).
    func ingestSettings(_ dump: String) {
        for line in dump.components(separatedBy: .newlines) where settings.ingest(line) { continue }
        refreshBaselineDiff()
    }
    private func refreshBaselineDiff() {
        baselineDiff = baseline?.diff(against: settings) ?? []
    }
    private func noteAlarm(_ value: CNCAlarm?) {
        guard let value else { return }
        alarm = value
        // A hard limit or an abort loses the step count. Nothing on screen is
        // a machine coordinate any more until the machine is homed again.
        if value.positionLost { homed = false }
        tick += 1
    }
    /// Save what the controller is reporting now as this machine's baseline.
    func saveBaseline(name: String? = nil) {
        guard !settings.isEmpty else { return }
        let saved = CNCMachineBaseline(name: name ?? "\(baselineMachine) · \(Date().formatted(date: .abbreviated, time: .shortened))",
                                       settings: settings)
        CNCMachineBaselineStore.save(saved, machine: baselineMachine)
        baseline = saved
        refreshBaselineDiff()
        appendLog("Saved \(counted(settings.numbers.count, "setting")) as the baseline for \(baselineMachine)")
        tick += 1
    }

    // MARK: - Setup motion: trace, probe, edge finding

    /// Run a short sequence of setup moves as one batch. Always behind a
    /// confirmation in the UI; never used for the job itself.
    func runSetupMoves(_ lines: [String]) {
        guard canCommand, !lines.isEmpty else { return }
        commandBatch(lines)
    }
    /// The same, but the last line is a probe whose result must survive.
    func runProbeSequence(_ lines: [String]) {
        guard canCommand, !lines.isEmpty, lines.contains(where: { $0.contains("G38.2") }) else { return }
        probePosition = nil; probeWorkspace = workspace; probeRequested = true
        commandBatch(lines, probing: true)
    }
    /// The probe contact in work coordinates. `PRB:` is reported in machine
    /// coordinates, and every decision made from it is a work-frame one.
    var probeWorkPosition: CNCVector? {
        guard let probePosition, probeWorkspace == workspace, let offset = status.offset else { return nil }
        return probePosition - offset
    }
    /// Probe along one axis, toward `distance` millimetres from here.
    func probe(axis: String, distance: Double, feed: Double) {
        guard canCommand, !status.probeTriggered, let work = status.work,
              distance.isFinite, abs(distance) >= 0.01, abs(distance) <= 50,
              feed.isFinite, (1...500).contains(feed) else { return }
        let start = axis.uppercased() == "X" ? work.x : axis.uppercased() == "Y" ? work.y : work.z
        guard let line = CNCMachineCommands.probe(axis: axis, to: start + distance, feed: feed) else { return }
        runProbeSequence([line])
    }
    /// Set a work coordinate at the position the machine is standing at. This
    /// is the paper touch-off: lower until the slip drags, then say the tool
    /// is one paper thickness above zero.
    func setWork(axis: String, value: Double) {
        guard canCommand, let line = CNCMachineCommands.setWork([(axis, value)], workspace: workspace) else { return }
        command(line)
    }
    /// Set X0 (or Y0) from a probed edge, allowing for the cutter's radius.
    func applyEdgeZero(axis: String, target: Double, toolDiameter: Double, direction: Double) {
        guard canCommand, probeWorkPosition != nil,
              let value = CNCSkewProbe.edgeZero(target: target, toolDiameter: toolDiameter, direction: direction),
              let line = CNCMachineCommands.setWork([(axis, value)], workspace: workspace) else { return }
        // The machine is standing where it touched, so the work coordinate set
        // here is the contact's — and the edge lands one radius beyond it.
        command(line)
        probePosition = nil
    }
    func setSkew(_ degrees: Double?) {
        skewDegrees = degrees.flatMap { $0.isFinite && abs($0) <= 90 ? $0 : nil }
        tick += 1
    }

    // MARK: - Simulator hooks

    /// Give the simulator a different `$$` set — the point of the baseline
    /// diff is that a machine can come back configured differently.
    func simulate(settings dump: String) {
        guard demo else { return }
        settings = CNCMachineSettings()
        ingestSettings(dump)
    }
    /// Put the simulator into an alarm, as the controller would report it.
    func simulate(alarm code: Int) {
        guard demo, let value = CNCAlarm(rawValue: code) else { return }
        noteAlarm(value)
        error = value.text
        appendLog("ALARM:\(code) (simulated)")
        reportDemo()
    }
    /// Where the simulated probe trips, in work coordinates. `nil` restores
    /// "contact at the target"; `misses` makes the probe fail like ALARM:5.
    func simulateProbe(contact: CNCVector?, misses: Bool = false) {
        guard demo else { return }
        simulatedProbeContact = contact; simulatedProbeMisses = misses
    }
    /// Pretend the machine has been homed (or has not).
    func simulate(homed value: Bool) {
        guard demo else { return }
        homed = value; if value { alarm = nil }
        tick += 1
    }

    func applyProbe(thickness: Double) {
        guard canApplyProbe, let probePosition, let position = status.machine,
              thickness.isFinite, (0...100).contains(thickness), let index = CNCCommands.workspaces.firstIndex(of: workspace) else { return }
        command(String(format: "G21 G10 L20 P%d Z%.3f", locale: Locale(identifier: "en_US_POSIX"), index + 1, thickness + position.z - probePosition.z))
        self.probePosition = nil
    }
    private func commandBatch(_ lines: [String], probing: Bool = false) {
        guard canCommand, let first = lines.first else { return }
        manualQueue = Array(lines.dropFirst())
        command(first, probing: probing)
    }
    private func command(_ line: String, allowAlarm: Bool = false, probing: Bool = false) {
        guard (allowAlarm ? canHome : canCommand) else { return }
        if !probing && !line.contains("G38.2") { probePosition = nil; probeRequested = false }
        appendLog("→ " + line)
        if demo {
            simulateCommand(line)
            for queued in manualQueue { appendLog("→ " + queued); simulateCommand(queued) }
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
            if line == "$H" {
                // Homing leaves the machine at the pull-off from the switches,
                // not at machine zero — which is the difference between "Z is
                // at 0" and "Z is at −3 with 127 mm of travel below it".
                let homedProfile = CNCMachineProfile(settings: settings)
                demoPosition = homedProfile.hasTravel
                    ? CNCVector(x: homedProfile.travelX.homePosition,
                                y: homedProfile.travelY.homePosition,
                                z: homedProfile.travelZ.homePosition)
                    : .init()
                homed = true; alarm = nil
            }
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
            if line.contains("G38.2") { simulateProbeMove(line) }
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

    /// A simulated `G38.2`. The contact is wherever the test says the work
    /// surface is; with nothing said it is the target, which is the old
    /// behaviour. A probe that would not reach the surface fails the way the
    /// controller does — ALARM:5, nothing probed, no zero touched.
    private func simulateProbeMove(_ line: String) {
        let words = line.split(separator: " ")
        guard let word = words.first(where: { "XYZ".contains($0.prefix(1)) && Double($0.dropFirst()) != nil }),
              let target = Double(word.dropFirst()) else { return }
        let axis = String(word.prefix(1))
        let offset = component(demoOffset, axis)
        let start = component(demoPosition, axis) - offset
        var contact = target
        if simulatedProbeMisses {
            contact = target
        } else if let expected = simulatedProbeContact {
            let surface = component(expected, axis)
            let low = Swift.min(start, target), high = Swift.max(start, target)
            guard surface >= low - 1e-9, surface <= high + 1e-9 else {
                set(&demoPosition, axis, target + offset)
                probePosition = nil; probeRequested = false
                simulate(alarm: CNCAlarm.probeFailContact.rawValue)
                return
            }
            contact = surface
        }
        set(&demoPosition, axis, contact + offset)
        if simulatedProbeMisses {
            probePosition = nil; probeRequested = false
            simulate(alarm: CNCAlarm.probeFailContact.rawValue)
            return
        }
        probePosition = demoPosition; probeWorkspace = workspace; probeRequested = false
        appendLog("[PRB:\(demoPosition.x),\(demoPosition.y),\(demoPosition.z):1] (simulated)")
    }
    private func component(_ v: CNCVector, _ axis: String) -> Double {
        switch axis { case "X": return v.x; case "Y": return v.y; default: return v.z }
    }
    private func set(_ v: inout CNCVector, _ axis: String, _ value: Double) {
        switch axis { case "X": v.x = value; case "Y": v.y = value; default: v.z = value }
    }
}
