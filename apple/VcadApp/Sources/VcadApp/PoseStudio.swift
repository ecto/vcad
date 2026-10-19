#if canImport(AppKit)
import AppKit
#endif
import CVcadFFI
import Foundation
import simd
import SwiftUI

// Pose studio: the design-iteration surface for a jointed assembly (a robot
// generated as a `.loon` file by an outside exporter). Four things, all on top
// of the kinematic channel playback already uses (`instanceTransforms` +
// `playbackDirty`), so every viewport draws them without knowing:
//
//  - joint sliders, solved by the kernel's FK (`vcad_scene_solve_fk`);
//  - explode: links pulled apart along their joint lines, shells pushed off
//    their links;
//  - hot reload: the open file is watched, a regenerate reloads it in place
//    and keeps the pose, explode and camera;
//  - a photoreal still of exactly what is on screen, path-traced by
//    `vcad-render --photoreal` from a document with the pose baked in.

extension EditorModel {

    // MARK: document bytes

    /// The document at `path` as `.vcad` JSON bytes. A `.loon` file is compiled
    /// by the kernel, with its modules and relative mesh imports resolved
    /// against its own directory; anything else is read as-is.
    nonisolated static func documentBytes(path: String) -> Data? {
        guard path.lowercased().hasSuffix(".loon") else {
            return try? Data(contentsOf: URL(fileURLWithPath: path))
        }
        let bytes = Array(path.utf8)
        guard let c = bytes.withUnsafeBufferPointer({ vcad_loon_file_to_json($0.baseAddress, $0.count) })
        else { return nil }
        defer { vcad_cam_free(c) }
        return Data(bytes: c, count: strlen(c))
    }

    /// The open document is generated loon source: read-only, watched.
    var isLoonDocument: Bool {
        if case let .document(path, _) = source { return path.lowercased().hasSuffix(".loon") }
        return false
    }

    // MARK: joints

    /// Joints a slider can drive, in document order.
    var drivenJoints: [DocJoint] { documentGraph?.joints.filter(\.isDriven) ?? [] }

    /// The studio applies to an assembly with something to pose and no
    /// timeline fighting it for the transforms.
    var hasPoseStudio: Bool { assemblyInstanceCount > 0 && timeline == nil && documentGraph != nil }

    static func poseKey(_ j: DocJoint) -> String { j.name ?? j.id }

    func jointValue(_ j: DocJoint) -> Double { jointPose[Self.poseKey(j)] ?? j.state }

    func setJoint(_ j: DocJoint, _ value: Double) {
        jointPose[Self.poseKey(j)] = value
        applyStudioPose()
    }

    func setExplode(_ t: Double) {
        explode = min(max(t, 0), 1)
        applyStudioPose()
    }

    func resetPose() {
        jointPose = [:]
        explode = 0
        applyStudioPose()
    }

    /// FK for the current joint values, then explode on top. Writes the same
    /// channel kinematic playback does.
    func applyStudioPose() {
        guard let world = solveStudioFK() else { return }
        instanceTransforms = explode > 0 ? explodedTransforms(world, t: Float(explode)) : world
        playbackDirty = true
        poseTick &+= 1
    }

    /// World transforms for the current joint values (authored pose when no
    /// joint has been moved).
    private func solveStudioFK() -> [float4x4]? {
        guard let scene = residentAssemblyScene, assemblyInstanceCount > 0 else { return nil }
        var values: [String: Double] = [:]
        for j in drivenJoints { values[j.id] = jointValue(j) }
        guard !values.isEmpty, let data = try? JSONSerialization.data(withJSONObject: values) else {
            return authoredInstanceTransforms
        }
        var out = [Double](repeating: 0, count: assemblyInstanceCount * 16)
        let n = data.withUnsafeBytes { raw in
            vcad_scene_solve_fk(scene, raw.bindMemory(to: UInt8.self).baseAddress,
                                data.count, &out, out.count)
        }
        guard n == assemblyInstanceCount else { return nil }
        return (0..<n).map { Self.mat4(out, at: $0 * 16) }
    }

    // MARK: explode

    /// Instance-name prefixes the robot exporter uses for cosmetic shells
    /// (covers, trims, pucks, visors, accents, soles). Everything else is a
    /// link's structure and travels with the link.
    static let shellPrefixes = ["cover-", "trim-", "puck-", "visor-", "accent-", "sole"]
    /// How far along its parent→child pivot vector each link travels at full
    /// explode, and how far shells leave their link, radially in XY (mm).
    static let explodeLinkFraction: Float = 0.35
    static let explodeShellMM: Float = 45

    /// Explode the posed assembly. Walk the joint tree from the ground: each
    /// driven joint moves its whole child subtree by `fraction` of the vector
    /// from the parent link's pivot to this joint's pivot, so links separate
    /// along the limb they belong to and the offsets accumulate down the
    /// chain. Fixed joints weld (no separation). Shells then leave their own
    /// link radially in XY, away from that link's pivot. Computed from the
    /// posed transforms, so explode follows whatever pose is on screen.
    func explodedTransforms(_ world: [float4x4], t: Float) -> [float4x4] {
        let ids = assemblyInstanceIDs
        guard ids.count == world.count, let joints = documentGraph?.joints else { return world }
        var index: [String: Int] = [:]
        for (i, id) in ids.enumerated() { index[id] = i }
        var children: [Int: [DocJoint]] = [:]
        var isChild = Set<Int>()
        for j in joints {
            guard let c = index[j.childInstanceId] else { continue }
            isChild.insert(c)
            if let p = j.parentInstanceId.flatMap({ index[$0] }) { children[p, default: []].append(j) }
        }
        func origin(_ m: float4x4) -> SIMD3<Float> { SIMD3(m.columns.3.x, m.columns.3.y, m.columns.3.z) }

        var offset = [SIMD3<Float>](repeating: .zero, count: world.count)
        var pivot = world.map(origin)
        var visited = Set<Int>()
        var queue = (0..<world.count).filter { !isChild.contains($0) }
        visited.formUnion(queue)
        while !queue.isEmpty {
            let p = queue.removeFirst()
            for j in children[p] ?? [] {
                guard let c = index[j.childInstanceId], !visited.contains(c) else { continue }
                visited.insert(c)
                if j.isDriven {
                    let a = world[p] * SIMD4<Float>(SIMD3<Float>(j.parentAnchor), 1)
                    let jp = SIMD3(a.x, a.y, a.z)
                    offset[c] = offset[p] + (jp - pivot[p]) * (Self.explodeLinkFraction * t)
                    pivot[c] = jp
                } else {
                    offset[c] = offset[p]
                    pivot[c] = pivot[p]
                }
                queue.append(c)
            }
        }

        var out = world
        for i in out.indices {
            var d = offset[i]
            if Self.shellPrefixes.contains(where: { ids[i].hasPrefix($0) }) {
                let c4 = world[i] * SIMD4<Float>(assemblyLocalCenters[i], 1)
                var r = SIMD3<Float>(c4.x, c4.y, c4.z) - pivot[i]
                r.z = 0
                let len = simd_length(r)
                if len > 1e-3 { d += r / len * (Self.explodeShellMM * t) }
            }
            out[i].columns.3 += SIMD4<Float>(d, 0)
        }
        return out
    }

    // MARK: hot reload

    /// Watch the open document's file and reload it in place when it changes.
    /// Generators usually replace the file (write-new + rename), which kills a
    /// vnode watch, so every event re-arms on the path after the reload.
    func watchDocumentFile() {
        let path: String? = { if case let .document(p, _) = source { return p } else { return nil } }()
        guard path != fileWatchPath || fileWatch == nil else { return }
        fileWatch?.cancel(); fileWatch = nil; fileWatchPath = nil
        guard let path else { return }
        let fd = open(path, O_EVTONLY)
        guard fd >= 0 else { return }
        let src = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd, eventMask: [.write, .extend, .delete, .rename, .attrib], queue: .main)
        src.setEventHandler { [weak self] in
            MainActor.assumeIsolated { self?.scheduleReload() }
        }
        src.setCancelHandler { close(fd) }
        fileWatch = src
        fileWatchPath = path
        src.resume()
    }

    /// Debounce a burst of file events (an exporter writes in several chunks,
    /// and replaces its STLs in the same pass) into one reload.
    private func scheduleReload() {
        reloadPending?.cancel()
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated { self?.reloadFromDisk() }
        }
        reloadPending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.6, execute: work)
    }

    /// Reload the document from disk, keeping pose, explode and camera.
    func reloadFromDisk() {
        guard case let .document(path, _) = source else { return }
        // Re-arm: a replaced file is a new inode.
        fileWatch?.cancel(); fileWatch = nil
        watchDocumentFile()
        guard FileManager.default.fileExists(atPath: path) else { return }   // mid-replace
        guard !documentDirty || isLoonDocument else { return }   // never clobber edits
        revertDocument()
        lastReload = Date()
    }

    // MARK: photoreal snapshot

    /// The `vcad-render` binary: $VCAD_RENDER, then the shared cargo target,
    /// then PATH.
    static func vcadRenderBinary() -> String? {
        var candidates: [String] = []
        if let env = ProcessInfo.processInfo.environment["VCAD_RENDER"] { candidates.append(env) }
        let home = NSHomeDirectory()
        candidates += ["\(home)/Developer/vcad/target/release/vcad-render",
                       "\(home)/.cargo/bin/vcad-render", "/opt/homebrew/bin/vcad-render",
                       "/usr/local/bin/vcad-render"]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    /// The document with the on-screen pose + explode baked into every
    /// instance transform and the joints dropped, so any renderer draws
    /// exactly what the viewport shows.
    func posedDocumentJSON() -> Data? {
        guard var doc = documentJSON,
              var instances = doc["instances"] as? [[String: Any]],
              instanceTransforms.count == assemblyInstanceIDs.count else { return nil }
        var byID: [String: float4x4] = [:]
        for (i, id) in assemblyInstanceIDs.enumerated() { byID[id] = instanceTransforms[i] }
        for k in instances.indices {
            guard let id = instances[k]["id"] as? String, let m = byID[id] else { continue }
            instances[k]["transform"] = Self.transform3D(m)
        }
        doc["instances"] = instances
        doc["joints"] = []
        doc.removeValue(forKey: "groundInstanceId")
        return try? JSONSerialization.data(withJSONObject: doc)
    }

    /// float4x4 → IR `Transform3D` (translation + Euler XYZ degrees, applied
    /// Rz·Ry·Rx, the convention `vcad_eval::kinematics` emits). Rigid only.
    static func transform3D(_ m: float4x4) -> [String: Any] {
        let r00 = Double(m.columns.0.x), r10 = Double(m.columns.0.y), r20 = Double(m.columns.0.z)
        let r21 = Double(m.columns.1.z), r22 = Double(m.columns.2.z)
        let r01 = Double(m.columns.1.x), r11 = Double(m.columns.1.y)
        let deg = 180 / Double.pi
        var rx: Double, ry: Double, rz: Double
        ry = asin(max(-1, min(1, -r20)))
        if abs(r20) < 0.999999 {
            rx = atan2(r21, r22)
            rz = atan2(r10, r00)
        } else {                                   // gimbal lock: fold x into z
            rx = 0
            rz = atan2(-r01, r11)
        }
        func v(_ x: Double, _ y: Double, _ z: Double) -> [String: Double] { ["x": x, "y": y, "z": z] }
        return ["translation": v(Double(m.columns.3.x), Double(m.columns.3.y), Double(m.columns.3.z)),
                "rotation": v(rx * deg, ry * deg, rz * deg),
                "scale": v(1, 1, 1)]
    }

    #if os(macOS)
    /// Path-trace the current pose/explode from the current camera direction
    /// and open the result. Runs `vcad-render --photoreal` off the main actor.
    func photorealSnapshot(spp: Int = 96) {
        guard !photorealBusy else { return }
        guard let bin = Self.vcadRenderBinary() else {
            photorealStatus = "vcad-render not found (build it: cargo build --release -p vcad-render, or set VCAD_RENDER)"
            return
        }
        guard let posed = posedDocumentJSON() else {
            photorealStatus = "Nothing posed to render yet."
            return
        }
        // Camera direction, kernel frame: azimuth CCW from +X, elevation above XY.
        var az = 35.0, el = 20.0
        if let (cam, target) = kernelCamera() {
            let d = cam - target
            let len = simd_length(d)
            if len > 1e-9 {
                az = atan2(d.y, d.x) * 180 / .pi
                el = asin(d.z / len) * 180 / .pi
            }
        }
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Pictures/vcad", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "-")
        let base = "\(documentName)-\(stamp)"
        let docURL = FileManager.default.temporaryDirectory.appendingPathComponent("\(base).vcad")
        let outURL = dir.appendingPathComponent("\(base).png")
        do { try posed.write(to: docURL) } catch {
            photorealStatus = "Could not stage the posed document: \(error.localizedDescription)"
            return
        }
        photorealBusy = true
        photorealStatus = "Path tracing…"
        let args = [docURL.path, "--photoreal", "--spp", "\(spp)",
                    // `=` form: a negative angle as its own argv would parse as a flag.
                    String(format: "--azimuth=%.2f", az), String(format: "--elevation=%.2f", el),
                    "--size", "1600x1200", "-o", outURL.path]
        Task.detached {
            let p = Process()
            p.executableURL = URL(fileURLWithPath: bin)
            p.arguments = args
            let errPipe = Pipe()
            p.standardError = errPipe
            p.standardOutput = FileHandle.nullDevice
            let started = Date()
            var failure: String?
            do {
                try p.run()
                let errData = errPipe.fileHandleForReading.readDataToEndOfFile()
                p.waitUntilExit()
                if p.terminationStatus != 0 {
                    let tail = String(decoding: errData, as: UTF8.self)
                        .split(separator: "\n").suffix(2).joined(separator: " ")
                    failure = "vcad-render failed (\(p.terminationStatus)): \(tail)"
                }
            } catch { failure = "Could not run vcad-render: \(error.localizedDescription)" }
            try? FileManager.default.removeItem(at: docURL)
            let secs = Date().timeIntervalSince(started)
            await MainActor.run { [weak self] in
                guard let self else { return }
                self.photorealBusy = false
                if let failure {
                    self.photorealStatus = failure
                } else {
                    self.photorealStatus = String(format: "Saved %@ (%.0f s)", outURL.lastPathComponent, secs)
                    NSWorkspace.shared.open(outURL)
                }
            }
        }
    }
    #endif
}

// MARK: - panel

/// Joint sliders grouped by limb, explode, reset, reload status and the
/// photoreal snapshot. Shown for any jointed assembly without a timeline.
struct PoseStudioWindow: View {
    @Bindable var model: EditorModel
    @State private var filter = ""

    private var groups: [(String, [DocJoint])] {
        let js = model.drivenJoints.filter {
            filter.isEmpty || EditorModel.poseKey($0).localizedCaseInsensitiveContains(filter)
        }
        var order: [String] = []
        var map: [String: [DocJoint]] = [:]
        for j in js {
            let g = Self.group(EditorModel.poseKey(j))
            if map[g] == nil { order.append(g) }
            map[g, default: []].append(j)
        }
        return order.map { ($0, map[$0]!) }
    }

    /// "l_knee_pitch" → "Left leg", "r_elbow" → "Right arm", "head_yaw" → "Head".
    static func group(_ name: String) -> String {
        let n = name.lowercased()
        let side = n.hasPrefix("l_") || n.hasPrefix("left") ? "Left " : n.hasPrefix("r_") || n.hasPrefix("right") ? "Right " : ""
        let arm = ["shoulder", "elbow", "wrist", "arm", "hand"].contains { n.contains($0) }
        let leg = ["hip", "knee", "ankle", "leg", "foot"].contains { n.contains($0) }
        if n.hasPrefix("head") || n.hasPrefix("neck") { return "Head" }
        if arm { return side + "arm" }
        if leg { return side + "leg" }
        return side.isEmpty ? "Body" : side + "side"
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.m) {
            PanelHeader(title: "Pose", systemImage: "figure.walk")
            HStack(spacing: Theme.Space.s) {
                Text("Explode").font(.callout)
                Slider(value: Binding(get: { model.explode }, set: { model.setExplode($0) }), in: 0...1)
                    .controlSize(.small)
                Text(String(format: "%3.0f%%", model.explode * 100))
                    .font(.caption.monospacedDigit()).foregroundStyle(.secondary).frame(width: 36, alignment: .trailing)
            }
            HStack(spacing: Theme.Space.s) {
                TextField("Filter joints", text: $filter).textFieldStyle(.roundedBorder).controlSize(.small)
                Button("Reset") { model.resetPose() }.controlSize(.small)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(groups, id: \.0) { group in
                        Text(group.0).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                            .padding(.top, 4)
                        ForEach(group.1, id: \.id) { j in JointRow(model: model, joint: j) }
                    }
                }
            }
            .frame(maxHeight: 300)
            Divider()
            HStack(spacing: Theme.Space.s) {
                Button {
                    model.photorealSnapshot()
                } label: {
                    Label("Photoreal still", systemImage: "camera.aperture")
                }
                .controlSize(.small)
                .disabled(model.photorealBusy)
                if model.photorealBusy { ProgressView().controlSize(.small) }
                Spacer(minLength: 0)
                if model.isLoonDocument {
                    Button { model.reloadFromDisk() } label: { Image(systemName: "arrow.clockwise") }
                        .buttonStyle(.plain).controlSize(.small).help("Reload from disk")
                }
            }
            if let s = model.photorealStatus {
                Text(s).font(.caption).foregroundStyle(.secondary).lineLimit(3)
            }
            if let t = model.lastReload {
                Text("Reloaded \(t.formatted(date: .omitted, time: .standard)) — watching for regenerate")
                    .font(.caption).foregroundStyle(.secondary)
            } else if model.isLoonDocument {
                Text("Watching \(model.documentName).loon for regenerate")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(Theme.Space.l).frame(width: Theme.Width.navigator).panelSurface()
    }
}

private struct JointRow: View {
    let model: EditorModel
    let joint: DocJoint

    var body: some View {
        let range = joint.limits ?? (joint.kind == "Revolute" ? -180...180 : -100...100)
        let unit = joint.kind == "Revolute" ? "°" : " mm"
        VStack(alignment: .leading, spacing: 1) {
            HStack {
                Text(EditorModel.poseKey(joint)).font(.caption).lineLimit(1).truncationMode(.middle)
                Spacer(minLength: 4)
                Text(String(format: "%.1f%@", model.jointValue(joint), unit))
                    .font(.caption.monospacedDigit()).foregroundStyle(.secondary)
            }
            Slider(value: Binding(get: { min(max(model.jointValue(joint), range.lowerBound), range.upperBound) },
                                  set: { model.setJoint(joint, $0) }),
                   in: range)
                .controlSize(.mini)
        }
        .contextMenu {
            Button("Zero") { model.setJoint(joint, 0) }
            Button("Authored (\(String(format: "%.1f", joint.state)))") { model.setJoint(joint, joint.state) }
        }
    }
}
