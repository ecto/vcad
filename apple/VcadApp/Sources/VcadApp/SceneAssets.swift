import RealityKit
import CoreGraphics
#if canImport(AppKit)
import AppKit
#endif

// Procedurally drawn scene assets shared by every viewport: the studio and
// zebra environments (image-based lighting), the zebra chrome, the reference
// grid and the contact shadow. No bundled HDRs or textures.

@MainActor
enum SceneAssets {
    /// A dark studio environment drawn procedurally, used for image-based
    /// reflections on the geometry. A soft ceiling, a horizon band, and three
    /// softbox highlights at varied azimuths give metals real reflection
    /// structure as the camera orbits.
    static let studioEnvironment: EnvironmentResource? = makeStudioEnvironment()
    static let zebraEnvironment: EnvironmentResource? = makeZebraEnvironment()

    /// Mirror-chrome for zebra analysis: near-zero roughness so the striped
    /// environment reflects as sharp bands.
    static let zebraChrome: PhysicallyBasedMaterial = {
        var m = PhysicallyBasedMaterial()
        m.baseColor = .init(tint: .white)
        m.metallic = 1.0
        m.roughness = 0.02
        return m
    }()

    /// Near-black unlit material for the feature-edge overlay — reads as ink
    /// lines over the shaded surfaces, the classic CAD look.
    static let edgeMaterial = UnlitMaterial(color: NSColor(white: 0.09, alpha: 1.0))

    /// Equirect of bold horizontal bands: the classic zebra-analysis light box.
    /// Reflected off a chromed surface, stripe kinks expose G1/G2 breaks.
    private static func makeZebraEnvironment() -> EnvironmentResource? {
        let w = 2048, h = 1024
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        ctx.setFillColor(CGColor(gray: 0.03, alpha: 1))
        ctx.fill(CGRect(x: 0, y: 0, width: w, height: h))
        let bands = 32
        let bandH = CGFloat(h) / CGFloat(bands)
        ctx.setFillColor(CGColor(gray: 0.98, alpha: 1))
        for i in stride(from: 0, to: bands, by: 2) {
            ctx.fill(CGRect(x: 0, y: CGFloat(i) * bandH, width: CGFloat(w), height: bandH))
        }
        guard let img = ctx.makeImage() else { return nil }
        return try? EnvironmentResource(equirectangular: img)
    }

    private static func makeStudioEnvironment() -> EnvironmentResource? {
        let w = 2048, h = 1024
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: w, height: h, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.noneSkipLast.rawValue) else { return nil }
        let fw = CGFloat(w), fh = CGFloat(h)
        let base = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0.20, green: 0.21, blue: 0.25, alpha: 1),
            CGColor(red: 0.13, green: 0.14, blue: 0.17, alpha: 1),
            CGColor(red: 0.07, green: 0.075, blue: 0.09, alpha: 1),
            CGColor(red: 0.025, green: 0.025, blue: 0.032, alpha: 1),
            CGColor(red: 0.008, green: 0.008, blue: 0.011, alpha: 1),
        ] as CFArray, locations: [0, 0.34, 0.52, 0.74, 1])!
        ctx.drawLinearGradient(base, start: CGPoint(x: 0, y: fh), end: CGPoint(x: 0, y: 0), options: [])

        ctx.setBlendMode(.plusLighter)
        let horizon = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0.16, green: 0.18, blue: 0.22, alpha: 0),
            CGColor(red: 0.16, green: 0.18, blue: 0.22, alpha: 0.7),
            CGColor(red: 0.16, green: 0.18, blue: 0.22, alpha: 0),
        ] as CFArray, locations: [0, 0.5, 1])!
        ctx.drawLinearGradient(horizon,
            start: CGPoint(x: 0, y: fh * 0.40), end: CGPoint(x: 0, y: fh * 0.56), options: [])

        func softbox(_ cx: CGFloat, _ cy: CGFloat, _ rad: CGFloat, _ c: CGColor) {
            let g = CGGradient(colorsSpace: cs, colors: [c, c.copy(alpha: 0)!] as CFArray, locations: [0, 1])!
            ctx.drawRadialGradient(g, startCenter: CGPoint(x: cx, y: cy), startRadius: 0,
                                   endCenter: CGPoint(x: cx, y: cy), endRadius: rad, options: [])
        }
        softbox(fw * 0.20, fh * 0.95, fh * 0.30, CGColor(red: 0.20, green: 0.23, blue: 0.28, alpha: 1))
        softbox(fw * 0.56, fh * 0.97, fh * 0.22, CGColor(red: 0.22, green: 0.20, blue: 0.17, alpha: 1))
        softbox(fw * 0.83, fh * 0.93, fh * 0.24, CGColor(red: 0.15, green: 0.18, blue: 0.22, alpha: 1))
        guard let img = ctx.makeImage() else { return nil }
        return try? EnvironmentResource(equirectangular: img)
    }

    // MARK: reference grid + contact shadow

    /// Pick a round millimeter grid step (1/2/5 decade) so ~10–25 minor
    /// cells span the part regardless of its size.
    static func gridStepMM(forPartSize size: Float) -> Float {
        guard size.isFinite, size > 0 else { return 10 }
        let target = size / 12
        let decade = pow(10, floor(log10(target)))
        for m in [1 as Float, 2, 5, 10] where m * decade >= target {
            return m * decade
        }
        return 10 * decade
    }

    /// A 48×48-cell grid atlas: hairline minor lines, brighter major lines
    /// every 10 cells, and a radial fade so the grid grounds the part without
    /// stretching to the horizon. Drawn in white; tinted per appearance.
    static let gridTexture: TextureResource? = makeGridTexture()
    private static func makeGridTexture() -> TextureResource? {
        let cells = 48, px = 32
        let sz = cells * px
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: sz, height: sz, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        let center = CGFloat(sz) / 2
        let maxR = center * 0.98
        func lineAlpha(_ x: CGFloat, _ y: CGFloat, major: Bool) -> CGFloat {
            let d = hypot(x - center, y - center)
            let fade = max(0, 1 - d / maxR)
            return (major ? 0.72 : 0.34) * fade * fade
        }
        ctx.setLineWidth(1)
        for i in 0...cells {
            let major = i % 10 == 0 || i == cells
            let v = CGFloat(i * px)
            let steps = 24
            for sIdx in 0..<steps {
                let t0 = CGFloat(sIdx) / CGFloat(steps) * CGFloat(sz)
                let t1 = CGFloat(sIdx + 1) / CGFloat(steps) * CGFloat(sz)
                let mid = (t0 + t1) / 2
                let aV = lineAlpha(v, mid, major: major)
                if aV > 0.003 {
                    ctx.setStrokeColor(CGColor(gray: 1.0, alpha: aV))
                    ctx.move(to: CGPoint(x: v, y: t0)); ctx.addLine(to: CGPoint(x: v, y: t1))
                    ctx.strokePath()
                }
                let aH = lineAlpha(mid, v, major: major)
                if aH > 0.003 {
                    ctx.setStrokeColor(CGColor(gray: 1.0, alpha: aH))
                    ctx.move(to: CGPoint(x: t0, y: v)); ctx.addLine(to: CGPoint(x: t1, y: v))
                    ctx.strokePath()
                }
            }
        }
        guard let img = ctx.makeImage() else { return nil }
        return try? TextureResource(image: img, options: .init(semantic: .color))
    }

    /// A soft radial alpha disc (dark centre → clear edge) for the pooled
    /// contact shadow. Built once and stretched per part.
    static let contactTexture: TextureResource? = makeContactTexture()
    private static func makeContactTexture() -> TextureResource? {
        let s = 256
        let cs = CGColorSpaceCreateDeviceRGB()
        guard let ctx = CGContext(data: nil, width: s, height: s, bitsPerComponent: 8,
                                  bytesPerRow: 0, space: cs,
                                  bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue) else { return nil }
        let g = CGGradient(colorsSpace: cs, colors: [
            CGColor(red: 0, green: 0, blue: 0, alpha: 0.5),
            CGColor(red: 0, green: 0, blue: 0, alpha: 0.32),
            CGColor(red: 0, green: 0, blue: 0, alpha: 0.0),
        ] as CFArray, locations: [0, 0.45, 1])!
        let c = CGFloat(s) / 2
        ctx.drawRadialGradient(g, startCenter: CGPoint(x: c, y: c), startRadius: 0,
                               endCenter: CGPoint(x: c, y: c), endRadius: c, options: [])
        guard let img = ctx.makeImage() else { return nil }
        return try? TextureResource(image: img, options: .init(semantic: .color))
    }

    /// The grounding set for a windowed viewport: a reference grid whose cells
    /// snap to a round millimetre step, plus a contact shadow under the part.
    /// Both sit just below the part's lowest point. `dark` tints the grid for
    /// the appearance it is drawn over.
    @MainActor
    static func groundingEntities(partSizeMM: SIMD3<Float>, sceneSize: Float,
                                  sceneScale: Float, dark: Bool) -> [Entity] {
        var out: [Entity] = []
        let floorY = -(partSizeMM.z * 0.5 * sceneScale) - 0.004
        let cellMM = gridStepMM(forPartSize: sceneSize)
        let cellWorld = cellMM * sceneScale
        if cellWorld > 1e-5, let gridTex = gridTexture {
            var gm = UnlitMaterial()
            gm.color = .init(tint: NSColor(white: dark ? 1.0 : 0.0, alpha: dark ? 0.55 : 0.28),
                             texture: .init(gridTex))
            gm.blending = .transparent(opacity: .init(floatLiteral: 1.0))
            let cells = 48
            let sizeW = cellWorld * Float(cells)
            var gd = MeshDescriptor(name: "grid")
            let h = sizeW / 2
            gd.positions = MeshBuffers.Positions([[-h, 0, -h], [h, 0, -h], [h, 0, h], [-h, 0, h]])
            gd.normals = MeshBuffers.Normals([[0, 1, 0], [0, 1, 0], [0, 1, 0], [0, 1, 0]])
            gd.textureCoordinates = MeshBuffers.TextureCoordinates([[0, 0], [1, 0], [1, 1], [0, 1]])
            gd.primitives = .triangles([0, 2, 1, 0, 3, 2, 0, 1, 2, 0, 2, 3])
            if let gmesh = try? MeshResource.generate(from: [gd]) {
                let grid = ModelEntity(mesh: gmesh, materials: [gm])
                grid.name = "grid"
                grid.position = [0, floorY + 0.0008, 0]
                out.append(grid)
            }
        }
        if let tex = contactTexture {
            var sm = UnlitMaterial()
            sm.color = .init(tint: .white, texture: .init(tex))
            sm.blending = .transparent(opacity: .init(floatLiteral: dark ? 1.0 : 0.6))
            let fwd = max(0.05, partSizeMM.x * sceneScale * 1.8)
            let dpt = max(0.05, partSizeMM.y * sceneScale * 1.8)
            let blob = ModelEntity(mesh: .generatePlane(width: fwd, depth: dpt), materials: [sm])
            blob.name = "contactShadow"
            blob.position = [0, floorY + 0.0015, 0]
            out.append(blob)
        }
        return out
    }
}
