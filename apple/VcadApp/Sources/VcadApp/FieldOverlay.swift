import SwiftUI
import RealityKit
import AppKit
import CTangCompress

// Compressed scalar fields (tang-compress TCG1/TCG3 streams) in the native
// app — the twin of the web app's `?field=` / `?fields=` overlay
// (docs/compressed-fields.md). Decode goes through libtang_compress_ffi (CPU
// integer reference decoder, C ABI; build-tc-ffi.sh).
//
// A *stack* is several co-registered fields on one part (`stack.json`: part
// geometry boxes + layers, one per physics). The active layer is drawn in the
// kernel-mm frame as either
// - "surface": the part's exposed voxel faces colored by the field, or
// - "slice": one colormapped cut plane along x/y/z in a ghosted part;
// a 2D layer (`meta.plane`, e.g. a busbar cross-section) sits on its world
// plane. The HUD shows layer tabs, legend, the max-error claim, per-layer
// hash verdicts against the native reference, and timings.
//
// VcadApp-only on purpose: Editor.swift and Shell.swift are symlinked into
// VcadVision, so nothing here hangs off EditorModel.

/// Sidecar metadata (`<name>.json` next to `<name>.tc`), same schema as the web.
struct FieldMeta: Decodable {
    struct Bound: Decodable { let kind: String; let value: Double; let units: String }
    let field: String
    let units: String?
    /// 2D field: the world plane its image lies in ("xy" | "yz" | "xz").
    let plane: String?
    let dims_zyx: [Int]
    let origin_mm: [Double]
    let voxel_mm: [Double]
    let min: Double
    let max: Double
    let bound: Bound
    let native_fnv1a: String?
    let format: String?
}

/// `stack.json`.
struct FieldStackFile: Decodable {
    struct Box: Decodable { let min: [Double]; let max: [Double] }
    struct Geometry: Decodable { let boxes: [Box] }
    struct Layer: Decodable { let id: String; let label: String; let file: String }
    let name: String
    let title: String
    let notes: String?
    let geometry: Geometry?
    let layers: [Layer]
}

struct DecodedField {
    let name: String
    let meta: FieldMeta
    let data: [Float]
    let dims: (z: Int, y: Int, x: Int)
    let hash: String
    let streamBytes: Int
    let readMs: Double
    let decodeMs: Double
    var hashMatch: Bool? { meta.native_fnv1a.map { $0 == hash } }
    var isPlane: Bool { meta.plane != nil }
}

struct FieldLayer: Identifiable {
    let id: String
    let label: String
    let field: DecodedField
    /// Colormap range: over solid cells for a volume with geometry, else the field's.
    let range: (Double, Double)
}

enum FieldDecodeError: Error, CustomStringConvertible {
    case decode(String), shape(String)
    var description: String {
        switch self { case .decode(let m), .shape(let m): m }
    }
}

/// Decode a TCG1/TCG3 stream with the Rust CPU reference decoder.
func tcDecode(_ bytes: Data) throws -> (data: [Float], dims: (Int, Int, Int), hash: String) {
    var field = TcField()
    var err: UnsafeMutablePointer<CChar>?
    let rc = bytes.withUnsafeBytes { raw in
        tc_decode_cpu(raw.bindMemory(to: UInt8.self).baseAddress, raw.count, &field, &err)
    }
    guard rc == 0 else {
        let msg = err.map { String(cString: $0) } ?? "decode failed"
        tc_string_free(err)
        throw FieldDecodeError.decode(msg)
    }
    defer { tc_field_free(&field) }
    let data = Array(UnsafeBufferPointer(start: field.data, count: field.len))
    return (data, (field.dims.0, field.dims.1, field.dims.2), String(format: "%016llx", field.fnv1a))
}

private func ms(_ a: DispatchTime, _ b: DispatchTime) -> Double { Double(b.uptimeNanoseconds - a.uptimeNanoseconds) / 1e6 }

/// Load `<base>.tc` + `<base>.json` (base = path without extension).
func loadCompressedField(base: URL) throws -> DecodedField {
    let t0 = DispatchTime.now()
    let stream = try Data(contentsOf: base.appendingPathExtension("tc"))
    let meta = try JSONDecoder().decode(FieldMeta.self, from: Data(contentsOf: base.appendingPathExtension("json")))
    let t1 = DispatchTime.now()
    let (data, dims, hash) = try tcDecode(stream)
    let t2 = DispatchTime.now()
    // A 2D stream reports its dims as (1, ny, nx) or (ny, nx, 1)-style; compare cell counts + trailing axes.
    let want = meta.dims_zyx
    guard want.reduce(1, *) == dims.0 * dims.1 * dims.2 else {
        throw FieldDecodeError.shape("stream dims \(dims) ≠ sidecar \(want)")
    }
    return DecodedField(name: base.lastPathComponent, meta: meta, data: data, dims: (want[0], want[1], want[2]),
                        hash: hash, streamBytes: stream.count, readMs: ms(t0, t1), decodeMs: ms(t1, t2))
}

/// Solid mask of a volume field's grid: cell centers inside any geometry box.
func solidMask(_ m: FieldMeta, _ boxes: [FieldStackFile.Box]) -> [Bool] {
    let (nz, ny, nx) = (m.dims_zyx[0], m.dims_zyx[1], m.dims_zyx[2])
    var mask = [Bool](repeating: false, count: nx * ny * nz)
    let n = [nx, ny, nz]
    for b in boxes {
        // Cells whose center lies in [min, max] on each axis, as an index range.
        let r = (0..<3).map { a -> ClosedRange<Int>? in
            let lo = Int(((b.min[a] - m.origin_mm[a]) / m.voxel_mm[a] - 0.5).rounded(.up))
            let hi = Int(((b.max[a] - m.origin_mm[a]) / m.voxel_mm[a] - 0.5).rounded(.down))
            let (l, h) = (Swift.max(0, lo), Swift.min(n[a] - 1, hi))
            return l <= h ? l...h : nil
        }
        guard let rx = r[0], let ry = r[1], let rz = r[2] else { continue }
        for k in rz { for j in ry { for i in rx { mask[(k * ny + j) * nx + i] = true } } }
    }
    return mask
}

/// Load a stack directory (`stack.json` + one `.tc`/`.json` per layer).
func loadFieldStack(dir: URL) throws -> (FieldStackFile, [FieldLayer], Double) {
    let t0 = DispatchTime.now()
    let stack = try JSONDecoder().decode(FieldStackFile.self, from: Data(contentsOf: dir.appendingPathComponent("stack.json")))
    var layers: [FieldLayer] = []
    var masks: [String: [Bool]] = [:]
    for l in stack.layers {
        let f = try loadCompressedField(base: dir.appendingPathComponent(l.file))
        var range = (f.meta.min, f.meta.max)
        if let boxes = stack.geometry?.boxes, !f.isPlane {
            let key = f.meta.dims_zyx.map(String.init).joined(separator: "x")
            let mask = masks[key] ?? solidMask(f.meta, boxes)
            masks[key] = mask
            var (lo, hi) = (Double.infinity, -Double.infinity)
            for (i, solid) in mask.enumerated() where solid { lo = Swift.min(lo, Double(f.data[i])); hi = Swift.max(hi, Double(f.data[i])) }
            if lo < hi { range = (lo, hi) }
        }
        layers.append(FieldLayer(id: l.id, label: l.label, field: f, range: range))
    }
    return (stack, layers, ms(t0, DispatchTime.now()))
}

/// Turbo-ish colormap (Google's polynomial approximation) — same as the web.
func turbo(_ t: Double) -> (UInt8, UInt8, UInt8) {
    let x = Swift.min(1, Swift.max(0, t))
    let r = 0.1357 + x * (4.5974 - x * (42.3277 - x * (130.5887 - x * (150.5666 - x * 58.1375))))
    let g = 0.0914 + x * (2.1856 + x * (4.8052 - x * (14.0195 - x * (4.2109 + x * 2.7747))))
    let b = 0.1067 + x * (12.5925 - x * (60.1097 - x * (109.0745 - x * (88.5066 - x * 26.8183))))
    let c = { (v: Double) in UInt8((255 * Swift.min(1, Swift.max(0, v))).rounded()) }
    return (c(r), c(g), c(b))
}

private func rgbaImage(_ rgba: [UInt8], _ w: Int, _ h: Int) -> CGImage? {
    guard let provider = CGDataProvider(data: Data(rgba) as CFData) else { return nil }
    return CGImage(width: w, height: h, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: w * 4,
                   space: CGColorSpace(name: CGColorSpace.sRGB)!,
                   bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                   provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
}

enum FieldAxis: String, CaseIterable { case x, y, z }

/// The field on a world plane: pixel (iu, iv) → cell, and the plane's corners
/// (u0,v0) (u1,v0) (u1,v1) (u0,v1) in kernel mm.
struct FieldPlaneImage {
    let w: Int, h: Int
    let cell: (Int, Int) -> Int
    let corners: [SIMD3<Float>]
}

extension DecodedField {
    func planeImage(axis: FieldAxis, k: Int) -> FieldPlaneImage {
        let (nz, ny, nx) = dims
        let o = meta.origin_mm.map(Float.init), v = meta.voxel_mm.map(Float.init)
        let e = [o[0] + Float(nx) * v[0], o[1] + Float(ny) * v[1], o[2] + Float(nz) * v[2]]
        if let plane = meta.plane {
            // 2D: columns → first plane axis, rows → second; origin/voxel are world xyz.
            let (a, b, n) = plane == "yz" ? (1, 2, 0) : plane == "xz" ? (0, 2, 1) : (0, 1, 2)
            let (u0, v0) = (o[a], o[b]), (u1, v1) = (o[a] + Float(nx) * v[a], o[b] + Float(ny) * v[b])
            func P(_ u: Float, _ w: Float) -> SIMD3<Float> { var p = SIMD3<Float>(repeating: 0); p[a] = u; p[b] = w; p[n] = o[n]; return p }
            return FieldPlaneImage(w: nx, h: ny, cell: { $1 * nx + $0 }, corners: [P(u0, v0), P(u1, v0), P(u1, v1), P(u0, v1)])
        }
        switch axis {
        case .z:
            let z = o[2] + (Float(k) + 0.5) * v[2]
            return FieldPlaneImage(w: nx, h: ny, cell: { (k * ny + $1) * nx + $0 },
                                   corners: [[o[0], o[1], z], [e[0], o[1], z], [e[0], e[1], z], [o[0], e[1], z]])
        case .y:
            let y = o[1] + (Float(k) + 0.5) * v[1]
            return FieldPlaneImage(w: nx, h: nz, cell: { ($1 * ny + k) * nx + $0 },
                                   corners: [[o[0], y, o[2]], [e[0], y, o[2]], [e[0], y, e[2]], [o[0], y, e[2]]])
        case .x:
            let x = o[0] + (Float(k) + 0.5) * v[0]
            return FieldPlaneImage(w: ny, h: nz, cell: { ($1 * ny + $0) * nx + k },
                                   corners: [[x, o[1], o[2]], [x, e[1], o[2]], [x, e[1], e[2]], [x, o[1], e[2]]])
        }
    }

    /// RGBA image of a plane; image row 0 is the TOP (v1) edge. Void cells transparent.
    func planeCGImage(_ img: FieldPlaneImage, range: (Double, Double), mask: [Bool]?) -> CGImage? {
        var rgba = [UInt8](repeating: 0, count: img.w * img.h * 4)
        let span = range.1 - range.0
        for iv in 0..<img.h { for iu in 0..<img.w {
            let c = img.cell(iu, iv)
            let (r, g, b) = turbo(span > 0 ? (Double(data[c]) - range.0) / span : 0)
            let o = ((img.h - 1 - iv) * img.w + iu) * 4
            let a: UInt8 = mask.map { $0[c] ? 255 : 0 } ?? 255
            rgba[o] = a == 0 ? 0 : r; rgba[o + 1] = a == 0 ? 0 : g; rgba[o + 2] = a == 0 ? 0 : b; rgba[o + 3] = a
        } }
        return rgbaImage(rgba, img.w, img.h)
    }

    /// One z-slice as an image (HUD thumbnail; single-field compatibility).
    func sliceImage(_ k: Int, range: (Double, Double)? = nil) -> CGImage? {
        planeCGImage(planeImage(axis: .z, k: k), range: range ?? (meta.min, meta.max), mask: nil)
    }

    /// The slice with the hottest cell — where the structure is.
    var hottestSlice: Int {
        guard let i = data.indices.max(by: { data[$0] < data[$1] }) else { return 0 }
        return i / (dims.x * dims.y)
    }
}

/// Exposed voxel faces of the solid: quads in kernel mm, one cell per quad.
struct FieldSkin {
    var positions: [SIMD3<Float>] = []
    var normals: [SIMD3<Float>] = []
    var cells: [Int] = []  // per quad

    init(meta m: FieldMeta, mask: [Bool]) {
        let (nz, ny, nx) = (m.dims_zyx[0], m.dims_zyx[1], m.dims_zyx[2])
        let o = m.origin_mm.map(Float.init), v = m.voxel_mm.map(Float.init)
        func solid(_ i: Int, _ j: Int, _ k: Int) -> Bool {
            i >= 0 && j >= 0 && k >= 0 && i < nx && j < ny && k < nz && mask[(k * ny + j) * nx + i]
        }
        // Neighbor offset + 4 unit-cube corners, CCW seen from outside.
        let faces: [(SIMD3<Int>, [SIMD3<Float>])] = [
            ([-1, 0, 0], [[0, 0, 0], [0, 0, 1], [0, 1, 1], [0, 1, 0]]),
            ([1, 0, 0], [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]]),
            ([0, -1, 0], [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]]),
            ([0, 1, 0], [[0, 1, 0], [0, 1, 1], [1, 1, 1], [1, 1, 0]]),
            ([0, 0, -1], [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
            ([0, 0, 1], [[0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]]),
        ]
        let scale = SIMD3<Float>(v[0], v[1], v[2]), origin = SIMD3<Float>(o[0], o[1], o[2])
        for k in 0..<nz { for j in 0..<ny { for i in 0..<nx where solid(i, j, k) {
            for (d, corners) in faces where !solid(i + d.x, j + d.y, k + d.z) {
                let base = SIMD3<Float>(Float(i), Float(j), Float(k))
                for c in corners { positions.append(origin + (base + c) * scale) }
                normals.append(contentsOf: Array(repeating: SIMD3<Float>(Float(d.x), Float(d.y), Float(d.z)), count: 4))
                cells.append((k * ny + j) * nx + i)
            }
        } } }
    }

    /// Mesh with UVs indexing a 256×1 colormap by each quad's field value.
    func mesh(field: DecodedField?, range: (Double, Double)) -> MeshResource? {
        var d = MeshDescriptor(name: "fieldSkin")
        d.positions = MeshBuffers.Positions(positions)
        d.normals = MeshBuffers.Normals(normals)
        if let field {
            let span = range.1 - range.0
            var uv: [SIMD2<Float>] = []
            uv.reserveCapacity(positions.count)
            for c in cells {
                let t = span > 0 ? Float((Double(field.data[c]) - range.0) / span) : 0
                let u = Swift.min(1, Swift.max(0, t)) * 255 / 256 + 0.5 / 256
                uv.append(contentsOf: Array(repeating: SIMD2<Float>(u, 0.5), count: 4))
            }
            d.textureCoordinates = MeshBuffers.TextureCoordinates(uv)
        }
        var idx: [UInt32] = []
        idx.reserveCapacity(cells.count * 6)
        for q in 0..<UInt32(cells.count) { idx += [4 * q, 4 * q + 1, 4 * q + 2, 4 * q, 4 * q + 2, 4 * q + 3] }
        d.primitives = .triangles(idx)
        return try? MeshResource.generate(from: [d])
    }
}

@MainActor
private let colormapTexture: TextureResource? = {
    var rgba = [UInt8]()
    for i in 0..<256 { let (r, g, b) = turbo(Double(i) / 255); rgba += [r, g, b, 255] }
    return rgbaImage(rgba, 256, 1).flatMap { try? TextureResource(image: $0, options: .init(semantic: .color)) }
}()

enum FieldViewMode: String, CaseIterable { case surface, slice }

@MainActor @Observable
final class FieldOverlayStore {
    static let shared = FieldOverlayStore()
    var stack: FieldStackFile?
    var layers: [FieldLayer] = []
    var activeId: String?
    var mode: FieldViewMode = .surface
    var axis: FieldAxis = .z
    var slice = 0
    var totalMs = 0.0
    var error: String?
    /// Bumped on any change the viewport must redraw.
    var revision = 0
    /// Skin geometry cache (one solid mask per stack).
    @ObservationIgnored private var skinCache: (key: String, skin: FieldSkin)?

    var active: FieldLayer? { layers.first { $0.id == activeId } ?? layers.first }
    /// The active layer's field (single-field compatibility).
    var field: DecodedField? { active?.field }

    func load(base: URL) {
        do {
            let t0 = DispatchTime.now()
            let f = try loadCompressedField(base: base)
            stack = nil
            layers = [FieldLayer(id: f.name, label: f.meta.field, field: f, range: (f.meta.min, f.meta.max))]
            activeId = f.name; mode = .slice; axis = .z; slice = f.hottestSlice; error = nil
            totalMs = ms(t0, DispatchTime.now())
            print("[field] \(f.name) hash \(f.hash) native \(f.meta.native_fnv1a ?? "-") match \(String(describing: f.hashMatch)) read \(String(format: "%.2f", f.readMs)) ms decode \(String(format: "%.2f", f.decodeMs)) ms")
        } catch {
            self.error = "\(error)"; layers = []
        }
        revision += 1
    }

    func loadStack(dir: URL) {
        do {
            let (s, ls, t) = try loadFieldStack(dir: dir)
            stack = s; layers = ls; activeId = ls.first?.id; totalMs = t; error = nil
            mode = s.geometry != nil ? .surface : .slice
            axis = .z; slice = (ls.first?.field.dims.z ?? 1) / 2
            for l in ls {
                print("[field] \(l.id) hash \(l.field.hash) native \(l.field.meta.native_fnv1a ?? "-") match \(String(describing: l.field.hashMatch)) decode \(String(format: "%.2f", l.field.decodeMs)) ms")
            }
            print("[field] stack \(s.name): \(ls.count) layers in \(String(format: "%.1f", t)) ms")
        } catch {
            self.error = "\(error)"; layers = []
        }
        revision += 1
    }

    private func axisLen(_ a: FieldAxis) -> Int {
        guard let f = field else { return 1 }
        return a == .x ? f.dims.x : a == .y ? f.dims.y : f.dims.z
    }

    func setActive(_ id: String) { activeId = id; slice = min(slice, axisLen(axis) - 1); revision += 1 }
    func setMode(_ m: FieldViewMode) { mode = m; revision += 1 }
    func setAxis(_ a: FieldAxis) { axis = a; slice = axisLen(a) / 2; revision += 1 }
    func setSlice(_ k: Int) { slice = k; revision += 1 }
    func clear() { layers = []; stack = nil; error = nil; revision += 1 }

    var axisCount: Int { axisLen(axis) }

    func mask(for f: DecodedField) -> [Bool]? {
        guard let boxes = stack?.geometry?.boxes, !f.isPlane else { return nil }
        return solidMask(f.meta, boxes)
    }

    /// The skin of the stack's volume grid (any volume layer — they are co-registered).
    func skin() -> FieldSkin? {
        guard let s = stack, let boxes = s.geometry?.boxes,
              let vol = layers.first(where: { !$0.field.isPlane })?.field else { return nil }
        let key = "\(s.name)-\(vol.meta.dims_zyx)"
        if let c = skinCache, c.key == key { return c.skin }
        let skin = FieldSkin(meta: vol.meta, mask: solidMask(vol.meta, boxes))
        skinCache = (key, skin)
        return skin
    }

    func openPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.init(filenameExtension: "tc") ?? .data, .json, .folder]
        panel.canChooseDirectories = true
        panel.message = "Choose a tang-compress field (.tc with a .json sidecar) or a stack folder (stack.json)"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        if url.hasDirectoryPath || url.lastPathComponent == "stack.json" {
            loadStack(dir: url.lastPathComponent == "stack.json" ? url.deletingLastPathComponent() : url)
        } else {
            load(base: url.deletingPathExtension())
        }
    }

    /// `VCAD_FIELDS=/path/to/stack-dir` or `VCAD_FIELD=/path/to/hot_chip` (no extension) at launch.
    func loadFromEnvironment() {
        let env = ProcessInfo.processInfo.environment
        if let d = env["VCAD_FIELDS"], !d.isEmpty { loadStack(dir: URL(fileURLWithPath: d)); return }
        guard let p = env["VCAD_FIELD"], !p.isEmpty else { return }
        load(base: URL(fileURLWithPath: p))
    }
}

/// Build or update the field entity under `parent` (the kernel-mm frame).
@MainActor
func syncFieldOverlay(_ store: FieldOverlayStore, in parent: Entity) {
    let key = "fieldRoot-\(store.revision)"
    if let existing = parent.findEntity(named: key) { existing.isEnabled = store.active != nil; return }
    parent.children.filter { $0.name.hasPrefix("fieldRoot") }.forEach { $0.removeFromParent() }
    guard let entity = makeFieldEntity(store) else { return }
    entity.name = key
    parent.addChild(entity)
}

@MainActor
func makeFieldEntity(_ store: FieldOverlayStore) -> Entity? {
    guard let layer = store.active else { return nil }
    let f = layer.field
    let root = Entity()
    let surface = store.stack?.geometry != nil && !f.isPlane && store.mode == .surface
    if let skin = store.skin() {
        if surface, let tex = colormapTexture, let mesh = skin.mesh(field: f, range: layer.range) {
            var m = SimpleMaterial()
            m.color = .init(tint: .white, texture: .init(tex))
            m.roughness = 0.8
            m.metallic = 0
            let e = ModelEntity(mesh: mesh, materials: [m])
            e.name = "fieldSkin"
            root.addChild(e)
        } else if let mesh = skin.mesh(field: nil, range: layer.range) {
            var ghost = SimpleMaterial(color: .gray.withAlphaComponent(0.18), isMetallic: false)
            ghost.faceCulling = .none
            let e = ModelEntity(mesh: mesh, materials: [ghost])
            e.name = "fieldGhost"
            root.addChild(e)
        }
    }
    if !surface {
        let img = f.planeImage(axis: store.axis, k: store.slice)
        if let image = f.planeCGImage(img, range: layer.range, mask: store.mask(for: f)),
           let texture = try? TextureResource(image: image, options: .init(semantic: .color)) {
            var d = MeshDescriptor(name: "fieldPlane")
            d.positions = MeshBuffers.Positions(img.corners)
            // Image row 0 is the top (v1) edge → v = 1 at corners 2, 3.
            d.textureCoordinates = MeshBuffers.TextureCoordinates([[0, 0], [1, 0], [1, 1], [0, 1]])
            d.primitives = .triangles([0, 1, 2, 0, 2, 3])
            var m = UnlitMaterial()
            m.color = .init(tint: .white, texture: .init(texture))
            m.opacityThreshold = 0.5
            m.faceCulling = .none
            if let mesh = try? MeshResource.generate(from: [d]) {
                let e = ModelEntity(mesh: mesh, materials: [m])
                e.name = "fieldSlice"
                root.addChild(e)
            }
        }
    }
    return root
}

/// HUD: layer tabs, legend, mode/axis/slice, error bound, verification, timings.
struct FieldHud: View {
    @Bindable var store: FieldOverlayStore

    private func num(_ v: Double) -> String {
        abs(v) >= 100 ? String(format: "%.1f", v) : abs(v) >= 1 ? String(format: "%.3f", v) : String(format: "%.4g", v)
    }

    var body: some View {
        if let layer = store.active {
            let f = layer.field
            let units = f.meta.units ?? f.meta.bound.units
            let stream = store.layers.reduce(0) { $0 + $1.field.streamBytes }
            let raw = store.layers.reduce(0) { $0 + $1.field.data.count * 4 }
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text(store.stack?.title ?? "\(f.meta.field) · \(f.name)").bold()
                    Spacer()
                    Button { store.clear() } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
                }
                if let notes = store.stack?.notes { Text(notes).foregroundStyle(.secondary) }
                if store.layers.count > 1 {
                    Picker("", selection: Binding(get: { layer.id }, set: { store.setActive($0) })) {
                        ForEach(store.layers) { Text($0.label).tag($0.id) }
                    }
                    .pickerStyle(.menu).labelsHidden()
                }
                if store.stack == nil, let image = f.sliceImage(store.slice) {
                    Image(decorative: image, scale: 1).interpolation(.none).resizable()
                        .aspectRatio(CGFloat(f.dims.x) / CGFloat(f.dims.y), contentMode: .fit)
                        .frame(maxWidth: .infinity)
                }
                LinearGradient(colors: (0...10).map { i in
                    let (r, g, b) = turbo(Double(i) / 10)
                    return Color(red: Double(r) / 255, green: Double(g) / 255, blue: Double(b) / 255)
                }, startPoint: .leading, endPoint: .trailing).frame(height: 10)
                HStack {
                    Text(num(layer.range.0))
                    Spacer()
                    Text("\(num(layer.range.1)) \(units)")
                }
                if f.isPlane {
                    Text("2D field on the \(f.meta.plane ?? "") plane").foregroundStyle(.secondary)
                } else {
                    HStack {
                        if store.stack?.geometry != nil {
                            Picker("", selection: Binding(get: { store.mode }, set: { store.setMode($0) })) {
                                ForEach(FieldViewMode.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                            }.pickerStyle(.segmented).labelsHidden().frame(width: 130)
                        }
                        if store.mode == .slice {
                            Picker("", selection: Binding(get: { store.axis }, set: { store.setAxis($0) })) {
                                ForEach(FieldAxis.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                            }.pickerStyle(.segmented).labelsHidden().frame(width: 90)
                        }
                    }
                    if store.mode == .slice {
                        HStack {
                            Text(store.axis.rawValue)
                            Slider(value: Binding(get: { Double(store.slice) }, set: { store.setSlice(Int($0.rounded())) }),
                                   in: 0...Double(max(1, store.axisCount - 1)), step: 1)
                        }
                    }
                }
                Text(String(format: "error bound: |Δ| ≤ %.3e %@", f.meta.bound.value, units))
                Grid(alignment: .leading, horizontalSpacing: 8, verticalSpacing: 2) {
                    ForEach(store.layers) { l in
                        GridRow {
                            Text(l.id)
                            Text(String(format: "%.1fK", Double(l.field.streamBytes) / 1024))
                            Text(l.field.hashMatch == nil ? "hash ?" : l.field.hashMatch! ? "hash ✓" : "hash ✗")
                            Text(String(format: "%.1f ms", l.field.decodeMs))
                        }
                        .opacity(l.id == layer.id ? 1 : 0.6)
                    }
                }
                Text(String(format: "%.1f KB of %.0f KB (%.0f×) · %@ · cpu · load %.1f ms", Double(stream) / 1024,
                            Double(raw) / 1024, Double(raw) / Double(max(1, stream)), f.meta.format ?? "tc", store.totalMs))
                    .foregroundStyle(.secondary)
            }
            .font(.system(size: 11, design: .monospaced))
            .padding(10)
            .frame(width: 330)
            .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
            .accessibilityIdentifier("compressed-field-hud")
        } else if let e = store.error {
            Text("field error: \(e)").font(.system(size: 11, design: .monospaced)).padding(8)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 8))
        }
    }
}

struct FieldCommands: Commands {
    var body: some Commands {
        CommandGroup(after: .importExport) {
            Button("Open Compressed Field…") { FieldOverlayStore.shared.openPanel() }
        }
    }
}
