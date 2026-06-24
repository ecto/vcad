import RealityKit
import AppKit
import simd
import Metal
import CVcadFFI

/// CPU-side mesh data copied out of a kernel `VcadMeshView`, with normals
/// synthesized when the kernel didn't provide a matching-length normal buffer
/// (grounding risk #4 — the normals==vertices invariant is debug-only).
struct KernelMesh {
    var positions: [SIMD3<Float>]
    var normals: [SIMD3<Float>]
    var indices: [UInt32]
    var minBound: SIMD3<Float>
    var maxBound: SIMD3<Float>

    var triangleCount: Int { indices.count / 3 }
    var isEmpty: Bool { positions.isEmpty || indices.count < 3 }

    static func fromView(_ v: VcadMeshView) -> KernelMesh {
        let vertexCount = v.vertices_len / 3

        var positions = [SIMD3<Float>](repeating: .zero, count: vertexCount)
        if let vp = v.vertices {
            for i in 0..<vertexCount {
                positions[i] = SIMD3<Float>(vp[i * 3], vp[i * 3 + 1], vp[i * 3 + 2])
            }
        }

        var indices = [UInt32](repeating: 0, count: v.indices_len)
        if let ip = v.indices {
            for i in 0..<v.indices_len { indices[i] = ip[i] }
        }

        var normals: [SIMD3<Float>]
        if v.normals_len == v.vertices_len, v.normals_len > 0, let np = v.normals {
            normals = [SIMD3<Float>](repeating: .zero, count: vertexCount)
            for i in 0..<vertexCount {
                normals[i] = SIMD3<Float>(np[i * 3], np[i * 3 + 1], np[i * 3 + 2])
            }
        } else {
            normals = Self.synthesizeNormals(positions: positions, indices: indices)
        }

        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
        var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
        for p in positions { lo = simd_min(lo, p); hi = simd_max(hi, p) }
        if vertexCount == 0 { lo = .zero; hi = .zero }

        return KernelMesh(positions: positions, normals: normals, indices: indices,
                          minBound: lo, maxBound: hi)
    }

    /// Area-weighted vertex normals from triangle faces.
    static func synthesizeNormals(positions: [SIMD3<Float>], indices: [UInt32]) -> [SIMD3<Float>] {
        var normals = [SIMD3<Float>](repeating: .zero, count: positions.count)
        var i = 0
        while i + 2 < indices.count {
            let a = Int(indices[i]), b = Int(indices[i + 1]), c = Int(indices[i + 2])
            if a < positions.count, b < positions.count, c < positions.count {
                let faceNormal = cross(positions[b] - positions[a], positions[c] - positions[a])
                normals[a] += faceNormal; normals[b] += faceNormal; normals[c] += faceNormal
            }
            i += 3
        }
        for j in 0..<normals.count {
            let len = length(normals[j])
            normals[j] = len > 1e-6 ? normals[j] / len : SIMD3<Float>(0, 0, 1)
        }
        return normals
    }

    func resource(name: String) -> MeshResource {
        var d = MeshDescriptor(name: name)
        d.positions = MeshBuffers.Positions(positions)
        d.normals = MeshBuffers.Normals(normals)
        d.primitives = .triangles(indices)
        return (try? MeshResource.generate(from: [d])) ?? .generateBox(size: 0.1)
    }
}

/// A GPU-resident mesh that streams kernel re-solves into its buffers without
/// reallocating — the M2 hot loop. Interleaves position+normal into one buffer
/// (stride 24: pos@0, normal@12) so RealityKit binds a single layout. Grows
/// (recreates) only when the re-solved mesh exceeds capacity, which bounds the
/// per-write work and prevents buffer overflow (grounding risk #3).
@MainActor
final class StreamingMesh {
    private var mesh: LowLevelMesh?
    private var vertexCapacity = 0
    private var indexCapacity = 0
    private(set) var resource: MeshResource?

    /// Stream a kernel mesh into the GPU buffers. Returns true when the
    /// underlying mesh (and thus `resource`) was recreated for more capacity —
    /// the caller must then reassign `resource` onto its entity.
    @discardableResult
    func update(from km: KernelMesh) -> Bool {
        let vcount = km.positions.count
        let icount = km.indices.count
        guard vcount > 0, icount > 0 else { return false }

        var recreated = false
        if mesh == nil || vcount > vertexCapacity || icount > indexCapacity {
            vertexCapacity = vcount + vcount / 2 + 64        // 1.5x headroom
            indexCapacity = icount + icount / 2 + 192
            mesh = try? Self.makeMesh(vCap: vertexCapacity, iCap: indexCapacity)
            recreated = true
        }
        guard let mesh else { resource = nil; return true }

        // Interleaved [px,py,pz, nx,ny,nz] per vertex — raw floats, not SIMD3
        // (which pads to 16). Stride 24 bytes matches the layout.
        mesh.replaceUnsafeMutableBytes(bufferIndex: 0) { raw in
            let f = raw.bindMemory(to: Float.self)
            for i in 0..<vcount {
                let p = km.positions[i], n = km.normals[i]
                let o = i * 6
                f[o] = p.x; f[o + 1] = p.y; f[o + 2] = p.z
                f[o + 3] = n.x; f[o + 4] = n.y; f[o + 5] = n.z
            }
        }
        mesh.replaceUnsafeMutableIndices { raw in
            let idx = raw.bindMemory(to: UInt32.self)
            for i in 0..<icount { idx[i] = km.indices[i] }
        }
        mesh.parts.replaceAll([
            LowLevelMesh.Part(indexCount: icount, topology: .triangle,
                              bounds: BoundingBox(min: km.minBound, max: km.maxBound))
        ])

        if recreated || resource == nil {
            resource = try? MeshResource(from: mesh)
        }
        return recreated
    }

    private static func makeMesh(vCap: Int, iCap: Int) throws -> LowLevelMesh {
        let desc = LowLevelMesh.Descriptor(
            vertexCapacity: vCap,
            vertexAttributes: [
                LowLevelMesh.Attribute(semantic: .position, format: .float3, layoutIndex: 0, offset: 0),
                LowLevelMesh.Attribute(semantic: .normal, format: .float3, layoutIndex: 0, offset: 12),
            ],
            vertexLayouts: [LowLevelMesh.Layout(bufferIndex: 0, bufferOffset: 0, bufferStride: 24)],
            indexCapacity: iCap,
            indexType: .uint32
        )
        return try LowLevelMesh(descriptor: desc)
    }
}

/// A renderable, auto-fit scene: meshes + their colors, plus the combined
/// bounds so the viewport can center and scale arbitrary part sizes.
struct RenderScene {
    var meshes: [(mesh: MeshResource, color: NSColor)]
    var center: SIMD3<Float>
    var size: Float
    var triangleCount: Int
    var partCount: Int

    static let empty = RenderScene(meshes: [], center: .zero, size: 1, triangleCount: 0, partCount: 0)
}
