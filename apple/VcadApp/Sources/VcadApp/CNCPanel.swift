import SwiftUI
import RealityKit
import CVcadFFI
import UniformTypeIdentifiers

struct CNCBar: View {
    @Bindable var cnc: CNCWorkspace
    var body: some View {
        HStack(spacing: 12) {
            Button {
                if !cnc.shown { ReleaseWindowController.shared.setWindowed(true) }
                cnc.shown.toggle()
            } label: { Label("CNC", systemImage: "point.3.connected.trianglepath.dotted") }
            Text(cnc.machine.summary).font(.system(size: 11, design: .monospaced))
            if cnc.machine.connected {
                Button("Hold") { cnc.machine.hold() }
                if cnc.machine.active {
                    Text("\(cnc.machine.stream.acknowledged)/\(cnc.machine.stream.lines.count) accepted")
                        .font(.caption.monospacedDigit())
                }
            }
        }
        .buttonStyle(.plain)
        .padding(.horizontal, 14).padding(.vertical, 8).glassCard(22)
    }
}

struct CNCPanel: View {
    @Bindable var cnc: CNCWorkspace
    @State private var confirmZero = false
    @State private var confirmHome = false
    @State private var confirmRun = false
    private var machine: CNCController { cnc.machine }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Anolex · CNC").font(.headline)
                Spacer()
                Button { cnc.shown = false } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    connection
                    Divider()
                    setup
                    Divider()
                    telemetry
                    controls
                    if let error = cnc.error ?? machine.error {
                        Text(error).font(.caption).foregroundStyle(.red).textSelection(.enabled)
                    }
                    DisclosureGroup("Controller log") {
                        Text(machine.log.suffix(20).joined(separator: "\n"))
                            .font(.system(size: 9, design: .monospaced)).textSelection(.enabled)
                    }
                    if let program = cnc.program {
                        DisclosureGroup("G-code · \(program.moves.count) moves") {
                            Text(program.gcode).font(.system(size: 9, design: .monospaced)).textSelection(.enabled)
                        }
                    }
                }
            }
        }
        .padding(14).frame(width: 330).frame(maxHeight: 680).glassCard()
        .confirmationDialog("Set G54 XYZ zero at the current tool position?", isPresented: $confirmZero) {
            Button("Set work zero") { cnc.setupConfirmed = false; machine.zeroWork() }
        } message: { Text("This changes all three G54 axes. Put the tool at the intended stock origin first.") }
        .confirmationDialog("Home the machine?", isPresented: $confirmHome) {
            Button("Run homing cycle") { cnc.setupConfirmed = false; machine.home() }
        } message: { Text("The axes will move toward the configured homing switches.") }
        .confirmationDialog("Run the generated toolpath?", isPresented: $confirmRun) {
            Button(machine.demo ? "Run in simulator" : "Start machining") {
                if cnc.current && cnc.setupConfirmed, let code = cnc.program?.gcode { machine.start(code) }
            }
        } message: { Text("G54 · millimetres · stock top Z0. The program retracts, starts M3, and cuts to −\(cnc.setup.depth.formatted()) mm.") }
        .onChange(of: cnc.setup) { _, _ in cnc.setupConfirmed = false }
        .onChange(of: machine.connected) { _, _ in cnc.setupConfirmed = false }
        .onChange(of: cnc.stockThickness) { _, _ in cnc.setupConfirmed = false }
    }

    private var connection: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text("4030 Ultra 2 · Grbl_ESP32").font(.subheadline)
            HStack {
                TextField("Host", text: Binding(get: { machine.host }, set: { machine.host = $0 }))
                TextField("Port", text: Binding(get: { machine.port }, set: { machine.port = $0 })).frame(width: 50)
            }.textFieldStyle(.roundedBorder).disabled(machine.connected || machine.connecting)
            HStack {
                if machine.connected || machine.connecting {
                    Button("Disconnect") { machine.disconnect() }
                } else {
                    Button("Connect") { machine.connect() }
                    Button("Simulator") { machine.connect(simulated: true) }
                }
                Spacer()
                Text(machine.summary).font(.caption).foregroundStyle(machine.status.isFresh ? .green : .secondary)
            }
            Text(machine.firmware).font(.system(size: 9, design: .monospaced)).foregroundStyle(.secondary)
        }
    }

    private var setup: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Rectangular toolpath").font(.subheadline.bold())
            Picker("Operation", selection: $cnc.setup.operation) {
                Text("Face").tag("face"); Text("Pocket").tag("pocket"); Text("Outside profile").tag("profile")
            }
            Grid(alignment: .leading, horizontalSpacing: 10, verticalSpacing: 5) {
                number("Width / height · mm", $cnc.setup.width, $cnc.setup.height)
                number("Depth / stock · mm", $cnc.setup.depth, $cnc.stockThickness)
                number("Tool Ø / stepover · mm", $cnc.setup.diameter, $cnc.setup.stepover)
                number("Stepdown / clearance", $cnc.setup.stepdown, $cnc.setup.clearance)
                number("Feed / plunge · mm/min", $cnc.setup.feed, $cnc.setup.plunge)
                GridRow {
                    Text("Spindle · RPM")
                    TextField("RPM", value: $cnc.setup.rpm, format: .number).textFieldStyle(.roundedBorder)
                }
            }.font(.caption)
            Text("XY lower-left, stock top Z0 in G54. Facing and profiles extend beyond the rectangle by the tool radius.")
                .font(.caption).foregroundStyle(.secondary)
            HStack {
                Button(cnc.generating ? "Generating…" : "Generate") { cnc.generate() }.disabled(cnc.generating)
                Button("Export .nc") { cnc.export() }.disabled(!cnc.current)
                Toggle("3D", isOn: $cnc.overlay).toggleStyle(.checkbox)
            }
            if cnc.program != nil && !cnc.current { Text("Setup changed — regenerate before running.").font(.caption).foregroundStyle(.orange) }
            DisclosureGroup("Place G54 origin in CAD · mm") {
                HStack {
                    TextField("X", value: $cnc.origin.x, format: .number)
                    TextField("Y", value: $cnc.origin.y, format: .number)
                    TextField("Z", value: $cnc.origin.z, format: .number)
                }.textFieldStyle(.roundedBorder)
                Text("Viewport alignment only; does not set machine zero.").font(.caption)
            }
        }.disabled(machine.active)
    }
    private func number(_ title: String, _ a: Binding<Double>, _ b: Binding<Double>) -> some View {
        GridRow {
            Text(title)
            HStack {
                TextField("", value: a, format: .number)
                TextField("", value: b, format: .number)
            }.textFieldStyle(.roundedBorder)
        }
    }
    private var telemetry: some View {
        TimelineView(.periodic(from: .now, by: 0.5)) { _ in
            VStack(alignment: .leading, spacing: 5) {
                Text(machine.status.isFresh ? machine.summary : "Position unavailable / stale").font(.subheadline.bold())
                position("Work", machine.status.work)
                position("Machine", machine.status.machine)
                Text("Feed \(machine.status.feed.formatted()) mm/min · Spindle \(machine.status.rpm.formatted()) RPM")
                    .font(.caption)
                Text("Job: \(machine.stream.phase.rawValue) · \(machine.stream.acknowledged)/\(machine.stream.lines.count) accepted")
                    .font(.caption.monospacedDigit())
            }.foregroundStyle(machine.status.isFresh ? .primary : .secondary)
        }
    }
    private func position(_ label: String, _ p: CNCVector?) -> some View {
        Text(p.map { String(format: "%@: X %.3f  Y %.3f  Z %.3f", label, $0.x, $0.y, $0.z) } ?? "\(label): —")
            .font(.system(size: 10, design: .monospaced))
    }
    private var controls: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Button("Home…") { confirmHome = true }.disabled(!machine.canHome)
                Button("Zero XYZ…") { confirmZero = true }.disabled(!machine.canCommand)
                Button("Cancel jog") { machine.cancelJog() }.disabled(!machine.connected)
            }
            HStack {
                Picker("Step", selection: $cnc.jogStep) {
                    Text("0.1 mm").tag(0.1); Text("1 mm").tag(1.0); Text("10 mm").tag(10.0)
                }
                TextField("Jog feed", value: $cnc.jogFeed, format: .number).frame(width: 65)
            }.font(.caption)
            HStack {
                ForEach(["X", "Y", "Z"], id: \.self) { axis in
                    Button("\(axis)−") { machine.jog(axis: axis, distance: -cnc.jogStep, feed: cnc.jogFeed) }
                    Button("\(axis)+") { machine.jog(axis: axis, distance: cnc.jogStep, feed: cnc.jogFeed) }
                }
            }.disabled(!machine.canCommand)
            if machine.connected && !machine.g54Active {
                Button("Use G54 work coordinates") { cnc.setupConfirmed = false; machine.selectG54() }
                    .disabled(!machine.canCommand)
            }
            Button("Stop spindle (M5)") { machine.stopSpindle() }.disabled(!machine.canCommand)
            Toggle("Tool, stock, G54 zero, clearance and workholding checked", isOn: $cnc.setupConfirmed)
                .font(.caption).disabled(machine.active)
            Text("No fixture collision or travel-limit verification. Profile has no holding tabs. Feed hold and soft reset are controller commands, not a hardware E-stop.")
                .font(.caption2).foregroundStyle(.secondary)
            HStack {
                Button("Run…") { confirmRun = true }
                    .disabled(!cnc.current || !cnc.setupConfirmed || !machine.canStart || !cnc.stockThickness.isFinite || cnc.stockThickness <= 0 || cnc.setup.depth > cnc.stockThickness)
                Button("Hold") { machine.hold() }.disabled(!machine.connected)
                Button("Resume") { machine.resume() }.disabled(!machine.status.isFresh || machine.status.state != "Hold:0" || machine.faulted)
                Button("Soft reset") { machine.reset() }.disabled(!machine.connected)
            }
        }.buttonStyle(.bordered).controlSize(.small)
    }
}

/// All entities live in the existing kernel-mm frame, inheriting CAD camera,
/// centering and Z-up transforms. Path meshes rebuild only when the job changes.
@MainActor
func syncCNCOverlay(_ cnc: CNCWorkspace, in parent: Entity, model: EditorModel? = nil) {
    if let model {
        let show = !model.isWindowed || !cnc.shown || cnc.showPart
        for child in parent.children {
            if child.name.hasPrefix("part"), let index = Int(child.name.dropFirst(4)) {
                child.isEnabled = show && model.isPartVisible(index)
            } else if child.name.hasPrefix("inst") {
                child.isEnabled = show
            }
            if child.name.hasPrefix("part") || child.name.hasPrefix("inst") {
                let ghost = model.isWindowed && cnc.shown && cnc.mode == .toolpaths
                child.components.set(OpacityComponent(opacity: ghost ? 0.28 : 1))
            }
        }
    }
    let root: Entity
    if let existing = parent.findEntity(named: "cncRoot") { root = existing }
    else { root = Entity(); root.name = "cncRoot"; parent.addChild(root) }
    root.isEnabled = cnc.overlay && (cnc.shown || cnc.program != nil || (cnc.machine.g54Active && cnc.machine.status.work != nil))
    guard root.isEnabled else { return }
    guard [cnc.origin.x, cnc.origin.y, cnc.origin.z].allSatisfy({ $0.isFinite && abs($0) < 1_000_000 }) else { root.isEnabled = false; return }
    root.position = cnc.origin.floats
    let key = "path-\(cnc.revision)-\(cnc.stockThickness)-\(cnc.displayKey)"
    if root.findEntity(named: key) == nil {
        let spec = cnc.mode == .setup ? cnc.setup : (cnc.generatedSetup ?? cnc.setup)
        root.children.removeAll()
        let group = Entity(); group.name = key
        for rapid in [false, true] where cnc.mode != .setup {
            for (index, program) in cnc.displayPrograms.enumerated() {
                if let mesh = cncLineMesh(program.moves, rapid: rapid) {
                    let path = ModelEntity(mesh: mesh, materials: [UnlitMaterial(color: rapid ? .cyan : .orange)])
                    path.name = "cncPath-\(index)-\(rapid)"
                    group.addChild(path)
                }
            }
        }
        if !cnc.usesImportedProgram && cnc.showStock && cnc.stockThickness.isFinite && cnc.stockThickness > 0 && cnc.stockThickness < 1000
            && spec.width.isFinite && spec.height.isFinite && spec.width > 0 && spec.height > 0 && max(spec.width, spec.height) <= 1000 {
            var material = SimpleMaterial(color: .gray.withAlphaComponent(0.12), isMetallic: false)
            material.faceCulling = .none
            let stock = ModelEntity(mesh: .generateBox(size: [Float(spec.width), Float(spec.height), Float(cnc.stockThickness)]), materials: [material])
            stock.position = [Float(spec.width / 2), Float(spec.height / 2), -Float(cnc.stockThickness / 2)]
            group.addChild(stock)
        }
        if !cnc.usesImportedProgram && cnc.showClearance && spec.clearance.isFinite && spec.clearance > 0 && spec.clearance < 1000 && spec.width > 0 && spec.height > 0 && max(spec.width, spec.height) <= 1000 {
            let plane = ModelEntity(mesh: .generateBox(size: [Float(spec.width), Float(spec.height), 0.1]), materials: [SimpleMaterial(color: .cyan.withAlphaComponent(0.12), isMetallic: false)])
            plane.position = [Float(spec.width / 2), Float(spec.height / 2), Float(spec.clearance)]
            plane.name = "cncClearance"; group.addChild(plane)
        }
        let origin = ModelEntity(mesh: .generateSphere(radius: 0.6), materials: [UnlitMaterial(color: .white)])
        origin.name = "cncOrigin"; group.addChild(origin)
        let radius = spec.diameter.isFinite && spec.diameter > 0 && spec.diameter < 100 ? Float(spec.diameter / 2) : 1.5
        let previewTool = ModelEntity(mesh: .generateCylinder(height: 12, radius: radius), materials: [SimpleMaterial(color: .orange.withAlphaComponent(0.65), isMetallic: false)])
        previewTool.name = "cncPreviewTool"
        previewTool.orientation = simd_quatf(angle: .pi / 2, axis: [1, 0, 0])
        group.addChild(previewTool)
        let tool = ModelEntity(mesh: .generateCylinder(height: 12, radius: radius), materials: [UnlitMaterial(color: .green)])
        tool.name = "cncTool"
        tool.orientation = simd_quatf(angle: .pi / 2, axis: [1, 0, 0])
        group.addChild(tool); root.addChild(group)
    }
    if let previewTool = root.findEntity(named: "cncPreviewTool") {
        previewTool.isEnabled = cnc.mode == .toolpaths && cnc.previewPosition != nil
        if let position = cnc.previewPosition { previewTool.position = position + [0, 0, 6] }
    }
    if let tool = root.findEntity(named: "cncTool") {
        tool.isEnabled = cnc.machine.status.isFresh && cnc.machine.g54Active && cnc.machine.status.work != nil
        if let position = cnc.machine.status.work { tool.position = position.floats + [0, 0, 6] }
    }
}

@MainActor
private func cncLineMesh(_ moves: [CNCMove], rapid: Bool) -> MeshResource? {
    var vertices: [SIMD3<Float>] = []
    var indices: [UInt32] = []
    var previous: SIMD3<Float>?
    for move in moves {
        let b = move.point
        defer { previous = b }
        guard let a = previous, move.rapid == rapid, simd_length(b-a) > 0.0001 else { continue }
        let d = simd_normalize(b-a)
        let u = simd_normalize(simd_cross(d, abs(d.z) < 0.9 ? SIMD3<Float>(0,0,1) : SIMD3<Float>(0,1,0))) * 0.10
        let v = simd_normalize(simd_cross(d, u)) * 0.10
        let n = UInt32(vertices.count)
        vertices += [a-u-v, a+u-v, a+u+v, a-u+v, b-u-v, b+u-v, b+u+v, b-u+v]
        indices += [0,1,2,0,2,3,4,6,5,4,7,6,0,4,5,0,5,1,1,5,6,1,6,2,2,6,7,2,7,3,3,7,4,3,4,0].map { n + UInt32($0) }
    }
    guard !vertices.isEmpty else { return nil }
    var descriptor = MeshDescriptor(name: rapid ? "CNC rapids" : "CNC cuts")
    descriptor.positions = MeshBuffers.Positions(vertices)
    descriptor.primitives = .triangles(indices)
    return try? MeshResource.generate(from: [descriptor])
}
