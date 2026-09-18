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

    /// The loop as a circle, when it is one.
    ///
    /// A DXF writes a hole as a tessellated polyline, so "this is a Ø2.5 pilot"
    /// has to be read back off the points rather than trusted from a flag. The
    /// test is the one that matters for machining: every point the same
    /// distance from the centre, to within a fortieth of a millimetre or 1% of
    /// the radius — anything looser and a slot mouth would pass as a bore.
    var circle: (centre: CGPoint, diameter: Double)? {
        guard points.count >= 8 else { return nil }
        let n = Double(points.count)
        let centre = CGPoint(x: points.reduce(0) { $0 + $1.x } / n,
                             y: points.reduce(0) { $0 + $1.y } / n)
        let radii = points.map { hypot($0.x - centre.x, $0.y - centre.y) }
        guard let mean = radii.reduce(0, +) / n as Double?, mean > 1e-6 else { return nil }
        let tolerance = max(0.025, mean * 0.01)
        guard radii.allSatisfy({ abs($0 - mean) <= tolerance }) else { return nil }
        return (centre, mean * 2)
    }
}

/// An imported outline: one outer loop plus any holes, already translated so
/// the outer loop's lower-left corner sits at the origin.
struct CNCOutline: Equatable {
    var outer: CNCLoop
    var holes: [CNCLoop]
    var name: String
    /// Where the stock frame's zero sits in the coordinates the outline was
    /// drawn in. Friction-log item 47: the toolpath was drawn in the stock
    /// frame while the part stayed where it was modelled, so the path floated
    /// beside the solid instead of lying on it. Putting this back on the
    /// overlay's root is what lands one on the other.
    var origin: CGPoint = .zero
    /// Stock extents that just contain the outer loop.
    var width: Double { outer.bounds.width }
    var height: Double { outer.bounds.height }

    /// Holes that are circles, by index, with their diameters.
    var circularHoles: [(index: Int, centre: CGPoint, diameter: Double)] {
        holes.enumerated().compactMap { index, hole in
            hole.circle.map { (index, $0.centre, $0.diameter) }
        }
    }

    /// An outline from a section of the part on screen.
    ///
    /// The section comes back where the part was modelled — the stator sits at
    /// z 11.1–17.1 and nowhere near the origin in XY — and CAM works in the
    /// stock frame, lower-left at zero. So this does for a sectioned solid
    /// exactly what the DXF reader does for a file: shift it, and remember by
    /// how much, so the toolpath is drawn on the part rather than beside it
    /// (items 18 and 47).
    static func from(region: CNCSection.Region, name: String) -> CNCOutline {
        let outer = CNCLoop(points: region.outer.map { CGPoint(x: $0[0], y: $0[1]) })
        let origin = outer.bounds.origin
        func shift(_ loop: CNCLoop) -> CNCLoop {
            CNCLoop(points: loop.points.map { CGPoint(x: $0.x - origin.x, y: $0.y - origin.y) })
        }
        let holes = region.holes.map { hole in
            shift(CNCLoop(points: hole.map { CGPoint(x: $0[0], y: $0[1]) }))
        }
        return CNCOutline(outer: shift(outer), holes: holes, name: name, origin: origin)
    }

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
        return CNCOutline(outer: outer, holes: holes, name: name, origin: origin)
    }
}
