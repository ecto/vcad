#if canImport(AppKit)
import AppKit
#endif
import CIrys
import CVcadFFI
import Foundation
import IOSurface
import QuartzCore
import simd
import SwiftUI

// Live path-traced viewport, rendered in-process by irys.
//
// irys-ffi builds a scene from our own meshes (one BLAS per instance mesh, a
// TLAS of placements), path-traces on the GPU and presents straight into an
// IOSurface that a CALayer shows — nothing crosses the CPU per frame. The
// image refines progressively while the view holds still: every display
// frame adds a few samples to a running mean. Moving the camera, a joint
// slider or explode restarts it; a pose change only rebuilds the TLAS
// (`irys_scene_set_transforms`), so the geometry is never re-uploaded.
//
// libirys_ffi.dylib is dlopen()ed, not linked: an app built without irys
// still runs, and the toggle says why path tracing is unavailable.

// MARK: - the dylib

/// The irys C ABI, resolved from `libirys_ffi.dylib` at runtime.
final class IrysLib: @unchecked Sendable {
    typealias ContextCreate = @convention(c) () -> OpaquePointer?
    typealias LastError = @convention(c) () -> UnsafePointer<CChar>?
    typealias FromMeshes = @convention(c) (OpaquePointer?, UnsafePointer<IrysMeshDesc>?, Int,
                                          UnsafePointer<IrysMaterialDesc>?, Int,
                                          UnsafePointer<UInt32>?, UnsafePointer<Float>?, Int,
                                          UInt32, UInt32, UInt32) -> OpaquePointer?
    typealias SetTransforms = @convention(c) (OpaquePointer?, UnsafePointer<Float>?, Int) -> Int32
    typealias SetLookAt = @convention(c) (OpaquePointer?, UnsafePointer<Double>?, UnsafePointer<Double>?,
                                         UnsafePointer<Double>?, Double) -> Int32
    typealias Accumulate = @convention(c) (OpaquePointer?, UnsafeRawPointer?, UInt32) -> Int32
    typealias SampleCount = @convention(c) (OpaquePointer?) -> UInt32
    typealias Destroy = @convention(c) (OpaquePointer?) -> Void

    let contextCreate: ContextCreate
    let lastErrorFn: LastError
    let fromMeshes: FromMeshes
    let setTransforms: SetTransforms
    let setLookAt: SetLookAt
    let accumulate: Accumulate
    let sampleCount: SampleCount
    let destroy: Destroy
    let path: String

    /// Where the dylib is looked for: $IRYS_FFI, the app bundle's Frameworks,
    /// then a sibling irys checkout's release build.
    static func candidates() -> [String] {
        var c: [String] = []
        if let env = ProcessInfo.processInfo.environment["IRYS_FFI"], !env.isEmpty { c.append(env) }
        if let fw = Bundle.main.privateFrameworksPath { c.append("\(fw)/libirys_ffi.dylib") }
        c.append("\(NSHomeDirectory())/Developer/irys/target/release/libirys_ffi.dylib")
        return c
    }

    /// The loaded library, or why it could not be loaded. Loaded once.
    static let shared: Result<IrysLib, PathTraceError> = load()

    private static func load() -> Result<IrysLib, PathTraceError> {
        var tried: [String] = []
        for p in candidates() where FileManager.default.fileExists(atPath: p) {
            guard let h = dlopen(p, RTLD_NOW | RTLD_LOCAL) else {
                tried.append("\(p): \(String(cString: dlerror()))")
                continue
            }
            func sym<T>(_ name: String, _: T.Type) -> T? {
                dlsym(h, name).map { unsafeBitCast($0, to: T.self) }
            }
            guard let a = sym("irys_context_create", ContextCreate.self),
                  let b = sym("irys_last_error", LastError.self),
                  let c = sym("irys_scene_from_meshes", FromMeshes.self),
                  let d = sym("irys_scene_set_transforms", SetTransforms.self),
                  let e = sym("irys_scene_set_look_at", SetLookAt.self),
                  let f = sym("irys_scene_accumulate_iosurface", Accumulate.self),
                  let g = sym("irys_scene_sample_count", SampleCount.self),
                  let k = sym("irys_scene_destroy", Destroy.self) else {
                tried.append("\(p): too old (no mesh-ingest entry points)")
                continue
            }
            return .success(IrysLib(contextCreate: a, lastErrorFn: b, fromMeshes: c, setTransforms: d,
                                    setLookAt: e, accumulate: f, sampleCount: g, destroy: k, path: p))
        }
        let detail = tried.isEmpty ? "libirys_ffi.dylib not found (set IRYS_FFI or build ~/Developer/irys: cargo build --release -p irys-ffi)"
                                   : tried.joined(separator: "; ")
        return .failure(PathTraceError(message: detail))
    }

    private init(contextCreate: ContextCreate, lastErrorFn: LastError, fromMeshes: FromMeshes,
                 setTransforms: SetTransforms, setLookAt: SetLookAt, accumulate: Accumulate,
                 sampleCount: SampleCount, destroy: Destroy, path: String) {
        self.contextCreate = contextCreate; self.lastErrorFn = lastErrorFn; self.fromMeshes = fromMeshes
        self.setTransforms = setTransforms; self.setLookAt = setLookAt; self.accumulate = accumulate
        self.sampleCount = sampleCount; self.destroy = destroy; self.path = path
    }

    func lastError() -> String { lastErrorFn().map { String(cString: $0) } ?? "unknown irys error" }
}

struct PathTraceError: Error { let message: String }

// MARK: - what the tracer needs from the model

/// Geometry handed to irys: one mesh per drawn instance (or part), already in
/// metres, with its material.
struct PathTraceGeometry: @unchecked Sendable {
    struct Item {
        var positions: [Float]     // metres, object space, xyz…
        var normals: [Float]       // same count, or empty
        var indices: [UInt32]
        var material: IrysMaterialDesc
    }
    var items: [Item]
    /// Which `instanceTransforms` slot each item follows (nil = static, identity).
    var instanceSlot: [Int?]
}

/// The view the tracer should converge on.
struct PathTraceView: Equatable, Sendable {
    var eye: SIMD3<Double>       // metres, kernel frame (Z up)
    var target: SIMD3<Double>
    var vfovDeg: Double
    var width: Int
    var height: Int
}

// MARK: - the engine

/// Owns the irys scene and the IOSurface, and runs every irys call on one
/// serial queue (irys handles are not thread-safe). The main actor feeds it
/// views and poses; it accumulates while they hold still.
@MainActor
final class PathTracer {
    private let queue = DispatchQueue(label: "io.vcad.pathtrace", qos: .userInteractive)
    nonisolated(unsafe) private var context: OpaquePointer?
    nonisolated(unsafe) private var scene: OpaquePointer?
    nonisolated(unsafe) private(set) var surface: IOSurface?
    private var sceneSize: (Int, Int) = (0, 0)
    private var builtGeneration = -1
    private var appliedView: PathTraceView?
    private var appliedPoseTick = -1
    private var geometry: PathTraceGeometry?
    private var busy = false
    private var building = false
    /// Samples per call: adapted so one call costs about a display frame.
    private var sppPerCall: UInt32 = 2
    /// Stop refining here; the GPU idles until something changes.
    static let maxSamples: UInt32 = 4096

    /// Samples per pixel in the current image.
    private(set) var samples: UInt32 = 0
    /// Called on the main actor after each presented step.
    var onFrame: ((UInt32) -> Void)?

    nonisolated init() {}

    var isReady: Bool { scene != nil && surface != nil }

    /// Advance one step: rebuild if the geometry or size changed, apply view
    /// and pose changes, then add samples. Cheap to call every display frame;
    /// it drops calls while the queue is busy.
    func step(model: EditorModel, view: PathTraceView) throws {
        guard !busy, !building else { return }
        let lib = try IrysLib.shared.get()
        let generation = model.sceneGeneration
        if generation != builtGeneration || sceneSize != (view.width, view.height) || scene == nil {
            guard let g = model.pathTraceGeometry() else {
                throw PathTraceError(message: "Nothing to path trace yet.")
            }
            rebuild(lib: lib, geometry: g, generation: generation, view: view,
                    transforms: model.instanceTransforms, poseTick: model.poseTick)
            return
        }
        let newView: PathTraceView? = appliedView != view ? view : nil
        if newView != nil { appliedView = view }
        var pending: [Float]? = nil
        if appliedPoseTick != model.poseTick, let g = geometry {
            pending = Self.flatTransforms(g, model.instanceTransforms)
            appliedPoseTick = model.poseTick
        }
        let newTransforms = pending
        let changed = newView != nil || newTransforms != nil
        if !changed && samples >= Self.maxSamples { return }
        if changed { samples = 0 }
        busy = true
        let spp = sppPerCall
        queue.async { [self] in
            var err: String?
            if let t = newTransforms, lib.setTransforms(scene, t, t.count / 12) != 0 { err = lib.lastError() }
            if let v = newView, !Self.lookAt(lib, scene, v) { err = lib.lastError() }
            let t0 = Date()
            if err == nil, let s = surface,
               lib.accumulate(scene, Unmanaged.passUnretained(s).toOpaque(), spp) != 0 { err = lib.lastError() }
            let dt = Date().timeIntervalSince(t0)
            let n = lib.sampleCount(scene)
            DispatchQueue.main.async { [self] in
                MainActor.assumeIsolated {
                    busy = false
                    if let err { model.pathTraceStatus = "irys: \(err)"; return }
                    samples = n
                    // Aim for ~25 ms of GPU per call so the UI stays live.
                    if dt < 0.012 { sppPerCall = min(sppPerCall * 2, 64) }
                    else if dt > 0.04 { sppPerCall = max(sppPerCall / 2, 1) }
                    onFrame?(n)
                }
            }
        }
    }

    private func rebuild(lib: IrysLib, geometry g: PathTraceGeometry, generation: Int,
                         view: PathTraceView, transforms: [float4x4], poseTick: Int) {
        building = true
        geometry = g
        let flat = Self.flatTransforms(g, transforms)
        let (w, h) = (view.width, view.height)
        model_status("Building path-traced scene…")
        queue.async { [self] in
            var err: String?
            if context == nil { context = lib.contextCreate(); if context == nil { err = lib.lastError() } }
            if let s = scene { lib.destroy(s); scene = nil }
            var newSurface: IOSurface?
            if err == nil {
                newSurface = IOSurface(properties: [.width: w, .height: h, .bytesPerElement: 4,
                                                    .pixelFormat: 0x5247_4241])   // 'RGBA'
                if newSurface == nil { err = "could not make a \(w)×\(h) IOSurface" }
            }
            if err == nil {
                scene = Self.makeScene(lib, context, g, flat, w, h)
                if scene == nil { err = lib.lastError() }
            }
            if err == nil, !Self.lookAt(lib, scene, view) { err = lib.lastError() }
            DispatchQueue.main.async { [self] in
                MainActor.assumeIsolated {
                    building = false
                    if let err {
                        statusSink?("irys: \(err)")
                        builtGeneration = generation      // don't retry in a tight loop
                        return
                    }
                    surface = newSurface
                    sceneSize = (w, h)
                    builtGeneration = generation
                    appliedView = view
                    appliedPoseTick = poseTick
                    samples = 0
                    sppPerCall = 2
                    statusSink?(nil)
                    onSurface?(newSurface)
                }
            }
        }
    }

    /// Where status text goes (the model's `pathTraceStatus`).
    var statusSink: ((String?) -> Void)?
    /// A new IOSurface to show (after a rebuild).
    var onSurface: ((IOSurface?) -> Void)?
    private func model_status(_ s: String?) { statusSink?(s) }

    /// Force the next step to rebuild the scene (e.g. a resize).
    func invalidate() { builtGeneration = -1 }

    /// Free everything on the render queue.
    func teardown() {
        let lib = try? IrysLib.shared.get()
        queue.async { [self] in
            if let s = scene, let lib { lib.destroy(s) }
            scene = nil
        }
        surface = nil
        geometry = nil
        builtGeneration = -1
        appliedView = nil
        samples = 0
    }

    // MARK: helpers (run on the queue)

    nonisolated private static func lookAt(_ lib: IrysLib, _ scene: OpaquePointer?, _ v: PathTraceView) -> Bool {
        let e = [v.eye.x, v.eye.y, v.eye.z], t = [v.target.x, v.target.y, v.target.z]
        // Z up, unless we're looking straight down/up it.
        let d = simd_normalize(v.target - v.eye)
        let up: [Double] = abs(d.z) > 0.995 ? [0, 1, 0] : [0, 0, 1]
        return lib.setLookAt(scene, e, t, up, v.vfovDeg) == 0
    }

    nonisolated private static func makeScene(_ lib: IrysLib, _ ctx: OpaquePointer?, _ g: PathTraceGeometry,
                                              _ xf: [Float], _ w: Int, _ h: Int) -> OpaquePointer? {
        // One contiguous buffer per attribute, so every descriptor points
        // into memory that is valid for the whole call.
        var pos: [Float] = [], nrm: [Float] = [], idx: [UInt32] = []
        var spans: [(p: Int, n: Int?, i: Int, nv: Int, ni: Int)] = []
        for it in g.items {
            let n: Int? = it.normals.count == it.positions.count ? nrm.count : nil
            spans.append((pos.count, n, idx.count, it.positions.count / 3, it.indices.count))
            pos += it.positions
            if n != nil { nrm += it.normals }
            idx += it.indices
        }
        var mats = g.items.map(\.material)
        let instMesh = (0..<g.items.count).map { UInt32($0) }
        if nrm.isEmpty { nrm = [0] }
        return pos.withUnsafeBufferPointer { pp in
            nrm.withUnsafeBufferPointer { np in
                idx.withUnsafeBufferPointer { ip in
                    let descs = spans.enumerated().map { k, s in
                        IrysMeshDesc(positions: pp.baseAddress! + s.p,
                                     normals: s.n.map { np.baseAddress! + $0 },
                                     vertex_count: s.nv,
                                     indices: ip.baseAddress! + s.i, index_count: s.ni,
                                     material: UInt32(k))
                    }
                    return lib.fromMeshes(ctx, descs, descs.count, &mats, mats.count, instMesh, xf,
                                          instMesh.count, UInt32(w), UInt32(h), UInt32(IRYS_FLOOR))
                }
            }
        }
    }

    /// Row-major 3×4 per item, metres.
    nonisolated static func flatTransforms(_ g: PathTraceGeometry, _ world: [float4x4]) -> [Float] {
        var out: [Float] = []
        out.reserveCapacity(g.items.count * 12)
        for slot in g.instanceSlot {
            let m = slot.flatMap { $0 < world.count ? world[$0] : nil } ?? matrix_identity_float4x4
            for r in 0..<3 {
                out += [m.columns.0[r], m.columns.1[r], m.columns.2[r], m.columns.3[r] * 0.001]
            }
        }
        return out
    }

    // MARK: saving

    /// The current image as PNG data (read straight from the IOSurface).
    func pngData() -> Data? {
        guard let s = surface else { return nil }
        s.lock(options: .readOnly, seed: nil)
        defer { s.unlock(options: .readOnly, seed: nil) }
        let cs = CGColorSpace(name: CGColorSpace.sRGB)!
        guard let cg = CGContext(data: s.baseAddress, width: s.width, height: s.height, bitsPerComponent: 8,
                                 bytesPerRow: s.bytesPerRow, space: cs,
                                 bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue),
              let img = cg.makeImage() else { return nil }
        return NSBitmapImageRep(cgImage: img).representation(using: .png, properties: [:])
    }
}

// MARK: - model glue

extension EditorModel {
    /// Linear-light colour of an sRGB NSColor.
    nonisolated static func linear(_ c: NSColor) -> (Float, Float, Float) {
        let s = c.usingColorSpace(.sRGB) ?? c
        func lin(_ v: CGFloat) -> Float {
            let x = Float(v)
            return x <= 0.04045 ? x / 12.92 : powf((x + 0.055) / 1.055, 2.4)
        }
        return (lin(s.redComponent), lin(s.greenComponent), lin(s.blueComponent))
    }

    /// Everything on screen as irys geometry: assembly instances (with their
    /// materials, following `instanceTransforms`), or the document's parts.
    func pathTraceGeometry() -> PathTraceGeometry? {
        var items: [PathTraceGeometry.Item] = []
        var slots: [Int?] = []
        func material(_ r: ResolvedMaterial) -> IrysMaterialDesc {
            let c = Self.linear(r.color)
            return IrysMaterialDesc(base_color: (c.0, c.1, c.2), metallic: r.metallic,
                                    roughness: r.roughness, transmission: r.transmission)
        }
        if let scene = residentAssemblyScene, assemblyInstanceCount > 0 {
            for i in 0..<assemblyInstanceCount where !hiddenInstance(i) {
                let v = vcad_scene_instance_mesh(scene, i)
                let nv = v.vertices_len / 3
                guard nv > 0, v.indices_len >= 3, let vp = v.vertices, let ip = v.indices else { continue }
                let pos = (0..<nv * 3).map { vp[$0] * 0.001 }
                let nrm: [Float] = (v.normals_len == v.vertices_len && v.normals != nil)
                    ? Array(UnsafeBufferPointer(start: v.normals, count: v.normals_len)) : []
                let idx = Array(UnsafeBufferPointer(start: ip, count: v.indices_len))
                var len = 0
                let key: String = vcad_scene_instance_material(scene, i, &len).map {
                    String(decoding: UnsafeBufferPointer(start: $0, count: len), as: UTF8.self)
                } ?? ""
                items.append(.init(positions: pos, normals: nrm, indices: idx,
                                   material: material(resolvedMaterial(named: key, fallbackIndex: i))))
                slots.append(i)
            }
        } else {
            for (i, m) in docPartMeshes.enumerated() where !hiddenParts.contains(i) && m.indices.count >= 3 {
                var pos: [Float] = []
                pos.reserveCapacity(m.positions.count * 3)
                for p in m.positions { pos += [p.x * 0.001, p.y * 0.001, p.z * 0.001] }
                items.append(.init(positions: pos, normals: [], indices: m.indices,
                                   material: material(ResolvedMaterial(color: documentBaseColor(i), metallic: 0.3,
                                                                       roughness: 0.4, transmission: 0))))
                slots.append(nil)
            }
        }
        return items.isEmpty ? nil : PathTraceGeometry(items: items, instanceSlot: slots)
    }

    /// Instances the user hid don't get traced. (Instance hiding isn't wired
    /// yet; kept as the one place to change when it is.)
    func hiddenInstance(_ i: Int) -> Bool { false }

    /// The orbit camera as an irys view (metres, kernel frame).
    func pathTraceView(width: Int, height: Int) -> PathTraceView? {
        guard let (cam, target) = kernelCamera() else { return nil }
        // RealityKit's PerspectiveCamera default: 60° vertical.
        return PathTraceView(eye: cam * 0.001, target: target * 0.001, vfovDeg: 60,
                             width: max(width, 8), height: max(height, 8))
    }

    func setPathTrace(_ on: Bool) {
        if on, case .failure(let e) = IrysLib.shared {
            pathTraceStatus = e.message
            pathTraceEnabled = false
            return
        }
        pathTraceEnabled = on
        if !on { pathTracer.teardown(); pathTraceSamples = 0; pathTraceStatus = nil }
    }

    #if os(macOS)
    /// Save the accumulated path-traced frame to ~/Pictures/vcad and open it.
    func savePathTracedStill() {
        guard let data = pathTracer.pngData() else {
            photorealStatus = "No path-traced frame yet."
            return
        }
        let dir = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Pictures/vcad", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "-")
        let url = dir.appendingPathComponent("\(documentName)-\(stamp).png")
        do {
            try data.write(to: url)
            photorealStatus = "Saved \(url.lastPathComponent) (\(pathTraceSamples) spp, path traced)"
            NSWorkspace.shared.open(url)
        } catch {
            photorealStatus = "Could not save: \(error.localizedDescription)"
        }
    }
    #endif
}

// MARK: - the view

/// The IOSurface, shown by a CALayer over the viewport, driven by a display
/// link. Mouse events pass through to the orbit layer above.
struct PathTracedViewport: NSViewRepresentable {
    let model: EditorModel

    func makeNSView(context: Context) -> PathTraceNSView {
        let v = PathTraceNSView()
        v.model = model
        return v
    }

    func updateNSView(_ v: PathTraceNSView, context: Context) { v.model = model }

    static func dismantleNSView(_ v: PathTraceNSView, coordinator: ()) { v.stop() }
}

final class PathTraceNSView: NSView {
    weak var model: EditorModel?
    private var link: CADisplayLink?
    private var lastSize: CGSize = .zero
    private var sizeStable: Date = .distantPast

    override init(frame: NSRect) {
        super.init(frame: frame)
        wantsLayer = true
        layer?.contentsGravity = .resize
        layer?.backgroundColor = NSColor.black.cgColor
    }
    required init?(coder: NSCoder) { fatalError() }

    override func hitTest(_ point: NSPoint) -> NSView? { nil }   // orbit layer owns the mouse

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard window != nil, link == nil else { if window == nil { stop() }; return }
        let l = displayLink(target: self, selector: #selector(tick))
        l.add(to: .main, forMode: .common)
        link = l
        guard let m = model else { return }
        let tracer = m.pathTracer
        tracer.onSurface = { [weak self] (s: IOSurface?) in self?.layer?.contents = s }
        tracer.statusSink = { [weak m] (s: String?) in m?.pathTraceStatus = s }
        tracer.onFrame = { [weak self, weak m, weak tracer] (n: UInt32) in
            // Re-hand the surface over so Core Animation picks up the new
            // pixels (it caches IOSurface contents by identity otherwise).
            CATransaction.begin(); CATransaction.setDisableActions(true)
            self?.layer?.contents = nil
            self?.layer?.contents = tracer?.surface
            CATransaction.commit()
            m?.pathTraceSamples = n
            m?.completePendingStill()
        }
        // A view made after the scene was built still shows its surface.
        if let s = tracer.surface { layer?.contents = s }
    }

    func stop() {
        link?.invalidate(); link = nil
    }

    @objc private func tick() {
        do {
            guard let m = model, m.pathTraceEnabled else { return }
            // Trace at point resolution (not Retina): 4× fewer rays, and the
            // noise is what limits sharpness anyway.
            let size = bounds.size
            if size != lastSize { lastSize = size; sizeStable = Date() }
            guard Date().timeIntervalSince(sizeStable) > 0.25 else { return }   // mid-resize
            guard let view = m.pathTraceView(width: Int(size.width), height: Int(size.height)) else { return }
            do { try m.pathTracer.step(model: m, view: view) }
            catch let e as PathTraceError { if m.pathTraceStatus != e.message { m.pathTraceStatus = e.message } }
            catch {}
        }
    }
}

extension EditorModel {
    /// A requested still waits for this many samples before it's saved.
    static let stillSamples: UInt32 = 256

    /// The Photoreal button on the path-traced path: save now if the image is
    /// converged enough, otherwise when it gets there.
    func requestPathTracedStill() {
        if !pathTraceEnabled { setPathTrace(true) }
        guard pathTraceEnabled else { return }
        pathTraceStillPending = true
        photorealStatus = "Converging to \(Self.stillSamples) spp…"
        completePendingStill()
    }

    func completePendingStill() {
        guard pathTraceStillPending, pathTraceSamples >= Self.stillSamples else { return }
        pathTraceStillPending = false
        #if os(macOS)
        savePathTracedStill()
        #endif
    }
}
