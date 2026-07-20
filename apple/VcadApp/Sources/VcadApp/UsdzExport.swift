import ModelIO
import AppKit
import simd

/// USDZ export via ModelIO: kernel part meshes (Z-up, millimeters) → one
/// MDLMesh per part with a PBR material, exported as a .usdz Quick Look /
/// AR-ready package at physical scale.
enum UsdzExport {
    /// A part ready for export: mesh in kernel coordinates (mm) plus its
    /// resolved appearance.
    struct Part {
        var mesh: KernelMesh
        var name: String
        var color: NSColor
        var metallic: Float = 0.55
        var roughness: Float = 0.34
    }

    /// USD stages default to centimeters (metersPerUnit = 0.01) and Y-up;
    /// kernel space is millimeters and Z-up. mm → cm, (x, y, z) → (x, z, −y).
    private static let mmToStage: Float = 0.1

    private static func toStage(_ p: SIMD3<Float>) -> SIMD3<Float> {
        SIMD3<Float>(p.x, p.z, -p.y) * mmToStage
    }
    private static func toStageDir(_ n: SIMD3<Float>) -> SIMD3<Float> {
        SIMD3<Float>(n.x, n.z, -n.y)
    }

    @discardableResult
    static func write(parts: [Part], to url: URL) -> Bool {
        guard !parts.isEmpty, MDLAsset.canExportFileExtension("usdc") else { return false }
        let allocator = MDLMeshBufferDataAllocator()
        let asset = MDLAsset(bufferAllocator: allocator)

        for part in parts {
            let km = part.mesh
            guard !km.isEmpty else { continue }
            let vcount = km.positions.count

            // Interleaved [pos.xyz, normal.xyz], stride 24 — same layout the
            // renderer uses, in USD stage space.
            var vertexData = [Float](repeating: 0, count: vcount * 6)
            for i in 0..<vcount {
                let p = toStage(km.positions[i])
                let n = toStageDir(km.normals[i])
                let o = i * 6
                vertexData[o] = p.x; vertexData[o + 1] = p.y; vertexData[o + 2] = p.z
                vertexData[o + 3] = n.x; vertexData[o + 4] = n.y; vertexData[o + 5] = n.z
            }

            let vertexBuffer = allocator.newBuffer(
                with: vertexData.withUnsafeBytes { Data($0) }, type: .vertex)
            let indexBuffer = allocator.newBuffer(
                with: km.indices.withUnsafeBytes { Data($0) }, type: .index)

            let descriptor = MDLVertexDescriptor()
            descriptor.attributes[0] = MDLVertexAttribute(
                name: MDLVertexAttributePosition, format: .float3, offset: 0, bufferIndex: 0)
            descriptor.attributes[1] = MDLVertexAttribute(
                name: MDLVertexAttributeNormal, format: .float3, offset: 12, bufferIndex: 0)
            descriptor.layouts[0] = MDLVertexBufferLayout(stride: 24)

            let scattering = MDLPhysicallyPlausibleScatteringFunction()
            let srgb = part.color.usingColorSpace(.sRGB) ?? part.color
            scattering.baseColor.type = .float3
            scattering.baseColor.float3Value = SIMD3<Float>(
                Float(srgb.redComponent), Float(srgb.greenComponent), Float(srgb.blueComponent))
            scattering.metallic.floatValue = part.metallic
            scattering.roughness.floatValue = part.roughness
            let material = MDLMaterial(name: "\(part.name)_mat", scatteringFunction: scattering)

            let submesh = MDLSubmesh(
                indexBuffer: indexBuffer, indexCount: km.indices.count,
                indexType: .uInt32, geometryType: .triangles, material: material)
            let mesh = MDLMesh(
                vertexBuffer: vertexBuffer, vertexCount: vcount,
                descriptor: descriptor, submeshes: [submesh])
            mesh.name = part.name
            asset.add(mesh)
        }

        guard asset.count > 0 else { return false }
        // ModelIO can't export .usdz directly — write a .usdc and package it
        // as USDZ (an uncompressed zip whose payload is 64-byte aligned).
        let tmp = FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-usdz-\(UUID().uuidString)")
        let usdc = tmp.appendingPathComponent("model.usdc")
        defer { try? FileManager.default.removeItem(at: tmp) }
        do {
            try FileManager.default.createDirectory(at: tmp, withIntermediateDirectories: true)
            try asset.export(to: usdc)
            let payload = try Data(contentsOf: usdc)
            try zipAsUsdz(name: "model.usdc", payload: payload).write(to: url)
            return true
        } catch {
            NSLog("[UsdzExport] export failed: \(error)")
            return false
        }
    }

    // MARK: usdz packaging — minimal stored-zip writer

    /// Wrap a single file in a USDZ-conformant zip: stored (no compression),
    /// with the local-header extra field padded so the payload starts on a
    /// 64-byte boundary (the alignment Quick Look / usdz validators require).
    private static func zipAsUsdz(name: String, payload: Data) -> Data {
        let nameBytes = Array(name.utf8)
        let crc = crc32(payload)

        // Local header is 30 bytes + name + extra. Pad `extra` so data offset
        // lands on 64. Extra field = ID 0x1986 ("usdz padding") + zeros.
        let headerLen = 30 + nameBytes.count
        var extraLen = (64 - (headerLen % 64)) % 64
        if extraLen > 0 && extraLen < 4 { extraLen += 64 }   // extra needs ≥4 bytes for ID+size

        var out = Data()
        func put16(_ v: UInt16) { withUnsafeBytes(of: v.littleEndian) { out.append(contentsOf: $0) } }
        func put32(_ v: UInt32) { withUnsafeBytes(of: v.littleEndian) { out.append(contentsOf: $0) } }

        // Local file header
        put32(0x04034b50); put16(20); put16(0); put16(0)     // sig, version, flags, method=stored
        put16(0); put16(0)                                   // mod time/date
        put32(crc); put32(UInt32(payload.count)); put32(UInt32(payload.count))
        put16(UInt16(nameBytes.count)); put16(UInt16(extraLen))
        out.append(contentsOf: nameBytes)
        if extraLen > 0 {
            var extra = Data(count: extraLen)
            extra[0] = 0x86; extra[1] = 0x19                 // ID 0x1986, little-endian
            let payloadSize = UInt16(extraLen - 4).littleEndian
            withUnsafeBytes(of: payloadSize) { extra[2] = $0[0]; extra[3] = $0[1] }
            out.append(extra)
        }
        out.append(payload)

        // Central directory
        let cdOffset = UInt32(out.count)
        put32(0x02014b50); put16(20); put16(20); put16(0); put16(0)
        put16(0); put16(0)
        put32(crc); put32(UInt32(payload.count)); put32(UInt32(payload.count))
        put16(UInt16(nameBytes.count)); put16(0); put16(0)
        put16(0); put16(0); put32(0); put32(0)               // disk, attrs, local header offset = 0
        out.append(contentsOf: nameBytes)
        let cdSize = UInt32(out.count) - cdOffset

        // End of central directory
        put32(0x06054b50); put16(0); put16(0); put16(1); put16(1)
        put32(cdSize); put32(cdOffset); put16(0)
        return out
    }

    private static let crcTable: [UInt32] = (0..<256).map { i -> UInt32 in
        var c = UInt32(i)
        for _ in 0..<8 { c = (c & 1) != 0 ? 0xEDB88320 ^ (c >> 1) : c >> 1 }
        return c
    }

    private static func crc32(_ data: Data) -> UInt32 {
        var c: UInt32 = 0xFFFFFFFF
        for b in data { c = crcTable[Int((c ^ UInt32(b)) & 0xFF)] ^ (c >> 8) }
        return c ^ 0xFFFFFFFF
    }
}
