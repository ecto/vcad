import Foundation

// The ncSender HTTP client.
//
// On 2026-09-17 the first real cut was not sent by this app. It was sent by
// ncSender running on `pika`, because ncSender's streaming, feed hold and
// overrides were already proven on that machine. The owner's default, and it
// is reversible: ncSender is the blessed sender, the native `CNCController`
// stays as the escape hatch.
//
// The discipline the native sender earned the hard way travels with the job:
// nothing that can move the machine leaves this file without a consent value
// that names the exact line a human confirmed, and a settings write needs the
// setting's name typed back. A sender that is one HTTP POST away from `$H` is
// otherwise more dangerous than the one with a wire protocol in front of it.

// MARK: - Wire values

/// A 3-vector as ncSender writes it.
///
/// The recorded session handed us the *values* (`MPos 113.900,-106.221,-118.200`)
/// and not the JSON shape, so the three plausible encodings all decode —
/// `[x, y, z]`, `{"x":…,"y":…,"z":…}` and `"x,y,z"` — and anything else is a
/// decode error rather than a silent zero. A zeroed position would place the
/// job at the machine origin, which is exactly the lie this file must not tell.
struct NcSenderVector: Decodable, Sendable, Equatable {
    var x = 0.0
    var y = 0.0
    var z = 0.0

    init(x: Double, y: Double, z: Double) { self.x = x; self.y = y; self.z = z }

    init(from decoder: Decoder) throws {
        if var list = try? decoder.unkeyedContainer() {
            var values: [Double] = []
            while !list.isAtEnd, values.count < 3 { values.append(try list.decode(Double.self)) }
            guard values.count == 3, values.allSatisfy(\.isFinite) else {
                throw DecodingError.dataCorruptedError(in: list, debugDescription: "Expected three finite numbers")
            }
            self.init(x: values[0], y: values[1], z: values[2]); return
        }
        if let keyed = try? decoder.container(keyedBy: Key.self),
           let x = try? keyed.decode(Double.self, forKey: .x) {
            let y = try keyed.decode(Double.self, forKey: .y)
            let z = try keyed.decode(Double.self, forKey: .z)
            guard [x, y, z].allSatisfy(\.isFinite) else {
                throw DecodingError.dataCorruptedError(forKey: .x, in: keyed, debugDescription: "Non-finite coordinate")
            }
            self.init(x: x, y: y, z: z); return
        }
        let single = try decoder.singleValueContainer()
        let text = try single.decode(String.self)
        let values = text.split(separator: ",").compactMap { Double($0.trimmingCharacters(in: .whitespaces)) }
        guard values.count == 3, values.allSatisfy(\.isFinite) else {
            throw DecodingError.dataCorruptedError(in: single, debugDescription: "Expected \"x,y,z\", got \(text)")
        }
        self.init(x: values[0], y: values[1], z: values[2])
    }

    private enum Key: String, CodingKey { case x, y, z }

    subscript(axis: Int) -> Double {
        switch axis { case 0: return x; case 1: return y; default: return z }
    }
}

/// `machineState` out of `GET /api/server-state`.
struct NcSenderMachineState: Decodable, Sendable, Equatable {
    var status = "Unknown"
    var mpos: NcSenderVector?
    var wpos: NcSenderVector?
    var wco: NcSenderVector?
    /// Grbl's `Pn` field, verbatim: the asserted input pins.
    var pins = ""
    var homed = false
    var alarmCode: Int?
    var spindleActive = false

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        status = (try? c.decode(String.self, forKey: .status)) ?? "Unknown"
        mpos = try? c.decode(NcSenderVector.self, forKey: .mpos)
        wpos = try? c.decode(NcSenderVector.self, forKey: .wpos)
        wco = try? c.decode(NcSenderVector.self, forKey: .wco)
        pins = (try? c.decode(String.self, forKey: .pins)) ?? ""
        homed = (try? c.decode(Bool.self, forKey: .homed)) ?? false
        alarmCode = try? c.decode(Int.self, forKey: .alarmCode)
        spindleActive = (try? c.decode(Bool.self, forKey: .spindleActive)) ?? false
    }

    private enum Key: String, CodingKey {
        case status, homed, alarmCode, spindleActive
        case mpos = "MPos", wpos = "WPos", wco = "WCO", pins = "Pn"
    }

    /// Where the job's work zero sits in machine coordinates.
    ///
    /// `WCO` when ncSender reports it, otherwise the difference the two
    /// positions imply. Never a guess: without either the envelope check
    /// refuses to run rather than assuming zero.
    var workOffset: NcSenderVector? {
        if let wco { return wco }
        guard let m = mpos, let w = wpos else { return nil }
        return NcSenderVector(x: m.x - w.x, y: m.y - w.y, z: m.z - w.z)
    }
}

/// `jobLoaded` — the file ncSender is holding, and how far through it is.
struct NcSenderJobLoaded: Decodable, Sendable, Equatable {
    var filename = ""
    var currentLine = 0
    var totalLines = 0
    var progressPercent = 0.0
    /// `running` / `paused` / `idle` when the job endpoint says so.
    var status: String?
    var elapsedSeconds: Double?

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        filename = (try? c.decode(String.self, forKey: .filename)) ?? ""
        currentLine = (try? c.decode(Int.self, forKey: .currentLine)) ?? 0
        totalLines = (try? c.decode(Int.self, forKey: .totalLines)) ?? 0
        progressPercent = (try? c.decode(Double.self, forKey: .progressPercent))
            ?? (totalLines > 0 ? 100 * Double(currentLine) / Double(totalLines) : 0)
        status = try? c.decode(String.self, forKey: .status)
        elapsedSeconds = try? c.decode(Double.self, forKey: .elapsedSeconds)
    }

    private enum Key: String, CodingKey {
        case filename, currentLine, totalLines, progressPercent, status, elapsedSeconds
    }

    var isRunning: Bool { (status ?? "").lowercased() == "running" }
    var isPaused: Bool { (status ?? "").lowercased() == "paused" }
}

/// `GET /api/server-state`.
struct NcSenderServerState: Decodable, Sendable, Equatable {
    var machineState = NcSenderMachineState()
    var jobLoaded: NcSenderJobLoaded?
    var version: String?

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        machineState = (try? c.decode(NcSenderMachineState.self, forKey: .machineState)) ?? NcSenderMachineState()
        jobLoaded = try? c.decode(NcSenderJobLoaded.self, forKey: .jobLoaded)
        version = try? c.decode(String.self, forKey: .version)
    }

    private enum Key: String, CodingKey { case machineState, jobLoaded, version }
}

/// `GET /api/health`.
struct NcSenderHealth: Decodable, Sendable, Equatable {
    var ok = false
    var version: String?

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        if let status = try? c.decode(String.self, forKey: .status) {
            ok = ["ok", "healthy", "up"].contains(status.lowercased())
        } else {
            ok = (try? c.decode(Bool.self, forKey: .ok)) ?? false
        }
        version = try? c.decode(String.self, forKey: .version)
    }

    private enum Key: String, CodingKey { case status, ok, version }
}

/// `GET /api/settings` — which controller ncSender is holding, and how it probes.
struct NcSenderSettings: Decodable, Sendable, Equatable {
    var controllerIP: String?
    var controllerPort: Int?
    var probeType: String?

    init() {}

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        if let connection = try? c.nestedContainer(keyedBy: Key.self, forKey: .connection) {
            controllerIP = try? connection.decode(String.self, forKey: .ip)
            controllerPort = (try? connection.decode(Int.self, forKey: .port))
                ?? (try? connection.decode(String.self, forKey: .port)).flatMap { Int($0) }
        }
        if let probe = try? c.nestedContainer(keyedBy: Key.self, forKey: .probe) {
            probeType = try? probe.decode(String.self, forKey: .type)
        }
        if probeType == nil { probeType = try? c.decode(String.self, forKey: .probeType) }
    }

    private enum Key: String, CodingKey { case connection, ip, port, probe, type, probeType }

    /// `ip:port`, the identity that matters: two senders on one controller is
    /// the failure this string exists to make visible.
    var controllerAddress: String? {
        guard let controllerIP else { return nil }
        return controllerPort.map { "\(controllerIP):\($0)" } ?? controllerIP
    }
}

/// `GET /api/firmware` — the controller's `$$` dump.
///
/// The body's shape was never recorded, so rather than guess one the whole
/// document is walked for anything that looks like a `$nnn` setting: a key
/// `"$130"`, or an object carrying a name and a value. Unknown shapes yield an
/// empty table, which every consumer treats as "not reported" and refuses to
/// check against — never as "zero travel".
struct NcSenderFirmware: Decodable, Sendable, Equatable {
    /// `$130` → `400`.
    var settings: [String: Double] = [:]

    init() {}
    init(settings: [String: Double]) { self.settings = settings }

    init(from decoder: Decoder) throws {
        let value = try NcSenderJSON(from: decoder)
        var found: [String: Double] = [:]
        NcSenderFirmware.harvest(value, into: &found)
        settings = found
    }

    private static func harvest(_ value: NcSenderJSON, into table: inout [String: Double]) {
        switch value {
        case .object(let fields):
            // {"key": "$130", "value": "400"} and its spellings.
            let name = ["key", "name", "setting", "id", "$"].compactMap { fields[$0]?.stringValue }.first
            let paired = ["value", "val", "v"].compactMap { fields[$0]?.numberValue }.first
            if let name, let paired, isSettingName(name) { table[normalized(name)] = paired }
            for (key, field) in fields {
                if isSettingName(key), let number = field.numberValue { table[normalized(key)] = number }
                harvest(field, into: &table)
            }
        case .array(let items):
            for item in items { harvest(item, into: &table) }
        default:
            break
        }
    }

    private static func isSettingName(_ key: String) -> Bool {
        let body = key.hasPrefix("$") ? String(key.dropFirst()) : key
        return !body.isEmpty && body.allSatisfy(\.isNumber)
    }
    private static func normalized(_ key: String) -> String {
        key.hasPrefix("$") ? key : "$" + key
    }

    subscript(setting: String) -> Double? { settings[setting] }

    /// `$130`, `$131`, `$132` — the per-axis maximum travel, in millimetres.
    var maxTravel: [Double]? {
        let values = ["$130", "$131", "$132"].compactMap { settings[$0] }
        guard values.count == 3, values.allSatisfy({ $0.isFinite && $0 > 0 }) else { return nil }
        return values
    }
    /// `$20` — soft limits.
    var softLimits: Bool? { settings["$20"].map { $0 != 0 } }
    /// `$21` — hard limits.
    var hardLimits: Bool? { settings["$21"].map { $0 != 0 } }
    /// `$23` — homing direction invert mask, used only to break a tie when the
    /// machine is sitting exactly on an axis origin.
    var homingInvertMask: Int? { settings["$23"].map { Int($0) } }
}

/// Just enough JSON to walk a document whose shape is unknown.
enum NcSenderJSON: Decodable, Sendable, Equatable {
    case null
    case bool(Bool)
    case number(Double)
    case string(String)
    case array([NcSenderJSON])
    case object([String: NcSenderJSON])

    init(from decoder: Decoder) throws {
        if let single = try? decoder.singleValueContainer(), !single.decodeNil() {
            if let v = try? single.decode(Bool.self) { self = .bool(v); return }
            if let v = try? single.decode(Double.self) { self = .number(v); return }
            if let v = try? single.decode(String.self) { self = .string(v); return }
        }
        if var list = try? decoder.unkeyedContainer() {
            var items: [NcSenderJSON] = []
            while !list.isAtEnd { items.append(try list.decode(NcSenderJSON.self)) }
            self = .array(items); return
        }
        if let keyed = try? decoder.container(keyedBy: AnyKey.self) {
            var fields: [String: NcSenderJSON] = [:]
            for key in keyed.allKeys { fields[key.stringValue] = try keyed.decode(NcSenderJSON.self, forKey: key) }
            self = .object(fields); return
        }
        self = .null
    }

    var stringValue: String? {
        switch self {
        case .string(let s): return s
        case .number(let n): return String(n)
        default: return nil
        }
    }
    var numberValue: Double? {
        switch self {
        case .number(let n): return n
        case .string(let s): return Double(s.trimmingCharacters(in: .whitespaces))
        case .bool(let b): return b ? 1 : 0
        default: return nil
        }
    }

    private struct AnyKey: CodingKey {
        var stringValue: String
        var intValue: Int? { nil }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { return nil }
    }
}

// MARK: - Commands, and the consent they need

/// What a line sent to ncSender can do.
enum NcSenderCommandKind: Sendable, Equatable {
    /// One of the handful of queries that cannot change anything.
    case readOnly
    /// It can move the machine, or move where zero is. `why` is the sentence
    /// the confirmation dialog shows.
    case motion(why: String)
    /// A `$nnn=` write. `$132=100` from ncSender's own wizard is what drove
    /// Z into the soft limit on 2026-09-17; these need the name typed back.
    case settingsWrite(setting: String)
}

/// Classification of a single line, allowlist-first.
///
/// The read-only set is closed: anything that is not on it is treated as
/// motion. A sender that guessed the other way would wave through every
/// command it had not heard of.
enum NcSenderCommand {
    /// Queries that return state and change none.
    static let readOnlyLines: Set<String> = ["$$", "$G", "$I", "$#", "$N", "?"]

    static func classify(_ raw: String) -> NcSenderCommandKind {
        let line = raw.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
        guard !line.isEmpty else { return .motion(why: "an empty line is not a command this app will send") }
        if readOnlyLines.contains(line) { return .readOnly }
        if let setting = settingName(in: line) { return .settingsWrite(setting: setting) }
        if line == "$H" || line.hasPrefix("$H ") { return .motion(why: "runs the homing cycle — every axis moves to its switch") }
        if line.hasPrefix("$J=") { return .motion(why: "jogs the machine") }
        if line == "$X" { return .motion(why: "clears the alarm lock; the controller will accept motion straight after") }
        if matches(line, #"\bG0*3[89](\.\d)?\b"#) { return .motion(why: "probes — the tool moves until it touches") }
        if matches(line, #"\bG0*10\b"#) { return .motion(why: "rewrites a work offset — the job's zero moves, not the tool") }
        if matches(line, #"\bG0*[0-3]\b"#) { return .motion(why: "commands a rapid or feed move") }
        if matches(line, #"\bG28|G30\b"#) { return .motion(why: "returns to a stored machine position") }
        if matches(line, #"\bM0*[34]\b"#) { return .motion(why: "starts the spindle") }
        return .motion(why: "this line is not on the read-only list, so it is treated as one that can move the machine")
    }

    /// `$132=130` → `$132`. Nil for anything that is not a settings write.
    static func settingName(in line: String) -> String? {
        let text = line.trimmingCharacters(in: .whitespaces)
        guard text.hasPrefix("$"), let equals = text.firstIndex(of: "=") else { return nil }
        let name = text[text.index(after: text.startIndex)..<equals]
        guard !name.isEmpty, name.allSatisfy(\.isNumber) else { return nil }
        return "$" + name
    }

    private static func matches(_ line: String, _ pattern: String) -> Bool {
        line.range(of: pattern, options: .regularExpression) != nil
    }
}

/// Proof that a human answered a confirmation for *this exact line*.
///
/// It carries the command it was granted for, so a consent obtained for a jog
/// cannot be reused to send a homing cycle, and a settings write carries the
/// setting name the operator typed back. The client re-checks both.
struct NcSenderConsent: Sendable, Equatable {
    enum Kind: Sendable, Equatable {
        case motion
        case setting(typed: String)
    }
    let kind: Kind
    let command: String

    /// The operator confirmed a motion dialog for `command`.
    static func motion(confirming command: String) -> Self {
        .init(kind: .motion, command: normalize(command))
    }
    /// The operator confirmed `command` *and* typed the setting's name.
    static func setting(confirming command: String, typedName: String) -> Self {
        .init(kind: .setting(typed: typedName.trimmingCharacters(in: .whitespaces).uppercased()),
              command: normalize(command))
    }

    static func normalize(_ command: String) -> String {
        command.trimmingCharacters(in: .whitespacesAndNewlines).uppercased()
    }
}

// MARK: - Errors

enum NcSenderError: LocalizedError, Equatable {
    case badBaseURL(String)
    case http(status: Int, path: String, detail: String)
    case transport(String)
    case decode(path: String, detail: String)
    /// A motion command arrived without consent, or with consent for another line.
    case confirmationRequired(command: String, why: String)
    /// A settings write whose typed name did not match the setting being written.
    case settingNameMismatch(setting: String, typed: String)
    case badFilename(String)
    case loadMismatch(String)

    var errorDescription: String? {
        switch self {
        case .badBaseURL(let text):
            return "“\(text)” is not an ncSender address. Use something like http://pika:8090."
        case .http(let status, let path, let detail):
            return "ncSender answered \(status) for \(path)\(detail.isEmpty ? "" : ": \(detail)")"
        case .transport(let detail):
            return "Could not reach ncSender: \(detail)"
        case .decode(let path, let detail):
            return "ncSender's reply to \(path) could not be read: \(detail)"
        case .confirmationRequired(let command, let why):
            return "“\(command)” needs confirming before it is sent — it \(why)."
        case .settingNameMismatch(let setting, let typed):
            return "This writes \(setting); “\(typed)” was typed. Type the setting's own name to write it."
        case .badFilename(let name):
            return "“\(name)” is not a filename ncSender will be given: no path separators, quotes or control characters."
        case .loadMismatch(let detail):
            return detail
        }
    }
}

// MARK: - The client

/// A stateless HTTP client for one ncSender. Everything is `async`; nothing
/// here touches the main actor, and nothing here holds state that a restart of
/// the sender would invalidate.
struct NcSenderClient: Sendable {
    let baseURL: URL
    let session: URLSession
    /// Stamped into `meta.sourceId` so ncSender's own log says which app sent
    /// a command — this is how the 2026-09-17 probe failure was attributed.
    let sourceID: String

    init(baseURL: URL, session: URLSession = .shared, sourceID: String = "vcad") {
        self.baseURL = baseURL
        self.session = session
        self.sourceID = sourceID
    }

    /// Parse a user-typed address. A bare `pika:8090` becomes `http://pika:8090`.
    static func url(from text: String) throws -> URL {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { throw NcSenderError.badBaseURL(text) }
        let withScheme = trimmed.contains("://") ? trimmed : "http://" + trimmed
        guard let url = URL(string: withScheme), let scheme = url.scheme?.lowercased(),
              ["http", "https"].contains(scheme), url.host?.isEmpty == false else {
            throw NcSenderError.badBaseURL(text)
        }
        return url
    }

    /// The default: ncSender on `pika`, the Raspberry-Pi-class host that holds
    /// the Anolex's telnet session.
    static let defaultBaseURL = "http://pika:8090"

    // MARK: reads

    func health() async throws -> NcSenderHealth { try await get("/api/health") }
    func serverState() async throws -> NcSenderServerState { try await get("/api/server-state") }
    func firmware() async throws -> NcSenderFirmware { try await get("/api/firmware") }
    func settings() async throws -> NcSenderSettings { try await get("/api/settings") }
    func jobStatus() async throws -> NcSenderJobLoaded { try await get("/api/gcode-job/status") }

    /// The daily log, as text. `date` is ncSender's own file naming (`YYYY-MM-DD`).
    func log(date: String) async throws -> String {
        let (data, _) = try await perform(request(path: "/api/logs/\(date).log", method: "GET"),
                                          path: "/api/logs/\(date).log")
        return String(decoding: data, as: UTF8.self)
    }

    // MARK: the job

    /// `POST /api/gcode-files` with a multipart `file` part. ncSender loads
    /// what it receives, so this is also the load step.
    @discardableResult
    func upload(filename: String, gcode: String, boundary: String = NcSenderClient.newBoundary()) async throws -> String {
        try Self.checkFilename(filename)
        var request = request(path: "/api/gcode-files", method: "POST")
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = Self.multipartBody(boundary: boundary, filename: filename, gcode: gcode)
        let (data, _) = try await perform(request, path: "/api/gcode-files")
        return String(decoding: data, as: UTF8.self)
    }

    func startJob(filename: String) async throws {
        try Self.checkFilename(filename)
        _ = try await postJSON("/api/gcode-job", ["filename": filename])
    }
    func pauseJob() async throws { _ = try await postJSON("/api/gcode-job/pause", [:]) }
    func resumeJob() async throws { _ = try await postJSON("/api/gcode-job/resume", [:]) }
    func stopJob() async throws { _ = try await postJSON("/api/gcode-job/stop", [:]) }

    // MARK: one command

    /// Send a single line through ncSender.
    ///
    /// Read-only queries go straight out. Anything else — and "anything else"
    /// is the default, not a listed set — needs a `NcSenderConsent` minted for
    /// this exact line; a settings write additionally needs the setting's own
    /// name typed back. Passing no consent is not a lenient path: it throws.
    @discardableResult
    func sendCommand(_ command: String, consent: NcSenderConsent? = nil, commandID: String = UUID().uuidString) async throws -> String {
        let line = command.trimmingCharacters(in: .whitespacesAndNewlines)
        let kind = NcSenderCommand.classify(line)
        switch kind {
        case .readOnly:
            break
        case .motion(let why):
            guard let consent, consent.kind == .motion,
                  consent.command == NcSenderConsent.normalize(line) else {
                throw NcSenderError.confirmationRequired(command: line, why: why)
            }
        case .settingsWrite(let setting):
            guard let consent, consent.command == NcSenderConsent.normalize(line),
                  case .setting(let typed) = consent.kind else {
                throw NcSenderError.confirmationRequired(
                    command: line,
                    why: "writes controller setting \(setting), which needs the setting's name typed back")
            }
            guard typed == setting.uppercased() else {
                throw NcSenderError.settingNameMismatch(setting: setting, typed: typed)
            }
        }
        let body: [String: Any] = [
            "command": line,
            "commandId": commandID,
            "meta": ["sourceId": sourceID],
        ]
        return try await postJSON("/api/send-command", body)
    }

    // MARK: multipart

    /// A fresh boundary. Injectable everywhere so a test can assert the body
    /// byte for byte.
    static func newBoundary() -> String { "vcad-\(UUID().uuidString)" }

    static func multipartBody(boundary: String, filename: String, gcode: String) -> Data {
        var body = Data()
        func append(_ text: String) { body.append(Data(text.utf8)) }
        append("--\(boundary)\r\n")
        append("Content-Disposition: form-data; name=\"file\"; filename=\"\(filename)\"\r\n")
        append("Content-Type: text/plain; charset=utf-8\r\n\r\n")
        append(gcode)
        append("\r\n--\(boundary)--\r\n")
        return body
    }

    /// Refuse a filename that would escape ncSender's own directory or break
    /// the multipart header. Rewriting it silently would hand the operator a
    /// file under a name the app did not choose.
    static func checkFilename(_ name: String) throws {
        guard !name.isEmpty, name.utf8.count <= 120,
              !name.contains("/"), !name.contains("\\"), !name.contains(".."),
              !name.contains("\""), !name.contains("\r"), !name.contains("\n"),
              name.unicodeScalars.allSatisfy({ $0.value >= 32 && $0.value != 127 }) else {
            throw NcSenderError.badFilename(name)
        }
    }

    // MARK: plumbing

    private func request(path: String, method: String) -> URLRequest {
        var request = URLRequest(url: URL(string: path, relativeTo: baseURL) ?? baseURL)
        request.httpMethod = method
        request.timeoutInterval = 15
        return request
    }

    private func get<T: Decodable>(_ path: String) async throws -> T {
        let (data, _) = try await perform(request(path: path, method: "GET"), path: path)
        do { return try JSONDecoder().decode(T.self, from: data) }
        catch { throw NcSenderError.decode(path: path, detail: error.localizedDescription) }
    }

    @discardableResult
    private func postJSON(_ path: String, _ body: [String: Any]) async throws -> String {
        var request = request(path: path, method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body.isEmpty ? Data("{}".utf8)
            : (try? JSONSerialization.data(withJSONObject: body)) ?? Data("{}".utf8)
        let (data, _) = try await perform(request, path: path)
        return String(decoding: data, as: UTF8.self)
    }

    private func perform(_ request: URLRequest, path: String) async throws -> (Data, HTTPURLResponse) {
        let data: Data, response: URLResponse
        do { (data, response) = try await session.data(for: request) }
        catch { throw NcSenderError.transport(error.localizedDescription) }
        guard let http = response as? HTTPURLResponse else {
            throw NcSenderError.transport("ncSender did not answer with HTTP")
        }
        guard (200..<300).contains(http.statusCode) else {
            let detail = String(decoding: data.prefix(400), as: UTF8.self)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            throw NcSenderError.http(status: http.statusCode, path: path, detail: detail)
        }
        return (data, http)
    }
}
