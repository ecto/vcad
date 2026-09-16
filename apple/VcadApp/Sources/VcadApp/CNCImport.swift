import Foundation

/// Import deliberately accepts the GRBL subset we can also preview accurately.
/// Coordinate-system changes, machine-coordinate moves and canned cycles need
/// controller offsets we don't have in a static file, and are rejected.
enum CNCImport {
    static func parse(_ source: String) throws -> CNCProgram {
        let lines = try CNCCommands.programLines(source)
        var p = [0.0, 0, 0], absolute = true, scale = 1.0, motion = 0, feed = 400.0
        var moves: [CNCMove] = []
        let regex = try NSRegularExpression(pattern: #"([A-Z])\s*([-+]?(?:\d+\.?\d*|\.\d+))"#)
        for (index, line) in lines.enumerated() {
            let text = line.uppercased()
            let matches = regex.matches(in: text, range: NSRange(text.startIndex..., in: text))
            var words: [String: Double] = [:]
            var dwell = false
            for match in matches {
                let letter = String(text[Range(match.range(at: 1), in: text)!])
                let number = Double(text[Range(match.range(at: 2), in: text)!])!
                guard number.isFinite, abs(number) < 1_000_000 else { throw CNCError.message("Invalid number on line \(index + 1).") }
                if letter == "G" {
                    switch number {
                    case 0, 1, 2, 3: motion = Int(number)
                    case 4: dwell = true
                    case 40, 49, 80: break
                    case 17, 21, 54, 94: if number == 21 { scale = 1 }
                    case 20: scale = 25.4
                    case 90: absolute = true
                    case 91: absolute = false
                    default: throw CNCError.message("G\(number.formatted()) on line \(index + 1) cannot be previewed. Import supports G54, XY arcs, G0–G3, G20/21 and G90/91.")
                    }
                } else {
                    guard ["X", "Y", "Z", "I", "J", "R", "F", "S", "M", "N", "T", "P"].contains(letter) else {
                        throw CNCError.message("Unsupported \(letter) word on line \(index + 1).")
                    }
                    if letter == "T", number > 1 { throw CNCError.message("Import requires one manually installed tool (T0 or T1).") }
                    if letter == "M", ![2.0, 3, 4, 5, 7, 8, 9, 30].contains(number) { throw CNCError.message("Unsupported M code on line \(index + 1).") }
                    words[letter] = number
                }
            }
            let residual = regex.stringByReplacingMatches(in: text, range: NSRange(text.startIndex..., in: text), withTemplate: "").trimmingCharacters(in: .whitespaces)
            guard residual.isEmpty else { throw CNCError.message("Unrecognized G-code on line \(index + 1).") }
            if let value = words["F"] { guard value > 0 else { throw CNCError.message("Feed must be positive.") }; feed = value * scale }
            if dwell { continue }
            guard ["X", "Y", "Z", "I", "J", "R"].contains(where: { words[$0] != nil }) else { continue }
            var end = p
            for (i, axis) in ["X", "Y", "Z"].enumerated() { if let v = words[axis] { end[i] = v * scale + (absolute ? 0 : p[i]) } }
            if moves.isEmpty { moves.append(CNCMove(to: p, rapid: true)) }
            if motion < 2 {
                moves.append(CNCMove(to: end, rapid: motion == 0, feed: feed))
            } else {
                var cx = p[0] + (words["I"] ?? 0) * scale, cy = p[1] + (words["J"] ?? 0) * scale
                if let r = words["R"] {
                    let radius = abs(r * scale), dx = end[0] - p[0], dy = end[1] - p[1], chord = hypot(dx, dy)
                    guard chord > 0, radius >= chord / 2 else { throw CNCError.message("Invalid radius arc on line \(index + 1).") }
                    let h = sqrt(max(0, radius * radius - chord * chord / 4)) * (motion == 2 ? -1.0 : 1.0) * (r < 0 ? -1.0 : 1.0)
                    cx = (p[0] + end[0]) / 2 - dy / chord * h; cy = (p[1] + end[1]) / 2 + dx / chord * h
                } else if words["I"] == nil && words["J"] == nil { throw CNCError.message("Arc needs I/J or R on line \(index + 1).") }
                let radius = hypot(p[0] - cx, p[1] - cy)
                guard radius > 0, abs(hypot(end[0] - cx, end[1] - cy) - radius) < max(0.005, radius * 0.001) else { throw CNCError.message("Arc endpoints do not share a radius on line \(index + 1).") }
                let start = atan2(p[1] - cy, p[0] - cx)
                var sweep = atan2(end[1] - cy, end[0] - cx) - start
                if motion == 2 { if sweep >= 0 { sweep -= 2 * .pi } } else { if sweep <= 0 { sweep += 2 * .pi } }
                let count = max(8, min(4096, Int(ceil(abs(sweep) / 0.035))))
                for i in 1...count {
                    let t = Double(i) / Double(count), angle = start + sweep * t
                    moves.append(CNCMove(to: [cx + radius * cos(angle), cy + radius * sin(angle), p[2] + (end[2] - p[2]) * t], rapid: false, feed: feed))
                }
            }
            p = end
            guard moves.count <= 250_000 else { throw CNCError.message("Preview exceeds 250,000 segments.") }
        }
        guard !moves.isEmpty else { throw CNCError.message("No previewable motion in this file.") }
        // Establish the same initial modal state used by the preview.
        return CNCProgram(gcode: "G21 G90 G17 G54 G94 F400\n" + lines.joined(separator: "\n"), moves: moves)
    }
}
