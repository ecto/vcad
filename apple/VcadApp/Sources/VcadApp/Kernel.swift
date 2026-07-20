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

    /// Bounds + counts computed while streaming, so callers don't need a
    /// CPU-side copy of the mesh just to know its extent.
    struct StreamStats {
        var minBound: SIMD3<Float>
        var maxBound: SIMD3<Float>
        var triangleCount: Int
        var vertexCount: Int
    }

    /// Direct FFI path: write the kernel's tessellation buffers straight into
    /// the GPU vertex/index buffers — no intermediate Swift arrays. Normals are
    /// synthesized in-buffer (area-weighted) when the kernel view doesn't carry
    /// a matching-length normal buffer. Falls back to the MeshDescriptor path
    /// (via KernelMesh) if LowLevelMesh creation fails.
    func update(fromView v: VcadMeshView) -> (recreated: Bool, stats: StreamStats)? {
        let vcount = v.vertices_len / 3
        let icount = v.indices_len
        guard vcount > 0, icount >= 3, let vp = v.vertices, let ip = v.indices else { return nil }

        var recreated = false
        if mesh == nil || vcount > vertexCapacity || icount > indexCapacity {
            vertexCapacity = vcount + vcount / 2 + 64        // 1.5x headroom
            indexCapacity = icount + icount / 2 + 192
            mesh = try? Self.makeMesh(vCap: vertexCapacity, iCap: indexCapacity)
            recreated = true
        }
        guard let mesh else {
            // MeshDescriptor fallback — still renders, just not streamable.
            let km = KernelMesh.fromView(v)
            resource = km.resource(name: "fallback")
            return (true, StreamStats(minBound: km.minBound, maxBound: km.maxBound,
                                      triangleCount: km.triangleCount,
                                      vertexCount: km.positions.count))
        }

        var lo = SIMD3<Float>(repeating: .greatestFiniteMagnitude)
        var hi = SIMD3<Float>(repeating: -.greatestFiniteMagnitude)
        let hasNormals = v.normals_len == v.vertices_len && v.normals != nil
        let np = v.normals

        mesh.replaceUnsafeMutableBytes(bufferIndex: 0) { raw in
            let f = raw.bindMemory(to: Float.self)
            for i in 0..<vcount {
                let s = i * 3, o = i * 6
                let x = vp[s], y = vp[s + 1], z = vp[s + 2]
                f[o] = x; f[o + 1] = y; f[o + 2] = z
                lo = simd_min(lo, SIMD3<Float>(x, y, z))
                hi = simd_max(hi, SIMD3<Float>(x, y, z))
                if hasNormals, let np {
                    f[o + 3] = np[s]; f[o + 4] = np[s + 1]; f[o + 5] = np[s + 2]
                } else {
                    f[o + 3] = 0; f[o + 4] = 0; f[o + 5] = 0
                }
            }
            if !hasNormals {
                // Area-weighted normals synthesized in the GPU buffer itself:
                // accumulate face normals into the normal slots, then normalize.
                var t = 0
                while t + 2 < icount {
                    let a = Int(ip[t]), b = Int(ip[t + 1]), c = Int(ip[t + 2])
                    t += 3
                    guard a < vcount, b < vcount, c < vcount else { continue }
                    let pa = SIMD3<Float>(f[a * 6], f[a * 6 + 1], f[a * 6 + 2])
                    let pb = SIMD3<Float>(f[b * 6], f[b * 6 + 1], f[b * 6 + 2])
                    let pc = SIMD3<Float>(f[c * 6], f[c * 6 + 1], f[c * 6 + 2])
                    let fn = cross(pb - pa, pc - pa)
                    for j in [a, b, c] {
                        f[j * 6 + 3] += fn.x; f[j * 6 + 4] += fn.y; f[j * 6 + 5] += fn.z
                    }
                }
                for i in 0..<vcount {
                    let o = i * 6
                    var n = SIMD3<Float>(f[o + 3], f[o + 4], f[o + 5])
                    let len = length(n)
                    n = len > 1e-6 ? n / len : SIMD3<Float>(0, 0, 1)
                    f[o + 3] = n.x; f[o + 4] = n.y; f[o + 5] = n.z
                }
            }
        }
        mesh.replaceUnsafeMutableIndices { raw in
            let idx = raw.bindMemory(to: UInt32.self)
            for i in 0..<icount { idx[i] = ip[i] }
        }
        mesh.parts.replaceAll([
            LowLevelMesh.Part(indexCount: icount, topology: .triangle,
                              bounds: BoundingBox(min: lo, max: hi))
        ])

        if recreated || resource == nil {
            resource = try? MeshResource(from: mesh)
        }
        return (recreated, StreamStats(minBound: lo, maxBound: hi,
                                       triangleCount: icount / 3, vertexCount: vcount))
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
/// `edges[i]` (when present) holds part i's feature-edge segments as flat
/// endpoint pairs, for the CAD edge overlay.
struct RenderScene {
    var meshes: [(mesh: MeshResource, color: NSColor)]
    var center: SIMD3<Float>
    var size: Float
    var triangleCount: Int
    var partCount: Int
    var edges: [[SIMD3<Float>]] = []
    /// Assembly instances (rendered INSTEAD of `meshes` when non-empty,
    /// mirroring the web viewport). Each carries its part-def-local mesh and
    /// its kernel-frame world transform, so playback can re-pose the entity
    /// per frame without touching the mesh.
    var instances: [RenderInstance] = []
    /// Index-aligned with `meshes`: non-nil when part i is a pattern rendered
    /// as one shared MeshResource + N per-instance transforms (else one entity).
    var instancing: [PatternInstancing?] = []

    static let empty = RenderScene(meshes: [], center: .zero, size: 1, triangleCount: 0, partCount: 0)
}

/// One assembly instance ready to render. `index` is the FFI instance index —
/// entity names ("inst<i>") and playback transform order both key off it.
struct RenderInstance {
    var index: Int
    var id: String
    var mesh: MeshResource
    var material: ResolvedMaterial
    /// World placement in kernel coordinates (Z-up, mm), column-major.
    var transform: float4x4
}

/// Feature-edge overlay support: converts a kernel `VcadEdgesView` into
/// segment endpoints and builds a renderable ribbon mesh (RealityKit has no
/// line primitive, so each segment becomes two crossed quads — reads as a
/// crisp line from every direction at CAD zoom levels).
enum EdgeOverlay {
    /// Flat endpoint pairs [a0, b0, a1, b1, ...] from the FFI view.
    static func segments(fromView v: VcadEdgesView) -> [SIMD3<Float>] {
        guard let p = v.floats, v.floats_len >= 6 else { return [] }
        let segCount = v.floats_len / 6
        var out = [SIMD3<Float>]()
        out.reserveCapacity(segCount * 2)
        for s in 0..<segCount {
            let o = s * 6
            out.append(SIMD3<Float>(p[o], p[o + 1], p[o + 2]))
            out.append(SIMD3<Float>(p[o + 3], p[o + 4], p[o + 5]))
        }
        return out
    }

    /// Build one mesh holding every segment as two crossed quads of the given
    /// world-space width. Returns nil for empty input.
    static func ribbonResource(segments: [SIMD3<Float>], width: Float, name: String) -> MeshResource? {
        let segCount = segments.count / 2
        guard segCount > 0, width > 0 else { return nil }
        var positions = [SIMD3<Float>]()
        var indices = [UInt32]()
        positions.reserveCapacity(segCount * 8)
        indices.reserveCapacity(segCount * 24)
        let h = width / 2
        for s in 0..<segCount {
            let a = segments[s * 2], b = segments[s * 2 + 1]
            let d = b - a
            let len = length(d)
            guard len > 1e-6 else { continue }
            let dir = d / len
            // Two perpendicular in-plane axes for the crossed quads.
            let ref: SIMD3<Float> = abs(dir.z) < 0.9 ? [0, 0, 1] : [1, 0, 0]
            let u = normalize(cross(dir, ref))
            let v = normalize(cross(dir, u))
            let base = UInt32(positions.count)
            positions.append(contentsOf: [
                a - u * h, a + u * h, b + u * h, b - u * h,
                a - v * h, a + v * h, b + v * h, b - v * h,
            ])
            for q in 0..<2 {
                let o = base + UInt32(q * 4)
                // Both windings so the quad is visible from either side
                // without needing a double-sided material.
                indices.append(contentsOf: [o, o + 1, o + 2, o, o + 2, o + 3])
                indices.append(contentsOf: [o, o + 2, o + 1, o, o + 3, o + 2])
            }
        }
        guard !positions.isEmpty else { return nil }
        var d = MeshDescriptor(name: name)
        d.positions = MeshBuffers.Positions(positions)
        // Unlit material ignores shading, but the generator wants normals.
        d.normals = MeshBuffers.Normals([SIMD3<Float>](repeating: [0, 0, 1], count: positions.count))
        d.primitives = .triangles(indices)
        return try? MeshResource.generate(from: [d])
    }
}
