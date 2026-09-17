import Foundation
import CoreGraphics

// Part outlines for contour machining: closed loops in millimetres, read from
// a DXF (LWPOLYLINE entities) and translated into the stock frame (XY
// lower-left at 0, the frame every CAM request uses).

/// A closed loop in the stock frame.
struct CNCLoop: Equatable {
    var points: [CGPoint]
    /// Signed area (shoelace); the outer loop is the largest by magnitude.
    var area: Double {
        guard points.count >= 3 else { return 0 }
        var a = 0.0
        for i in points.indices {
            let p = points[i], q = points[(i + 1) % points.count]
            a += p.x * q.y - q.x * p.y
        }
        return a / 2
    }
    var bounds: CGRect {
        guard let first = points.first else { return .zero }
        var r = CGRect(origin: first, size: .zero)
        for p in points { r = r.union(CGRect(origin: p, size: .zero)) }
        return r
    }
}

/// An imported outline: one outer loop plus any holes, already translated so
/// the outer loop's lower-left corner sits at the origin.
struct CNCOutline: Equatable {
    var outer: CNCLoop
    var holes: [CNCLoop]
    var name: String
    /// Stock extents that just contain the outer loop.
    var width: Double { outer.bounds.width }
    var height: Double { outer.bounds.height }

    /// Parse every closed LWPOLYLINE in a DXF. Bulge arcs are not supported
    /// (export the outline pre-tessellated); a polyline carrying a bulge is
    /// refused rather than silently flattened wrong.
    static func parseDXF(_ text: String, name: String) throws -> CNCOutline {
        let lines = text.replacingOccurrences(of: "\r", with: "").components(separatedBy: "\n")
        var loops: [CNCLoop] = []
        var i = 0
        func pair(_ i: Int) -> (String, String)? {
            guard i + 1 < lines.count else { return nil }
            return (lines[i].trimmingCharacters(in: .whitespaces), lines[i + 1].trimmingCharacters(in: .whitespaces))
        }
        while let (code, value) = pair(i) {
            guard code == "0", value == "LWPOLYLINE" else { i += 2; continue }
            i += 2
            var xs: [Double] = [], ys: [Double] = []
            var closed = false
            while let (c, v) = pair(i), c != "0" {
                switch c {
                case "70": closed = (Int(v) ?? 0) & 1 == 1
                case "10": xs.append(Double(v) ?? .nan)
                case "20": ys.append(Double(v) ?? .nan)
                case "42": if abs(Double(v) ?? 0) > 1e-9 { throw CNCError.message("\(name) has arc segments (bulges); export the outline as straight polylines.") }
                default: break
                }
                i += 2
            }
            guard xs.count == ys.count, xs.count >= 3, xs.allSatisfy(\.isFinite), ys.allSatisfy(\.isFinite) else { continue }
            var pts = zip(xs, ys).map { CGPoint(x: $0, y: $1) }
            if !closed, let f = pts.first, let l = pts.last, hypot(f.x - l.x, f.y - l.y) < 1e-6 { pts.removeLast() }
            loops.append(CNCLoop(points: pts))
        }
        guard !loops.isEmpty else { throw CNCError.message("\(name) has no closed polylines.") }
        loops.sort { abs($0.area) > abs($1.area) }
        var outer = loops.removeFirst()
        // Into the stock frame: lower-left of the outer loop at the origin.
        let origin = outer.bounds.origin
        func shift(_ loop: CNCLoop) -> CNCLoop {
            CNCLoop(points: loop.points.map { CGPoint(x: $0.x - origin.x, y: $0.y - origin.y) })
        }
        outer = shift(outer)
        let holes = loops.map(shift).filter { outer.bounds.contains($0.bounds) }
        guard outer.bounds.width >= 0.01, outer.bounds.height >= 0.01 else {
            throw CNCError.message("\(name)'s outline is degenerate.")
        }
        return CNCOutline(outer: outer, holes: holes, name: name)
    }
}
