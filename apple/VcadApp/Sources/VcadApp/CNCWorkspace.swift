import SwiftUI
import simd
import CVcadFFI
import UniformTypeIdentifiers

struct CNCSetup: Codable, Equatable, Sendable {
    var operation = "face"
    var width = 40.0
    var height = 30.0
    var depth = 1.0
    var diameter = 3.175
    var stepdown = 0.5
    var stepover = 1.5
    var feed = 400.0
    var plunge = 100.0
    var rpm = 10000.0
    var clearance = 5.0
}
struct CNCMove: Decodable, Sendable {
    var to: [Double]
    var rapid: Bool
    var feed: Double?
    var point: SIMD3<Float> { .init(Float(to[0]), Float(to[1]), Float(to[2])) }
}
struct CNCProgram: Decodable, Sendable {
    var gcode: String
    var moves: [CNCMove]
}

enum CNCMode: String, CaseIterable, Identifiable {
    case setup = "Setup", toolpaths = "Toolpaths", machine = "Machine"
    var id: String { rawValue }
}
enum CNCSelection: Hashable { case stock, origin, tool, operation(UUID) }

struct CNCOperation: Identifiable {
    let id = UUID()
    var setup: CNCSetup
    var program: CNCProgram?
    var generatedSetup: CNCSetup?
    var preview = CNCPreview()
    var current: Bool { program != nil && setup == generatedSetup }
    var name: String { Self.name(setup.operation) }
    static func name(_ kind: String) -> String {
        switch kind { case "pocket": return "Clear pocket"; case "profile": return "Outer profile"; default: return "Face stock" }
    }
    var symbol: String {
        switch setup.operation { case "pocket": return "square.dashed.inset.filled"; case "profile": return "square.dashed"; default: return "square.3.layers.3d" }
    }
}

/// Timed interpolation of the generated path, independent of controller state.
/// Rapid travel uses a labelled estimate; cutting feeds come from the CAM bridge.
struct CNCPreview {
    struct Segment { var from: SIMD3<Float>; var to: SIMD3<Float>; var start: Double; var end: Double }
    var segments: [Segment] = []
    var duration: Double = 0
    var first: SIMD3<Float>?
    init() {}
    init(program: CNCProgram, fallbackFeed: Double) {
        first = program.moves.first?.point
        for pair in zip(program.moves, program.moves.dropFirst()) {
            let distance = Double(simd_distance(pair.0.point, pair.1.point))
            let feed = pair.1.rapid ? 3000 : (pair.1.feed ?? fallbackFeed)
            guard distance.isFinite, distance > 0, feed.isFinite, feed > 0 else { continue }
            let end = duration + distance / feed * 60
            segments.append(.init(from: pair.0.point, to: pair.1.point, start: duration, end: end))
            duration = end
        }
    }
    func position(at fraction: Double) -> SIMD3<Float>? {
        guard let last = segments.last else { return first }
        let time = max(0, min(1, fraction.isFinite ? fraction : 0)) * duration
        var lo = 0, hi = segments.count - 1
        while lo < hi {
            let mid = (lo + hi) / 2
            if segments[mid].end < time { lo = mid + 1 } else { hi = mid }
        }
        let segment = segments[lo]
        if time >= duration { return last.to }
        let t = Float(max(0, min(1, (time - segment.start) / (segment.end - segment.start))))
        return segment.from + (segment.to - segment.from) * t
    }
}

@MainActor @Observable
final class CNCWorkspace {
    let machine = CNCController()
    var inspectorTab: CNCInspectorTab = .inspector
    var followSpindle = false
    var autoFit = true
    private(set) var importedProgram: CNCProgram?
    private(set) var importedName = ""
    private(set) var usesImportedProgram = false
    private var importedPreview = CNCPreview()
    var macros: [CNCMacro] = []
    var leftPanelShown = true
    var rightPanelShown = true
    var bottomPanelShown = true
    var shown = false {
        didSet { if !shown { pausePreview() } }
    }
    var mode: CNCMode = .setup {
        didSet {
            if mode != .toolpaths { pausePreview() }
            if mode == .toolpaths {
                if case .operation = selection {} else { selection = .operation(selectedOperationID) }
            } else if mode == .setup, case .operation = selection {
                selection = .stock
            }
            previewTick += 1
        }
    }
    var selection: CNCSelection {
        didSet {
            if case .operation(let id) = selection, operations.contains(where: { $0.id == id }) {
                selectedOperationID = id
                pausePreview(); previewFraction = 0; revision += 1
            }
        }
    }
    private(set) var operations: [CNCOperation]
    private var selectedOperationID: UUID
    var overlay = true
    var showStock = true
    var showClearance = false
    var showPart = true
    var origin = CNCVector() { didSet { setupConfirmed = false } }
    var stockThickness = 10.0 { didSet { setupConfirmed = false } }
    var jogStep = 1.0
    var jogFeed = 300.0
    var setupConfirmed = false
    private(set) var generating = false
    private(set) var revision = 0
    var error: String?
    var previewFraction = 0.0 { didSet { previewTick += 1 } }
    var previewSpeed = 5.0
    private(set) var previewTick = 0
    private(set) var previewPlaying = false
    private var previewTask: Task<Void, Never>?

    init() {
        let initial = CNCOperation(setup: CNCSetup())
        operations = [initial]; selectedOperationID = initial.id
        selection = .stock
        if let data = UserDefaults.standard.data(forKey: "cnc.console.macros"),
           let stored = try? JSONDecoder().decode([CNCMacro].self, from: data) { macros = stored }
    }
    private var index: Int { operations.firstIndex(where: { $0.id == selectedOperationID }) ?? 0 }
    var selectedOperation: CNCOperation { operations[index] }
    var setup: CNCSetup {
        get { operations[index].setup }
        set { guard !machine.active else { return }; operations[index].setup = newValue; setupConfirmed = false }
    }
    var stockWidth: Double {
        get { operations[0].setup.width }
        set { changeShared { $0.width = newValue } }
    }
    var stockHeight: Double {
        get { operations[0].setup.height }
        set { changeShared { $0.height = newValue } }
    }
    var toolDiameter: Double {
        get { operations[0].setup.diameter }
        set { changeShared { $0.diameter = newValue } }
    }
    private func changeShared(_ edit: (inout CNCSetup) -> Void) {
        guard !machine.active else { return }
        for i in operations.indices { edit(&operations[i].setup) }
        setupConfirmed = false; revision += 1
    }
    var program: CNCProgram? { usesImportedProgram ? importedProgram : selectedOperation.program }
    var generatedSetup: CNCSetup? { selectedOperation.generatedSetup }
    var current: Bool { usesImportedProgram ? importedProgram != nil : selectedOperation.current }
    var preview: CNCPreview { usesImportedProgram ? importedPreview : selectedOperation.preview }
    var previewTitle: String { usesImportedProgram ? importedName : selectedOperation.name }
    var jobTitle: String { usesImportedProgram ? importedName : "Manufacturing job" }
    var displayPrograms: [CNCProgram] {
        if usesImportedProgram { return importedProgram.map { [$0] } ?? [] }
        if mode == .machine { return operations.compactMap(\.program) }
        return program.map { [$0] } ?? []
    }
    var previewPosition: SIMD3<Float>? { preview.position(at: previewFraction) }
    var jobCurrent: Bool { usesImportedProgram ? importedProgram != nil : operations.allSatisfy(\.current) }
    var sameTool: Bool { operations.allSatisfy { $0.setup.diameter == toolDiameter } }
    var jobDuration: Double { usesImportedProgram ? importedPreview.duration : operations.reduce(0) { $0 + $1.preview.duration } }
    var stockValid: Bool {
        stockThickness.isFinite && stockThickness > 0 && stockThickness < 1000
            && operations.allSatisfy { $0.setup.depth <= stockThickness }
    }
    var displayKey: String {
        "\(mode.rawValue)-\(shown)-\(showStock)-\(showClearance)-\(showPart)-\(stockWidth)-\(stockHeight)-\(setup.clearance)"
    }
    var runBlocker: String? {
        if generating { return "Wait for toolpath generation." }
        if !jobCurrent { return "Generate every operation after editing the setup." }
        if !usesImportedProgram && !sameTool { return "This job requires one tool diameter for all operations." }
        if !usesImportedProgram && !stockValid { return "Check stock thickness and operation depths." }
        if !machine.connected { return "Connect the Anolex or choose Simulator." }
        if machine.faulted { return machine.error ?? "Reconnect after resolving the controller fault." }
        if !machine.status.isFresh { return "Waiting for fresh controller telemetry." }
        if !machine.g54Active { return "Select G54 work coordinates on the controller." }
        if !machine.canStart { return "Waiting for Idle and a known work position." }
        if !setupConfirmed { return "Confirm the tool, workholding, clearance and G54 zero." }
        return nil
    }
    /// Strip only the intermediate program-end blocks, retaining each operation's
    /// retract and spindle stop. All operations use one manually installed tool.
    var jobCode: String? {
        if usesImportedProgram { return importedProgram?.gcode }
        guard jobCurrent, sameTool else { return nil }
        return operations.enumerated().map { i, op in
            let code = op.program!.gcode
            return i == operations.count - 1 ? code : code.components(separatedBy: .newlines)
                .filter { $0.trimmingCharacters(in: .whitespaces) != "M2" }.joined(separator: "\n")
        }.joined(separator: "\n")
    }
    func startJob() {
        guard runBlocker == nil, let code = jobCode else { return }
        pausePreview(); machine.start(code)
    }
    func select(_ selection: CNCSelection) {
        guard !machine.active else { return }
        useGeneratedJob()
        inspectorTab = .inspector
        self.selection = selection
        switch selection { case .operation: mode = .toolpaths; default: mode = .setup }
    }
    func addOperation(_ kind: String) {
        guard !machine.active, !generating, ["face", "pocket", "profile"].contains(kind) else { return }
        var spec = setup; spec.operation = kind
        let operation = CNCOperation(setup: spec)
        operations.append(operation); select(.operation(operation.id)); setupConfirmed = false
    }
    func removeSelectedOperation() {
        guard !machine.active, !generating, operations.count > 1 else { return }
        operations.remove(at: index)
        select(.operation(operations[0].id)); setupConfirmed = false
    }
    func moveSelectedOperation(by offset: Int) {
        guard !machine.active, !generating, operations.indices.contains(index + offset) else { return }
        operations.swapAt(index, index + offset); setupConfirmed = false; revision += 1
    }
    func generate(all: Bool = false) {
        guard !generating, !machine.active else { return }
        let requests = all ? operations.map { ($0.id, $0.setup) } : [(selectedOperation.id, setup)]
        generating = true; error = nil; setupConfirmed = false; pausePreview()
        Task {
            defer { generating = false }
            for (id, spec) in requests {
                do {
                    let result = try await Task.detached { try Self.generateProgram(spec) }.value
                    guard let i = operations.firstIndex(where: { $0.id == id }) else { continue }
                    operations[i].program = result; operations[i].generatedSetup = spec
                    operations[i].preview = CNCPreview(program: result, fallbackFeed: spec.feed)
                    revision += 1; previewFraction = 0
                } catch { self.error = error.localizedDescription; return }
            }
        }
    }
    nonisolated private static func generateProgram(_ spec: CNCSetup) throws -> CNCProgram {
        let input = String(decoding: try JSONEncoder().encode(spec), as: UTF8.self)
        return try input.withCString { request in
            guard let raw = vcad_cam_generate(request) else {
                var count = 0
                let message = vcad_last_error(&count).map { String(decoding: UnsafeBufferPointer(start: $0, count: count), as: UTF8.self) }
                throw CNCError.message(message ?? "CAM generation failed")
            }
            defer { vcad_cam_free(raw) }
            return try JSONDecoder().decode(CNCProgram.self, from: Data(String(cString: raw).utf8))
        }
    }
    func togglePreview() {
        if previewPlaying { pausePreview(); return }
        guard !machine.active, program != nil, preview.duration > 0 else { return }
        if previewFraction >= 1 { previewFraction = 0 }
        mode = .toolpaths; previewPlaying = true
        previewTask = Task { [weak self] in
            var last = Date()
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(33))
                guard !Task.isCancelled, let self, self.previewPlaying else { break }
                let now = Date(), elapsed = min(0.25, now.timeIntervalSince(last)); last = now
                self.previewFraction = min(1, self.previewFraction + elapsed * self.previewSpeed / max(0.001, self.preview.duration))
                if self.previewFraction >= 1 { self.pausePreview(); break }
            }
        }
    }
    func pausePreview() { previewPlaying = false; previewTask?.cancel(); previewTask = nil }
    func export(job: Bool = false) {
        guard let code = job ? jobCode : (current ? program?.gcode : nil) else { return }
        let panel = NSSavePanel()
        panel.nameFieldStringValue = usesImportedProgram ? importedName : job ? "anolex-job.nc" : "anolex-\(setup.operation).nc"
        panel.allowedContentTypes = [UTType(filenameExtension: "nc") ?? .plainText]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do { try code.write(to: url, atomically: true, encoding: .utf8) }
        catch { self.error = error.localizedDescription }
    }
    func importFile() {
        guard !machine.active, !generating else { return }
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [UTType(filenameExtension: "nc") ?? .plainText, UTType(filenameExtension: "gcode") ?? .plainText, .plainText]
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize ?? 0
            guard size <= 8_000_000 else { throw CNCError.message("Choose a G-code file smaller than 8 MB.") }
            try importProgram(String(contentsOf: url, encoding: .utf8), name: url.lastPathComponent)
        } catch { self.error = error.localizedDescription }
    }
    func importProgram(_ code: String, name: String) throws {
        guard !machine.active, !generating else { return }
        let result = try CNCImport.parse(code)
        importedProgram = result; importedName = name
        importedPreview = CNCPreview(program: result, fallbackFeed: 400)
        useImportedJob(); error = nil
    }
    func useImportedJob() {
        guard !machine.active, !generating, importedProgram != nil else { return }
        pausePreview(); usesImportedProgram = true; setupConfirmed = false
        previewFraction = 0; revision += 1; mode = .toolpaths; inspectorTab = .gcode
    }
    func useGeneratedJob() {
        guard !machine.active else { return }
        if usesImportedProgram {
            pausePreview(); usesImportedProgram = false; setupConfirmed = false
            previewFraction = 0; revision += 1
        }
    }
    func saveMacro(name: String, command: String) {
        guard !name.trimmingCharacters(in: .whitespaces).isEmpty, CNCCommands.validManual(command) else { error = "Give the macro a name and one valid command."; return }
        macros.append(.init(name: name, command: command)); saveMacros()
    }
    func removeMacro(_ id: UUID) { macros.removeAll { $0.id == id }; saveMacros() }
    private func saveMacros() {
        if let data = try? JSONEncoder().encode(macros) { UserDefaults.standard.set(data, forKey: "cnc.console.macros") }
    }
    static func durationLabel(_ value: Double) -> String {
        guard value.isFinite, value >= 0, value < Double(Int.max / 2) else { return "—" }
        let seconds = Int(value.rounded())
        return String(format: "%02d:%02d", seconds / 60, seconds % 60)
    }
}
