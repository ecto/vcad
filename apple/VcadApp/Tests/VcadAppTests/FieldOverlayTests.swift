import XCTest
import SwiftUI
import RealityKit
import AppKit
@testable import VcadApp

/// Compressed-field decode through libtang_compress_ffi. The fixture is the
/// web app's sample (packages/app/public/fields/hot_chip.{tc,json}); its
/// sidecar carries the native Rust decode hash from tang-compress `corpus`.
@MainActor
final class FieldOverlayTests: XCTestCase {
    static let base = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        .deletingLastPathComponent().deletingLastPathComponent()
        .appendingPathComponent("packages/app/public/fields/hot_chip")

    func testDecodeMatchesNativeHash() throws {
        let f = try loadCompressedField(base: Self.base)
        XCTAssertEqual(f.dims.z * f.dims.y * f.dims.x, 16 * 128 * 128)
        XCTAssertEqual(f.hash, "e1a3021ce69b8d0d")
        XCTAssertEqual(f.hashMatch, true)
        // Physically sane: ambient floor, die hot spot near 74.6 °C.
        XCTAssertEqual(Double(f.data.min()!), 25, accuracy: f.meta.bound.value)
        XCTAssertEqual(Double(f.data.max()!), f.meta.max, accuracy: f.meta.bound.value)
        // The hottest cell sits over the 45–55 mm die.
        let i = f.data.indices.max(by: { f.data[$0] < f.data[$1] })!
        let x = Double(i % f.dims.x) * f.meta.voxel_mm[0], y = Double((i / f.dims.x) % f.dims.y) * f.meta.voxel_mm[1]
        XCTAssert((44...56).contains(x) && (44...56).contains(y), "hot spot at (\(x), \(y))")
    }

    static let stackDir = base.deletingLastPathComponent().appendingPathComponent("bracket")

    /// Every layer of the multiphysics stack decodes to its native Rust hash.
    /// (`stress_mech` reproduces the pre-coupling stress layer bit for bit.)
    func testStackMatchesNativeHashes() throws {
        let (stack, layers, ms) = try loadFieldStack(dir: Self.stackDir)
        XCTAssertEqual(layers.map(\.id), ["thermal", "stress", "stress_mech", "displacement", "busbar"])
        // Coupled chain: busbar Joule heat → thermal → thermal strain + load → stress.
        let want = ["thermal": "3048f6d56ae47311", "stress": "76cd14f4923fec65", "stress_mech": "7e7509c29c235505",
                    "displacement": "2f50b000e9f9b475", "busbar": "2a433f8f79030415"]
        for l in layers {
            XCTAssertEqual(l.field.hash, want[l.id], l.id)
            XCTAssertEqual(l.field.hashMatch, true, l.id)
        }
        XCTAssertEqual(layers.last?.field.isPlane, true)
        // Solid-only range: the wall is held at 40 °C, so the part never reads ambient.
        XCTAssertEqual(layers[0].range.0, 40, accuracy: 0.1)
        let skin = FieldSkin(meta: layers[0].field.meta, mask: solidMask(layers[0].field.meta, stack.geometry!.boxes))
        XCTAssertGreaterThan(skin.cells.count, 10_000)
        print("[field] native stack load (\(layers.count) layers, read+decode+range): \(String(format: "%.1f", ms)) ms; skin quads \(skin.cells.count)")
    }

    func testRejectsGarbage() {
        XCTAssertThrowsError(try tcDecode(Data("nope, not a stream".utf8)))
    }

    func testDecodeTiming() throws {
        let stream = try Data(contentsOf: Self.base.appendingPathExtension("tc"))
        var best = Double.infinity
        for _ in 0..<5 {
            let t = DispatchTime.now()
            _ = try tcDecode(stream)
            best = min(best, Double(DispatchTime.now().uptimeNanoseconds - t.uptimeNanoseconds) / 1e6)
        }
        print("[field] native Swift→Rust CPU decode best of 5: \(String(format: "%.2f", best)) ms")
    }

    func testFieldSnapshots() async throws {
        guard ProcessInfo.processInfo.environment["VCAD_CNC_SNAPSHOTS"] == "1" else { throw XCTSkip("Opt-in field snapshots") }
        _ = NSApplication.shared
        let store = FieldOverlayStore.shared
        store.load(base: Self.base)
        store.setSlice(9)
        let dir = URL(fileURLWithPath: "/tmp/vcad-field")
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        // HUD (SwiftUI).
        let hosting = NSHostingView(rootView: FieldHud(store: store).padding(12)
            .background(Color(nsColor: .windowBackgroundColor)))
        let w1 = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 330, height: 520), styleMask: [.borderless], backing: .buffered, defer: false)
        w1.contentView = hosting; w1.orderFront(nil)
        try? await Task.sleep(for: .milliseconds(300))
        let bitmap = try XCTUnwrap(hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds))
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        try XCTUnwrap(bitmap.representation(using: .png, properties: [:])).write(to: dir.appendingPathComponent("hud.png"))

        // The 3D overlay entity, in an ARView with the viewport's Z-up → Y-up wrap.
        let ar = ARView(frame: NSRect(x: 0, y: 0, width: 900, height: 600))
        let w2 = NSWindow(contentRect: ar.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        w2.contentView = ar; w2.orderFront(nil)
        let anchor = AnchorEntity(world: .zero)
        let kernel = Entity()
        kernel.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])
        kernel.scale = [0.01, 0.01, 0.01]
        anchor.addChild(kernel)
        syncFieldOverlay(store, in: kernel)
        ar.scene.addAnchor(anchor)
        let camera = PerspectiveCamera()
        camera.look(at: [0.5, 0, -0.5], from: [0.5, 1.1, 0.35], relativeTo: nil)
        let cameraAnchor = AnchorEntity(world: .zero); cameraAnchor.addChild(camera)
        ar.scene.addAnchor(cameraAnchor)
        try? await Task.sleep(for: .milliseconds(800))
        let image: NSImage? = await withCheckedContinuation { c in ar.snapshot(saveToHDR: false) { c.resume(returning: $0) } }
        let tiff = try XCTUnwrap(image?.tiffRepresentation)
        try XCTUnwrap(NSBitmapImageRep(data: tiff)?.representation(using: .png, properties: [:]))
            .write(to: dir.appendingPathComponent("viewport.png"))

        // The multiphysics stack: every layer (surface for volumes, plane for 2D),
        // plus a y-slice of the thermal layer.
        store.loadStack(dir: Self.stackDir)
        camera.look(at: [0.48, 0.12, -0.24], from: [1.25, 0.95, 0.75], relativeTo: nil)
        var shots: [(String, () -> Void)] = store.layers.map { l in (l.id, { store.setActive(l.id); store.setMode(.surface) }) }
        shots.append(("thermal-slice", { store.setActive("thermal"); store.setMode(.slice); store.setAxis(.y) }))
        for (name, apply) in shots {
            apply()
            syncFieldOverlay(store, in: kernel)
            try? await Task.sleep(for: .milliseconds(600))
            let shot: NSImage? = await withCheckedContinuation { c in ar.snapshot(saveToHDR: false) { c.resume(returning: $0) } }
            try XCTUnwrap(NSBitmapImageRep(data: try XCTUnwrap(shot?.tiffRepresentation))?.representation(using: .png, properties: [:]))
                .write(to: dir.appendingPathComponent("stack-\(name).png"))
        }
        store.setActive("thermal"); store.setMode(.surface)
        let stackHud = NSHostingView(rootView: FieldHud(store: store).padding(12)
            .background(Color(nsColor: .windowBackgroundColor)))
        w1.contentView = stackHud; w1.orderFront(nil)
        try? await Task.sleep(for: .milliseconds(300))
        let hb = try XCTUnwrap(stackHud.bitmapImageRepForCachingDisplay(in: stackHud.bounds))
        stackHud.cacheDisplay(in: stackHud.bounds, to: hb)
        try XCTUnwrap(hb.representation(using: .png, properties: [:])).write(to: dir.appendingPathComponent("stack-hud.png"))
        w1.orderOut(nil)
        w2.orderOut(nil)
    }
}

/// Animation frames for the bracket power cycle, rendered from the
/// *compressed* streams: `VCAD_FIELD_FRAMES=<dir>` with `frames.json`,
/// `tc/manifest.json` and `tc/{T,R}_####*_m1e-4_tcg3s.tc` (R = reduced-order stress frames) (see
/// docs/compressed-fields.md). Every decode is hash-checked against the
/// native Rust decode; PNGs land in `<dir>/png/`.
@MainActor
final class FieldAnimationTests: XCTestCase {
    struct ManifestEntry: Decodable { let file: String; let hash: String }

    func testRenderPowerCycleFrames() async throws {
        guard let dirPath = ProcessInfo.processInfo.environment["VCAD_FIELD_FRAMES"] else {
            throw XCTSkip("Set VCAD_FIELD_FRAMES=<frames dir> to render the animation")
        }
        _ = NSApplication.shared
        let dir = URL(fileURLWithPath: dirPath)
        let manifest = try JSONDecoder().decode([ManifestEntry].self, from: Data(contentsOf: dir.appendingPathComponent("tc/manifest.json")))
        let hashes = Dictionary(uniqueKeysWithValues: manifest.map { ($0.file, $0.hash) })
        let stackDir = FieldOverlayTests.stackDir
        let stackFile = try JSONDecoder().decode(FieldStackFile.self, from: Data(contentsOf: stackDir.appendingPathComponent("stack.json")))
        let template = try JSONDecoder().decode(FieldMeta.self, from: Data(contentsOf: stackDir.appendingPathComponent("bracket_vm.json")))
        let frameCount = try (JSONSerialization.jsonObject(with: Data(contentsOf: dir.appendingPathComponent("frames.json"))) as! [Any]).count

        // Decode every frame first (hash-checked), then fix one color range per quantity.
        var decodeMs = 0.0, streamBytes = 0
        func decode(_ name: String) throws -> [Float] {
            let file = "\(name)_m1e-4_tcg3s.tc"
            let bytes = try Data(contentsOf: dir.appendingPathComponent("tc/\(file)"))
            let t = DispatchTime.now()
            let (data, _, hash) = try tcDecode(bytes)
            decodeMs += Double(DispatchTime.now().uptimeNanoseconds - t.uptimeNanoseconds) / 1e6
            streamBytes += bytes.count
            XCTAssertEqual(hash, hashes[file], file)
            return data
        }
        let mask = solidMask(template, stackFile.geometry!.boxes)
        var frames: [(t: [Float], s: [Float])] = []
        for k in 0..<frameCount {
            let id = String(format: "%04d", k)
            frames.append((try decode("T_\(id)"), try decode("R_\(id)_vm")))
        }
        func solidRange(_ pick: ((t: [Float], s: [Float])) -> [Float]) -> (Double, Double) {
            var (lo, hi) = (Double.infinity, -Double.infinity)
            for f in frames { let v = pick(f); for i in mask.indices where mask[i] { lo = min(lo, Double(v[i])); hi = max(hi, Double(v[i])) } }
            return (lo, hi)
        }
        let tRange = solidRange { $0.t }, sRange = solidRange { $0.s }
        print(String(format: "[field] animation: %d frames × 2 fields decoded in %.1f ms (%.1f KB compressed); T %.2f–%.2f °C, σ %.3f–%.3f MPa",
                     frameCount, decodeMs, Double(streamBytes) / 1024, tRange.0, tRange.1, sRange.0, sRange.1))
        try Data("{\"t_range\":[\(tRange.0),\(tRange.1)],\"s_range\":[\(sRange.0),\(sRange.1)],\"decode_ms\":\(decodeMs),\"stream_bytes\":\(streamBytes)}\n".utf8)
            .write(to: dir.appendingPathComponent("render.json"))

        // One ARView, the viewport's Z-up → Y-up wrap, same framing as the stack snapshots.
        let ar = ARView(frame: NSRect(x: 0, y: 0, width: 960, height: 600))
        let window = NSWindow(contentRect: ar.frame, styleMask: [.borderless], backing: .buffered, defer: false)
        window.contentView = ar; window.orderFront(nil)
        let anchor = AnchorEntity(world: .zero)
        let kernel = Entity()
        kernel.orientation = simd_quatf(angle: -.pi / 2, axis: [1, 0, 0])
        kernel.scale = [0.01, 0.01, 0.01]
        anchor.addChild(kernel)
        ar.scene.addAnchor(anchor)
        let camera = PerspectiveCamera()
        camera.look(at: [0.5, 0.1, -0.24], from: [1.08, 0.7, 0.5], relativeTo: nil)
        let cameraAnchor = AnchorEntity(world: .zero); cameraAnchor.addChild(camera)
        ar.scene.addAnchor(cameraAnchor)
        let png = dir.appendingPathComponent("png")
        try FileManager.default.createDirectory(at: png, withIntermediateDirectories: true)

        let store = FieldOverlayStore()
        store.stack = stackFile
        store.mode = .surface
        let info = try (JSONSerialization.jsonObject(with: Data(contentsOf: dir.appendingPathComponent("frames.json"))) as! [[String: Any]])
        var shots: [String: NSImage] = [:]
        for (k, f) in frames.enumerated() {
            for (tag, data, range, label) in [("T", f.t, tRange, "temperature"), ("S", f.s, sRange, "von_mises")] {
                let field = DecodedField(name: "\(tag)_\(k)", meta: template, data: data, dims: (48, 64, 128),
                                         hash: "", streamBytes: 0, readMs: 0, decodeMs: 0)
                store.layers = [FieldLayer(id: label, label: label, field: field, range: range)]
                store.activeId = label
                store.revision += 1
                syncFieldOverlay(store, in: kernel)
                try? await Task.sleep(for: .milliseconds(k == 0 ? 600 : 120))
                let shot: NSImage? = await withCheckedContinuation { c in ar.snapshot(saveToHDR: false) { c.resume(returning: $0) } }
                shots[tag] = try XCTUnwrap(shot)
            }
            // Compose: temperature | coupled von Mises, colorbars, caption (AppKit text —
            // the bundled ffmpeg has no drawtext).
            let fi = info[k]
            let (time, power, tmax) = (fi["time_s"] as! Double, fi["power_w"] as! Double, fi["t_max_c"] as! Double)
            try composeFrame(t: shots["T"]!, s: shots["S"]!, tRange: tRange, sRange: sRange,
                             caption: String(format: "t = %3.0f s    power %@ %.2f W    T_max %.2f °C", time, power > 0 ? "ON " : "OFF", power, tmax),
                             on: power > 0)
                .write(to: png.appendingPathComponent(String(format: "C_%04d.png", k)))
        }
        window.orderOut(nil)
    }

    /// 1920×720 frame: two 960×600 panels, titles, turbo colorbars, caption.
    private func composeFrame(t: NSImage, s: NSImage, tRange: (Double, Double), sRange: (Double, Double),
                              caption: String, on: Bool) throws -> Data {
        let (w, h) = (1920, 720)
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: w, pixelsHigh: h, bitsPerSample: 8, samplesPerPixel: 4,
                                   hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
        NSColor(red: 0.086, green: 0.094, blue: 0.114, alpha: 1).setFill()
        NSRect(x: 0, y: 0, width: w, height: h).fill()
        // Panels on a light card so the white-background renders read as plates.
        for (i, img) in [t, s].enumerated() {
            let r = NSRect(x: i * 960 + 8, y: 70, width: 944, height: 590)
            NSColor.white.setFill(); NSBezierPath(roundedRect: r, xRadius: 10, yRadius: 10).fill()
            img.draw(in: r.insetBy(dx: 4, dy: 4))
        }
        func text(_ str: String, _ x: CGFloat, _ y: CGFloat, _ size: CGFloat, _ color: NSColor = .white, bold: Bool = false) {
            let font = NSFont.monospacedSystemFont(ofSize: size, weight: bold ? .semibold : .regular)
            (str as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: [.font: font, .foregroundColor: color])
        }
        text("vcad · bracket power cycle — every frame decoded from a tang-compress stream (hash-verified)", 24, 682, 20, bold: true)
        text("temperature", 28, 626, 20, .black, bold: true)
        text("von Mises · 60 N + thermal strain α(T − 40 °C)", 988, 626, 20, .black, bold: true)
        for (i, (range, units)) in [(tRange, "°C"), (sRange, "MPa")].enumerated() {
            let x0 = CGFloat(i * 960 + 300)
            for px in 0..<360 {
                let (r, g, b) = turbo(Double(px) / 359)
                NSColor(red: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: 1).setFill()
                NSRect(x: x0 + CGFloat(px), y: 40, width: 1, height: 14).fill()
            }
            text(String(format: "%.2f", range.0), x0 - 80, 36, 16)
            text(String(format: "%.2f %@", range.1, units), x0 + 370, 36, 16)
        }
        text(caption, 24, 8, 20, on ? NSColor(red: 1, green: 0.82, blue: 0.4, alpha: 1) : NSColor(white: 0.62, alpha: 1), bold: true)
        NSGraphicsContext.restoreGraphicsState()
        return try XCTUnwrap(rep.representation(using: .png, properties: [:]))
    }
}
