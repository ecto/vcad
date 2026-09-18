import Foundation

// What the app knows about the machine itself, as opposed to the job: the
// controller's `$$` settings, the travel they imply, whether the machine has
// been homed, which alarm it is sitting in, and the spindle it actually has
// (a trim router on a dial, whose `S` word does nothing).
//
// This exists because of 2026-09-17: the sender's setup wizard had written
// `$132=100` over a 130 mm Z travel and `$21=0`, and nothing said so. The
// controller answers `$$` on every connect — the app just threw the reply into
// the log. Now it is parsed, kept whole, and diffed against a saved baseline.

// MARK: - Settings

/// Every `$n=v` the controller reports, kept whole. Nothing is dropped as
/// uninteresting: the setting that ruins the day is the one nobody modelled.
struct CNCMachineSettings: Equatable, Sendable {
    private(set) var values: [Int: Double] = [:]
    /// The text as reported, so a value is shown the way the controller said it.
    private(set) var raw: [Int: String] = [:]

    init() {}
    init(_ values: [Int: Double]) {
        self.values = values
        raw = values.mapValues { String(format: "%g", $0) }
    }

    var isEmpty: Bool { values.isEmpty }
    var numbers: [Int] { values.keys.sorted() }
    subscript(number: Int) -> Double? { values[number] }

    /// Take one line of a `$$` listing. Returns false for anything else, so the
    /// caller can hand it every line the controller sends.
    @discardableResult mutating func ingest(_ line: String) -> Bool {
        let text = line.trimmingCharacters(in: .whitespaces)
        guard text.hasPrefix("$"), let equals = text.firstIndex(of: "=") else { return false }
        guard let number = Int(text[text.index(after: text.startIndex)..<equals]) else { return false }
        // Grbl_ESP32 appends a comment: "$130=400.000 (x max travel, mm)".
        let tail = text[text.index(after: equals)...]
        let field = tail.split(separator: " ", maxSplits: 1).first.map(String.init) ?? ""
        guard let value = Double(field), value.isFinite else { return false }
        values[number] = value
        raw[number] = field
        return true
    }

    static func parse(_ dump: String) -> Self {
        var settings = Self()
        for line in dump.components(separatedBy: .newlines) { settings.ingest(line) }
        return settings
    }

    func text(_ number: Int) -> String {
        raw[number] ?? values[number].map { String(format: "%g", $0) } ?? "—"
    }

    /// The machinist-readable name of a Grbl setting, for the baseline diff.
    /// Unknown numbers keep their `$n`, which is still actionable.
    static func label(_ number: Int) -> String {
        switch number {
        case 0: return "step pulse, µs"
        case 1: return "step idle delay, ms"
        case 2: return "step port invert mask"
        case 3: return "direction port invert mask"
        case 4: return "step enable invert"
        case 5: return "limit pins invert"
        case 6: return "probe pin invert"
        case 10: return "status report mask"
        case 11: return "junction deviation, mm"
        case 12: return "arc tolerance, mm"
        case 13: return "report inches"
        case 20: return "soft limits"
        case 21: return "hard limits"
        case 22: return "homing cycle"
        case 23: return "homing direction invert mask"
        case 24: return "homing feed, mm/min"
        case 25: return "homing seek, mm/min"
        case 26: return "homing debounce, ms"
        case 27: return "homing pull-off, mm"
        case 30: return "maximum spindle speed, rpm"
        case 31: return "minimum spindle speed, rpm"
        case 32: return "laser mode"
        case 100...102: return "\(axisName(number - 100)) steps/mm"
        case 110...112: return "\(axisName(number - 110)) maximum rate, mm/min"
        case 120...122: return "\(axisName(number - 120)) acceleration, mm/s²"
        case 130...132: return "\(axisName(number - 130)) maximum travel, mm"
        default: return "setting"
        }
    }
    private static func axisName(_ index: Int) -> String { ["X", "Y", "Z"][max(0, min(2, index))] }
}

// MARK: - Travel

/// One axis's machine-coordinate travel.
///
/// Grbl's machine space runs from `-$13x` to `0` whatever the homing direction:
/// `limits_go_home` stores a negative `max_travel` and puts the machine at
/// `-pull_off` (homing positive) or `-max + pull_off` (homing negative). The
/// direction therefore says where the tool *parks* after `$H`, not which sign
/// the travel has — which is why a stale G54 Z offset of −138 on a 130 mm axis
/// was outside travel rather than comfortably inside it.
struct CNCAxisTravel: Equatable, Sendable {
    var axis: String
    var length: Double
    var homesNegative: Bool
    var pullOff: Double

    var min: Double { -length }
    var max: Double { 0 }
    /// Where `$H` leaves this axis.
    var homePosition: Double { homesNegative ? -length + pullOff : -pullOff }

    func contains(_ value: Double) -> Bool { value >= min - 1e-9 && value <= max + 1e-9 }
    /// How far outside travel a machine coordinate is: negative below, positive
    /// above, zero inside.
    func excess(_ value: Double) -> Double {
        if value > max { return value - max }
        if value < min { return value - min }
        return 0
    }
    /// The end of this axis a machinist points at.
    func endName(positive: Bool) -> String {
        switch axis {
        case "X": return positive ? "right" : "left"
        case "Y": return positive ? "back" : "front"
        default: return positive ? "top" : "bottom"
        }
    }
}

// MARK: - Alarms

/// Grbl's alarm codes, with what each one means for the next thing the
/// operator does. ALARM:1 is the one that matters most: the controller lost a
/// step count it cannot recover, so every coordinate on screen is a guess.
enum CNCAlarm: Int, Equatable, Sendable, CaseIterable {
    case hardLimit = 1
    case softLimit = 2
    case abortCycle = 3
    case probeFailInitial = 4
    case probeFailContact = 5
    case homingFailReset = 6
    case homingFailDoor = 7
    case homingFailPullOff = 8
    case homingFailApproach = 9

    /// True when the machine position on screen can no longer be believed.
    var positionLost: Bool {
        switch self {
        case .hardLimit, .abortCycle, .homingFailReset, .homingFailDoor,
             .homingFailPullOff, .homingFailApproach:
            return true
        case .softLimit, .probeFailInitial, .probeFailContact:
            return false
        }
    }

    var text: String {
        switch self {
        case .hardLimit:
            return "ALARM:1 — a limit switch was hit during motion. The controller stopped mid-step, so the machine position is not trusted: re-home before running."
        case .softLimit:
            return "ALARM:2 — the move asked to leave the machine's travel, so the controller refused it and nothing moved. Check the work offset and where the job sits on the table."
        case .abortCycle:
            return "ALARM:3 — a reset arrived while the machine was moving, so the position is lost. Re-home before running."
        case .probeFailInitial:
            return "ALARM:4 — the probe was already touching before the move started, so nothing was probed and no zero changed."
        case .probeFailContact:
            return "ALARM:5 — the probe never made contact within the travel given, so nothing was probed and no zero changed."
        case .homingFailReset:
            return "ALARM:6 — homing was reset part way. The machine position is not trusted: home again."
        case .homingFailDoor:
            return "ALARM:7 — the safety door opened during homing. The machine position is not trusted: home again."
        case .homingFailPullOff:
            return "ALARM:8 — a limit switch was still closed after the pull-off. Clear the switch, then home again."
        case .homingFailApproach:
            return "ALARM:9 — an axis never found its limit switch within its travel. Check the switch and the travel setting, then home again."
        }
    }

    /// Parse either the pushed `ALARM:1` message or an `<Alarm:1|…>` state.
    static func parse(_ text: String) -> CNCAlarm? {
        let parts = text.trimmingCharacters(in: .whitespaces).split(separator: ":")
        guard parts.count >= 2, parts[0].lowercased() == "alarm",
              let code = Int(parts[1].prefix(while: \.isNumber)) else { return nil }
        return CNCAlarm(rawValue: code)
    }
}

// MARK: - Spindle

/// One mark on the router's speed dial.
struct CNCDialStop: Equatable, Sendable, Codable {
    var dial: Double
    var rpm: Double
}

/// The spindle as it actually is. On the Anolex the router is switched by a
/// relay: `M3 S10000` closes the relay and the number does nothing, so the
/// only thing that sets the speed is the dial under the operator's thumb.
struct CNCSpindleModel: Equatable, Sendable {
    enum Kind: String, Equatable, Sendable { case relay, pwm }
    var kind: Kind = .relay
    /// Dial marks in increasing order. Measured point: dial 2 ≈ 13,500 rpm.
    var dialTable: [CNCDialStop] = [
        .init(dial: 1, rpm: 10_000), .init(dial: 2, rpm: 13_500), .init(dial: 3, rpm: 17_000),
        .init(dial: 4, rpm: 20_500), .init(dial: 5, rpm: 24_000), .init(dial: 6, rpm: 27_500),
    ]
    /// How long the relay-switched router needs before it is at speed.
    var spinUpSeconds: Double = 3

    /// The dial mark for a commanded rpm, linearly interpolated between marks.
    /// Outside the table there is no answer — saying "dial 0.4" would be worse
    /// than saying the router cannot run that slowly.
    func dial(forRPM rpm: Double) -> Double? {
        guard kind == .relay, rpm.isFinite, let first = dialTable.first, let last = dialTable.last,
              rpm >= first.rpm - 1e-9, rpm <= last.rpm + 1e-9 else { return nil }
        for pair in zip(dialTable, dialTable.dropFirst()) where rpm <= pair.1.rpm + 1e-9 {
            let span = pair.1.rpm - pair.0.rpm
            guard span > 0 else { return pair.0.dial }
            let t = (rpm - pair.0.rpm) / span
            return pair.0.dial + t * (pair.1.dial - pair.0.dial)
        }
        return last.dial
    }

    /// "Set the router dial to 2 (≈ 13,500 rpm)" — or the honest refusal.
    func dialAdvice(forRPM rpm: Double) -> String {
        guard kind == .relay else { return "Spindle \(Int(rpm.rounded())) rpm" }
        guard let dial = dial(forRPM: rpm) else {
            let low = dialTable.first?.rpm ?? 0, high = dialTable.last?.rpm ?? 0
            return "\(speed(rpm)) rpm is outside the router's dial (\(speed(low))–\(speed(high)) rpm). Change the job's speed or fit another spindle."
        }
        return "Set the router dial to \(trim(dial)) (≈ \(speed(rpm)) rpm) — the S word only closes the relay."
    }
    private func speed(_ rpm: Double) -> String { Int(rpm.rounded()).formatted(.number.grouping(.automatic)) }
    private func trim(_ value: Double) -> String {
        let rounded = (value * 10).rounded() / 10
        return rounded == rounded.rounded() ? String(Int(rounded)) : String(format: "%.1f", rounded)
    }
}

// MARK: - The profile

/// Everything the app knows about the machine under the job. Optional on the
/// controller: before `$$` has been read there is no profile, and a consumer
/// must treat "unknown" as unknown rather than as "fine".
struct CNCMachineProfile: Equatable, Sendable {
    /// Every `$$` setting, kept whole.
    var settings = CNCMachineSettings()
    /// Per-axis travel in machine millimetres, from `$130–$132` and `$23`.
    var travelX = CNCAxisTravel(axis: "X", length: 0, homesNegative: false, pullOff: 0)
    var travelY = CNCAxisTravel(axis: "Y", length: 0, homesNegative: false, pullOff: 0)
    var travelZ = CNCAxisTravel(axis: "Z", length: 0, homesNegative: false, pullOff: 0)
    /// `$20` — the controller refuses a move that would leave travel.
    var softLimits = false
    /// `$21` — the limit switches stop motion.
    var hardLimits = false
    /// `$22` — a homing cycle exists at all.
    var homingEnabled = false
    /// `$27`, mm.
    var pullOff = 0.0
    /// `$110–$112`, mm/min.
    var maxRate = CNCVector()
    /// `$120–$122`, mm/s².
    var acceleration = CNCVector()
    /// `$30`, rpm.
    var maxSpindleRPM = 0.0
    /// Whether this session has seen a homing cycle complete. Grbl does not
    /// report it, so it is tracked here and dropped by anything that loses the
    /// position (a hard limit, a reset, a reconnect).
    var homed = false
    /// The work offset the controller reports for the active coordinate system.
    var workOffset: CNCVector?
    /// Which coordinate system that offset belongs to.
    var workspace = "Unknown"
    /// The alarm the controller is sitting in, if any.
    var alarm: CNCAlarm?
    /// The stock's rotation about +Z measured by the two-point edge probe,
    /// degrees, counter-clockwise positive. See `CNCSkewProbe`.
    var skewDegrees: Double?
    var spindle = CNCSpindleModel()
    /// The baseline this machine's settings are compared against, if saved.
    var baselineName: String?
    /// Settings that differ from that baseline. Empty when there is no
    /// baseline — which is not the same as "nothing changed", so the UI says
    /// which case it is.
    var baselineDiff: [CNCSettingDelta] = []
    /// True when this profile describes the Simulator rather than hardware.
    var simulated = false

    var travels: [CNCAxisTravel] { [travelX, travelY, travelZ] }
    /// True once `$$` produced travel numbers to check a job against.
    var hasTravel: Bool { travels.allSatisfy { $0.length > 0 } }

    /// Build a profile from a `$$` listing. Everything derived is derived here,
    /// so there is one place where a setting number becomes a meaning.
    init(settings: CNCMachineSettings) {
        self.settings = settings
        let mask = Int(settings[23] ?? 0)
        pullOff = settings[27] ?? 0
        travelX = CNCAxisTravel(axis: "X", length: settings[130] ?? 0, homesNegative: mask & 1 != 0, pullOff: pullOff)
        travelY = CNCAxisTravel(axis: "Y", length: settings[131] ?? 0, homesNegative: mask & 2 != 0, pullOff: pullOff)
        travelZ = CNCAxisTravel(axis: "Z", length: settings[132] ?? 0, homesNegative: mask & 4 != 0, pullOff: pullOff)
        softLimits = (settings[20] ?? 0) != 0
        hardLimits = (settings[21] ?? 0) != 0
        homingEnabled = (settings[22] ?? 0) != 0
        maxRate = CNCVector(x: settings[110] ?? 0, y: settings[111] ?? 0, z: settings[112] ?? 0)
        acceleration = CNCVector(x: settings[120] ?? 0, y: settings[121] ?? 0, z: settings[122] ?? 0)
        maxSpindleRPM = settings[30] ?? 0
    }

    func travel(axis: String) -> CNCAxisTravel? {
        switch axis.uppercased() {
        case "X": return travelX
        case "Y": return travelY
        case "Z": return travelZ
        default: return nil
        }
    }
}

// MARK: - Baseline

/// One setting that differs from the saved baseline.
struct CNCSettingDelta: Equatable, Sendable, Identifiable {
    var number: Int
    var baseline: Double?
    var current: Double?
    var id: Int { number }
    var label: String { CNCMachineSettings.label(number) }

    /// "$132 Z maximum travel, mm · 130 → 100". The numbers are both there
    /// because "changed" without them is a shrug, and `$132=100` on a 130 mm
    /// axis is the whole story.
    var summary: String {
        "$\(number) \(label) · \(Self.number(baseline)) → \(Self.number(current))"
    }
    private static func number(_ value: Double?) -> String {
        guard let value, value.isFinite else { return "not set" }
        return value == value.rounded() ? String(Int(value)) : String(format: "%g", value)
    }
}

/// A saved `$$` snapshot for one machine, so the next connect can say what
/// moved. Stored in `UserDefaults` through `Prefs`.
struct CNCMachineBaseline: Equatable, Sendable, Codable {
    var name: String
    var savedAt: Date
    /// Keyed by the setting number as a string, so it round-trips through JSON.
    var settings: [String: Double]

    init(name: String, savedAt: Date = Date(), settings: CNCMachineSettings) {
        self.name = name
        self.savedAt = savedAt
        self.settings = Dictionary(uniqueKeysWithValues: settings.values.map { (String($0.key), $0.value) })
    }

    var values: [Int: Double] {
        Dictionary(uniqueKeysWithValues: settings.compactMap { key, value in
            Int(key).map { ($0, value) }
        })
    }

    /// What changed between this baseline and what the controller just said.
    /// A setting that exists on only one side is a difference too: a firmware
    /// update that drops a setting changes the machine as surely as an edit.
    func diff(against current: CNCMachineSettings) -> [CNCSettingDelta] {
        let mine = values
        let numbers = Set(mine.keys).union(current.values.keys).sorted()
        return numbers.compactMap { number in
            let before = mine[number], after = current[number]
            if let before, let after, abs(before - after) <= 1e-9 { return nil }
            if before == nil && after == nil { return nil }
            return CNCSettingDelta(number: number, baseline: before, current: after)
        }
    }
}

/// The AnoleX 4030-Evo Ultra 2 as it is configured on the bench (Grbl_ESP32
/// 1.3a, build 20211103). Shipped as data so the first connect has something
/// to diff against even before anyone presses "Save as baseline" — this is the
/// listing that `$132=100` and `$21=0` differed from on 2026-09-17.
enum CNCAnolexBaseline {
    static let name = "AnoleX 4030 Ultra 2 (factory + bench)"
    static let dump = """
    $0=10
    $1=255
    $2=0
    $3=0
    $4=0
    $5=1
    $6=0
    $10=1
    $11=0.010
    $12=0.002
    $13=0
    $20=1
    $21=1
    $22=1
    $23=3
    $24=100.000
    $25=1000.000
    $26=250
    $27=3.000
    $30=10000
    $31=0
    $32=0
    $100=80.000
    $101=80.000
    $102=800.000
    $110=4000.000
    $111=4000.000
    $112=2000.000
    $120=300.000
    $121=300.000
    $122=200.000
    $130=400.000
    $131=300.000
    $132=130.000
    """
    static var settings: CNCMachineSettings { CNCMachineSettings.parse(dump) }
    static var baseline: CNCMachineBaseline {
        CNCMachineBaseline(name: name, savedAt: Date(timeIntervalSince1970: 1_758_067_200), settings: settings)
    }
}

/// Where baselines live between launches. One per machine, keyed by the host
/// the controller is reached on, so a second machine cannot inherit the
/// first's travel.
enum CNCMachineBaselineStore {
    static let key = "vcad.cnc.baselines"

    static func load(machine: String) -> CNCMachineBaseline? {
        guard let data = UserDefaults.standard.data(forKey: key),
              let all = try? JSONDecoder().decode([String: CNCMachineBaseline].self, from: data) else {
            return machine == defaultMachine ? CNCAnolexBaseline.baseline : nil
        }
        // The shipped AnoleX listing is the fallback, never an override: once
        // the owner saves their own, theirs is the truth.
        return all[machine] ?? (machine == defaultMachine ? CNCAnolexBaseline.baseline : nil)
    }

    static func save(_ baseline: CNCMachineBaseline, machine: String) {
        var all = stored()
        all[machine] = baseline
        if let data = try? JSONEncoder().encode(all) { UserDefaults.standard.set(data, forKey: key) }
    }

    static func forget(machine: String) {
        var all = stored()
        all.removeValue(forKey: machine)
        if let data = try? JSONEncoder().encode(all) { UserDefaults.standard.set(data, forKey: key) }
    }

    private static func stored() -> [String: CNCMachineBaseline] {
        guard let data = UserDefaults.standard.data(forKey: key),
              let all = try? JSONDecoder().decode([String: CNCMachineBaseline].self, from: data) else { return [:] }
        return all
    }

    /// The machine the shipped baseline describes.
    static let defaultMachine = "192.168.2.226"
}
