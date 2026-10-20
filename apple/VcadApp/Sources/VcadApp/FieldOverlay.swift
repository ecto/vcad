import SwiftUI
import RealityKit
import AppKit
import CTangCompress

// Compressed scalar fields (tang-compress TCG1/TCG3 streams) in the native
// app — the twin of the web app's `?field=` overlay (docs/compressed-fields.md).
// Decode goes through libtang_compress_ffi (CPU integer reference decoder, C
// ABI; build-tc-ffi.sh). One z-slice is drawn as a colormapped plane in the
// kernel-mm frame; a HUD shows the slice slider, legend, the stream's
// max-error claim, and the decode hash against the native reference.
//
// VcadApp-only on purpose: Editor.swift and Shell.swift are symlinked into
// VcadVision, so nothing here hangs off EditorModel.

/// Sidecar metadata (`<name>.json` next to `<name>.tc`), same schema as the web.
struct FieldMeta: Decodable {
    struct Bound: Decodable { let kind: String; let value: Double; let units: String }
    let field: String
    let dims_zyx: [Int]
    let origin_mm: [Double]
    let voxel_mm: [Double]
    let min: Double
    let max: Double
    let bound: Bound
    let native_fnv1a: String?
    let format: String?
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

/// Load `<base>.tc` + `<base>.json` (base = path without extension).
func loadCompressedField(base: URL) throws -> DecodedField {
    let t0 = DispatchTime.now()
    let stream = try Data(contentsOf: base.appendingPathExtension("tc"))
    let meta = try JSONDecoder().decode(FieldMeta.self, from: Data(contentsOf: base.appendingPathExtension("json")))
    let t1 = DispatchTime.now()
    let (data, dims, hash) = try tcDecode(stream)
    let t2 = DispatchTime.now()
    guard meta.dims_zyx == [dims.0, dims.1, dims.2] else {
        throw FieldDecodeError.shape("stream dims \(dims) ≠ sidecar \(meta.dims_zyx)")
    }
    let ms = { (a: DispatchTime, b: DispatchTime) in Double(b.uptimeNanoseconds - a.uptimeNanoseconds) / 1e6 }
    return DecodedField(name: base.lastPathComponent, meta: meta, data: data, dims: (dims.0, dims.1, dims.2),
                        hash: hash, streamBytes: stream.count, readMs: ms(t0, t1), decodeMs: ms(t1, t2))
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

extension DecodedField {
    /// One z-slice as an RGBA image; image row 0 is the TOP (y = ny-1) so it
    /// maps onto a plane the same way the field's +y does.
    func sliceImage(_ k: Int) -> CGImage? {
        let (nx, ny) = (dims.x, dims.y)
        var rgba = [UInt8](repeating: 255, count: nx * ny * 4)
        let span = meta.max - meta.min
        for j in 0..<ny {
            for i in 0..<nx {
                let v = Double(data[k * nx * ny + j * nx + i])
                let (r, g, b) = turbo(span > 0 ? (v - meta.min) / span : 0)
                let o = ((ny - 1 - j) * nx + i) * 4
                rgba[o] = r; rgba[o + 1] = g; rgba[o + 2] = b
            }
        }
        guard let provider = CGDataProvider(data: Data(rgba) as CFData) else { return nil }
        return CGImage(width: nx, height: ny, bitsPerComponent: 8, bitsPerPixel: 32, bytesPerRow: nx * 4,
                       space: CGColorSpace(name: CGColorSpace.sRGB)!,
                       bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
                       provider: provider, decode: nil, shouldInterpolate: false, intent: .defaultIntent)
    }

    /// The slice with the hottest cell — where the structure is.
    var hottestSlice: Int {
        guard let i = data.indices.max(by: { data[$0] < data[$1] }) else { return 0 }
        return i / (dims.x * dims.y)
    }
}

@MainActor @Observable
final class FieldOverlayStore {
    static let shared = FieldOverlayStore()
    var field: DecodedField?
    var slice = 0
    var error: String?
    /// Bumped on any change the viewport must redraw.
    var revision = 0

    func load(base: URL) {
        do {
            let f = try loadCompressedField(base: base)
            field = f; slice = f.hottestSlice; error = nil
            print("[field] \(f.name) hash \(f.hash) native \(f.meta.native_fnv1a ?? "-") match \(String(describing: f.hashMatch)) read \(String(format: "%.2f", f.readMs)) ms decode \(String(format: "%.2f", f.decodeMs)) ms")
        } catch {
            self.error = "\(error)"; field = nil
        }
        revision += 1
    }

    func setSlice(_ k: Int) { slice = k; revision += 1 }
    func clear() { field = nil; error = nil; revision += 1 }

    func openPanel() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.init(filenameExtension: "tc") ?? .data]
        panel.message = "Choose a tang-compress field (.tc with a .json sidecar)"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        load(base: url.deletingPathExtension())
    }

    /// `VCAD_FIELD=/path/to/hot_chip` (no extension) loads at launch.
    func loadFromEnvironment() {
        guard let p = ProcessInfo.processInfo.environment["VCAD_FIELD"], !p.isEmpty else { return }
        load(base: URL(fileURLWithPath: p))
    }
}

/// Build or update the field entity under `parent` (the kernel-mm frame).
@MainActor
func syncFieldOverlay(_ store: FieldOverlayStore, in parent: Entity) {
    let key = "fieldRoot-\(store.revision)"
    if let existing = parent.findEntity(named: key) { existing.isEnabled = store.field != nil; return }
    parent.children.filter { $0.name.hasPrefix("fieldRoot") }.forEach { $0.removeFromParent() }
    guard let entity = makeFieldEntity(store) else { return }
    entity.name = key
    parent.addChild(entity)
}

@MainActor
func makeFieldEntity(_ store: FieldOverlayStore) -> Entity? {
    guard let f = store.field, let image = f.sliceImage(store.slice),
          let texture = try? TextureResource(image: image, options: .init(semantic: .color)) else { return nil }
    let (o, v) = (f.meta.origin_mm.map(Float.init), f.meta.voxel_mm.map(Float.init))
    let size: SIMD3<Float> = [Float(f.dims.x) * v[0], Float(f.dims.y) * v[1], Float(f.dims.z) * v[2]]
    var material = UnlitMaterial()
    material.color = .init(tint: .white, texture: .init(texture))
    material.faceCulling = .none
    // generatePlane(width:depth:) lies in XZ; .generatePlane(width:height:) in XY (Z-up kernel frame).
    let plane = ModelEntity(mesh: .generatePlane(width: size.x, height: size.y), materials: [material])
    plane.name = "fieldSlice"
    plane.position = [o[0] + size.x / 2, o[1] + size.y / 2, o[2] + (Float(store.slice) + 0.5) * v[2]]
    let root = Entity()
    root.addChild(plane)
    // Domain box as a faint shell.
    var shell = SimpleMaterial(color: .gray.withAlphaComponent(0.08), isMetallic: false)
    shell.faceCulling = .none
    let box = ModelEntity(mesh: .generateBox(size: size), materials: [shell])
    box.position = [o[0] + size.x / 2, o[1] + size.y / 2, o[2] + size.z / 2]
    box.name = "fieldDomain"
    root.addChild(box)
    return root
}

/// HUD: slice slider, legend, error bound, hash verdict, timings.
struct FieldHud: View {
    @Bindable var store: FieldOverlayStore

    var body: some View {
        if let f = store.field {
            let rawKB = Double(f.data.count * 4) / 1024
            let zmm = f.meta.origin_mm[2] + (Double(store.slice) + 0.5) * f.meta.voxel_mm[2]
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text("\(f.meta.field) · \(f.name)").bold()
                    Spacer()
                    Button { store.clear() } label: { Image(systemName: "xmark") }.buttonStyle(.plain)
                }
                if let image = f.sliceImage(store.slice) {
                    Image(decorative: image, scale: 1).interpolation(.none).resizable()
                        .aspectRatio(CGFloat(f.dims.x) / CGFloat(f.dims.y), contentMode: .fit)
                        .frame(maxWidth: .infinity)
                }
                LinearGradient(colors: (0...10).map { i in
                    let (r, g, b) = turbo(Double(i) / 10)
                    return Color(red: Double(r) / 255, green: Double(g) / 255, blue: Double(b) / 255)
                }, startPoint: .leading, endPoint: .trailing).frame(height: 10)
                HStack {
                    Text(String(format: "%.2f", f.meta.min))
                    Spacer()
                    Text(String(format: "%.2f %@", f.meta.max, f.meta.bound.units))
                }
                HStack {
                    Text("z")
                    Slider(value: Binding(get: { Double(store.slice) }, set: { store.setSlice(Int($0.rounded())) }),
                           in: 0...Double(max(1, f.dims.z - 1)), step: 1)
                    Text(String(format: "%.2f mm", zmm))
                }
                Text(String(format: "error bound: |Δ| ≤ %.3e %@", f.meta.bound.value, f.meta.bound.units))
                Text(String(format: "%.1f KB of %.0f KB (%.1f×) · %@ · cpu", Double(f.streamBytes) / 1024, rawKB,
                            Double(f.data.count * 4) / Double(f.streamBytes), f.meta.format ?? "tc"))
                Text("hash \(f.hash) " + (f.hashMatch == nil ? "(no native hash)"
                                          : f.hashMatch! ? "= native ✓" : "≠ native \(f.meta.native_fnv1a!) ✗"))
                Text(String(format: "read %.1f ms · decode %.1f ms", f.readMs, f.decodeMs)).foregroundStyle(.secondary)
            }
            .font(.system(size: 11, design: .monospaced))
            .padding(10)
            .frame(width: 300)
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
