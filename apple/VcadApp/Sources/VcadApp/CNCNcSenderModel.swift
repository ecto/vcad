import Foundation
import Observation

// What the app knows about ncSender: the machine's travel, whether this job
// fits inside it, what an alarm code means, and the polled state behind the
// panel. Everything here is pure enough to test without a sender; the session
// at the bottom is the only part that talks to one.

// MARK: - Travel

/// One axis's machine-coordinate interval.
struct NcSenderAxisTravel: Sendable, Equatable {
    /// How the interval's sign was settled — the panel says which.
    enum Source: String, Sendable, Equatable {
        /// Where the machine is standing decided it.
        case position
        /// `$23`, the homing-direction invert mask, decided it.
        case homingMask
    }
    var lower: Double
    var upper: Double
    var maxTravel: Double
    var source: Source

    func contains(_ value: Double) -> Bool { value >= lower - 1e-6 && value <= upper + 1e-6 }
    /// How far outside this interval `value` is, or zero.
    func overshoot(_ value: Double) -> Double {
        if value > upper { return value - upper }
        if value < lower { return lower - value }
        return 0
    }
}

/// The machine's travel box, as ncSender's own `$$` describes it.
///
/// Grbl variants disagree about which end of an axis is machine zero, and a
/// guess would put the job on the wrong side of the envelope. So the interval
/// is read off the machine instead: `$130` gives the length, and the machine's
/// *current* position picks which of `[0, max]` and `[-max, 0]` it lives in.
/// (`MPos 113.900, -106.221, -118.200` on 2026-09-17: X runs positive, Y and Z
/// run negative.) When an axis is sitting exactly on its origin both intervals
/// contain it, and `$23` breaks the tie — a set bit means the axis homes to its
/// minimum, so machine coordinates run positive. With neither, the axis is
/// unknown and the check refuses rather than assuming.
struct NcSenderTravel: Sendable, Equatable {
    var axes: [NcSenderAxisTravel]
    static let names = ["X", "Y", "Z"]

    static func resolve(firmware: NcSenderFirmware, machinePosition: NcSenderVector?) -> NcSenderTravelResult {
        guard let maxima = firmware.maxTravel else {
            return .unknown("ncSender did not report $130–$132, so the machine's travel is unknown.")
        }
        var axes: [NcSenderAxisTravel] = []
        for index in 0..<3 {
            let maximum = maxima[index]
            let position = machinePosition?[index]
            let mask = firmware.homingInvertMask
            let positive = NcSenderAxisTravel(lower: 0, upper: maximum, maxTravel: maximum, source: .position)
            let negative = NcSenderAxisTravel(lower: -maximum, upper: 0, maxTravel: maximum, source: .position)
            if let position, position > 1e-6 { axes.append(positive); continue }
            if let position, position < -1e-6 { axes.append(negative); continue }
            guard let mask else {
                return .unknown("\(names[index]) is sitting on its machine origin and ncSender did not report $23, so which way \(names[index]) travel runs is unknown.")
            }
            var axis = (mask & (1 << index)) != 0 ? positive : negative
            axis.source = .homingMask
            axes.append(axis)
        }
        return .known(.init(axes: axes))
    }

    /// "400 / 300 / 130 mm"
    var summary: String {
        axes.map { NcSenderJobPlan.mm($0.maxTravel, 0) }.joined(separator: " / ") + " mm"
    }
}

/// Travel, or the reason it is not known. Deliberately not `Result`: "unknown"
/// is an answer the panel shows, not an error anything throws.
enum NcSenderTravelResult: Sendable, Equatable {
    case known(NcSenderTravel)
    case unknown(String)

    var travel: NcSenderTravel? { if case .known(let value) = self { return value }; return nil }
    var reason: String? { if case .unknown(let why) = self { return why }; return nil }
}

// MARK: - Does this job fit?

/// The pre-send envelope check: the job's own swept extent, placed in machine
/// coordinates by ncSender's work offset, against ncSender's travel.
struct NcSenderEnvelopeCheck: Sendable, Equatable {
    /// Nil when the check could not run at all; a check that cannot run blocks.
    var travel: NcSenderTravel?
    var unavailable: String?
    /// Per axis, how far the job reaches outside travel. Zero means it fits.
    var overshoot: [Double] = [0, 0, 0]
    var machineMin: [Double] = [0, 0, 0]
    var machineMax: [Double] = [0, 0, 0]

    var fits: Bool { unavailable == nil && overshoot.allSatisfy { $0 <= 1e-9 } }

    /// The one sentence that blocks the send, or nil.
    var blocker: String? {
        if let unavailable { return unavailable }
        guard let worst = worstAxis else { return nil }
        return "Job leaves \(NcSenderTravel.names[worst]) travel by \(NcSenderJobPlan.mm(overshoot[worst], 1)) mm."
    }
    var worstAxis: Int? {
        guard let index = overshoot.indices.max(by: { overshoot[$0] < overshoot[$1] }),
              overshoot[index] > 1e-9 else { return nil }
        return index
    }

    /// `workMin`/`workMax` come straight from the job's own verification
    /// envelope — the swept extent with the cutter included, in work
    /// coordinates. `offset` is ncSender's `WCO`.
    static func run(workMin: [Double], workMax: [Double],
                    offset: NcSenderVector?, travel: NcSenderTravelResult) -> Self {
        var check = NcSenderEnvelopeCheck()
        guard workMin.count >= 3, workMax.count >= 3,
              (workMin + workMax).allSatisfy(\.isFinite) else {
            check.unavailable = "This job has no envelope to place: generate and verify it first."
            return check
        }
        guard let offset else {
            check.unavailable = "ncSender reported no work offset, so the job cannot be placed in machine travel."
            return check
        }
        switch travel {
        case .unknown(let why):
            check.unavailable = why
            return check
        case .known(let travel):
            check.travel = travel
            for index in 0..<3 {
                let low = workMin[index] + offset[index]
                let high = workMax[index] + offset[index]
                check.machineMin[index] = low
                check.machineMax[index] = high
                check.overshoot[index] = max(travel.axes[index].overshoot(low),
                                             travel.axes[index].overshoot(high))
            }
            return check
        }
    }
}

// MARK: - Alarms

/// Grbl alarm codes, said the way a machinist needs them: what happened, and
/// whether anything moved.
enum NcSenderAlarm {
    static func explain(_ code: Int) -> String {
        switch code {
        case 1: return "ALARM:1 hard limit — a limit switch tripped during motion and the controller halted. Machine position is lost: re-home before anything else."
        case 2: return "ALARM:2 soft limit — the target was outside travel, so nothing moved. Position is safe; unlock and fix the job or the work offset."
        case 3: return "ALARM:3 reset while moving — steps were probably lost. Re-home before trusting position."
        case 4: return "ALARM:4 probe fail — the probe was already in its triggered state before the cycle started. Check the clip and the plate."
        case 5: return "ALARM:5 probe fail — the probe never touched within the programmed travel. This is the 2026-09-17 failure: check the plate, the clip, and the probe type."
        case 6: return "ALARM:6 homing fail — reset during homing."
        case 7: return "ALARM:7 homing fail — the safety door opened during homing."
        case 8: return "ALARM:8 homing fail — an axis could not clear its limit switch on pull-off. Check wiring, or raise $27."
        case 9: return "ALARM:9 homing fail — a limit switch was never found within the search distance."
        default: return "ALARM:\(code) — the controller halted. Clear it at ncSender before sending anything else."
        }
    }

    /// Grbl's `Pn` field spelled out. "Y" on 2026-09-17, alongside ALARM:1.
    static func pins(_ field: String) -> [String] {
        let names: [Character: String] = [
            "X": "X limit", "Y": "Y limit", "Z": "Z limit",
            "P": "probe", "D": "door", "H": "feed hold", "R": "reset", "S": "cycle start",
        ]
        return field.uppercased().compactMap { names[$0] }
    }
}

// MARK: - The job on its way out

/// The filename and line count the app commits to before it uploads.
enum NcSenderJobPlan {
    /// `<document>-<tool>-<date>.nc`, deterministic so re-sending the same job
    /// overwrites rather than littering ncSender's file list.
    ///
    /// The tool diameter's decimal point becomes `p` (`d3p175`): the extension
    /// should be the only dot in the name, because not every tool that touches
    /// a filename splits on the last one.
    static func filename(document: String, toolDiameter: Double, date: Date, calendar: Calendar = .current) -> String {
        let stem = slug(document.isEmpty ? "untitled" : document)
        let diameter = mm(toolDiameter, 3).replacingOccurrences(of: ".", with: "p")
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        let stamp = String(format: "%04d%02d%02d", parts.year ?? 0, parts.month ?? 0, parts.day ?? 0)
        return "\(stem)-d\(diameter)-\(stamp).nc"
    }

    private static func slug(_ text: String) -> String {
        var name = text
        if let dot = name.lastIndex(of: "."), dot != name.startIndex { name = String(name[name.startIndex..<dot]) }
        let mapped = name.lowercased().map { character -> Character in
            character.isLetter || character.isNumber ? character : "-"
        }
        let collapsed = String(mapped).split(separator: "-", omittingEmptySubsequences: true).joined(separator: "-")
        return collapsed.isEmpty ? "untitled" : String(collapsed.prefix(48))
    }

    /// Lines in the file, the way a sender counts them: every line, minus the
    /// trailing empty one a text file ends with.
    static func fileLineCount(_ gcode: String) -> Int {
        var lines = gcode.components(separatedBy: .newlines)
        if lines.last?.isEmpty == true { lines.removeLast() }
        return lines.count
    }
    /// The same file, counting only lines that carry something.
    static func codeLineCount(_ gcode: String) -> Int {
        gcode.components(separatedBy: .newlines)
            .filter { !$0.trimmingCharacters(in: .whitespaces).isEmpty }.count
    }

    /// A duration for the panel. `CNCWorkspace.durationLabel` is MM:SS, which
    /// reads as "143:53" for the two-and-a-half hours a stator takes; this
    /// carries the hour so a long estimate cannot be misread as minutes.
    static func duration(_ seconds: TimeInterval) -> String {
        guard seconds.isFinite, seconds >= 0, seconds < Double(Int.max / 2) else { return "—" }
        let total = Int(seconds.rounded())
        if total < 3600 { return String(format: "%02d:%02d", total / 60, total % 60) }
        return String(format: "%d:%02d:%02d", total / 3600, (total % 3600) / 60, total % 60)
    }

    /// Format a number the way the panel does: fixed places, trailing zeros cut.
    static func mm(_ value: Double, _ places: Int = 2) -> String {
        guard value.isFinite else { return "—" }
        var text = String(format: "%.\(places)f", value)
        if text.contains(".") {
            while text.hasSuffix("0") { text.removeLast() }
            if text.hasSuffix(".") { text.removeLast() }
        }
        return text == "-0" ? "0" : text
    }
}

/// What ncSender says it is holding, checked against what was sent.
///
/// ncSender's own line count is the only independent witness that the upload
/// arrived whole; a short file cuts a short job and says nothing. Two counts
/// are accepted because it was never recorded whether ncSender counts blank
/// lines — matching either is a match, matching neither is loud.
struct NcSenderLoadCheck: Sendable, Equatable {
    var expectedFilename: String
    var fileLines: Int
    var codeLines: Int
    var reportedFilename: String
    var reportedLines: Int

    var filenameMatches: Bool { reportedFilename == expectedFilename }
    var lineCountMatches: Bool { reportedLines == fileLines || reportedLines == codeLines }
    var ok: Bool { filenameMatches && lineCountMatches }

    /// Nil when the sender is holding what was sent; otherwise the refusal.
    var failure: String? {
        if !filenameMatches {
            return "ncSender is holding “\(reportedFilename.isEmpty ? "nothing" : reportedFilename)”, not “\(expectedFilename)”. Upload again before starting."
        }
        if !lineCountMatches {
            return "ncSender loaded \(reportedLines) lines; this job has \(fileLines) (\(codeLines) with content). The upload did not arrive whole — upload again."
        }
        return nil
    }
}

// MARK: - Log tail

/// ncSender's daily log, cut down to what the app can say something about.
enum NcSenderLogFilter {
    enum Kind: String, Sendable, Equatable {
        case command, alarm, probe
    }
    struct Line: Sendable, Equatable, Identifiable {
        var id: Int
        var kind: Kind
        var text: String
    }

    /// The last `limit` lines that are a command, an alarm or a probe result.
    /// This is the view that diagnosed the 2026-09-17 probe failure.
    static func tail(_ log: String, limit: Int = 40) -> [Line] {
        var kept: [Line] = []
        for (index, raw) in log.components(separatedBy: .newlines).enumerated() {
            let line = raw.trimmingCharacters(in: .whitespaces)
            guard !line.isEmpty, let kind = classify(line) else { continue }
            kept.append(Line(id: index, kind: kind, text: line))
        }
        return Array(kept.suffix(max(0, limit)))
    }

    private static func classify(_ line: String) -> Kind? {
        let upper = line.uppercased()
        if upper.contains("ALARM") || upper.contains("ERROR:") { return .alarm }
        if upper.contains("[PRB:") || upper.contains("G38") || upper.contains("PROBE") { return .probe }
        if upper.contains("$H") || upper.contains("$J=") || upper.contains("[SENT]")
            || upper.contains("COMMAND") || upper.range(of: #"\b[GM]\d"#, options: .regularExpression) != nil {
            return .command
        }
        return nil
    }
}

// MARK: - The session

/// One live conversation with one ncSender: polled state, the send path, and
/// the transport buttons.
///
/// It owns no job. The workspace owns the job; this reads `jobCode`,
/// `jobCurrent` and `blockers` from it and refuses on its own account when any
/// of them says no.
@MainActor @Observable
final class NcSenderSession {
    /// What `Test connection` found.
    struct Probe: Sendable, Equatable {
        var reachable = false
        var version: String?
        var controllerAddress: String?
        var probeType: String?
        var detail = ""
        /// Both senders on one controller: the failure mode that has no
        /// symptom until the machine takes two command streams at once.
        var contention: String?
    }

    /// Where the send path is.
    enum Send: Sendable, Equatable {
        case idle
        case uploading
        case loaded(filename: String, lines: Int)
        case failed(String)
    }

    var baseURLText: String {
        didSet { Prefs.ncSenderURL = baseURLText }
    }
    private(set) var state: NcSenderServerState?
    private(set) var firmware: NcSenderFirmware?
    private(set) var settings: NcSenderSettings?
    private(set) var probe: Probe?
    private(set) var send: Send = .idle
    private(set) var logLines: [NcSenderLogFilter.Line] = []
    /// The log tail's own failure. A missing daily log must not read as the
    /// sender being unreachable — it is its own line, in its own section.
    private(set) var logError: String?
    private(set) var lastPoll: Date?
    /// When this app first saw the job running, so elapsed/remaining can be
    /// shown even when ncSender does not report an elapsed time.
    private(set) var runSince: Date?
    private(set) var lastError: String?
    private(set) var busy = false
    /// Bumped whenever the poll lands, so time-based views refresh.
    private(set) var tick = 0

    /// Injected so tests can drive a mock `URLProtocol`.
    private let session: URLSession
    private var poller: Task<Void, Never>?

    init(session: URLSession = .shared, baseURL: String? = nil) {
        self.session = session
        self.baseURLText = baseURL ?? Prefs.ncSenderURL
    }

    var client: NcSenderClient? {
        guard let url = try? NcSenderClient.url(from: baseURLText) else { return nil }
        return NcSenderClient(baseURL: url, session: session)
    }

    // MARK: derived

    var machine: NcSenderMachineState? { state?.machineState }
    var job: NcSenderJobLoaded? { state?.jobLoaded }
    var travel: NcSenderTravelResult? {
        firmware.map { NcSenderTravel.resolve(firmware: $0, machinePosition: machine?.mpos) }
    }
    /// "Not homed" is a warning, never a block: an operator who zeroes by hand
    /// on a machine that has not homed is doing what was done on 2026-09-17.
    var notHomedWarning: String? {
        guard let machine else { return nil }
        return machine.homed ? nil : "The controller has not homed. Machine coordinates and the travel check below are only as good as where it thinks it is."
    }
    var alarmExplanation: String? {
        guard let machine else { return nil }
        if let code = machine.alarmCode, code > 0 { return NcSenderAlarm.explain(code) }
        guard machine.status.uppercased().hasPrefix("ALARM") else { return nil }
        return "The controller is in alarm. Clear it at ncSender before sending anything else."
    }
    var assertedPins: [String] { NcSenderAlarm.pins(machine?.pins ?? "") }

    /// How long the job has been running, and what is left at the rate so far.
    ///
    /// The estimate comes from lines, not the reported percentage, and only
    /// once enough lines have gone by for a rate to mean anything. Below that
    /// the remaining time is nil and the panel shows a dash: no estimate is
    /// better than one drawn from three seconds of rapids.
    var progressTimes: (elapsed: TimeInterval, remaining: TimeInterval?)? {
        guard let job, job.totalLines > 0 else { return nil }
        let elapsed = job.elapsedSeconds ?? runSince.map { Date().timeIntervalSince($0) } ?? 0
        guard elapsed > 0, job.currentLine >= 10, job.currentLine < job.totalLines else { return (elapsed, nil) }
        let remaining = elapsed * Double(job.totalLines - job.currentLine) / Double(job.currentLine)
        return (elapsed, max(0, remaining))
    }

    /// The job's envelope in machine coordinates, against ncSender's travel.
    func envelopeCheck(workMin: [Double], workMax: [Double]) -> NcSenderEnvelopeCheck {
        NcSenderEnvelopeCheck.run(workMin: workMin, workMax: workMax,
                                  offset: machine?.workOffset,
                                  travel: travel ?? .unknown("ncSender's $$ settings have not been read yet."))
    }

    /// Everything standing between this job and a start, in the order it is
    /// worth fixing. Empty means Send is allowed.
    func sendBlockers(jobCurrent: Bool, jobBlocked: Bool, hasCode: Bool,
                      envelope: NcSenderEnvelopeCheck?) -> [String] {
        var out: [String] = []
        if !jobCurrent { out.append("Build the job after editing the setup.") }
        if jobBlocked { out.append("The job is blocked by its own verification; fix that before sending it anywhere.") }
        if !hasCode { out.append("This job produced no G-code.") }
        if state == nil { out.append("Not connected to ncSender — test the connection first.") }
        if let blocker = envelope?.blocker { out.append(blocker) }
        return out
    }

    // MARK: connection

    func testConnection(nativeHost: String, nativePort: String) async {
        guard let client else {
            probe = Probe(detail: NcSenderError.badBaseURL(baseURLText).localizedDescription)
            return
        }
        busy = true
        defer { busy = false }
        var found = Probe()
        do {
            let health = try await client.health()
            let settings = try await client.settings()
            let state = try await client.serverState()
            self.settings = settings
            self.state = state
            found.reachable = health.ok || state.version != nil
            found.version = health.version ?? state.version
            found.controllerAddress = settings.controllerAddress
            found.probeType = settings.probeType
            found.detail = "\(found.reachable ? "Reachable" : "Answered, but not healthy") · controller \(settings.controllerAddress ?? "not reported") · \(state.machineState.status)"
            found.contention = Self.contention(ncSenderHolds: settings.controllerAddress,
                                               nativeHost: nativeHost, nativePort: nativePort)
            lastError = nil
        } catch {
            found.detail = error.localizedDescription
            lastError = error.localizedDescription
        }
        probe = found
        // Firmware is a separate read so a controller that answers state but
        // not $$ still shows a connection instead of one blanket failure.
        if let firmware = try? await client.firmware() { self.firmware = firmware }
    }

    /// Record a probe result without a sender to probe.
    ///
    /// Named for what it does rather than hidden behind `#if DEBUG`: the
    /// contention guard is the one thing here that stops a machine, and a
    /// guard whose test needs a live ncSender on the bench is a guard nobody
    /// runs. Everything it sets is what `testConnection` would have set.
    func simulate(controllerAddress: String?, reachable: Bool = true,
                  nativeHost: String, nativePort: String = "23") {
        var found = Probe()
        found.reachable = reachable
        found.controllerAddress = controllerAddress
        found.detail = reachable ? "Reachable · controller \(controllerAddress ?? "not reported")" : "Not reachable"
        found.contention = Self.contention(ncSenderHolds: controllerAddress,
                                           nativeHost: nativeHost, nativePort: nativePort)
        probe = found
    }

    /// Two senders, one controller. ncSender holds the Anolex's telnet session;
    /// if the native sender is pointed at the same address, whichever connects
    /// second either fails or — worse — interleaves commands.
    static func contention(ncSenderHolds: String?, nativeHost: String, nativePort: String) -> String? {
        guard let ncSenderHolds else { return nil }
        let holdsHost = ncSenderHolds.split(separator: ":").first.map(String.init) ?? ncSenderHolds
        let holdsPort = ncSenderHolds.split(separator: ":").dropFirst().first.map(String.init)
        let host = nativeHost.trimmingCharacters(in: .whitespaces).lowercased()
        guard !host.isEmpty, holdsHost.lowercased() == host else { return nil }
        let port = nativePort.trimmingCharacters(in: .whitespaces)
        guard holdsPort == nil || port.isEmpty || holdsPort == port else { return nil }
        return "ncSender is holding \(ncSenderHolds) — the same controller the built-in sender is configured for. Only one sender may hold the controller: disconnect the built-in one before sending."
    }

    // MARK: polling

    /// 1 Hz while a job is running, 5 s otherwise. Restarting is idempotent.
    func startPolling() {
        guard poller == nil else { return }
        poller = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.pollOnce()
                let running = self.job?.isRunning ?? false
                try? await Task.sleep(for: .milliseconds(running ? 1000 : 5000))
            }
        }
    }
    func stopPolling() { poller?.cancel(); poller = nil }

    func pollOnce() async {
        guard let client else { return }
        do {
            var state = try await client.serverState()
            // The job endpoint is the authority on progress; server-state may
            // carry a stale `jobLoaded` between polls.
            if let status = try? await client.jobStatus(), status.totalLines > 0 { state.jobLoaded = status }
            self.state = state
            if state.jobLoaded?.isRunning == true {
                if runSince == nil { runSince = Date() }
            } else if state.jobLoaded?.isPaused != true {
                runSince = nil
            }
            lastPoll = Date()
            lastError = nil
            tick += 1
        } catch {
            lastError = error.localizedDescription
            tick += 1
        }
    }

    func refreshFirmware() async {
        guard let client else { return }
        do { firmware = try await client.firmware(); lastError = nil }
        catch { lastError = error.localizedDescription }
    }

    func refreshLog(date: Date = Date(), limit: Int = 40) async {
        guard let client else { return }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd"
        do {
            logLines = NcSenderLogFilter.tail(try await client.log(date: formatter.string(from: date)), limit: limit)
            logError = nil
        } catch {
            logLines = []
            logError = error.localizedDescription
        }
    }

    // MARK: sending

    /// Upload the job and confirm ncSender is holding exactly it.
    ///
    /// The caller has already decided the job is current, verified and not
    /// blocked; this adds the only thing the app can check afterwards — that
    /// what landed is what left.
    @discardableResult
    func sendJob(gcode: String, filename: String) async -> Bool {
        guard let client else { send = .failed(NcSenderError.badBaseURL(baseURLText).localizedDescription); return false }
        send = .uploading
        busy = true
        defer { busy = false }
        do {
            _ = try await client.upload(filename: filename, gcode: gcode)
            // ncSender loads on upload; read back what it thinks it has.
            let loaded = try await client.jobStatus()
            let check = NcSenderLoadCheck(expectedFilename: filename,
                                          fileLines: NcSenderJobPlan.fileLineCount(gcode),
                                          codeLines: NcSenderJobPlan.codeLineCount(gcode),
                                          reportedFilename: loaded.filename,
                                          reportedLines: loaded.totalLines)
            if let failure = check.failure {
                send = .failed(failure)
                lastError = failure
                return false
            }
            send = .loaded(filename: filename, lines: loaded.totalLines)
            await pollOnce()
            return true
        } catch {
            send = .failed(error.localizedDescription)
            lastError = error.localizedDescription
            return false
        }
    }

    /// Start the loaded job. The caller must already have both confirmations:
    /// the setup tick and the start dialog.
    @discardableResult
    func startLoadedJob(filename: String) async -> Bool {
        await act { try await $0.startJob(filename: filename) }
    }
    @discardableResult func pause() async -> Bool { await act { try await $0.pauseJob() } }
    @discardableResult func resume() async -> Bool { await act { try await $0.resumeJob() } }
    @discardableResult func stop() async -> Bool { await act { try await $0.stopJob() } }

    /// A single command, with the consent the client insists on.
    @discardableResult
    func sendCommand(_ command: String, consent: NcSenderConsent) async -> Bool {
        await act { _ = try await $0.sendCommand(command, consent: consent) }
    }

    private func act(_ body: @Sendable (NcSenderClient) async throws -> Void) async -> Bool {
        guard let client else { lastError = NcSenderError.badBaseURL(baseURLText).localizedDescription; return false }
        busy = true
        defer { busy = false }
        do { try await body(client); lastError = nil; await pollOnce(); return true }
        catch { lastError = error.localizedDescription; return false }
    }
}
