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
    func testStackMatchesNativeHashes() throws {
        let (stack, layers, ms) = try loadFieldStack(dir: Self.stackDir)
        XCTAssertEqual(layers.map(\.id), ["thermal", "stress", "displacement", "busbar"])
        let want = ["thermal": "885c6c38b04aa965", "stress": "7e7509c29c235505",
                    "displacement": "647b1b6f2ac39355", "busbar": "2a433f8f79030415"]
        for l in layers {
            XCTAssertEqual(l.field.hash, want[l.id], l.id)
            XCTAssertEqual(l.field.hashMatch, true, l.id)
        }
        XCTAssertEqual(layers.last?.field.isPlane, true)
        // Solid-only range: the wall is held at 40 °C, so the part never reads ambient.
        XCTAssertEqual(layers[0].range.0, 40, accuracy: 0.1)
        let skin = FieldSkin(meta: layers[0].field.meta, mask: solidMask(layers[0].field.meta, stack.geometry!.boxes))
        XCTAssertGreaterThan(skin.cells.count, 10_000)
        print("[field] native stack load (4 layers, read+decode+range): \(String(format: "%.1f", ms)) ms; skin quads \(skin.cells.count)")
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
